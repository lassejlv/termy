use crate::multiplexer::protocol::*;
use crate::{remote::*, *};
use anyhow::{Context, bail};
use flume::{Receiver, Sender};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Condvar, Mutex, MutexGuard, Weak},
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime},
};

const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Progress or a working status not updated for this long, while no command
/// is known to run, is treated as gone: the program that set it was probably
/// killed without clearing it (audit L16). A prompt clears both already, so
/// this matters for shells without OSC 133 integration and for reports made
/// after a command finished. Blocked records wait for the user and never
/// expire.
const STALE_ACTIVITY_AFTER: Duration = Duration::from_secs(30 * 60);
/// How long a clipboard request waits for the attached UI, which may be
/// asking the user. The program is then told it was denied (audit B7).
const HOST_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// How often the worker checks that the subscriber holding a clipboard
/// request is still attached, so a closed view's requests are denied soon.
const HOST_CHECK_INTERVAL: Duration = Duration::from_millis(250);

enum Work {
    Wake,
    Command(RemoteCommand, Sender<RemoteReply>),
    Viewport(RemoteCommand, Sender<(bool, u64)>),
    Refresh(Sender<Arc<RemoteState>>),
    Snapshot(Sender<Arc<RemoteState>>),
    Size(Sender<TerminalSize>),
    HostReply(u64, RemoteHostReply),
    Close,
    /// Close, and wait until the program has ended: the host is exiting.
    Shutdown,
    /// A view was shown again: capture a frame now and send it to that view.
    Resume(Weak<Subscription>),
    /// Makes the worker panic, to test that a panic still ends the pane.
    #[cfg(all(test, unix))]
    Panic,
}

struct Shared {
    info: PaneInfo,
    /// What the pane was started with, for a pane split from it.
    launch: LaunchConfig,
    /// Follows the command the latest input started.
    input: InputTracker,
    state: Arc<RemoteState>,
    /// The latest event of each [`sticky_key`], and when it arrived. Wall
    /// clock time, so that system sleep counts towards staleness.
    sticky_events: Vec<(TerminalEvent, SystemTime)>,
    /// The latest OSC 7501 snapshot, if any, with when each record last
    /// changed.
    program_status: Option<Vec<(ProgramStatusRecord, SystemTime)>>,
    subscribers: Vec<Weak<Subscription>>,
    /// False once the worker stopped: the pane was closed, the host is
    /// shutting down or the worker panicked. Nothing can subscribe then.
    alive: bool,
}

#[derive(Default)]
struct Pending {
    state: bool,
    /// The replay seed of a new subscription, sent before any live event.
    replay: Vec<TerminalEvent>,
    events: Vec<TerminalEvent>,
    /// Clipboard requests in the order the program made them, each with the
    /// time the worker denies it.
    host: VecDeque<(u64, RemoteHostRequest, Instant)>,
    /// The host is shutting down: sent once, after the queued events.
    host_stopping: bool,
    closed: bool,
    /// A hidden view: send events, but no frames until it resumes (audit I7).
    paused: bool,
    /// The pause or resume to confirm to the view, before any later frame.
    acknowledge: Option<bool>,
    /// The view lost track of the frames and images sent before: start the
    /// next frame afresh.
    resync: bool,
}

pub(crate) struct Subscription {
    pending: Mutex<Pending>,
    ready: Condvar,
    shared: Weak<Mutex<Shared>>,
    /// A view that can answer clipboard requests (see [`HostBridge`]).
    host_capable: bool,
    /// Wakes the worker, which captures no frames while every view is paused.
    worker: flume::WeakSender<Work>,
}

impl Subscription {
    pub(crate) fn close(&self) {
        lock_recovering(&self.pending).closed = true;
        self.ready.notify_one();
    }

    /// Pauses or resumes frames for a view that is hidden or shown again,
    /// and confirms it with [`Update::Paused`]. Events and clipboard requests
    /// are still sent while paused. On resume the worker captures a frame at
    /// once and sends only that one: a frame published while the view was
    /// hidden may be older than the screen.
    pub(crate) fn set_paused(self: &Arc<Self>, paused: bool) {
        let mut pending = self.pending.lock().unwrap();
        if pending.paused == paused {
            return;
        }
        pending.paused = paused;
        pending.acknowledge = Some(paused);
        let resume = !paused
            && self
                .worker
                .upgrade()
                .is_some_and(|worker| worker.try_send(Work::Resume(Arc::downgrade(self))).is_ok());
        // Without a worker to capture a frame, the latest one must do.
        pending.state = !paused && !resume;
        drop(pending);
        self.ready.notify_one();
    }

    /// Sends the next frame in full and its images anew; see
    /// [`Self::take_resync`].
    pub(crate) fn resync(&self) {
        let mut pending = self.pending.lock().unwrap();
        pending.resync = true;
        pending.state = !pending.paused;
        drop(pending);
        self.ready.notify_one();
    }

