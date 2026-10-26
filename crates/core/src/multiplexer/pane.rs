use crate::multiplexer::protocol::*;
use crate::{remote::*, *};
use anyhow::{Context, bail};
use flume::{Receiver, Sender};
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex, Weak},
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

enum Work {
    Wake,
    Command(RemoteCommand, Sender<RemoteReply>),
    Viewport(RemoteCommand, Sender<(bool, u64)>),
    Refresh(Sender<Arc<RemoteState>>),
    Close,
}

struct Shared {
    info: PaneInfo,
    state: Arc<RemoteState>,
    /// The latest event of each [`sticky_key`], and when it arrived. Wall
    /// clock time, so that system sleep counts towards staleness.
    sticky_events: Vec<(TerminalEvent, SystemTime)>,
    /// The latest OSC 7501 snapshot, if any, with when each record last
    /// changed.
    program_status: Option<Vec<(ProgramStatusRecord, SystemTime)>>,
    subscribers: Vec<Weak<Subscription>>,
    host_replies: HashMap<u64, Sender<RemoteHostReply>>,
    next_host_id: u64,
}

#[derive(Default)]
struct Pending {
    state: bool,
    /// The replay seed of a new subscription, sent before any live event.
    replay: Vec<TerminalEvent>,
    events: Vec<TerminalEvent>,
    host: Vec<(u64, RemoteHostRequest)>,
    closed: bool,
}

pub(crate) struct Subscription {
    pending: Mutex<Pending>,
    ready: Condvar,
    shared: Weak<Mutex<Shared>>,
}

