use crate::multiplexer::{discovery, protocol::*};
use crate::session_model::{StoredMultiplexer, StoredWorkspace, WorkspaceEdit};
use crate::{remote::*, *};
use anyhow::{bail, ensure};
use flume::Sender;
use std::{
    collections::VecDeque,
    net::{Shutdown, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Longer than the host's own per-request deadline, so a slow host answers
/// with its error before the client gives up on the reply.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
/// Read-modify-write attempts before a busy layout is reported to the caller.
const LAYOUT_ATTEMPTS: usize = 16;

struct Connection {
    root: PathBuf,
    /// `None` after a failed exchange; the next request reconnects.
    stream: Mutex<Option<TcpStream>>,
}

impl Connection {
    fn connect(root: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            root: root.to_owned(),
            stream: Mutex::new(Some(open_rpc_stream(root)?)),
        })
    }
    fn request(&self, request: Request) -> anyhow::Result<Response> {
        let mut slot = self.stream.lock().unwrap();
        let stream = match slot.as_mut() {
            Some(stream) => stream,
            None => slot.insert(open_rpc_stream(&self.root)?),
        };
        let response = match write_message(stream, &request).and_then(|()| read_message(stream)) {
            Ok(response) => response,
            Err(error) => {
                // A timed-out or broken exchange leaves the stream mid-reply:
                // its late answer would be read as the next request's reply.
                *slot = None;
                return Err(error);
            }
        };
        drop(slot);
        match response {
            Response::Error(error) => bail!(error),
            response => Ok(response),
        }
    }
}

fn open_rpc_stream(root: &Path) -> anyhow::Result<TcpStream> {
    let stream = discovery::connect(root)?;
    stream.set_read_timeout(Some(RESPONSE_TIMEOUT))?;
    Ok(stream)
}

#[derive(Clone)]
pub struct SessionClient {
    root: PathBuf,
    connection: Arc<Connection>,
}

impl SessionClient {
    /// Apply a workspace edit without replacing another client's layout edits.
    pub fn edit_workspace(
        &self,
        window_id: &str,
        index: usize,
        expected: &StoredWorkspace,
        edit: &WorkspaceEdit,
    ) -> anyhow::Result<StoredMultiplexer> {
        let update = self.update_layout(|layout| {
            let workspace = layout
                .windows
                .iter_mut()
                .find(|window| window.id == window_id)
                .and_then(|window| window.session.workspaces.get_mut(index))
                .ok_or_else(|| anyhow::anyhow!("Workspace no longer exists"))?;
            ensure!(
                workspace == expected,
                "Workspace changed in another client; refresh before editing"
            );
            edit.apply(workspace).map_err(anyhow::Error::msg)
        })?;
        Ok(update.layout)
    }

    /// Reads the layout, lets `edit` change it and publishes the result only
    /// if no other client wrote in between, retrying a bounded number of times.
    /// `edit` runs once per attempt on a fresh copy; an error from it ends the
    /// update without writing; when it changes nothing, nothing is written.
    pub fn update_layout<T>(
        &self,
        mut edit: impl FnMut(&mut StoredMultiplexer) -> anyhow::Result<T>,
    ) -> anyhow::Result<LayoutUpdate<T>> {
        for attempt in 0..LAYOUT_ATTEMPTS {
            if attempt > 0 {
                // Writers that collided retry at different moments.
                let jitter = u64::from(rand_core::RngCore::next_u32(&mut rand_core::OsRng) % 4);
                std::thread::sleep(Duration::from_millis(attempt as u64 + jitter));
            }
            let (revision, json) = self.layout_revision()?;
            let original = StoredMultiplexer::parse(json.as_deref()).map_err(anyhow::Error::msg)?;
            let mut layout = original.clone();
            let value = edit(&mut layout)?;
            let revision = if layout == original {
                Some(revision)
            } else {
                self.compare_and_set_layout(revision, serde_json::to_string(&layout)?)?
            };
            if let Some(revision) = revision {
                return Ok(LayoutUpdate {
                    value,
                    layout,
                    revision,
                });
            }
        }
        bail!("Session layout is changing; retry")
    }