    /// Whether the view asked to start afresh since the last call.
    pub(crate) fn take_resync(&self) -> bool {
        std::mem::take(&mut self.pending.lock().unwrap().resync)
    }

    /// Marks the frame captured for a view that resumed as due.
    fn resumed(&self) {
        let mut pending = self.pending.lock().unwrap();
        if !pending.paused {
            pending.state = true;
            drop(pending);
            self.ready.notify_one();
        }
    }

    fn wants_frames(&self) -> bool {
        !self.pending.lock().unwrap().paused
    }

    /// The next update to send. A closed subscription still delivers the
    /// events queued before it closed, so a pane that ends sends its `Exit`
    /// (audit B10); frames and clipboard requests are no longer sent.
    pub(crate) fn next(&self) -> Option<Update> {
        let mut pending = lock_recovering(&self.pending);
        loop {
            if !pending.closed
                && let Some(paused) = pending.acknowledge.take()
            {
                return Some(Update::Paused(paused));
            }
            if !pending.closed
                && let Some((id, request, deadline)) = pending.host.pop_front()
            {
                if deadline <= Instant::now() {
                    // Already denied; the client would only drop it.
                    continue;
                }
                return Some(Update::HostRequest {
                    id,
                    request,
                    deadline: Some(deadline),
                });
            }
            if !pending.replay.is_empty() {
                return Some(Update::Replay(std::mem::take(&mut pending.replay)));
            }
            if !pending.events.is_empty() {
                return Some(Update::Events(std::mem::take(&mut pending.events)));
            }
            if std::mem::take(&mut pending.host_stopping) {
                return Some(Update::HostShutdown);
            }
            if pending.closed {
                return None;
            }
            if pending.state && !pending.paused {
                pending.state = false;
                // Never acquire Shared with Pending held: the worker publishes
                // in the opposite order.
                drop(pending);
                let shared = self.shared.upgrade()?;
                return Some(Update::State(Arc::clone(&lock_recovering(&shared).state)));
            }
            pending = self.ready.wait(pending).unwrap();
        }
    }
}

/// The pane does not exist, or has ended: a client asking for it will never
/// get it, unlike when a request merely fails. Holds the whole message, which
/// starts like the server's `is_missing_pane` and `is_exited_pane` expect.
#[derive(Debug)]
pub(crate) struct PaneNotFound(pub(crate) String);

impl std::fmt::Display for PaneNotFound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PaneNotFound {}

/// Called once when the pane's process exits, with whether a client was
/// attached to the pane at that moment.
type ExitHook = Box<dyn FnOnce(bool) + Send>;

#[derive(Clone)]
pub(crate) struct Pane {
    tx: Sender<Work>,
    shared: Arc<Mutex<Shared>>,
    exit_hook: Arc<Mutex<Option<ExitHook>>>,
    /// The worker, which owns the terminal; `None` once joined.
    worker: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl Pane {
    /// Starts the pane's program. `root` is the host's session directory,
    /// exported to the program as `TERMY_MUX_DIR` with the pane's ID as
    /// `TERMY_MUX_PANE`, so `termy mux` inside the pane finds this host.
    pub(crate) fn create(launch: PaneLaunch, root: &Path) -> anyhow::Result<Self> {
        Self::spawn(launch, root, HOST_REQUEST_TIMEOUT)
    }

    fn spawn(mut launch: PaneLaunch, root: &Path, host_timeout: Duration) -> anyhow::Result<Self> {
        let id = uuid::Uuid::new_v4().to_string();
        // Its variables belong to the launched program (an SSH tab's
        // askpass helper), not to a shell split from it.
        let launch_config = LaunchConfig {
            config: TerminalRuntimeConfig {
                environment: Default::default(),
                ..launch.config.clone()
            },
            shell_integration: launch.shell_integration.clone(),
        };
        // The directory the terminal starts in, resolved here so that the
        // pane lists it rather than the one requested, which may not exist
        // (audit B27).
        launch.working_directory = crate::resolve_launch_working_directory(
            launch.working_directory.as_deref(),
            launch.config.working_dir_fallback,
        )
        .map(|directory| directory.to_string_lossy().into_owned());
        let environment = &mut launch.config.environment;
        environment.insert("TERMY_MUX_DIR".into(), root.to_string_lossy().into_owned());
        environment.insert("TERMY_MUX_PANE".into(), id.clone());
        let (tx, rx) = flume::bounded(256);
        let wake_tx = tx.clone();
        let notifier = Some(TerminalWakeupNotifier::new(move || {
            let _ = wake_tx.try_send(Work::Wake);
        }));
        let terminal = match &launch.environment {
            Some(environment) => Terminal::new_with_launch_in_environment(
                launch.size,
                launch.working_directory.as_deref(),
                notifier,
                launch.shell_integration.as_ref(),
                Some(&launch.config),
                launch.launch.as_ref(),
                environment,
            ),
            None => Terminal::new_with_launch_and_wakeup_notifier(
                launch.size,
                launch.working_directory.as_deref(),
                notifier,
                launch.shell_integration.as_ref(),
                Some(&launch.config),
                launch.launch.as_ref(),
            ),
        }?;
        let state = Arc::new(RemoteState::capture(&terminal));
        let shared = Arc::new(Mutex::new(Shared {
            info: PaneInfo {
                id,
                child_pid: terminal.child_pid(),
                title: None,
                working_directory: launch.working_directory,
                exited: false,
                shell: None,
            },
            launch: launch_config,
            input: InputTracker::default(),
            state,
            sticky_events: Vec::new(),
            program_status: None,
            subscribers: Vec::new(),
            alive: true,
        }));
        let worker_shared = Arc::clone(&shared);
        let exit_hook = Arc::new(Mutex::new(None));
        let worker_exit_hook = Arc::clone(&exit_hook);
        let worker = std::thread::Builder::new()
            .name("termy-session".into())
            .spawn(move || run(terminal, rx, worker_shared, worker_exit_hook, host_timeout))?;
        Ok(Self {
            tx,
            shared,
            exit_hook,
            worker: Arc::new(Mutex::new(Some(worker))),
        })
    }

    /// Runs `hook` once the pane's process exits; at once if it already has.
    pub(crate) fn on_exit(&self, hook: impl FnOnce(bool) + Send + 'static) {
        // Lock the slot before reading `exited`: the worker marks the pane
        // exited before it takes the slot, so one of the two runs the hook.
        let mut slot = lock_recovering(&self.exit_hook);
        let shared = lock_recovering(&self.shared);
        if shared.info.exited {
            let attached = is_attached(&shared);
            drop((shared, slot));
            hook(attached);
        } else {
            *slot = Some(Box::new(hook));
        }
    }

    pub(crate) fn info(&self) -> PaneInfo {
        lock_recovering(&self.shared).info.clone()
    }

    pub(crate) fn launch_config(&self) -> LaunchConfig {
        self.shared.lock().unwrap().launch.clone()
    }

    /// Queues work and waits for its reply, both within one `REQUEST_TIMEOUT`
    /// so the host answers before the client's response timeout.
    fn call<T>(
        &self,
        work: impl FnOnce(Sender<T>) -> Work,
        what: &'static str,
    ) -> anyhow::Result<T> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let (tx, rx) = flume::bounded(1);
        self.tx.send_deadline(work(tx), deadline)?;
        rx.recv_deadline(deadline).context(what)
    }