impl Subscription {
    pub(crate) fn close(&self) {
        self.pending.lock().unwrap().closed = true;
        self.ready.notify_one();
    }
    pub(crate) fn next(&self) -> Option<Update> {
        let mut pending = self.pending.lock().unwrap();
        loop {
            if pending.closed {
                return None;
            }
            if let Some((id, request)) = pending.host.pop() {
                return Some(Update::Host { id, request });
            }
            if !pending.replay.is_empty() {
                return Some(Update::Replay(std::mem::take(&mut pending.replay)));
            }
            if !pending.events.is_empty() {
                return Some(Update::Events(std::mem::take(&mut pending.events)));
            }
            if pending.state {
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
}

impl Pane {
    pub(crate) fn create(launch: PaneLaunch) -> anyhow::Result<Self> {
        let (tx, rx) = flume::bounded(256);
        let wake_tx = tx.clone();
        let terminal = Terminal::new_with_launch_and_wakeup_notifier(
            launch.size,
            launch.working_directory.as_deref(),
            Some(TerminalWakeupNotifier::new(move || {
                let _ = wake_tx.try_send(Work::Wake);
            })),
            launch.shell_integration.as_ref(),
            Some(&launch.config),
            launch.launch.as_ref(),
        )?;
        let state = Arc::new(RemoteState::capture(&terminal));
        let shared = Arc::new(Mutex::new(Shared {
            info: PaneInfo {
                id: uuid::Uuid::new_v4().to_string(),
                child_pid: terminal.child_pid(),
                title: None,
                working_directory: launch.working_directory,
                exited: false,
            },
            state,
            sticky_events: Vec::new(),
            program_status: None,
            subscribers: Vec::new(),
            host_replies: HashMap::new(),
            next_host_id: 1,
        }));
        let worker_shared = Arc::clone(&shared);
        let exit_hook = Arc::new(Mutex::new(None));
        let worker_exit_hook = Arc::clone(&exit_hook);
        std::thread::Builder::new()
            .name("termy-session".into())
            .spawn(move || run(terminal, rx, worker_shared, worker_exit_hook))?;
        Ok(Self {
            tx,
            shared,
            exit_hook,
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

    pub(crate) fn subscribe(&self) -> anyhow::Result<Arc<Subscription>> {
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
        });
        shared.subscribers.push(Arc::downgrade(&subscription));
        drop(shared);
        // Output can arrive between refresh and registration while the worker
        // still sees no subscribers. Wake it to publish that pending frame.
        let _ = self.tx.try_send(Work::Wake);
        Ok(subscription)
    }

    pub(crate) fn host_reply(&self, id: u64, reply: RemoteHostReply) {
        if let Some(tx) = self.shared.lock().unwrap().host_replies.remove(&id) {
            let _ = tx.try_send(reply);
        }
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
    shared.subscribers.retain(|weak| {
        let Some(subscriber) = weak.upgrade() else {
            return false;
        };
        let mut pending = subscriber.pending.lock().unwrap();
        pending.state |= changed;
        for event in &events {
            if !coalesce_event(&mut pending.events, event) {
                continue;
            }
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
        subscriber.ready.notify_one();
        !pending.closed
    });
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

fn run(
    mut terminal: Terminal,
    rx: Receiver<Work>,
    shared: Arc<Mutex<Shared>>,
    exit_hook: Arc<Mutex<Option<ExitHook>>>,
) {
    let mut host = HostBridge {
        shared: Arc::clone(&shared),
    };
    let mut dirty = true;
    let mut pending_events = false;
    let mut last_frame = Instant::now() - FRAME_INTERVAL;
    loop {
        let timeout = {
            let state = shared.lock().unwrap();
            let attached = state.subscribers.iter().any(|weak| weak.strong_count() > 0);
            if pending_events {
                Duration::ZERO
            } else if attached && dirty {
                FRAME_INTERVAL.saturating_sub(last_frame.elapsed())
            } else if attached && let Some(deadline) = state.state.graphics_deadline {
                deadline.saturating_duration_since(Instant::now())
            } else {
                Duration::from_secs(60)
            }
        };
        match rx.recv_timeout(timeout) {
            Ok(Work::Close) | Err(flume::RecvTimeoutError::Disconnected) => break,
            Ok(Work::Command(command, reply)) => {
                let changes_state = !matches!(
                    command,
                    RemoteCommand::Snapshot
                        | RemoteCommand::Lines { .. }
                        | RemoteCommand::Search { .. }
                        | RemoteCommand::Hyperlink { .. }
                        | RemoteCommand::Link { .. }
                        | RemoteCommand::Graphics
                );
                let result = crate::remote::execute(&mut terminal, command);
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
            Ok(Work::Wake) => dirty = true,
            Err(flume::RecvTimeoutError::Timeout) => {}
        }
        let (events, more) = terminal.drain_events(&mut host);
        pending_events = more;
        dirty |= !events.is_empty() || more;
        let (attached, animation_due) = {
            let state = shared.lock().unwrap();
            let attached = state.subscribers.iter().any(|weak| weak.strong_count() > 0);
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

struct HostBridge {
    shared: Arc<Mutex<Shared>>,
}

impl HostBridge {
    fn request(&self, request: RemoteHostRequest) -> anyhow::Result<RemoteHostReply> {
        let (tx, rx) = flume::bounded(1);
        let (id, subscriber) = {
            let mut shared = self.shared.lock().unwrap();
            let Some(subscriber) = shared.subscribers.iter().rev().find_map(Weak::upgrade) else {
                bail!("no attached clipboard host");
            };
            let id = shared.next_host_id;
            shared.next_host_id = id.wrapping_add(1);
            shared.host_replies.insert(id, tx);
            drop(shared);
            (id, subscriber)
        };
        subscriber.pending.lock().unwrap().host.push((id, request));
        subscriber.ready.notify_one();
        let result = rx.recv_timeout(Duration::from_millis(500));
        self.shared.lock().unwrap().host_replies.remove(&id);
        Ok(result?)
    }
}

impl TerminalReplyHost for HostBridge {
    fn load_clipboard(&mut self, target: TerminalClipboardTarget) -> Option<String> {
        match self.request(RemoteHostRequest::Load(target)) {
            Ok(RemoteHostReply::Load(value)) => value,
            _ => None,
        }
    }
    fn read_clipboard(
        &mut self,
        request: TerminalClipboardReadRequest,
    ) -> TerminalClipboardReadResult {
        match self.request(RemoteHostRequest::Read(request)) {
            Ok(RemoteHostReply::Read(value)) => value,
            _ => TerminalClipboardReadResult::Denied,
        }
    }
    fn write_clipboard(
        &mut self,
        request: TerminalClipboardWriteRequest,
    ) -> TerminalClipboardWriteResult {
        match self.request(RemoteHostRequest::Write(request)) {
            Ok(RemoteHostReply::Write(value)) => value,
            _ => TerminalClipboardWriteResult::Denied,
        }
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
            },
            state: Arc::clone(&state),
            sticky_events: Vec::new(),
            program_status: None,
            subscribers: Vec::new(),
            host_replies: HashMap::new(),
            next_host_id: 1,
        }));
        let (tx, rx) = flume::bounded(256);
        let pane = Pane {
            tx,
            shared,
            exit_hook: Arc::default(),
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
        let subscription = pane.subscribe().unwrap();
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
            },
            state: Arc::new(RemoteState::capture(&terminal)),
            sticky_events: Vec::new(),
            program_status: None,
            subscribers: Vec::new(),
            host_replies: HashMap::new(),
            next_host_id: 1,
        }));
        let subscription = Arc::new(Subscription {
            pending: Mutex::new(Pending::default()),
            ready: Condvar::new(),
            shared: Arc::downgrade(&shared),
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
            },
            state: Arc::clone(&state),
            sticky_events: Vec::new(),
            program_status: None,
            subscribers: Vec::new(),
            host_replies: HashMap::new(),
            next_host_id: 1,
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
        let subscription = pane.subscribe().unwrap();
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
        let subscription = pane.subscribe().unwrap();
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
        let subscription = pane.subscribe().unwrap();
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
        let subscription = pane.subscribe().unwrap();
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
}