    pub fn connect(root: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            root: root.to_owned(),
            connection: Arc::new(Connection::connect(root)?),
        })
    }

    /// Creates a pane and attaches a view to it, which answers the pane's
    /// clipboard requests (see [`Self::attach`]).
    pub fn create(
        &self,
        launch: PaneLaunch,
        wakeup: Option<TerminalWakeupNotifier>,
    ) -> anyhow::Result<(String, Terminal)> {
        let (info, state) = self.create_pane(launch)?;
        let terminal = self.attach_state(&info.id, state, wakeup, true)?;
        Ok((info.id, terminal))
    }

    /// Creates a pane without attaching to it, for clients that only start
    /// programs. Returns the pane ID.
    pub fn create_detached(&self, launch: PaneLaunch) -> anyhow::Result<String> {
        Ok(self.create_pane(launch)?.0.id)
    }

    fn create_pane(&self, launch: PaneLaunch) -> anyhow::Result<(PaneInfo, Arc<RemoteState>)> {
        let Response::Attached(info, state) =
            self.connection.request(Request::Create(Box::new(launch)))?
        else {
            bail!("invalid create response");
        };
        Ok((info, state))
    }

    /// Attaches a view that streams the pane's frames and events. The view
    /// also answers the pane's clipboard requests: `drain_events` runs them
    /// through its reply host, so a view must drain promptly, even while
    /// hidden (see [`TerminalWakeupNotifier`]).
    pub fn attach(
        &self,
        id: &str,
        wakeup: Option<TerminalWakeupNotifier>,
    ) -> anyhow::Result<Terminal> {
        self.attach_with(id, wakeup, true)
    }

    /// Attaches like [`Self::attach`], but the pane's clipboard requests are
    /// never sent to this view, for tools that watch a pane with no user to
    /// ask.
    pub fn observe(
        &self,
        id: &str,
        wakeup: Option<TerminalWakeupNotifier>,
    ) -> anyhow::Result<Terminal> {
        self.attach_with(id, wakeup, false)
    }

    fn attach_with(
        &self,
        id: &str,
        wakeup: Option<TerminalWakeupNotifier>,
        host_capable: bool,
    ) -> anyhow::Result<Terminal> {
        let Response::Attached(_, state) =
            self.connection.request(Request::Attach(id.to_owned()))?
        else {
            bail!("invalid attach response");
        };
        self.attach_state(id, state, wakeup, host_capable)
    }

    /// The pane's current state, read once without attaching.
    pub fn snapshot(&self, id: &str) -> anyhow::Result<Arc<RemoteState>> {
        let Response::Attached(_, state) =
            self.connection.request(Request::Snapshot(id.to_owned()))?
        else {
            bail!("invalid snapshot response");
        };
        validate_state(&state)?;
        Ok(state)
    }

    fn attach_state(
        &self,
        id: &str,
        state: Arc<RemoteState>,
        wakeup: Option<TerminalWakeupNotifier>,
        host_capable: bool,
    ) -> anyhow::Result<Terminal> {
        validate_state(&state)?;
        let rpc = Connection::connect(&self.root)?;
        let input = Connection::connect(&self.root)?;
        let mut stream = discovery::connect(&self.root)?;
        let subscribe = Request::Subscribe {
            pane: id.to_owned(),
            host_capable,
        };
        write_message(&mut stream, &subscribe)?;
        ensure!(
            matches!(read_message(&mut stream)?, Response::Ok),
            "cannot subscribe to terminal session"
        );
        stream.set_read_timeout(None)?;
        let close_stream = stream.try_clone()?;
        let (input_tx, input_rx) = flume::bounded(256);
        let initial_state = awaiting_graphics((*state).clone());
        let shared = Arc::new(ClientState {
            size_claim: Mutex::new(SizeClaim::attached(&initial_state)),
            state: Mutex::new(Arc::new(initial_state)),
            state_updated: Condvar::new(),
            pending: Mutex::new(Pending::default()),
            wakeup,
            wakeup_enabled: AtomicBool::new(true),
            disconnected: AtomicBool::new(false),
            closing: AtomicBool::new(false),
        });
        let reader_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("termy-session-read".into())
            .spawn(move || {
                let mut graphics = graphics::GraphicsDecoder::default();
                let result = (|| -> anyhow::Result<()> {
                    loop {
                        let mut tab_chrome = false;
                        match read_message(&mut stream)? {
                            Update::State(_) | Update::RetiredHost => {
                                bail!("unexpected terminal update")
                            }
                            Update::GraphicsState(mut state, update) => {
                                validate_state(&state)?;
                                Arc::make_mut(&mut state).graphics = Some(graphics.decode(update)?);
                                reader_shared.publish_state(state);
                            }
                            Update::Events(events) => {
                                tab_chrome = events.iter().any(drives_tab_chrome);
                                reader_shared
                                    .pending
                                    .lock()
                                    .unwrap()
                                    .extend_events(events)?;
                            }
                            Update::Replay(events) => {
                                tab_chrome = events.iter().any(drives_tab_chrome);
                                reader_shared
                                    .pending
                                    .lock()
                                    .unwrap()
                                    .extend_replay(events)?;
                            }
                            Update::HostRequest {
                                id,
                                request,
                                deadline,
                            } => {
                                // Answered by draining; a hidden tab must
                                // drain before the host gives up (audit B7).
                                tab_chrome = true;
                                let deadline = deadline.unwrap_or_else(Instant::now);
                                reader_shared
                                    .pending
                                    .lock()
                                    .unwrap()
                                    .hosts
                                    .push_back((id, request, deadline));
                            }
                        }
                        if tab_chrome {
                            reader_shared.notify_tab_chrome();
                        } else {
                            reader_shared.notify();
                        }
                    }
                })();
                if let Err(error) = result {
                    reader_shared.fail(&error);
                }
            })?;
        let input_shared = Arc::clone(&shared);
        let pane_id = id.to_owned();
        std::thread::Builder::new()
            .name("termy-session-input".into())
            .spawn(move || {
                while let Ok(Input { command, reply }) = input_rx.recv() {
                    let response = input.request(Request::Command {
                        pane: pane_id.clone(),
                        command,
                    });
                    match response {
                        Ok(response) => {
                            if let Some(reply) = reply {
                                let _ = reply.send(match response {
                                    Response::Reply(result) => Ok(result),
                                    _ => Err(anyhow::anyhow!("invalid terminal response")),
                                });
                            }
                        }
                        Err(error) => {
                            input_shared.fail(&error);
                            if let Some(reply) = reply {
                                let _ = reply.send(Err(error));
                            }
                            break;
                        }
                    }
                }
            })?;
        Ok(Terminal::from_remote(Arc::new(ClientTerminal {
            id: id.to_owned(),
            rpc,
            host: Arc::clone(&self.connection),
            shared,
            input: input_tx,
            close_stream,
        })))
    }

    pub fn list(&self) -> anyhow::Result<Vec<PaneInfo>> {
        match self.connection.request(Request::List)? {
            Response::Panes(panes) => Ok(panes),
            _ => bail!("invalid session list"),
        }
    }
    pub fn close(&self, id: &str) -> anyhow::Result<()> {
        self.connection.request(Request::Close(id.to_owned()))?;
        Ok(())
    }
    /// Wait for the host to accept input before returning. Unlike a view's
    /// asynchronous input queue, this is safe for a short-lived CLI process.
    pub fn write(&self, id: &str, bytes: Vec<u8>) -> anyhow::Result<()> {
        self.send_command(id, RemoteCommand::Write(bytes))
    }
    /// The cell metrics (pixels) the attached UI last applied to `id`.
    pub(crate) fn cell_size(&self, id: &str) -> anyhow::Result<(f32, f32)> {
        let Response::Size(size) = self.connection.request(Request::Size(id.to_owned()))? else {
            bail!("invalid size response");
        };
        Ok((size.cell_width, size.cell_height))
    }
    pub fn resize(&self, id: &str, size: TerminalSize) -> anyhow::Result<()> {
        self.send_command(id, RemoteCommand::Resize(size))
    }
    fn send_command(&self, id: &str, command: RemoteCommand) -> anyhow::Result<()> {
        match self.connection.request(Request::Command {
            pane: id.to_owned(),
            command,
        })? {
            Response::Reply(_) => Ok(()),
            _ => bail!("invalid terminal command response"),
        }
    }
    /// The layout as JSON, `None` before any client stored one.
    pub fn layout(&self) -> anyhow::Result<Option<String>> {
        Ok(self.layout_revision()?.1)
    }
    /// The layout with its revision, which every accepted write increments.
    pub fn layout_revision(&self) -> anyhow::Result<(u64, Option<String>)> {
        match self.connection.request(Request::GetLayout)? {
            Response::Layout { revision, layout } => Ok((revision, layout)),
            _ => bail!("invalid session layout"),
        }
    }
    /// Replaces the layout regardless of other clients' edits. The host
    /// rejects a layout it cannot read. Returns the new revision.
    pub fn set_layout(&self, layout: String) -> anyhow::Result<u64> {
        match self.connection.request(Request::SetLayout(layout))? {
            Response::LayoutUpdated { revision, .. } => Ok(revision),
            _ => bail!("invalid session layout update response"),
        }
    }
    /// Publish a layout only if it is still at revision `expected`. Returns
    /// the new revision, or `None` (leaving the layout untouched) when another
    /// client wrote first. Prefer [`Self::update_layout`].
    pub fn compare_and_set_layout(
        &self,
        expected: u64,
        replacement: String,
    ) -> anyhow::Result<Option<u64>> {
        match self.connection.request(Request::CompareAndSetLayout {
            expected,
            replacement,
        })? {
            Response::LayoutUpdated { updated, revision } => Ok(updated.then_some(revision)),
            _ => bail!("invalid session layout update response"),
        }
    }
    /// Opens a stream that reports each new layout revision.
    pub fn watch_layout(&self) -> anyhow::Result<LayoutWatch> {
        let mut stream = discovery::connect(&self.root)?;
        write_message(&mut stream, &Request::WatchLayout)?;
        stream.set_read_timeout(None)?;
        Ok(LayoutWatch { stream })
    }
    /// Explicitly terminate all hosted terminals and the background host.
    /// [`crate::multiplexer::shutdown_host`] also stops a host of another
    /// protocol version.
    pub fn shutdown(&self) -> anyhow::Result<()> {
        self.connection.request(Request::Shutdown)?;
        Ok(())
    }
}

