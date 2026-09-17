use crate::multiplexer::protocol::*;
use crate::{remote::*, *};
use anyhow::{Context, bail};
use flume::{Receiver, Sender};
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex, Weak},
    time::{Duration, Instant},
};

const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

enum Work {
    Wake,
    Command(RemoteCommand, Sender<RemoteReply>),
    Refresh(Sender<Arc<RemoteState>>),
    Close,
}

struct Shared {
    info: PaneInfo,
    state: Arc<RemoteState>,
    sticky_events: Vec<TerminalEvent>,
    subscribers: Vec<Weak<Subscription>>,
    host_replies: HashMap<u64, Sender<RemoteHostReply>>,
    next_host_id: u64,
}

#[derive(Default)]
struct Pending {
    state: bool,
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

#[derive(Clone)]
pub(crate) struct Pane {
    tx: Sender<Work>,
    shared: Arc<Mutex<Shared>>,
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
            subscribers: Vec::new(),
            host_replies: HashMap::new(),
            next_host_id: 1,
        }));
        let worker_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("termy-session".into())
            .spawn(move || run(terminal, rx, worker_shared))?;
        Ok(Self { tx, shared })
    }

    pub(crate) fn info(&self) -> PaneInfo {
        self.shared.lock().unwrap().info.clone()
    }

    pub(crate) fn refresh(&self) -> anyhow::Result<Arc<RemoteState>> {
        let (tx, rx) = flume::bounded(1);
        self.tx.send_timeout(Work::Refresh(tx), REQUEST_TIMEOUT)?;
        rx.recv_timeout(REQUEST_TIMEOUT)
            .context("refresh terminal session")
    }

    pub(crate) fn command(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
        let (tx, rx) = flume::bounded(1);
        self.tx
            .send_timeout(Work::Command(command, tx), REQUEST_TIMEOUT)?;
        rx.recv_timeout(REQUEST_TIMEOUT)
            .context("terminal session did not answer")
    }

    pub(crate) fn close(&self) {
        let _ = self.tx.send_timeout(Work::Close, REQUEST_TIMEOUT);
    }

    pub(crate) fn subscribe(&self) -> anyhow::Result<Arc<Subscription>> {
        self.refresh()?;
        let mut shared = self.shared.lock().unwrap();
        let mut events = shared.sticky_events.clone();
        if shared.info.exited {
            events.push(TerminalEvent::Exit);
        }
        let subscription = Arc::new(Subscription {
            pending: Mutex::new(Pending {
                state: true,
                events,
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

fn sticky_key(event: &TerminalEvent) -> Option<u8> {
    match event {
        TerminalEvent::Title(_) | TerminalEvent::ResetTitle => Some(0),
        TerminalEvent::WorkingDirectory(_) => Some(1),
        TerminalEvent::Progress(_) => Some(2),
        TerminalEvent::ShellPromptStart
        | TerminalEvent::ShellCommandStart
        | TerminalEvent::ShellCommandExecuting
        | TerminalEvent::ShellCommandFinished(_) => Some(3),
        _ => None,
    }
}

fn publish(shared: &mut Shared, state: Option<Arc<RemoteState>>, events: Vec<TerminalEvent>) {
    let changed = state.is_some();
    if let Some(state) = state {
        shared.state = state;
    }
    let events: Vec<_> = events
        .into_iter()
        .filter(|event| !matches!(event, TerminalEvent::Wakeup))
        .collect();
    for event in &events {
        match event {
            TerminalEvent::Title(title) => shared.info.title = Some(title.clone()),
            TerminalEvent::ResetTitle => shared.info.title = None,
            TerminalEvent::WorkingDirectory(cwd) => {
                shared.info.working_directory = Some(cwd.clone());
            }
            TerminalEvent::Exit => shared.info.exited = true,
            _ => {}
        }
        if let Some(key) = sticky_key(event) {
            shared
                .sticky_events
                .retain(|old| sticky_key(old) != Some(key));
            shared.sticky_events.push(event.clone());
        }
    }
    shared.subscribers.retain(|weak| {
        let Some(subscriber) = weak.upgrade() else {
            return false;
        };
        let mut pending = subscriber.pending.lock().unwrap();
        pending.state |= changed;
        for event in &events {
            if let Some(key) = sticky_key(event) {
                pending.events.retain(|old| sticky_key(old) != Some(key));
            }
            // A slow or suspended client cannot retain an unbounded stream of
            // bells or clipboard writes. Its terminal continues independently.
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

fn run(mut terminal: Terminal, rx: Receiver<Work>, shared: Arc<Mutex<Shared>>) {
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
                let result = crate::remote::execute(&mut terminal, command);
                let _ = reply.try_send(result);
                dirty = true;
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
        let mut state = shared.lock().unwrap();
        let attached = state.subscribers.iter().any(|weak| weak.strong_count() > 0);
        let animation_due = state
            .state
            .graphics_deadline
            .is_some_and(|deadline| deadline <= Instant::now());
        let update =
            if attached && (dirty || animation_due) && last_frame.elapsed() >= FRAME_INTERVAL {
                last_frame = Instant::now();
                dirty = false;
                Some(Arc::new(RemoteState::capture(&terminal)))
            } else {
                None
            };
        publish(&mut state, update, events);
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
        let id = {
            let mut shared = self.shared.lock().unwrap();
            let Some(subscriber) = shared.subscribers.iter().rev().find_map(Weak::upgrade) else {
                bail!("no attached clipboard host");
            };
            let id = shared.next_host_id;
            shared.next_host_id = id.wrapping_add(1);
            shared.host_replies.insert(id, tx);
            subscriber.pending.lock().unwrap().host.push((id, request));
            subscriber.ready.notify_one();
            id
        };
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
            subscribers: Vec::new(),
            host_replies: HashMap::new(),
            next_host_id: 1,
        }));
        let (tx, rx) = flume::bounded(256);
        let pane = Pane { tx, shared };
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
}