    pub(crate) fn refresh(&self) -> anyhow::Result<Arc<RemoteState>> {
        self.call(Work::Refresh, "refresh terminal session")
    }

    /// The current state, without publishing it to subscribers.
    pub(crate) fn snapshot(&self) -> anyhow::Result<Arc<RemoteState>> {
        self.call(Work::Snapshot, "terminal session did not answer")
    }

    pub(crate) fn size(&self) -> anyhow::Result<TerminalSize> {
        self.call(Work::Size, "terminal session did not answer")
    }

    pub(crate) fn command(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
        self.call(
            |tx| Work::Command(command, tx),
            "terminal session did not answer",
        )
    }

    /// Runs a viewport command and, when it changes the view, publishes that
    /// frame before replying. The client waits for the returned generation
    /// instead of attaching again for a second full state.
    pub(crate) fn viewport_command(&self, command: RemoteCommand) -> anyhow::Result<(bool, u64)> {
        self.call(
            |tx| Work::Viewport(command, tx),
            "terminal session did not answer",
        )
    }

    /// Ends the pane: its subscribers get `Exit`, then its process is
    /// terminated. Fails only when the worker did not take the request in
    /// time, so the pane still runs; a worker that already stopped needs
    /// nothing more.
    pub(crate) fn close(&self) -> anyhow::Result<()> {
        match self.tx.send_timeout(Work::Close, REQUEST_TIMEOUT) {
            Ok(()) | Err(flume::SendTimeoutError::Disconnected(_)) => Ok(()),
            Err(flume::SendTimeoutError::Timeout(_)) => {
                bail!("terminal session did not answer the close request")
            }
        }
    }

    /// Closes the pane for a host that is about to exit: the worker ends
    /// the program itself and waits for it (audit L1), since a hang-up
    /// escalation timer would die with the host. See [`Self::wait_closed`].
    pub(crate) fn shutdown(&self) {
        let _ = self.tx.send_timeout(Work::Shutdown, REQUEST_TIMEOUT);
    }

    /// Waits until the worker has dropped the terminal after [`Self::close`]
    /// or [`Self::shutdown`]. False if `deadline` passed first.
    pub(crate) fn wait_closed(&self, deadline: Instant) -> bool {
        let mut worker = lock_recovering(&self.worker);
        let Some(handle) = worker.as_ref() else {
            return true;
        };
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = worker.take().map(JoinHandle::join);
        true
    }