/// The outcome of [`SessionClient::update_layout`].
pub struct LayoutUpdate<T> {
    /// What the edit returned.
    pub value: T,
    /// The layout as published, or as read when the edit changed nothing.
    pub layout: StoredMultiplexer,
    /// The revision of `layout`.
    pub revision: u64,
}

/// Layout revisions as the host accepts writes, from
/// [`SessionClient::watch_layout`]. Dropping it ends the stream.
pub struct LayoutWatch {
    stream: TcpStream,
}

impl LayoutWatch {
    /// Blocks until the host reports a revision: the current one first, then
    /// each newer one. A slow reader may skip revisions, never the latest.
    pub fn recv(&mut self) -> anyhow::Result<u64> {
        match read_message(&mut self.stream)? {
            Response::LayoutChanged { revision } => Ok(revision),
            Response::Error(error) => bail!(error),
            _ => bail!("invalid layout notification"),
        }
    }

    /// Makes a blocked or later [`Self::recv`] fail, from any thread.
    pub fn closer(&self) -> anyhow::Result<impl Fn() + Send + Sync + 'static> {
        let stream = self.stream.try_clone()?;
        Ok(move || {
            let _ = stream.shutdown(Shutdown::Both);
        })
    }
}

impl Drop for LayoutWatch {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

fn awaiting_graphics(mut state: RemoteState) -> RemoteState {
    // Keep painting local while the subscription loads its first image snapshot.
    // Do not label this empty placeholder with the completed host revision:
    // the renderer must invalidate it when those first streamed images arrive.
    state.graphics_revision = 0;
    state.graphics = Some(Vec::new());
    state
}

fn validate_state(state: &RemoteState) -> anyhow::Result<()> {
    let metadata = state.render.metadata;
    ensure!(
        metadata.cols > 0
            && metadata.rows > 0
            && metadata.cols == state.size.cols
            && metadata.rows == state.size.rows,
        "invalid remote terminal dimensions"
    );
    ensure!(
        state.render.cells.len() == usize::from(metadata.cols) * usize::from(metadata.rows),
        "incomplete remote terminal viewport"
    );
    Ok(())
}

fn drives_tab_chrome(event: &TerminalEvent) -> bool {
    matches!(
        event,
        TerminalEvent::ProgramStatus(_) | TerminalEvent::Title(_) | TerminalEvent::ResetTitle
    )
}

/// Whether a replayed event only restores state. The view handles events as
/// things that just happened: a replayed OSC 133 event would restart its
/// command timer or report an old command as finished, and a bell would set
/// workspace attention. The command lifecycle starts again at the next live
/// prompt instead (audit I18).
fn restores_state(event: &TerminalEvent) -> bool {
    matches!(
        event,
        TerminalEvent::Title(_)
            | TerminalEvent::ResetTitle
            | TerminalEvent::WorkingDirectory(_)
            | TerminalEvent::Progress(_)
            | TerminalEvent::ProgramStatus(_)
            | TerminalEvent::Exit
    )
}

#[derive(Default)]
struct Pending {
    wakeup: bool,
    events: Vec<TerminalEvent>,
    /// Clipboard requests in arrival order, each with the time the host
    /// denies it.
    hosts: VecDeque<(u64, RemoteHostRequest, Instant)>,
}

impl Pending {
    fn extend_events(&mut self, events: Vec<TerminalEvent>) -> anyhow::Result<()> {
        for event in events {
            // A suspended UI keeps only the latest state, one of each
            // repeated notification and the latest OSC 133 events, like the
            // host's queue for slow clients.
            if !coalesce_event(&mut self.events, &event) {
                continue;
            }
            ensure!(self.events.len() < 1024, "session event queue overflow");
            self.events.push(event);
        }
        Ok(())
    }

