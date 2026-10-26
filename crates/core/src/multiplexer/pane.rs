use crate::multiplexer::protocol::*;
use crate::{remote::*, *};
use anyhow::Context;
use flume::{Receiver, Sender};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Condvar, Mutex, Weak},
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
        self.pending.lock().unwrap().closed = true;
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
    pub(crate) fn next(&self) -> Option<Update> {
        let mut pending = self.pending.lock().unwrap();
        loop {
            if pending.closed {
                return None;
            }
            if let Some(paused) = pending.acknowledge.take() {
                return Some(Update::Paused(paused));
            }
            if let Some((id, request, deadline)) = pending.host.pop_front() {
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
            if pending.state && !pending.paused {
                pending.state = false;
                // Never acquire Shared with Pending held: the worker publishes
                // in the opposite order.
                drop(pending);
                let shared = self.shared.upgrade()?;
                return Some(Update::State(Arc::clone(&shared.lock().unwrap().state)));
            }
            pending = self.ready.wait(pending).unwrap();
        }
    }
}

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
        let mut slot = self.exit_hook.lock().unwrap();
        let shared = self.shared.lock().unwrap();
        if shared.info.exited {
            let attached = is_attached(&shared);
            drop((shared, slot));
            hook(attached);
        } else {
            *slot = Some(Box::new(hook));
        }
    }

    pub(crate) fn info(&self) -> PaneInfo {
        self.shared.lock().unwrap().info.clone()
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

    pub(crate) fn close(&self) {
        let _ = self.tx.send_timeout(Work::Close, REQUEST_TIMEOUT);
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
        let mut worker = self.worker.lock().unwrap();
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
        self.refresh()?;
        let mut shared = self.shared.lock().unwrap();
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

fn run(
    mut terminal: Terminal,
    rx: Receiver<Work>,
    shared: Arc<Mutex<Shared>>,
    exit_hook: Arc<Mutex<Option<ExitHook>>>,
    host_timeout: Duration,
) {
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
    let mut state = shared.lock().unwrap();
    for subscriber in state
        .subscribers
        .drain(..)
        .filter_map(|weak| weak.upgrade())
    {
        subscriber.pending.lock().unwrap().closed = true;
        subscriber.ready.notify_one();
    }
    // Dropping the authoritative terminal closes its PTY and terminates the
    // shell. App/client disconnect never reaches this path.
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
mod tests {
    use super::*;

    #[test]
    fn subscribing_wakes_a_worker_that_refreshed_while_detached() {
        let terminal = Terminal::new_display(TerminalSize::default(), None);
        let state = Arc::new(RemoteState::capture(&terminal));
        let shared = Arc::new(Mutex::new(Shared {
            info: PaneInfo {
                id: "test-pane".into(),
                child_pid: None,
                title: None,
                working_directory: None,
                exited: false,
                shell: None,
            },
            launch: LaunchConfig {
                config: TerminalRuntimeConfig::default(),
                shell_integration: None,
            },
            input: InputTracker::default(),
            state: Arc::clone(&state),
            sticky_events: Vec::new(),
            program_status: None,
            subscribers: Vec::new(),
        }));
        let (tx, rx) = flume::bounded(256);
        let pane = Pane {
            tx,
            shared,
            exit_hook: Arc::default(),
            worker: Arc::new(Mutex::new(None)),
        };
        let worker = std::thread::spawn(move || {
            let Work::Refresh(reply) = rx.recv_timeout(REQUEST_TIMEOUT).unwrap() else {
                panic!("subscription must first refresh the pane");
            };
            reply.send(state).unwrap();
            // The worker has no subscribers yet and can go idle. Registration
            // must wake it even if no more terminal output arrives afterward.
            assert!(matches!(rx.recv_timeout(REQUEST_TIMEOUT), Ok(Work::Wake)));
        });
        let subscription = pane.subscribe(true).unwrap();
        worker.join().unwrap();
        assert!(matches!(subscription.next(), Some(Update::State(_))));
    }

    #[test]
    fn bursts_of_bells_and_clipboard_writes_coalesce_for_slow_clients() {
        let terminal = Terminal::new_display(TerminalSize::default(), None);
        let shared = Arc::new(Mutex::new(Shared {
            info: PaneInfo {
                id: "test-pane".into(),
                child_pid: None,
                title: None,
                working_directory: None,
                exited: false,
                shell: None,
            },
            launch: LaunchConfig {
                config: TerminalRuntimeConfig::default(),
                shell_integration: None,
            },
            input: InputTracker::default(),
            state: Arc::new(RemoteState::capture(&terminal)),
            sticky_events: Vec::new(),
            program_status: None,
            subscribers: Vec::new(),
        }));
        let subscription = Arc::new(Subscription {
            pending: Mutex::new(Pending::default()),
            ready: Condvar::new(),
            shared: Arc::downgrade(&shared),
            host_capable: true,
            worker: flume::bounded(1).0.downgrade(),
        });
        let mut state = shared.lock().unwrap();
        state.subscribers.push(Arc::downgrade(&subscription));

        // `printf '\a%.0s' {1..300}` arrives as one drain; a client that has
        // not read yet then sees more bursts, clipboard writes and dropped input.
        publish(&mut state, None, vec![TerminalEvent::Bell; 300]);
        for round in 0..300 {
            publish(
                &mut state,
                None,
                vec![
                    TerminalEvent::Bell,
                    TerminalEvent::InputDropped,
                    TerminalEvent::ClipboardStore(format!("copy {round}")),
                ],
            );
        }
        assert_eq!(state.subscribers.len(), 1, "subscriber must stay attached");
        drop(state);

        let Some(Update::Events(events)) = subscription.next() else {
            panic!("expected coalesced events");
        };
        assert_eq!(events.len(), 3, "{events:?}");
        assert!(matches!(events[0], TerminalEvent::Bell));
        assert!(matches!(events[1], TerminalEvent::InputDropped));
        assert!(matches!(&events[2], TerminalEvent::ClipboardStore(text) if text == "copy 299"));
    }

    /// A pane whose worker answers refreshes until the pane is dropped.
    fn detached_pane() -> Pane {
        let terminal = Terminal::new_display(TerminalSize::default(), None);
        let state = Arc::new(RemoteState::capture(&terminal));
        let shared = Arc::new(Mutex::new(Shared {
            info: PaneInfo {
                id: "test-pane".into(),
                child_pid: None,
                title: None,
                working_directory: None,
                exited: false,
                shell: None,
            },
            launch: LaunchConfig {
                config: TerminalRuntimeConfig::default(),
                shell_integration: None,
            },
            input: InputTracker::default(),
            state: Arc::clone(&state),
            sticky_events: Vec::new(),
            program_status: None,
            subscribers: Vec::new(),
        }));
        let (tx, rx) = flume::bounded(256);
        std::thread::spawn(move || {
            while let Ok(work) = rx.recv() {
                if let Work::Refresh(reply) = work {
                    let _ = reply.send(Arc::clone(&state));
                }
            }
        });
        Pane {
            tx,
            shared,
            exit_hook: Arc::default(),
            worker: Arc::new(Mutex::new(None)),
        }
    }

    fn publish_to(pane: &Pane, events: Vec<TerminalEvent>) {
        publish(&mut pane.shared.lock().unwrap(), None, events);
    }

    /// The next update other than a frame.
    fn next_events(subscription: &Subscription) -> Update {
        loop {
            match subscription.next() {
                Some(Update::State(_)) => {}
                Some(update) => return update,
                None => panic!("subscription closed"),
            }
        }
    }

    #[test]
    fn command_finish_and_prompt_in_one_drain_reach_a_subscriber_in_order() {
        let pane = detached_pane();
        let subscription = pane.subscribe(true).unwrap();
        publish_to(
            &pane,
            vec![
                TerminalEvent::ShellCommandFinished(Some(1)),
                TerminalEvent::ShellPromptStart,
            ],
        );
        let Update::Events(events) = next_events(&subscription) else {
            panic!("expected live events");
        };
        assert!(
            matches!(
                events.as_slice(),
                [
                    TerminalEvent::ShellCommandFinished(Some(1)),
                    TerminalEvent::ShellPromptStart
                ]
            ),
            "{events:?}"
        );
    }

    #[test]
    fn a_hidden_view_stays_subscribed_through_a_flood_of_prompts() {
        let pane = detached_pane();
        let subscription = pane.subscribe(true).unwrap();
        for code in 0..10_000 {
            publish_to(
                &pane,
                vec![
                    TerminalEvent::ShellCommandExecuting,
                    TerminalEvent::ShellCommandFinished(Some(code)),
                    TerminalEvent::ShellPromptStart,
                ],
            );
        }
        assert_eq!(pane.shared.lock().unwrap().subscribers.len(), 1);
        let Update::Events(events) = next_events(&subscription) else {
            panic!("expected live events");
        };
        assert!(events.len() <= 64, "{}", events.len());
        assert!(matches!(
            &events[events.len() - 2..],
            [
                TerminalEvent::ShellCommandFinished(Some(9_999)),
                TerminalEvent::ShellPromptStart
            ]
        ));
    }

    #[test]
    fn reattach_replays_current_state_as_a_marked_seed() {
        let pane = detached_pane();
        publish_to(
            &pane,
            vec![
                TerminalEvent::Title("vim".into()),
                TerminalEvent::Bell,
                TerminalEvent::ShellCommandExecuting,
                TerminalEvent::ShellCommandFinished(Some(2)),
                TerminalEvent::ShellPromptStart,
                TerminalEvent::ShellCommandStart,
                TerminalEvent::ShellCommandExecuting,
                TerminalEvent::Progress(ProgressState::InProgress(40)),
            ],
        );
        let subscription = pane.subscribe(true).unwrap();
        publish_to(&pane, vec![TerminalEvent::Bell]);
        let Update::Replay(replay) = next_events(&subscription) else {
            panic!("the seed must be marked as a replay");
        };
        assert!(
            matches!(
                replay.as_slice(),
                [
                    TerminalEvent::Title(_),
                    TerminalEvent::ShellCommandExecuting,
                    TerminalEvent::Progress(ProgressState::InProgress(40)),
                ]
            ),
            "{replay:?}"
        );
        let Update::Events(live) = next_events(&subscription) else {
            panic!("expected live events after the replay");
        };
        assert!(matches!(live.as_slice(), [TerminalEvent::Bell]));
    }

    #[test]
    fn prompt_start_clears_the_replayed_progress_and_shell_phase_follows_finish() {
        let pane = detached_pane();
        publish_to(
            &pane,
            vec![
                TerminalEvent::ShellCommandExecuting,
                TerminalEvent::Progress(ProgressState::Indeterminate),
                TerminalEvent::ShellCommandFinished(None),
            ],
        );
        let replay = seed(&pane, SystemTime::now());
        // The command finished: neither it nor its start is replayed.
        assert!(
            matches!(
                replay.as_slice(),
                [TerminalEvent::Progress(ProgressState::Indeterminate)]
            ),
            "{replay:?}"
        );
        publish_to(&pane, vec![TerminalEvent::ShellPromptStart]);
        let replay = seed(&pane, SystemTime::now());
        assert!(
            matches!(replay.as_slice(), [TerminalEvent::ShellPromptStart]),
            "{replay:?}"
        );
    }

    fn host_requests(subscription: &Subscription, count: usize) -> Vec<u64> {
        (0..count)
            .map(|_| match next_events(subscription) {
                Update::HostRequest { id, .. } => id,
                _ => panic!("expected a clipboard request"),
            })
            .collect()
    }

    #[test]
    fn clipboard_requests_go_to_the_newest_view_and_are_denied_when_it_leaves() {
        let pane = detached_pane();
        let older_view = pane.subscribe(true).unwrap();
        let view = pane.subscribe(true).unwrap();
        // A script subscribes last; it cannot ask the user.
        let script = pane.subscribe(false).unwrap();
        let mut bridge = HostBridge {
            shared: Arc::clone(&pane.shared),
            timeout: HOST_REQUEST_TIMEOUT,
            parked: Vec::new(),
        };
        let load = || RemoteHostRequest::Load(TerminalClipboardTarget::Clipboard);
        for id in [7, 8] {
            assert!(bridge.defer_clipboard_request(id, load()).is_ok());
        }
        assert!(script.pending.lock().unwrap().host.is_empty());
        assert!(older_view.pending.lock().unwrap().host.is_empty());
        // First in, first out.
        assert_eq!(host_requests(&view, 2), [7, 8]);
        assert!(bridge.expired().is_empty());
        assert!(bridge.next_check() <= HOST_CHECK_INTERVAL);

        // The view leaves: its requests are denied, and new ones go to the
        // older view that is still open.
        view.close();
        assert_eq!(bridge.expired(), [7, 8]);
        assert!(!bridge.unpark(7), "a late answer must not be written");
        assert!(bridge.defer_clipboard_request(9, load()).is_ok());
        assert_eq!(host_requests(&older_view, 1), [9]);
        older_view.close();
        assert!(
            bridge.defer_clipboard_request(10, load()).is_err(),
            "with no view to ask, the request is denied at once"
        );
        assert!(bridge.unpark(9));
        assert_eq!(bridge.next_check(), Duration::MAX);
    }

    #[test]
    fn expired_clipboard_requests_are_not_sent() {
        let pane = detached_pane();
        let view = pane.subscribe(true).unwrap();
        let now = Instant::now();
        let load = RemoteHostRequest::Load(TerminalClipboardTarget::Clipboard);
        let mut pending = view.pending.lock().unwrap();
        for (id, deadline) in [
            (1, now + HOST_REQUEST_TIMEOUT),
            (2, now),
            (3, now + HOST_REQUEST_TIMEOUT),
        ] {
            pending.host.push_back((id, load.clone(), deadline));
        }
        drop(pending);
        assert_eq!(host_requests(&view, 2), [1, 3]);
    }

    /// Audit B7: the worker keeps running while a clipboard request waits,
    /// and denies it at its deadline when nobody answers.
    #[cfg(unix)]
    #[test]
    fn an_unanswered_clipboard_request_is_denied_at_its_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let reply = temp.path().join("reply");
        let expected = b"\x1b]5522;type=read:status=EPERM:id=late\x1b\\";
        let script = format!(
            "stty raw -echo; dd bs=1 count=1 >/dev/null 2>/dev/null; \
             printf '\\033]5522;type=read:id=late;dGV4dC9wbGFpbg==\\033\\\\'; \
             dd bs=1 count={} > '{}' 2>/dev/null; sleep 30",
            expected.len(),
            reply.display()
        );
        let timeout = Duration::from_millis(500);
        let pane = Pane::spawn(
            PaneLaunch {
                size: TerminalSize::default(),
                working_directory: None,
                shell_integration: None,
                config: TerminalRuntimeConfig::default(),
                launch: Some(TerminalLaunch::Program {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), script],
                }),
                environment: None,
            },
            &std::env::temp_dir(),
            timeout,
        )
        .unwrap();
        let view = pane.subscribe(true).unwrap();
        // Before the request exists, so its deadline cannot precede this.
        let sent = Instant::now();
        pane.command(RemoteCommand::Write(b"x".to_vec())).unwrap();
        let [id] = host_requests(&view, 1)[..] else {
            unreachable!()
        };
        // Other work is still answered meanwhile.
        pane.snapshot().unwrap();
        let deadline = sent + Duration::from_secs(5);
        while std::fs::read(&reply).map_or(true, |bytes| bytes.len() < expected.len()) {
            assert!(Instant::now() < deadline, "the request was never denied");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(sent.elapsed() >= timeout);
        assert_eq!(std::fs::read(&reply).unwrap(), expected);
        // A late answer is dropped.
        let late = RemoteHostReply::Read(TerminalClipboardReadResult::Denied);
        pane.host_reply(id, late).unwrap();
        pane.close();
    }

    /// Audit B14: an animation deadline inside the frame interval, or one
    /// already past while the interval runs, must not make the worker poll.
    #[test]
    fn animation_deadlines_inside_the_frame_interval_do_not_spin_the_worker() {
        let now = Instant::now();
        let just_captured = now;
        for deadline in [
            now - Duration::from_millis(1),
            now + Duration::from_millis(2),
        ] {
            assert_eq!(
                worker_timeout(false, false, Some(deadline), just_captured, now),
                FRAME_INTERVAL
            );
        }
        // A later deadline is kept, and a due frame is not delayed.
        let later = now + FRAME_INTERVAL * 3;
        assert_eq!(
            worker_timeout(false, false, Some(later), just_captured, now),
            FRAME_INTERVAL * 3
        );
        let long_ago = now - FRAME_INTERVAL * 2;
        assert_eq!(
            worker_timeout(false, false, Some(now), long_ago, now),
            Duration::ZERO
        );
        assert_eq!(
            worker_timeout(false, true, None, just_captured, now),
            FRAME_INTERVAL
        );
        assert_eq!(
            worker_timeout(true, true, None, just_captured, now),
            Duration::ZERO
        );
    }

    /// A pane running `script` with `/bin/sh -c`.
    #[cfg(unix)]
    fn script_pane(script: &str) -> Pane {
        Pane::spawn(
            PaneLaunch {
                size: TerminalSize::default(),
                working_directory: None,
                shell_integration: None,
                config: TerminalRuntimeConfig::default(),
                launch: Some(TerminalLaunch::Program {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), script.into()],
                }),
                environment: None,
            },
            &std::env::temp_dir(),
            HOST_REQUEST_TIMEOUT,
        )
        .unwrap()
    }

    fn screen(state: &RemoteState) -> String {
        state
            .render
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect()
    }

    /// The next frame, skipping events.
    fn next_frame(view: &Subscription) -> Arc<RemoteState> {
        loop {
            match view.next() {
                Some(Update::State(state)) => return state,
                Some(_) => {}
                None => panic!("subscription closed"),
            }
        }
    }

    /// Audit I7: a paused view gets events but no frames; on resume it gets
    /// one frame captured then, never the one from before it was hidden.
    #[cfg(unix)]
    #[test]
    fn a_paused_view_gets_events_and_only_a_fresh_frame_on_resume() {
        let pane =
            script_pane("stty -echo; printf A; read x; printf 'B\\033]2;hidden\\007'; sleep 30");
        let view = pane.subscribe(true).unwrap();
        while !screen(&next_frame(&view)).contains('A') {}
        view.set_paused(true);
        assert!(matches!(view.next(), Some(Update::Paused(true))));
        assert!(!pane.shared.lock().unwrap().wants_frames());
        pane.command(RemoteCommand::Write(b"x\n".to_vec())).unwrap();
        loop {
            match view.next() {
                Some(Update::Events(events))
                    if events
                        .iter()
                        .any(|event| matches!(event, TerminalEvent::Title(_))) =>
                {
                    break;
                }
                Some(Update::State(_)) => panic!("a paused view received a frame"),
                Some(_) => {}
                None => panic!("subscription closed"),
            }
        }
        assert!(!view.pending.lock().unwrap().state);
        view.set_paused(false);
        assert!(matches!(view.next(), Some(Update::Paused(false))));
        let Some(Update::State(state)) = view.next() else {
            panic!("expected the frame captured on resume");
        };
        assert!(screen(&state).contains("AB"), "{}", screen(&state));
        pane.close();
    }

    /// Audit I1: a view that lost track of its frames gets the next one, and
    /// the server loop learns to send it in full.
    #[test]
    fn a_resync_sends_a_frame_and_is_reported_once() {
        let pane = detached_pane();
        let view = pane.subscribe(true).unwrap();
        assert!(matches!(view.next(), Some(Update::State(_))));
        assert!(!view.take_resync());
        view.resync();
        assert!(matches!(view.next(), Some(Update::State(_))));
        assert!(view.take_resync());
        assert!(!view.take_resync());
    }

    /// Audit B14: a worker pass with no frame and no events must not wake
    /// the subscriber threads.
    #[test]
    fn publishing_nothing_leaves_subscribers_alone() {
        let pane = detached_pane();
        let subscription = pane.subscribe(true).unwrap();
        // Holding a subscriber's queue blocks any publish that touches it.
        let held = subscription.pending.lock().unwrap();
        let shared = Arc::clone(&pane.shared);
        let (done, published) = flume::bounded(1);
        std::thread::spawn(move || {
            publish(
                &mut shared.lock().unwrap(),
                None,
                vec![TerminalEvent::Wakeup],
            );
            let _ = done.send(());
        });
        let result = published.recv_timeout(Duration::from_secs(5));
        drop(held);
        assert!(result.is_ok(), "an empty publish woke the subscribers");
    }

    /// Audit I8: a capture identical to the published state is not sent.
    #[test]
    fn an_unchanged_capture_is_not_published_again() {
        let terminal = Terminal::new_display(TerminalSize::default(), None);
        let pane = detached_pane();
        pane.shared.lock().unwrap().state = Arc::new(RemoteState::capture(&terminal));
        let subscription = pane.subscribe(true).unwrap();
        assert!(matches!(subscription.next(), Some(Update::State(_))));
        let republish = |terminal: &Terminal| {
            publish(
                &mut pane.shared.lock().unwrap(),
                Some(Arc::new(RemoteState::capture(terminal))),
                Vec::new(),
            );
            std::mem::take(&mut subscription.pending.lock().unwrap().state)
        };
        assert!(!republish(&terminal), "an identical frame was published");
        terminal.feed_output(b"x");
        assert!(republish(&terminal));
        assert!(!republish(&terminal));
    }

    /// Audit B22: hovering over a program that stopped reading fills at most
    /// half of the input backlog, so a key typed afterwards still queues and
    /// nothing reports dropped input.
    #[cfg(unix)]
    #[test]
    fn motion_over_a_stalled_program_leaves_room_for_keys() {
        // Raw mode, so a full input queue blocks the writer instead of the
        // line discipline discarding characters; READY says it is set.
        let stalled = || {
            let pane = script_pane("stty raw -echo; printf READY; exec sleep 30");
            let view = pane.subscribe(true).unwrap();
            while !screen(&next_frame(&view)).contains("READY") {}
            (pane, view)
        };
        // 6 MB in 6000 reports: far more than the PTY buffers, and more
        // entries than the whole input backlog holds (4096).
        let flood = |pane: &Pane, command: fn(Vec<u8>) -> RemoteCommand| {
            for _ in 0..6000 {
                pane.command(command(vec![b'm'; 1024])).unwrap();
            }
        };
        let dropped = |pane: &Pane, view: &Subscription| {
            // The worker drains events after each command; one more round
            // trip makes sure it published those of the last one.
            pane.snapshot().unwrap();
            let pending = view.pending.lock().unwrap();
            pending
                .events
                .iter()
                .any(|event| matches!(event, TerminalEvent::InputDropped))
        };
        let key = RemoteCommand::Write(b"q".to_vec());

        let (pane, view) = stalled();
        flood(&pane, RemoteCommand::WriteMotion);
        pane.command(key.clone()).unwrap();
        assert!(!dropped(&pane, &view), "motion crowded out a key");
        // The program really stopped reading: plain input past the backlog
        // is refused.
        flood(&pane, RemoteCommand::Write);
        assert!(dropped(&pane, &view), "the input backlog never filled");
        pane.close();

        // The same hover sent as plain input fills the backlog and drops
        // the key (the bug).
        let (pane, view) = stalled();
        flood(&pane, RemoteCommand::Write);
        pane.command(key).unwrap();
        assert!(dropped(&pane, &view));
        pane.close();
    }

    fn seed(pane: &Pane, now: SystemTime) -> Vec<TerminalEvent> {
        let shared = pane.shared.lock().unwrap();
        replay_seed(&shared.sticky_events, shared.program_status.as_deref(), now)
    }

    fn record(id: &str, state: ProgramState) -> ProgramStatusRecord {
        ProgramStatusRecord {
            id: id.into(),
            state,
            kind: None,
            progress: None,
            app: None,
            title: None,
            msg: None,
        }
    }

    fn states(records: &[ProgramStatusRecord]) -> Vec<ProgramState> {
        records.iter().map(|record| record.state).collect()
    }

    #[test]
    fn stale_progress_and_working_records_expire_only_outside_a_running_command() {
        let since = SystemTime::now();
        let mut sticky = vec![
            (TerminalEvent::Title("build".into()), since),
            (TerminalEvent::Progress(ProgressState::Indeterminate), since),
        ];
        let status = [
            (record("", ProgramState::Working), since),
            (record("ask", ProgramState::Blocked), since),
            (record("done", ProgramState::Done), since),
        ];
        let fresh = replay_seed(&sticky, Some(&status), since + STALE_ACTIVITY_AFTER / 2);
        assert!(
            matches!(
                fresh.as_slice(),
                [TerminalEvent::Title(_), TerminalEvent::Progress(_), TerminalEvent::ProgramStatus(records)]
                    if records.len() == 3
            ),
            "{fresh:?}"
        );
        // A clock set back does not expire anything.
        let earlier = since - STALE_ACTIVITY_AFTER;
        assert_eq!(
            format!("{:?}", replay_seed(&sticky, Some(&status), earlier)),
            format!("{fresh:?}")
        );

        // No command runs (no shell integration, or it finished): expire the
        // progress and the working record, but never the blocked one.
        let stale = replay_seed(&sticky, Some(&status), since + STALE_ACTIVITY_AFTER);
        let [
            TerminalEvent::Title(_),
            TerminalEvent::ProgramStatus(records),
        ] = stale.as_slice()
        else {
            panic!("unexpected replay {stale:?}");
        };
        assert_eq!(states(records), [ProgramState::Blocked, ProgramState::Done]);

        // A 45-minute build keeps its spinner and status.
        sticky.push((TerminalEvent::ShellCommandExecuting, since));
        let running = replay_seed(&sticky, Some(&status), since + STALE_ACTIVITY_AFTER * 2);
        assert!(
            matches!(
                running.as_slice(),
                [
                    TerminalEvent::Title(_),
                    TerminalEvent::Progress(_),
                    TerminalEvent::ShellCommandExecuting,
                    TerminalEvent::ProgramStatus(records),
                ] if records.len() == 3
            ),
            "{running:?}"
        );
    }

    #[test]
    fn status_records_keep_their_times_until_reported_again() {
        let (then, now) = (SystemTime::UNIX_EPOCH, SystemTime::now());
        let old = vec![
            (record("a", ProgramState::Working), then),
            (record("b", ProgramState::Working), then),
            (record("c", ProgramState::Done), then),
        ];
        let times = |new: Vec<ProgramStatusRecord>| -> Vec<(String, SystemTime)> {
            status_times(&old, new, now)
                .into_iter()
                .map(|(record, time)| (record.id, time))
                .collect()
        };
        let a = record("a", ProgramState::Working);
        let b = record("b", ProgramState::Working);
        let c = record("c", ProgramState::Done);
        // `b` reported unchanged moves to the end.
        assert_eq!(
            times(vec![a.clone(), c.clone(), b.clone()]),
            [("a".into(), then), ("c".into(), then), ("b".into(), now)]
        );
        // `b` reported with a new state.
        let blocked = record("b", ProgramState::Blocked);
        assert_eq!(
            times(vec![a.clone(), c.clone(), blocked]),
            [("a".into(), then), ("c".into(), then), ("b".into(), now)]
        );
        // `a` cleared: nothing was reported.
        assert_eq!(
            times(vec![b.clone(), c.clone()]),
            [("b".into(), then), ("c".into(), then)]
        );
        // The last record reported again unchanged.
        assert_eq!(
            times(vec![a, b, c]),
            [("a".into(), then), ("b".into(), then), ("c".into(), now)]
        );
    }

    #[test]
    fn stale_working_records_stay_hidden_when_the_engine_resends_the_snapshot() {
        let pane = detached_pane();
        let subscription = pane.subscribe(true).unwrap();
        let stuck = record("", ProgramState::Working);
        let long_ago = SystemTime::now() - STALE_ACTIVITY_AFTER * 2;
        pane.shared.lock().unwrap().program_status = Some(vec![(stuck.clone(), long_ago)]);
        // Another program reports; the snapshot still holds the stuck record.
        publish_to(
            &pane,
            vec![TerminalEvent::ProgramStatus(vec![
                stuck,
                record("tests", ProgramState::Done),
            ])],
        );
        let Update::Events(events) = next_events(&subscription) else {
            panic!("expected live events");
        };
        let [TerminalEvent::ProgramStatus(records)] = events.as_slice() else {
            panic!("unexpected events {events:?}");
        };
        assert_eq!(states(records), [ProgramState::Done]);
        let replay = seed(&pane, SystemTime::now());
        assert!(
            matches!(replay.as_slice(), [TerminalEvent::ProgramStatus(records)] if records.len() == 1),
            "{replay:?}"
        );
    }

    #[cfg(unix)]
    fn launched_pane(script: &str, working_directory: Option<&str>) -> Pane {
        Pane::spawn(
            PaneLaunch {
                size: TerminalSize::default(),
                working_directory: working_directory.map(Into::into),
                shell_integration: Some(TabTitleShellIntegration {
                    enabled: true,
                    explicit_prefix: String::new(),
                }),
                config: TerminalRuntimeConfig {
                    scrollback_history: 1234,
                    working_dir_fallback: WorkingDirFallback::Process,
                    environment: [("SSH_ASKPASS".into(), "helper".into())].into(),
                    ..TerminalRuntimeConfig::default()
                },
                launch: Some(TerminalLaunch::Program {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), script.into()],
                }),
                environment: None,
            },
            &std::env::temp_dir(),
            HOST_REQUEST_TIMEOUT,
        )
        .unwrap()
    }

    #[cfg(unix)]
    fn until(pane: &Pane, what: &str, done: impl Fn(&PaneInfo) -> bool) -> PaneInfo {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let info = pane.info();
            if done(&info) {
                return info;
            }
            assert!(Instant::now() < deadline, "{what}: {info:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Audit B24: input for a program that exited, or one that stopped
    /// reading until the backlog filled, is rejected instead of accepted.
    #[cfg(unix)]
    #[test]
    fn input_that_cannot_reach_the_program_is_rejected() {
        let exited = launched_pane("exit 0", None);
        until(&exited, "the program never exited", |info| info.exited);
        assert!(matches!(
            exited.command(RemoteCommand::Write(b"x".to_vec())).unwrap(),
            RemoteReply::InputRejected(InputRejection::Exited)
        ));
        exited.close();

        // Raw mode: a full input queue blocks the writer instead of
        // discarding characters, as for a stalled full-screen program;
        // READY says it is set.
        let stalled = launched_pane("stty raw -echo; printf READY; exec sleep 30", None);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !screen(&stalled.snapshot().unwrap()).contains("READY") {
            assert!(Instant::now() < deadline, "the program never got ready");
            std::thread::sleep(Duration::from_millis(10));
        }
        let chunk = vec![b'x'; 64 * 1024];
        let rejected = (0..400).find_map(|_| {
            match stalled
                .command(RemoteCommand::Write(chunk.clone()))
                .unwrap()
            {
                RemoteReply::InputRejected(rejection) => Some(rejection),
                _ => None,
            }
        });
        assert_eq!(rejected, Some(InputRejection::BacklogFull));
        stalled.close();
    }

    #[cfg(unix)]
    fn write(pane: &Pane, input: &str) -> u64 {
        match pane
            .command(RemoteCommand::Write(input.as_bytes().to_vec()))
            .unwrap()
        {
            RemoteReply::Written { anchor } => anchor,
            reply => panic!("input was not written: {reply:?}"),
        }
    }

    #[cfg(unix)]
    fn command_done(pane: &Pane) -> Option<i32> {
        let info = until(pane, "the input's command never finished", |info| {
            matches!(
                info.shell.and_then(|shell| shell.input_command),
                Some(InputCommand::Finished(_))
            )
        });
        let Some(InputCommand::Finished(code)) = info.shell.unwrap().input_command else {
            unreachable!()
        };
        code
    }

    /// Audit I14: the pane follows the OSC 133 command lifecycle and the
    /// command that input started.
    #[cfg(unix)]
    #[test]
    fn shell_state_follows_commands_and_input() {
        let pane = launched_pane(
            "printf '\\033]133;A\\007'; read line; \
             printf '\\033]133;C\\007'; sleep 0.2; \
             printf '\\033]133;D;3\\007\\033]133;A\\007'; sleep 30",
            None,
        );
        let info = until(&pane, "no prompt", |info| info.shell.is_some());
        assert_eq!(info.shell, Some(ShellState::default()));
        write(&pane, "go\r");
        until(&pane, "the command never ran", |info| {
            info.shell.is_some_and(|shell| shell.running)
        });
        assert_eq!(command_done(&pane), Some(3));
        assert_eq!(
            pane.info().shell,
            Some(ShellState {
                running: false,
                commands_started: 1,
                commands_finished: 1,
                exit_code: Some(3),
                input_command: Some(InputCommand::Finished(Some(3))),
            })
        );
        // Later input waits for a later command.
        write(&pane, "more");
        assert_eq!(
            pane.info().shell.unwrap().input_command,
            Some(InputCommand::Pending)
        );
        pane.close();
    }

    /// Audit I14: fish reports a finish at its first prompt. Input typed
    /// before that prompt finishes with the command it starts, not then.
    #[cfg(unix)]
    #[test]
    fn a_first_prompt_finish_is_not_the_inputs_command() {
        let pane = launched_pane(
            "sleep 0.5; printf '\\033]133;D;0\\007\\033]133;A\\007\\033]133;B\\007'; \
             read line; printf '\\033]133;C\\007'; sleep 0.2; \
             printf '\\033]133;D;5\\007\\033]133;A\\007'; sleep 30",
            None,
        );
        write(&pane, "go\r");
        assert!(pane.info().shell.is_none(), "the prompt came too early");
        assert_eq!(command_done(&pane), Some(5));
        pane.close();
    }

    /// Audit I14: input typed while a command still runs (its prompt hook
    /// reports the finish late) finishes with the next command.
    #[cfg(unix)]
    #[test]
    fn a_late_finish_belongs_to_the_earlier_command() {
        let pane = launched_pane(
            "printf '\\033]133;A\\007'; read first; printf '\\033]133;C\\007'; \
             read second; printf '\\033]133;D;1\\007\\033]133;A\\007'; \
             printf '\\033]133;C\\007'; sleep 0.2; \
             printf '\\033]133;D;7\\007\\033]133;A\\007'; sleep 30",
            None,
        );
        until(&pane, "no prompt", |info| info.shell.is_some());
        write(&pane, "first\r");
        until(&pane, "the first command never ran", |info| {
            info.shell.is_some_and(|shell| shell.running)
        });
        write(&pane, "second\r");
        assert_eq!(command_done(&pane), Some(7));
        pane.close();
    }

    /// Audit I14: a shell that reports no command starts (PowerShell)
    /// finishes the input's command with the next finish after a prompt.
    #[cfg(unix)]
    #[test]
    fn shells_without_command_starts_finish_after_a_prompt() {
        let pane = launched_pane(
            "printf '\\033]133;A\\007\\033]133;B\\007'; read line; sleep 0.2; \
             printf '\\033]133;D;4\\007\\033]133;A\\007'; sleep 30",
            None,
        );
        until(&pane, "no prompt", |info| info.shell.is_some());
        write(&pane, "go\r");
        assert_eq!(command_done(&pane), Some(4));
        pane.close();
    }

    /// Audit I14: a write's anchor is the line the cursor was on, counted
    /// from the first line that scrolled into history.
    #[cfg(unix)]
    #[test]
    fn writes_report_the_cursor_line_as_their_anchor() {
        let pane = launched_pane("stty -echo; seq 1 100; printf READY; exec sleep 30", None);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !screen(&pane.snapshot().unwrap()).contains("READY") {
            assert!(Instant::now() < deadline, "the program never got ready");
            std::thread::sleep(Duration::from_millis(10));
        }
        let state = pane.snapshot().unwrap();
        let anchor = write(&pane, "x");
        assert_eq!(
            anchor,
            state.lines_scrolled + state.cursor_position.1 as u64
        );
        let RemoteReply::LineTexts(texts) = pane
            .command(RemoteCommand::LineTexts {
                first: 0,
                last: 0,
                anchor: Some(anchor),
            })
            .unwrap()
        else {
            panic!("expected lines");
        };
        assert_eq!(texts.lines[0].text, "READY");
        assert_eq!(texts.rows, 24);
        assert_eq!(texts.scrolled, state.lines_scrolled);
        pane.close();
    }

    /// Audit B26/B27: a pane lists the directory it started in, not a
    /// missing one it was asked for, and keeps its launch settings without
    /// the variables of its launch or those the host adds.
    #[cfg(unix)]
    #[test]
    fn panes_keep_their_launch_settings_and_actual_directory() {
        let missing = std::env::temp_dir().join("termy-no-such-directory");
        let pane = launched_pane("sleep 30", Some(&missing.to_string_lossy()));
        let started = std::env::current_dir().unwrap();
        assert_eq!(
            pane.info().working_directory.as_deref(),
            Some(&*started.to_string_lossy())
        );
        let settings = pane.launch_config();
        assert_eq!(settings.config.scrollback_history, 1234);
        assert!(settings.config.environment.is_empty());
        assert!(
            settings
                .shell_integration
                .is_some_and(|shell| shell.enabled)
        );
        pane.close();
    }
}