    pub(crate) fn subscribe(&self, host_capable: bool) -> anyhow::Result<Arc<Subscription>> {
        // The worker ends every subscription it knows of when it stops; one
        // registered after that would never get its `Exit`.
        let ensure_alive = |shared: &Shared| {
            if shared.alive {
                Ok(())
            } else {
                Err(anyhow::Error::new(PaneNotFound(format!(
                    "terminal session no longer exists: {}",
                    shared.info.id
                ))))
            }
        };
        ensure_alive(&lock_recovering(&self.shared))?;
        // A worker that stopped meanwhile fails the refresh: the pane ended.
        self.refresh()
            .or_else(|error| ensure_alive(&lock_recovering(&self.shared)).and(Err(error)))?;
        let mut shared = lock_recovering(&self.shared);
        ensure_alive(&shared)?;
        let mut replay = replay_seed(
            &shared.sticky_events,
            shared.program_status.as_deref(),
            SystemTime::now(),
        );
        if shared.info.exited {
            replay.push(TerminalEvent::Exit);
        }
        let subscription = Arc::new(Subscription {
            pending: Mutex::new(Pending {
                state: true,
                replay,
                ..Pending::default()
            }),
            ready: Condvar::new(),
            shared: Arc::downgrade(&self.shared),
            host_capable,
            worker: self.tx.downgrade(),
        });
        shared.subscribers.push(Arc::downgrade(&subscription));
        drop(shared);
        // Output can arrive between refresh and registration while the worker
        // still sees no subscribers. Wake it to publish that pending frame.
        let _ = self.tx.try_send(Work::Wake);
        Ok(subscription)
    }

    /// Hands the answer to a clipboard request to the worker, which writes
    /// the reply unless the request was already denied.
    pub(crate) fn host_reply(&self, id: u64, reply: RemoteHostReply) -> anyhow::Result<()> {
        self.tx
            .send_timeout(Work::HostReply(id, reply), REQUEST_TIMEOUT)
            .context("terminal session did not answer")
    }
}

impl Shared {
    /// Whether a subscriber is attached that is not paused, so frames are
    /// worth capturing.
    fn wants_frames(&self) -> bool {
        self.subscribers
            .iter()
            .filter_map(Weak::upgrade)
            .any(|subscriber| subscriber.wants_frames())
    }
}

/// Whether the shell phase says a command is running.
fn command_running(sticky: &[(TerminalEvent, SystemTime)]) -> bool {
    sticky
        .iter()
        .any(|(event, _)| matches!(event, TerminalEvent::ShellCommandExecuting))
}

/// Whether activity reported at `since` is stale; see [`STALE_ACTIVITY_AFTER`].
/// A clock set back makes nothing stale.
fn expired(running: bool, since: SystemTime, now: SystemTime) -> bool {
    !running
        && now
            .duration_since(since)
            .is_ok_and(|age| age >= STALE_ACTIVITY_AFTER)
}

/// The records of a status snapshot that are not stale working records.
fn current_status(
    status: &[(ProgramStatusRecord, SystemTime)],
    running: bool,
    now: SystemTime,
) -> Vec<ProgramStatusRecord> {
    status
        .iter()
        .filter(|(record, since)| {
            record.state != ProgramState::Working || !expired(running, *since, now)
        })
        .map(|(record, _)| record.clone())
        .collect()
}

/// When each record of the new snapshot `new` last changed. The engine
/// resends the whole snapshot on every report, least recently updated first,
/// and moves a reported record to the end. So the records that were not
/// reported are the longest prefix of `new` found unchanged and in order in
/// `old`; they keep their times. When nothing else changed, the last record
/// was reported again unchanged. Several identical reports of different
/// records in one drain refresh only the last of them.
fn status_times(
    old: &[(ProgramStatusRecord, SystemTime)],
    new: Vec<ProgramStatusRecord>,
    now: SystemTime,
) -> Vec<(ProgramStatusRecord, SystemTime)> {
    let mut from = 0;
    let mut unchanged = 0;
    let mut times = Vec::with_capacity(new.len());
    for record in &new {
        let kept = old[from..]
            .iter()
            .position(|(previous, _)| previous == record)
            .filter(|_| unchanged == times.len());
        match kept {
            Some(offset) => {
                times.push(old[from + offset].1);
                from += offset + 1;
                unchanged += 1;
            }
            None => times.push(now),
        }
    }
    if unchanged == new.len()
        && new.len() == old.len()
        && let Some(last) = times.last_mut()
    {
        *last = now;
    }
    new.into_iter().zip(times).collect()
}

/// The state a new subscription starts from, without stale progress and
/// working records.
fn replay_seed(
    sticky: &[(TerminalEvent, SystemTime)],
    status: Option<&[(ProgramStatusRecord, SystemTime)]>,
    now: SystemTime,
) -> Vec<TerminalEvent> {
    let running = command_running(sticky);
    let mut replay: Vec<_> = sticky
        .iter()
        .filter(|(event, since)| {
            !matches!(event, TerminalEvent::Progress(_)) || !expired(running, *since, now)
        })
        .map(|(event, _)| event.clone())
        .collect();
    if let Some(status) = status {
        replay.push(TerminalEvent::ProgramStatus(current_status(
            status, running, now,
        )));
    }
    replay
}