    fn extend_replay(&mut self, events: Vec<TerminalEvent>) -> anyhow::Result<()> {
        self.extend_events(events.into_iter().filter(restores_state).collect())
    }

    /// The clipboard requests to run now, oldest first. Expired ones were
    /// already denied to the program: running them now would prompt for, or
    /// write, something nobody waits for.
    fn take_host_requests(&mut self, now: Instant) -> Vec<(u64, RemoteHostRequest)> {
        self.hosts
            .drain(..)
            .filter(|(_, _, deadline)| *deadline > now)
            .map(|(id, request, _)| (id, request))
            .collect()
    }
}

/// A command for the input connection, with a channel for its reply when
/// the sender waits for it.
struct Input {
    command: RemoteCommand,
    reply: Option<Sender<anyhow::Result<RemoteReply>>>,
}

/// The grid size this client last asked the host for, and whether a frame
/// has shown it. Frames lag queued resizes, so a different host size means
/// another client resized the pane only after this size was confirmed.
/// Confirmation is recorded as frames arrive, also for panes not on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SizeClaim {
    grid: (u16, u16),
    confirmed: bool,
}

impl SizeClaim {
    /// A created or attached pane starts out claiming the host's size.
    fn attached(state: &RemoteState) -> Self {
        Self {
            grid: grid(state),
            confirmed: true,
        }
    }
    fn requested(&mut self, size: TerminalSize, state: &RemoteState) {
        let size = size.clamped();
        self.grid = (size.cols, size.rows);
        self.confirmed = grid(state) == self.grid;
    }
    fn published(&mut self, state: &RemoteState) {
        self.confirmed |= grid(state) == self.grid;
    }
    fn overridden(self, state: &RemoteState) -> bool {
        self.confirmed && grid(state) != self.grid
    }
}