fn publish(shared: &mut Shared, state: Option<Arc<RemoteState>>, events: Vec<TerminalEvent>) {
    // A capture that matches the published state (a wakeup or a command
    // that changed nothing on screen) is not sent again (audit I8).
    let state = state.filter(|state| !state.same_frame(&shared.state));
    let changed = state.is_some();
    if let Some(state) = state {
        shared.state = state;
    }
    let now = SystemTime::now();
    let mut events: Vec<_> = events
        .into_iter()
        .filter(|event| !matches!(event, TerminalEvent::Wakeup))
        .collect();
    for event in &mut events {
        follow_shell(shared, event);
        match event {
            TerminalEvent::Title(title) => shared.info.title = Some(title.clone()),
            TerminalEvent::ResetTitle => shared.info.title = None,
            TerminalEvent::WorkingDirectory(cwd) => {
                shared.info.working_directory = Some(cwd.clone());
            }
            TerminalEvent::Exit => shared.info.exited = true,
            // A prompt means the program that reported progress is gone.
            TerminalEvent::ShellPromptStart => shared
                .sticky_events
                .retain(|(old, _)| !matches!(old, TerminalEvent::Progress(_))),
            // The command no longer runs. The finish itself is not state.
            TerminalEvent::ShellCommandFinished(_) => shared
                .sticky_events
                .retain(|(old, _)| sticky_key(old) != Some(3)),
            // Clients get the snapshot without stale working records, live
            // too, so that they do not return with the next report.
            TerminalEvent::ProgramStatus(records) => {
                let old = shared.program_status.take().unwrap_or_default();
                let status = status_times(&old, std::mem::take(records), now);
                *records = current_status(&status, command_running(&shared.sticky_events), now);
                shared.program_status = Some(status);
            }
            _ => {}
        }
        if let Some(key) = sticky_key(event) {
            shared
                .sticky_events
                .retain(|(old, _)| sticky_key(old) != Some(key));
            shared.sticky_events.push((event.clone(), now));
        }
    }
    if !changed && events.is_empty() {
        // Nothing to deliver: waking every subscriber thread would only
        // send it back to sleep (audit B14).
        return;
    }
    shared.subscribers.retain(|weak| {
        let Some(subscriber) = weak.upgrade() else {
            return false;
        };
        let mut pending = subscriber.pending.lock().unwrap();
        // A paused view gets a frame captured for it when it resumes, so a
        // frame neither waits for it nor wakes it (audit I7).
        let mut wake = changed && !pending.paused;
        pending.state |= wake;
        for event in &events {
            if !coalesce_event(&mut pending.events, event) {
                continue;
            }
            wake = true;
            // Coalescing bounds the queue by the number of event kinds and a
            // few prompt cycles; this only guards against a future kind that
            // is neither merged nor capped, so a slow client cannot retain an
            // unbounded stream.
            if pending.events.len() >= 256 {
                pending.closed = true;
                break;
            }
            pending.events.push(event.clone());
        }
        if wake || pending.closed {
            subscriber.ready.notify_one();
        }
        !pending.closed
    });
}

/// Follows the command that the pane's latest input started; see
/// [`InputCommand`] for the rules.
#[derive(Default)]
struct InputTracker {
    command: Option<InputCommand>,
    /// A prompt showed when the input arrived, or appeared since.
    prompt: bool,
    /// The last command lifecycle event was a prompt (`A` or `B`).
    at_prompt: bool,
}

/// Notes input that reached the program. The worker publishes the events
/// of everything the program printed before, first, so that no earlier
/// event is taken for one that followed the input.
fn note_input(shared: &mut Shared) {
    let running = shared.info.shell.is_some_and(|shell| shell.running);
    let input = &mut shared.input;
    input.command = Some(InputCommand::Pending);
    input.prompt = input.at_prompt && !running;
    if let Some(shell) = &mut shared.info.shell {
        shell.input_command = input.command;
    }
}

/// Follows the shell's command lifecycle (audit I14).
fn follow_shell(shared: &mut Shared, event: &TerminalEvent) {
    if !matches!(
        event,
        TerminalEvent::ShellPromptStart
            | TerminalEvent::ShellCommandStart
            | TerminalEvent::ShellCommandExecuting
            | TerminalEvent::ShellCommandFinished(_)
    ) {
        return;
    }
    let shell = shared.info.shell.get_or_insert_default();
    let input = &mut shared.input;
    let waiting = matches!(
        input.command,
        Some(InputCommand::Pending | InputCommand::Running)
    );
    match event {
        TerminalEvent::ShellCommandExecuting => {
            shell.running = true;
            shell.commands_started += 1;
            input.at_prompt = false;
            if waiting {
                input.command = Some(InputCommand::Running);
            }
        }
        TerminalEvent::ShellCommandFinished(code) => {
            shell.running = false;
            shell.commands_finished += 1;
            shell.exit_code = *code;
            input.at_prompt = false;
            // Only after the input's own start, or for a shell that reports
            // no starts at all, after a prompt.
            let started = input.command == Some(InputCommand::Running);
            if waiting && (started || (input.prompt && shell.commands_started == 0)) {
                input.command = Some(InputCommand::Finished(*code));
            }
        }
        // A new prompt without a finish: the command line was abandoned.
        _ => {
            shell.running = false;
            input.at_prompt = true;
            if input.command == Some(InputCommand::Pending) {
                input.prompt = true;
            }
        }
    }
    shell.input_command = input.command;
}