fn grid(state: &RemoteState) -> (u16, u16) {
    (state.size.cols, state.size.rows)
}

struct ClientState {
    /// Locked before `state` wherever both are held.
    size_claim: Mutex<SizeClaim>,
    state: Mutex<Arc<RemoteState>>,
    state_updated: Condvar,
    pending: Mutex<Pending>,
    wakeup: Option<TerminalWakeupNotifier>,
    wakeup_enabled: AtomicBool,
    disconnected: AtomicBool,
    closing: AtomicBool,
}

impl ClientState {
    fn publish_state(&self, state: Arc<RemoteState>) {
        let mut claim = self.size_claim.lock().unwrap();
        claim.published(&state);
        *self.state.lock().unwrap() = state;
        drop(claim);
        self.pending.lock().unwrap().wakeup = true;
        self.state_updated.notify_all();
    }

    fn wait_for_generation(&self, generation: u64) -> anyhow::Result<()> {
        let (state, _) = self
            .state_updated
            .wait_timeout_while(
                self.state.lock().unwrap(),
                Duration::from_secs(5),
                |state| {
                    state.render.metadata.generation < generation
                        && !self.disconnected.load(Ordering::Acquire)
                },
            )
            .unwrap();
        ensure!(
            !self.disconnected.load(Ordering::Acquire),
            "terminal session disconnected"
        );
        ensure!(
            state.render.metadata.generation >= generation,
            "terminal viewport did not catch up"
        );
        Ok(())
    }

    fn notify(&self) {
        if self.wakeup_enabled.load(Ordering::Acquire)
            && let Some(wakeup) = &self.wakeup
        {
            wakeup.notify();
        }
    }
    // Titles and program status drive tab chrome, which stays visible for
    // hidden tabs, so they wake the host even while render wakeups are suspended.
    fn notify_tab_chrome(&self) {
        if let Some(wakeup) = &self.wakeup {
            wakeup.notify();
        }
    }
    fn fail(&self, error: &anyhow::Error) {
        if self.closing.load(Ordering::Acquire) || self.disconnected.swap(true, Ordering::AcqRel) {
            return;
        }
        log::error!("terminal session connection lost: {error}");
        // Pair the notification with the state mutex used by scroll waiters.
        let state = self.state.lock().unwrap();
        self.state_updated.notify_all();
        drop(state);
        self.pending
            .lock()
            .unwrap()
            .events
            .push(TerminalEvent::Title(
                "Session disconnected — reopen Termy to reconnect".into(),
            ));
        self.notify();
    }
}