/// Drains the engine's events and publishes them, without a frame. Returns
/// whether it found any, and whether more are pending.
fn publish_events(
    terminal: &mut Terminal,
    host: &mut HostBridge,
    shared: &Mutex<Shared>,
    exit_hook: &Mutex<Option<ExitHook>>,
) -> (bool, bool) {
    let (events, more) = terminal.drain_events(host);
    let found = !events.is_empty();
    let exited = events
        .iter()
        .any(|event| matches!(event, TerminalEvent::Exit));
    publish(&mut shared.lock().unwrap(), None, events);
    if exited {
        notify_exit(shared, exit_hook);
    }
    (found, more)
}

fn is_attached(shared: &Shared) -> bool {
    shared
        .subscribers
        .iter()
        .any(|weak| weak.strong_count() > 0)
}

fn notify_exit(shared: &Mutex<Shared>, exit_hook: &Mutex<Option<ExitHook>>) {
    let Some(hook) = exit_hook.lock().unwrap().take() else {
        return;
    };
    let attached = is_attached(&shared.lock().unwrap());
    hook(attached);
}

/// How long the worker may wait for work. A frame is captured at most once
/// per [`FRAME_INTERVAL`], so neither a dirty screen nor an animation deadline
/// inside that interval may wake it earlier: it would find nothing to do and
/// spin until the interval ends (audit B14).
fn worker_timeout(
    pending_events: bool,
    frame_wanted: bool,
    animation_deadline: Option<Instant>,
    last_frame: Instant,
    now: Instant,
) -> Duration {
    let next_frame = last_frame + FRAME_INTERVAL;
    if pending_events {
        Duration::ZERO
    } else if frame_wanted {
        next_frame.saturating_duration_since(now)
    } else if let Some(deadline) = animation_deadline {
        deadline.max(next_frame).saturating_duration_since(now)
    } else {
        Duration::from_secs(60)
    }
}

/// Locks `mutex` even when a panicking worker poisoned it, and clears the
/// poison so the host's other threads keep serving the pane's data.
pub(crate) fn lock_recovering<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        let guard = poisoned.into_inner();
        mutex.clear_poison();
        guard
    })
}

/// Ends the pane for its subscribers when the worker stops, however it stops:
/// closed by a client, at host shutdown, or by a panic (audit B10, I10). Each
/// subscriber gets `Exit` (or, at host shutdown, `Update::HostShutdown`)
/// before its subscription closes, the pane is listed as exited, and nothing
/// can subscribe any more. Runs while unwinding, so it must not panic: every
/// lock it takes recovers from poison.
struct WorkerExit {
    shared: Arc<Mutex<Shared>>,
    exit_hook: Arc<Mutex<Option<ExitHook>>>,
    /// Set when the host is shutting down: the pane ends with its host, not
    /// because its program exited, and views keep their tabs.
    host_stopping: bool,
}

impl Drop for WorkerExit {
    fn drop(&mut self) {
        let panicked = std::thread::panicking();
        let mut shared = lock_recovering(&self.shared);
        if panicked {
            log::error!(
                "terminal session {} stopped after a panic; ending it",
                shared.info.id
            );
        }
        // `publish` locks each subscriber's queue.
        for subscriber in shared.subscribers.iter().filter_map(Weak::upgrade) {
            drop(lock_recovering(&subscriber.pending));
        }
        shared.alive = false;
        if self.host_stopping {
            shared.info.exited = true;
            for subscriber in shared.subscribers.iter().filter_map(Weak::upgrade) {
                lock_recovering(&subscriber.pending).host_stopping = true;
            }
        } else if !shared.info.exited {
            publish(&mut shared, None, vec![TerminalEvent::Exit]);
        }
        drop(shared);
        // A panic ends the pane as if its process exited. A close does not
        // run the hook: whoever closes the pane updates the layout. The hook
        // runs on its own thread, where a panic of its own cannot abort.
        if panicked {
            let hook = lock_recovering(&self.exit_hook).take();
            if let Some(hook) = hook {
                let attached = is_attached(&lock_recovering(&self.shared));
                let _ = std::thread::Builder::new()
                    .name("termy-session-exit".into())
                    .spawn(move || hook(attached));
            }
        }
        let mut shared = lock_recovering(&self.shared);
        for subscriber in shared
            .subscribers
            .drain(..)
            .filter_map(|weak| weak.upgrade())
        {
            lock_recovering(&subscriber.pending).closed = true;
            subscriber.ready.notify_one();
        }
    }
}

fn run(
    mut terminal: Terminal,
    rx: Receiver<Work>,
    shared: Arc<Mutex<Shared>>,
    exit_hook: Arc<Mutex<Option<ExitHook>>>,
    host_timeout: Duration,
) {
    // Dropped before `terminal`: subscribers learn that the pane ended
    // before its process is terminated.
    let mut exit = WorkerExit {
        shared: Arc::clone(&shared),
        exit_hook: Arc::clone(&exit_hook),
        host_stopping: false,
    };
    let mut host = HostBridge {
        shared: Arc::clone(&shared),
        timeout: host_timeout,
        parked: Vec::new(),
    };
    let mut dirty = true;
    let mut pending_events = false;
    let mut last_frame = Instant::now() - FRAME_INTERVAL;
    loop {
        let timeout = {
            let state = shared.lock().unwrap();
            let attached = state.wants_frames();
            worker_timeout(
                pending_events,
                attached && dirty,
                attached.then_some(state.state.graphics_deadline).flatten(),
                last_frame,
                Instant::now(),
            )
        }
        .min(host.next_check());
        match rx.recv_timeout(timeout) {
            Ok(Work::Close) | Err(flume::RecvTimeoutError::Disconnected) => break,
            Ok(Work::Shutdown) => {
                exit.host_stopping = true;
                terminal.terminate_and_wait();
                break;
            }
            Ok(Work::Command(command, reply)) => {
                // Input changes the screen only once the program answers,
                // and that output marks the pane dirty itself (audit I8).
                let changes_state = !matches!(
                    command,
                    RemoteCommand::Write(_)
                        | RemoteCommand::WriteMotion(_)
                        | RemoteCommand::Snapshot
                        | RemoteCommand::Lines { .. }
                        | RemoteCommand::LineTexts { .. }
                        | RemoteCommand::Search { .. }
                        | RemoteCommand::HyperlinkAt { .. }
                        | RemoteCommand::LinkAt { .. }
                        | RemoteCommand::RetiredHyperlink
                        | RemoteCommand::RetiredLink
                        | RemoteCommand::Graphics
                );
                let input = matches!(command, RemoteCommand::Write(_));
                if input {
                    // What the program printed before the input comes first.
                    let (found, more) =
                        publish_events(&mut terminal, &mut host, &shared, &exit_hook);
                    dirty |= found || more;
                }
                let result = if input && shared.lock().unwrap().info.exited {
                    // The PTY may still accept bytes nobody reads (audit B24).
                    RemoteReply::InputRejected(InputRejection::Exited)
                } else {
                    crate::remote::execute(&mut terminal, command)
                };
                if matches!(result, RemoteReply::Written { .. }) {
                    note_input(&mut shared.lock().unwrap());
                }
                dirty |= changes_state && !matches!(result, RemoteReply::Changed(false));
                let _ = reply.try_send(result);
            }
            Ok(Work::Viewport(command, reply)) => {
                let changed = !matches!(
                    crate::remote::execute(&mut terminal, command),
                    RemoteReply::Changed(false)
                );
                let mut generation = shared.lock().unwrap().state.render.metadata.generation;
                if changed {
                    let state = Arc::new(RemoteState::capture(&terminal));
                    generation = state.render.metadata.generation;
                    publish(&mut shared.lock().unwrap(), Some(state), Vec::new());
                    last_frame = Instant::now();
                    dirty = false;
                }
                let _ = reply.try_send((changed, generation));
            }
            Ok(Work::Refresh(reply)) => {
                let state = Arc::new(RemoteState::capture(&terminal));
                publish(
                    &mut shared.lock().unwrap(),
                    Some(Arc::clone(&state)),
                    Vec::new(),
                );
                let _ = reply.try_send(state);
                last_frame = Instant::now();
                dirty = false;
            }
            Ok(Work::Snapshot(reply)) => {
                let _ = reply.try_send(Arc::new(RemoteState::capture(&terminal)));
            }
            Ok(Work::Size(reply)) => {
                let _ = reply.try_send(terminal.size());
            }
            Ok(Work::HostReply(id, reply)) => {
                if host.unpark(id) {
                    terminal.complete_clipboard_request(id, reply);
                }
            }
            Ok(Work::Wake) => dirty = true,
            Ok(Work::Resume(view)) => {
                let state = Arc::new(RemoteState::capture(&terminal));
                publish(&mut shared.lock().unwrap(), Some(state), Vec::new());
                last_frame = Instant::now();
                dirty = false;
                if let Some(view) = view.upgrade() {
                    view.resumed();
                }
            }
            #[cfg(all(test, unix))]
            Ok(Work::Panic) => panic!("injected terminal session panic"),
            Err(flume::RecvTimeoutError::Timeout) => {}
        }
        for id in host.expired() {
            terminal.deny_clipboard_request(id);
        }
        let (events, more) = terminal.drain_events(&mut host);
        pending_events = more;
        dirty |= !events.is_empty() || more;
        let (attached, animation_due) = {
            let state = shared.lock().unwrap();
            let attached = state.wants_frames();
            let deadline = state.state.graphics_deadline;
            drop(state);
            (
                attached,
                deadline.is_some_and(|deadline| deadline <= Instant::now()),
            )
        };
        // Capture the frame without blocking pane lookups and subscribers.
        let update =
            if attached && (dirty || animation_due) && last_frame.elapsed() >= FRAME_INTERVAL {
                last_frame = Instant::now();
                dirty = false;
                Some(Arc::new(RemoteState::capture(&terminal)))
            } else {
                None
            };
        let exited = events
            .iter()
            .any(|event| matches!(event, TerminalEvent::Exit));
        publish(&mut shared.lock().unwrap(), update, events);
        if exited {
            notify_exit(&shared, &exit_hook);
        }
    }
    // Dropping the authoritative terminal (after `exit`) closes its PTY and
    // terminates the shell. App/client disconnect never reaches this path.
}