struct ClientTerminal {
    id: String,
    rpc: Connection,
    host: Arc<Connection>,
    shared: Arc<ClientState>,
    input: Sender<Input>,
    close_stream: TcpStream,
}

impl Drop for ClientTerminal {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::Release);
        let _ = self.close_stream.shutdown(Shutdown::Both);
    }
}

impl ClientTerminal {
    // The host publishes the new frame before replying, so selection waits for
    // one subscription update rather than attaching for a second full state.
    fn viewport_request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
        let result = (|| {
            let Response::Viewport {
                changed,
                generation,
            } = self.rpc.request(Request::ViewportCommand {
                pane: self.id.clone(),
                command,
            })?
            else {
                bail!("invalid terminal viewport response");
            };
            if changed {
                self.shared.wait_for_generation(generation)?;
            }
            Ok(RemoteReply::Changed(changed))
        })();
        if let Err(error) = &result {
            self.shared.fail(error);
        }
        result
    }
}

impl RemoteTransport for ClientTerminal {
    fn state(&self) -> Arc<RemoteState> {
        Arc::clone(&self.shared.state.lock().unwrap())
    }
    fn request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
        let changes_viewport = matches!(
            command,
            RemoteCommand::Scroll(_)
                | RemoteCommand::ScrollToBottom
                | RemoteCommand::ClearScrollback
        );
        if changes_viewport {
            return self.viewport_request(command);
        }
        match self.rpc.request(Request::Command {
            pane: self.id.clone(),
            command,
        }) {
            Ok(Response::Reply(reply)) => Ok(reply),
            Ok(_) => bail!("invalid terminal response"),
            Err(error) => {
                self.shared.fail(&error);
                Err(error)
            }
        }
    }
    fn send(&self, command: RemoteCommand) {
        if self.shared.disconnected.load(Ordering::Acquire) {
            return;
        }
        if let RemoteCommand::Resize(size) = &command {
            let mut claim = self.shared.size_claim.lock().unwrap();
            claim.requested(*size, &self.shared.state.lock().unwrap());
        }
        let input = Input {
            command,
            reply: None,
        };
        if let Err(error) = self.input.send_timeout(input, Duration::from_millis(100)) {
            self.shared.fail(&anyhow::anyhow!(error.to_string()));
        }
    }
    fn send_and_wait(&self, command: RemoteCommand) -> anyhow::Result<Option<RemoteReply>> {
        ensure!(
            !self.shared.disconnected.load(Ordering::Acquire),
            "terminal session disconnected"
        );
        let (tx, rx) = flume::bounded(1);
        let input = Input {
            command,
            reply: Some(tx),
        };
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        self.input
            .send_deadline(input, deadline)
            .map_err(|_| anyhow::anyhow!("terminal input queue is full"))?;
        // Queued: the host may run it whatever happens next, so a failure
        // from here on is not a reason to send it another way.
        Ok(rx.recv_deadline(deadline).ok().and_then(Result::ok))
    }
    fn take_events(&self) -> Vec<TerminalEvent> {
        let mut pending = self.shared.pending.lock().unwrap();
        let mut events = std::mem::take(&mut pending.events);
        let wakeup = std::mem::take(&mut pending.wakeup);
        drop(pending);
        if wakeup {
            events.push(TerminalEvent::Wakeup);
        }
        events
    }
    fn take_host_requests(&self) -> Vec<(u64, RemoteHostRequest)> {
        self.shared
            .pending
            .lock()
            .unwrap()
            .take_host_requests(Instant::now())
    }
    fn reply_to_host(&self, id: u64, reply: RemoteHostReply) {
        if let Err(error) = self.host.request(Request::HostReply {
            pane: self.id.clone(),
            id,
            reply,
        }) {
            self.shared.fail(&error);
        }
    }
    fn has_pending_events(&self) -> bool {
        let pending = self.shared.pending.lock().unwrap();
        pending.wakeup || !pending.events.is_empty() || !pending.hosts.is_empty()
    }
    fn set_wakeup_enabled(&self, enabled: bool) {
        self.shared.wakeup_enabled.store(enabled, Ordering::Release);
        if enabled && self.has_pending_events() {
            self.shared.notify();
        }
    }
    fn host_size_overridden(&self) -> bool {
        let claim = self.shared.size_claim.lock().unwrap();
        claim.overridden(&self.shared.state.lock().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(cols: u16, rows: u16) -> TerminalSize {
        TerminalSize {
            cols,
            rows,
            ..TerminalSize::default()
        }
    }

    fn frame(cols: u16, rows: u16) -> RemoteState {
        RemoteState::capture(&Terminal::new_display(size(cols, rows), None))
    }

    #[test]
    fn size_claim_reports_an_override_only_after_a_frame_showed_this_size() {
        let mut claim = SizeClaim::attached(&frame(80, 24));
        assert!(!claim.overridden(&frame(80, 24)));
        // Created or reattached at this size: another client's resize counts.
        assert!(claim.overridden(&frame(40, 10)));

        claim.requested(size(120, 40), &frame(80, 24));
        // The resize is still queued: the host reports its old size.
        claim.published(&frame(80, 24));
        assert!(!claim.overridden(&frame(80, 24)));
        // A frame showed it (whether or not a view drew that frame), then
        // another client resized the pane.
        claim.published(&frame(120, 40));
        claim.published(&frame(40, 10));
        assert!(claim.overridden(&frame(40, 10)));

        // Asking for the size the host already has is confirmed at once.
        claim.requested(size(40, 10), &frame(40, 10));
        assert!(!claim.overridden(&frame(40, 10)));
        assert!(claim.overridden(&frame(41, 10)));
    }

    #[test]
    fn first_streamed_images_invalidate_the_pending_graphics_snapshot() {
        let terminal = Terminal::new_display(crate::TerminalSize::default(), None);
        terminal.feed_output(b"\x1b_Ga=T,i=1,f=32,s=1,v=1,C=1,q=2;AQID/w==\x1b\\");
        let ready = RemoteState::capture(&terminal);
        assert_eq!(ready.graphics.as_ref().unwrap().len(), 1);
        let pending = awaiting_graphics(ready.clone());
        assert!(pending.graphics.as_ref().unwrap().is_empty());
        // A renderer may cache the initial empty snapshot before the reader
        // publishes these same host placements. Its revision must then change
        // even when there is no further PTY output or window resize.
        assert_ne!(pending.graphics_revision, ready.graphics_revision);
    }

    #[test]
    fn program_status_snapshots_do_not_accumulate_for_slow_clients() {
        let mut pending = Pending::default();
        pending.extend_events(vec![TerminalEvent::Bell]).unwrap();
        for _ in 0..2048 {
            pending
                .extend_events(vec![TerminalEvent::ProgramStatus(vec![])])
                .unwrap();
        }
        assert_eq!(pending.events.len(), 2);
        assert!(matches!(pending.events[0], TerminalEvent::Bell));
        assert!(matches!(pending.events[1], TerminalEvent::ProgramStatus(_)));
    }

    #[test]
    fn late_reply_after_timeout_is_not_returned_for_the_next_request() {
        use std::net::TcpListener;
        let temp = crate::multiplexer::discovery::private_tempdir();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write(
            temp.path().join("endpoint.json"),
            serde_json::json!({ "version": VERSION, "port": port, "token": "token" }).to_string(),
        )
        .unwrap();
        let host = std::thread::spawn(move || {
            let accept = || {
                let (mut stream, _) = listener.accept().unwrap();
                let Request::Authenticate { client_nonce, .. } = read_message(&mut stream).unwrap()
                else {
                    panic!("expected authentication");
                };
                let server_nonce = [2; 32];
                let server_proof = host_proof("token", &client_nonce, &server_nonce);
                let challenge = Response::Challenge {
                    server_nonce,
                    server_proof,
                };
                write_message(&mut stream, &challenge).unwrap();
                let _: Request = read_message(&mut stream).unwrap();
                write_message(&mut stream, &Response::Ok).unwrap();
                let _: Request = read_message(&mut stream).unwrap();
                stream
            };
            // The first request is answered only after the client gave up.
            let mut slow = accept();
            std::thread::sleep(Duration::from_millis(300));
            let layout = |text: &str| Response::Layout {
                revision: 1,
                layout: Some(text.into()),
            };
            let _ = write_message(&mut slow, &layout("first"));
            let mut next = accept();
            write_message(&mut next, &layout("second")).unwrap();
        });

        let connection = Connection::connect(temp.path()).unwrap();
        let short_timeout = Some(Duration::from_millis(100));
        let stream = connection.stream.lock().unwrap();
        stream
            .as_ref()
            .unwrap()
            .set_read_timeout(short_timeout)
            .unwrap();
        drop(stream);
        assert!(connection.request(Request::GetLayout).is_err());
        let Response::Layout { layout, .. } = connection.request(Request::GetLayout).unwrap()
        else {
            panic!("expected a layout reply");
        };
        assert_eq!(layout.as_deref(), Some("second"));
        host.join().unwrap();
    }

    #[test]
    fn command_finish_and_prompt_reach_the_view_in_order() {
        let mut pending = Pending::default();
        pending
            .extend_events(vec![
                TerminalEvent::ShellCommandFinished(Some(1)),
                TerminalEvent::ShellPromptStart,
            ])
            .unwrap();
        assert!(matches!(
            pending.events.as_slice(),
            [
                TerminalEvent::ShellCommandFinished(Some(1)),
                TerminalEvent::ShellPromptStart
            ]
        ));
    }

    #[test]
    fn suspended_ui_queue_survives_a_flood_of_prompts() {
        let mut pending = Pending::default();
        for code in 0..10_000 {
            pending
                .extend_events(vec![
                    TerminalEvent::ShellCommandExecuting,
                    TerminalEvent::ShellCommandFinished(Some(code)),
                    TerminalEvent::ShellPromptStart,
                ])
                .unwrap();
        }
        assert!(pending.events.len() <= 64, "{}", pending.events.len());
        assert!(matches!(
            pending.events.last(),
            Some(TerminalEvent::ShellPromptStart)
        ));
    }

    #[test]
    fn replayed_events_restore_state_without_reporting_what_happened() {
        let mut pending = Pending::default();
        pending
            .extend_replay(vec![
                TerminalEvent::Title("vim".into()),
                TerminalEvent::WorkingDirectory("/tmp".into()),
                TerminalEvent::ShellCommandExecuting,
                TerminalEvent::ShellCommandFinished(Some(1)),
                TerminalEvent::Bell,
                TerminalEvent::Progress(ProgressState::Indeterminate),
                TerminalEvent::ProgramStatus(vec![]),
                TerminalEvent::Exit,
            ])
            .unwrap();
        assert!(
            matches!(
                pending.events.as_slice(),
                [
                    TerminalEvent::Title(_),
                    TerminalEvent::WorkingDirectory(_),
                    TerminalEvent::Progress(ProgressState::Indeterminate),
                    TerminalEvent::ProgramStatus(_),
                    TerminalEvent::Exit,
                ]
            ),
            "{:?}",
            pending.events
        );
        // Live events after the replay are delivered as they happen.
        pending
            .extend_events(vec![TerminalEvent::ShellCommandExecuting])
            .unwrap();
        assert!(matches!(
            pending.events.last(),
            Some(TerminalEvent::ShellCommandExecuting)
        ));
    }

    /// Audit B7: a request the host already denied must not prompt or write
    /// later, when a hidden view finally drains; the rest run in order.
    #[test]
    fn expired_clipboard_requests_are_dropped_and_the_rest_run_in_order() {
        let mut pending = Pending::default();
        let now = Instant::now();
        let load = || RemoteHostRequest::Load(TerminalClipboardTarget::Clipboard);
        let later = now + Duration::from_secs(60);
        for (id, deadline) in [(1, later), (2, now), (3, later)] {
            pending.hosts.push_back((id, load(), deadline));
        }
        let ids: Vec<_> = pending
            .take_host_requests(now)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, [1, 3]);
        assert!(pending.hosts.is_empty());
    }

    #[test]
    fn suspended_ui_queue_coalesces_bells_progress_and_clipboard_writes() {
        let mut pending = Pending::default();
        for round in 0..2048 {
            pending
                .extend_events(vec![
                    TerminalEvent::Bell,
                    TerminalEvent::Progress(ProgressState::InProgress((round % 100) as u8)),
                    TerminalEvent::ClipboardStore(format!("copy {round}")),
                ])
                .unwrap();
        }
        assert_eq!(pending.events.len(), 3);
        assert!(matches!(pending.events[0], TerminalEvent::Bell));
        assert!(matches!(
            pending.events[1],
            TerminalEvent::Progress(ProgressState::InProgress(47))
        ));
        assert!(
            matches!(&pending.events[2], TerminalEvent::ClipboardStore(text) if text == "copy 2047")
        );
    }
}