/// A clipboard request sent to a subscriber, waiting for its answer.
struct Parked {
    id: u64,
    deadline: Instant,
    subscriber: Weak<Subscription>,
}

/// Sends the engine's clipboard requests to an attached view and lets the
/// worker carry on: the engine writes the reply when [`Work::HostReply`]
/// arrives, or denies the request at its deadline or when the view detaches.
struct HostBridge {
    shared: Arc<Mutex<Shared>>,
    timeout: Duration,
    parked: Vec<Parked>,
}

impl HostBridge {
    /// The view that answers clipboard requests: the newest open
    /// host-capable subscription. This assumes, and nothing enforces, that
    /// one view shows a pane at a time: a desktop view subscribes when it
    /// shows the pane, and an older subscription that is still open is
    /// usually a view being replaced (a reopened window or a reattach)
    /// whose connection has not closed yet, so the newest one is the view in
    /// front of the user. Should two views ever show one pane, the newer
    /// one is asked. Subscribers that are not host-capable, like scripts,
    /// never receive requests.
    fn clipboard_host(&self) -> Option<Arc<Subscription>> {
        let shared = self.shared.lock().unwrap();
        shared
            .subscribers
            .iter()
            .rev()
            .filter_map(Weak::upgrade)
            .find(|subscriber| {
                subscriber.host_capable && !subscriber.pending.lock().unwrap().closed
            })
    }

    /// Takes a parked request off the list; false when it was already
    /// denied, or never sent.
    fn unpark(&mut self, id: u64) -> bool {
        let Some(index) = self.parked.iter().position(|parked| parked.id == id) else {
            return false;
        };
        self.parked.remove(index);
        true
    }

    /// Requests to deny: past their deadline, or held by a subscriber that
    /// has detached.
    fn expired(&mut self) -> Vec<u64> {
        let now = Instant::now();
        let mut expired = Vec::new();
        self.parked.retain(|parked| {
            let attached = parked
                .subscriber
                .upgrade()
                .is_some_and(|subscriber| !subscriber.pending.lock().unwrap().closed);
            if attached && parked.deadline > now {
                return true;
            }
            expired.push(parked.id);
            false
        });
        expired
    }

    /// How long the worker may sleep before [`Self::expired`] has work.
    fn next_check(&self) -> Duration {
        self.parked
            .iter()
            .map(|parked| parked.deadline.saturating_duration_since(Instant::now()))
            .min()
            .map_or(Duration::MAX, |due| due.min(HOST_CHECK_INTERVAL))
    }
}

impl TerminalReplyHost for HostBridge {
    fn defer_clipboard_request(
        &mut self,
        id: u64,
        request: RemoteHostRequest,
    ) -> Result<(), RemoteHostRequest> {
        // Without a view to ask, the methods below deny the request now.
        let Some(subscriber) = self.clipboard_host() else {
            return Err(request);
        };
        let deadline = Instant::now() + self.timeout;
        let mut pending = subscriber.pending.lock().unwrap();
        if pending.closed {
            return Err(request);
        }
        pending.host.push_back((id, request, deadline));
        drop(pending);
        subscriber.ready.notify_one();
        self.parked.push(Parked {
            id,
            deadline,
            subscriber: Arc::downgrade(&subscriber),
        });
        Ok(())
    }

    fn load_clipboard(&mut self, _target: TerminalClipboardTarget) -> Option<String> {
        // An empty clipboard, as the desktop's refusal sends, so the
        // program stops waiting.
        Some(String::new())
    }
    fn read_clipboard(
        &mut self,
        _request: TerminalClipboardReadRequest,
    ) -> TerminalClipboardReadResult {
        TerminalClipboardReadResult::Denied
    }
    fn write_clipboard(
        &mut self,
        _request: TerminalClipboardWriteRequest,
    ) -> TerminalClipboardWriteResult {
        TerminalClipboardWriteResult::Denied
    }
}

#[cfg(test)]
mod tests;
