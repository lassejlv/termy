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
/// Commands a view may queue for the input connection before typed input is
/// dropped. Commands that replace their predecessor are always queued.
const INPUT_CAPACITY: usize = 256;
/// Pauses between attempts to re-subscribe after a lost stream.
const REATTACH_MIN_DELAY: Duration = Duration::from_millis(100);
const REATTACH_MAX_DELAY: Duration = Duration::from_secs(5);
/// A stream that lasted this long was healthy: the next loss is retried at
/// once. One lost sooner keeps backing off, so a host that drops each new
/// stream is not reconnected in a loop.
const HEALTHY_STREAM: Duration = Duration::from_secs(5);
/// Streams in a row that end before showing a frame before the tab says it
/// is disconnected, though each new subscription succeeded.
const UNHEALTHY_STREAMS: u32 = 3;
/// The pause before subscribing again after the host sent something this
/// client cannot read, or after [`UNHEALTHY_STREAMS`]: retrying sooner would
/// only fail the same way.
const PROTOCOL_RETRY_DELAY: Duration = Duration::from_secs(30);
/// How long a host that is stopping may hold its lock before
/// [`SessionClient::reconnect`] gives up waiting to start a new one.
const STOPPING_HOST_GRACE: Duration = Duration::from_secs(8);
/// How long a `Create` reply that came too late is still waited for, so the
/// pane it started can be closed.
const LATE_REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// The host answered a request with an error: it is reachable, and only this
/// request failed.
#[derive(Debug)]
struct RequestFailed(String);

impl std::fmt::Display for RequestFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RequestFailed {}

/// Whether `error`, from a [`SessionClient`] call, means that the host could
/// not be reached (or answered something unreadable), rather than that it
/// refused the request. Starting a new host may help then (see
/// [`SessionClient::reconnect`]).
pub fn is_connection_error(error: &anyhow::Error) -> bool {
    discovery::is_host_unavailable(error) || !error.chain().any(|cause| cause.is::<RequestFailed>())
}

/// The host refused a subscription: the pane ended, or never existed.
#[derive(Debug)]
struct PaneGone(String);

impl std::fmt::Display for PaneGone {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            formatter,
            "cannot subscribe to terminal session: {}",
            self.0
        )
    }
}

impl std::error::Error for PaneGone {}

/// The host that served a subscription is gone: it stopped, another one
/// replaced it, or one of another protocol version runs now. Its panes ended
/// with it.
#[derive(Debug)]
struct HostGone(String);

impl std::fmt::Display for HostGone {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "the multiplexer host stopped: {}", self.0)
    }
}

impl std::error::Error for HostGone {}

/// Whether reading timed out, as opposed to the connection failing.
fn timed_out(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            )
        })
    })
}

/// Whether a subscription stream ended because its bytes could not be
/// read, rather than because the connection failed.
fn is_protocol_error(error: &anyhow::Error) -> bool {
    !error
        .chain()
        .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
}

/// Something the client gets late: a reply it stopped waiting for.
type LateReply = Box<dyn FnOnce(Response) + Send>;

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
        self.exchange(request, None)
    }

    /// Like [`Self::request`]. When the reply does not come in time, a
    /// thread keeps waiting for it and hands it to `late`.
    fn exchange(&self, request: Request, late: Option<LateReply>) -> anyhow::Result<Response> {
        let mut slot = self.stream.lock().unwrap();
        let stream = match slot.as_mut() {
            Some(stream) => stream,
            None => slot.insert(open_rpc_stream(&self.root)?),
        };
        if let Err(error) = write_message(stream, &request) {
            *slot = None;
            return Err(error);
        }
        let response = match read_message(stream) {
            Ok(response) => response,
            Err(error) => {
                // A timed-out or broken exchange leaves the stream mid-reply:
                // its late answer would be read as the next request's reply.
                let stream = slot.take();
                // A host that is gone is reported as such, so callers can
                // tell it from a request that failed (`is_host_unavailable`).
                if !timed_out(&error) && !discovery::host_is_running(&self.root) {
                    return Err(discovery::no_host(&self.root).context(format!("{error:#}")));
                }
                if let (Some(late), Some(mut stream)) = (late, stream)
                    && timed_out(&error)
                {
                    let _ = std::thread::Builder::new()
                        .name("termy-session-late-reply".into())
                        .spawn(move || {
                            let _ = stream.set_read_timeout(Some(LATE_REPLY_TIMEOUT));
                            if let Ok(response) = read_message(&mut stream) {
                                late(response);
                            }
                        });
                }
                return Err(error);
            }
        };
        drop(slot);
        match response {
            Response::Error(error) => Err(RequestFailed(error).into()),
            response => Ok(response),
        }
    }

    /// Drops the open stream, so the next request connects anew.
    fn reset(&self) {
        *self.stream.lock().unwrap() = None;
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
    /// The program that starts a host here, when this client came from
    /// [`connect_or_start`](crate::multiplexer::connect_or_start).
    executable: Option<PathBuf>,
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
            executable: None,
        })
    }

    pub(crate) fn starting_hosts_with(mut self, executable: &Path) -> Self {
        self.executable = Some(executable.to_owned());
        self
    }

    /// Makes sure a host runs again after the one this client used died or
    /// was shut down, starting a new one like
    /// [`connect_or_start`](crate::multiplexer::connect_or_start) (under
    /// `launch.lock`, so concurrent callers start one host). Every clone of
    /// this client then reaches it: connections find the host anew through
    /// `endpoint.json`. The old host's panes are gone; their views get
    /// `Exit`. Fails for a client made by [`Self::connect`], which does not
    /// know how to start a host.
    pub fn reconnect(&self) -> anyhow::Result<()> {
        let Some(executable) = &self.executable else {
            bail!("this client cannot start a session host");
        };
        // The open stream may lead to the dead host.
        self.connection.reset();
        match crate::multiplexer::connect_or_start(&self.root, executable) {
            Ok(_) => Ok(()),
            // A host that is stopping holds its lock until it has ended its
            // terminals, and a client's probe holds it for a moment; wait
            // for it rather than give up on a host that is about to go.
            Err(error)
                if discovery::is_unreachable(&error)
                    && !discovery::is_version_mismatch(&error)
                    && discovery::wait_for_exit(&self.root, STOPPING_HOST_GRACE) =>
            {
                log::info!("waited for a stopping session host: {error:#}");
                crate::multiplexer::connect_or_start(&self.root, executable).map(drop)
            }
            Err(error) => Err(error),
        }
    }

    /// Creates a pane and attaches a view to it, which answers the pane's
    /// clipboard requests (see [`Self::attach`]).
    pub fn create(
        &self,
        launch: PaneLaunch,
        wakeup: Option<TerminalWakeupNotifier>,
    ) -> anyhow::Result<(String, Terminal)> {
        let (info, state) = self.create_pane(launch)?;
        match self.attach_state(&info.id, state, wakeup, true) {
            Ok(terminal) => Ok((info.id, terminal)),
            Err(error) => {
                // Nobody would know of the new pane, or ever close it (audit L3).
                if let Err(close) = self.close(&info.id) {
                    log::warn!(
                        "cannot close terminal session {} after a failed attach: {close:#}",
                        info.id
                    );
                }
                Err(error)
            }
        }
    }

    /// Creates a pane without attaching to it, for clients that only start
    /// programs. Returns the pane ID.
    pub fn create_detached(&self, launch: PaneLaunch) -> anyhow::Result<String> {
        Ok(self.create_pane(launch)?.0.id)
    }

    fn create_pane(&self, launch: PaneLaunch) -> anyhow::Result<(PaneInfo, Arc<RemoteState>)> {
        // A reply that comes after this client gave up names a pane nobody
        // knows of: close it (audit L3).
        let root = self.root.clone();
        let late: LateReply = Box::new(move |response| {
            let Response::Attached(info, _) = response else {
                return;
            };
            log::warn!(
                "closing terminal session {}: it was created after its client gave up",
                info.id
            );
            let closed = SessionClient::connect(&root).and_then(|client| client.close(&info.id));
            if let Err(error) = closed {
                log::warn!("cannot close terminal session {}: {error:#}", info.id);
            }
        });
        let Response::Attached(info, state) = self
            .connection
            .exchange(Request::Create(Box::new(launch)), Some(late))?
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
        Ok(Terminal::from_remote(self.attach_transport(
            id,
            state,
            wakeup,
            host_capable,
        )?))
    }

    fn attach_transport(
        &self,
        id: &str,
        state: Arc<RemoteState>,
        wakeup: Option<TerminalWakeupNotifier>,
        host_capable: bool,
    ) -> anyhow::Result<Arc<ClientTerminal>> {
        validate_state(&state)?;
        let rpc = Connection::connect(&self.root)?;
        let input = Connection::connect(&self.root)?;
        let (mut stream, host) = Subscription::open_on(&self.root, id, host_capable, None)?;
        let subscription = Subscription {
            root: self.root.clone(),
            pane: id.to_owned(),
            host_capable,
            host,
        };
        let initial_state = awaiting_graphics((*state).clone());
        let shared = Arc::new(ClientState {
            size_claim: Mutex::new(SizeClaim::attached(&initial_state)),
            state: Mutex::new(Arc::new(initial_state)),
            state_updated: Condvar::new(),
            pending: Mutex::new(Pending::default()),
            wakeup,
            wakeup_enabled: AtomicBool::new(true),
            disconnected: AtomicBool::new(false),
            host_lost: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            damage: Mutex::new(crate::remote::frame::DamageTracker::default()),
            stream: Mutex::new(Some(control_handle(&stream)?)),
            closed: Condvar::new(),
            paused: Mutex::new(false),
            frames_paused: AtomicBool::new(false),
            settings: Mutex::new(Vec::new()),
        });
        let input_queue = Arc::new(InputQueue::default());
        let reader_shared = Arc::clone(&shared);
        let reader_input = Arc::clone(&input_queue);
        std::thread::Builder::new()
            .name("termy-session-read".into())
            .spawn(move || {
                let mut delay = Duration::ZERO;
                let mut short_streams = 0;
                let mut resubscribed = false;
                loop {
                    let opened = Instant::now();
                    // A pane that ended sends `Exit`, then its stream ends.
                    let mut ended = false;
                    // The host is shutting down: the pane ends with it.
                    let mut host_stopping = false;
                    // Whether this stream delivered a frame.
                    let mut shown = false;
                    // Each subscription starts with a full frame and its
                    // images anew.
                    let mut frames = FrameReader::default();
                    let result = (|| -> anyhow::Result<std::convert::Infallible> {
                        loop {
                            let mut tab_chrome = false;
                            match read_message(&mut stream)? {
                                Update::State(_)
                                | Update::RetiredHost
                                | Update::RetiredGraphicsState => {
                                    bail!("unexpected terminal update")
                                }
                                Update::Image(image) => {
                                    frames.graphics.insert(image);
                                    continue;
                                }
                                Update::Frame(frame, images) => {
                                    let full = frame.is_full();
                                    // Only this thread replaces the state, so it
                                    // is the frame the delta was made against.
                                    let current = reader_shared.state();
                                    match frames.frame(&current, *frame, images) {
                                        Ok(Some((state, change))) => {
                                            reader_shared.publish_state(Arc::new(state), change);
                                            shown = true;
                                            // A new subscription works once its
                                            // first frame is shown.
                                            if std::mem::take(&mut resubscribed) {
                                                reader_shared.resubscribed(&reader_input);
                                            }
                                        }
                                        Ok(None) => continue,
                                        // Even a full frame does not fit: start
                                        // a new subscription (audit B9).
                                        Err(error) if full => return Err(error),
                                        Err(error) => {
                                            // Ask for a full frame rather than
                                            // give up on the stream.
                                            log::warn!("terminal frame skipped: {error:#}");
                                            reader_shared.send_control(&Request::Resync)?;
                                            continue;
                                        }
                                    }
                                }
                                Update::Paused(paused) => {
                                    frames.paused(paused, &reader_shared.state());
                                    reader_shared.frames_paused.store(paused, Ordering::Release);
                                    continue;
                                }
                                Update::HostShutdown => host_stopping = true,
                                Update::Events(events) => {
                                    tab_chrome = events.iter().any(drives_tab_chrome);
                                    ended |= events.iter().any(is_exit);
                                    reader_shared
                                        .pending
                                        .lock()
                                        .unwrap()
                                        .extend_events(events)?;
                                }
                                Update::Replay(events) => {
                                    tab_chrome = events.iter().any(drives_tab_chrome);
                                    ended |= events.iter().any(is_exit);
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
                    // The end of a pane, or of this view, is not a failure
                    // (audit B10).
                    if ended || reader_shared.closing.load(Ordering::Acquire) {
                        return;
                    }
                    let Err(error) = result;
                    if host_stopping {
                        reader_shared.host_lost(&HostGone("it shut down".into()).into());
                        return;
                    }
                    if opened.elapsed() >= HEALTHY_STREAM {
                        delay = Duration::ZERO;
                    }
                    short_streams = if shown { 0 } else { short_streams + 1 };
                    // A host whose updates cannot be read, or that keeps
                    // dropping new streams, will do it again: say so, and
                    // retry slowly (audit B9).
                    let failing = is_protocol_error(&error) || short_streams >= UNHEALTHY_STREAMS;
                    if failing {
                        reader_shared.disconnect(&error);
                        delay = delay.max(PROTOCOL_RETRY_DELAY);
                    }
                    let max_delay = if failing {
                        PROTOCOL_RETRY_DELAY
                    } else {
                        REATTACH_MAX_DELAY
                    };
                    match reader_shared.reattach(&subscription, &error, &mut delay, max_delay) {
                        Some(reopened) => stream = reopened,
                        None => return,
                    }
                    resubscribed = true;
                }
            })?;
        let input_shared = Arc::clone(&shared);
        let input_rx = Arc::clone(&input_queue);
        let pane_id = id.to_owned();
        std::thread::Builder::new()
            .name("termy-session-input".into())
            .spawn(move || {
                while let Some(Input { command, reply }) = input_rx.pop() {
                    // While the stream is lost, the host is out of reach or
                    // the pane is gone: drop the input rather than reconnect
                    // for each key (audit I3). The tab title says why; the
                    // program is not to blame, so no `InputDropped`. The
                    // latest size and options are sent again on reattach.
                    if input_shared.disconnected.load(Ordering::Acquire) {
                        if let Some(reply) = reply {
                            let _ =
                                reply.send(Err(anyhow::anyhow!("terminal session disconnected")));
                        }
                        continue;
                    }
                    let response = input.request(Request::Command {
                        pane: pane_id.clone(),
                        command,
                    });
                    // An error fails this command only; the pane stays
                    // usable (audit B9). Only a lost stream disconnects it.
                    // A write's reply, `Written` or `InputRejected`, goes
                    // to a sender that waits for it.
                    let result = response.and_then(|response| match response {
                        Response::Reply(result) => Ok(result),
                        _ => Err(anyhow::anyhow!("invalid terminal response")),
                    });
                    if let Err(error) = &result {
                        log::warn!("terminal input was not delivered: {error:#}");
                    }
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                    }
                }
            })?;
        Ok(Arc::new(ClientTerminal {
            id: id.to_owned(),
            rpc,
            host: Arc::clone(&self.connection),
            shared,
            input: input_queue,
        }))
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
    /// Input that does not reach the program fails with an
    /// [`InputRejection`] (audit B24). Returns the line the input was typed
    /// on, as an anchor for [`Self::line_texts_anchored`] (see
    /// [`RemoteReply::Written`]).
    pub fn write(&self, id: &str, bytes: Vec<u8>) -> anyhow::Result<u64> {
        match self.connection.request(Request::Command {
            pane: id.to_owned(),
            command: RemoteCommand::Write(bytes),
        })? {
            Response::Reply(RemoteReply::InputRejected(rejection)) => Err(rejection.into()),
            Response::Reply(RemoteReply::Written { anchor }) => Ok(anchor),
            _ => bail!("invalid terminal command response"),
        }
    }
    /// The text of `id`'s buffer lines `first..=last`, without subscribing.
    /// Lines are numbered from the top of the screen: 0 is its first row and
    /// -1 the newest history line. Long ranges are read in several requests,
    /// numbered as the first one even when output scrolls meanwhile. Lines
    /// outside the buffer are left out; `first` of the result is the first
    /// line returned.
    pub fn line_texts(&self, id: &str, first: i32, last: i32) -> anyhow::Result<LineTexts> {
        self.line_texts_anchored(id, first, last, None)
    }
    /// [`Self::line_texts`] numbered as an earlier reply was: `anchor` is its
    /// `lines_scrolled`, so line numbers stay put while output scrolls.
    pub fn line_texts_anchored(
        &self,
        id: &str,
        first: i32,
        last: i32,
        anchor: Option<u64>,
    ) -> anyhow::Result<LineTexts> {
        let mut failure = None;
        let texts = read_line_texts(first, last, anchor, |command| {
            match self.connection.request(Request::Command {
                pane: id.to_owned(),
                command,
            }) {
                Ok(Response::Reply(reply)) => Some(reply),
                Ok(_) => {
                    failure = Some(anyhow::anyhow!("invalid line text response"));
                    None
                }
                Err(error) => {
                    failure = Some(error);
                    None
                }
            }
        });
        match (texts, failure) {
            (Some(texts), _) => Ok(texts),
            (None, Some(error)) => Err(error),
            (None, None) => bail!("invalid line text response"),
        }
    }
    /// The cell metrics (pixels) the attached UI last applied to `id`.
    pub(crate) fn cell_size(&self, id: &str) -> anyhow::Result<(f32, f32)> {
        let size = self.size(id)?;
        Ok((size.cell_width, size.cell_height))
    }
    /// The size of `id`, with the cell metrics its UI last applied.
    pub fn size(&self, id: &str) -> anyhow::Result<TerminalSize> {
        let Response::Size(size) = self.connection.request(Request::Size(id.to_owned()))? else {
            bail!("invalid size response");
        };
        Ok(size)
    }
    /// The settings `id` was started with, for a pane split from it.
    pub fn launch_config(&self, id: &str) -> anyhow::Result<LaunchConfig> {
        let Response::LaunchConfig(config) = self
            .connection
            .request(Request::LaunchConfig(id.to_owned()))?
        else {
            bail!("invalid launch config response");
        };
        Ok(*config)
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

/// Events the window shows while their tab is hidden: tab titles and
/// status, an ended pane, and a finished command's workspace attention
/// (audit B11).
fn drives_tab_chrome(event: &TerminalEvent) -> bool {
    matches!(
        event,
        TerminalEvent::ProgramStatus(_)
            | TerminalEvent::Title(_)
            | TerminalEvent::ResetTitle
            | TerminalEvent::Exit
            | TerminalEvent::ShellCommandFinished(_)
    )
}

fn is_exit(event: &TerminalEvent) -> bool {
    matches!(event, TerminalEvent::Exit)
}

/// Commands whose latest value is all that matters: a newer one replaces
/// the queued one instead of waiting behind it, so they are never dropped.
fn replaces_queued(command: &RemoteCommand) -> bool {
    matches!(
        command,
        RemoteCommand::Resize(_)
            | RemoteCommand::NudgeResize
            | RemoteCommand::SetOptions(_)
            | RemoteCommand::SetScrollback(_)
            | RemoteCommand::SetQueryColors(_)
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

/// Decodes the frames of one subscription (audit I1, I2).
#[derive(Default)]
struct FrameReader {
    graphics: graphics::GraphicsDecoder,
    /// Waiting for the full frame asked for with [`Request::Resync`].
    resyncing: bool,
}

impl FrameReader {
    /// The state `frame` describes on top of `shown`, and what changed.
    /// `None` while deltas are ignored until a full frame. An error means
    /// the frame does not fit what this client has: the caller asks the host
    /// for a full frame, and deltas are ignored until it arrives.
    fn frame(
        &mut self,
        shown: &RemoteState,
        frame: crate::remote::frame::FrameUpdate,
        images: graphics::GraphicsUpdate,
    ) -> anyhow::Result<Option<(RemoteState, crate::remote::frame::FrameChange)>> {
        if self.resyncing && !frame.is_full() {
            return Ok(None);
        }
        let decoded = (|| {
            let (mut state, change) = frame.apply(shown)?;
            validate_state(&state)?;
            state.graphics = Some(self.graphics.decode(images)?);
            anyhow::Ok((state, change))
        })();
        match decoded {
            Ok(decoded) => {
                if std::mem::take(&mut self.resyncing) {
                    // Images sent before the host started afresh are unknown
                    // to it now; keep only those on screen.
                    self.graphics
                        .retain_shown(decoded.0.graphics.as_deref().unwrap_or_default());
                }
                Ok(Some(decoded))
            }
            Err(error) => {
                self.resyncing = true;
                self.graphics = graphics::GraphicsDecoder::default();
                Err(error)
            }
        }
    }

    /// Follows [`Update::Paused`]: a hidden view keeps only the images its
    /// frame shows; on resume the host sends every image again.
    fn paused(&mut self, paused: bool, shown: &RemoteState) {
        if paused {
            self.graphics
                .retain_shown(shown.graphics.as_deref().unwrap_or_default());
        } else {
            self.graphics = graphics::GraphicsDecoder::default();
        }
    }
}

/// A command for the input connection, with a channel for its reply when
/// the sender waits for it.
struct Input {
    command: RemoteCommand,
    reply: Option<Sender<anyhow::Result<RemoteReply>>>,
}

/// Commands waiting for the input connection. Queuing never blocks the UI
/// (audit B9): a command that [`replaces_queued`] takes the place of the
/// queued one of its kind, a mouse-motion report replaces one still waiting
/// at the end of the queue (audit B22), and once [`INPUT_CAPACITY`]
/// commands wait, motion is dropped quietly and other input is dropped and
/// reported as `InputDropped`.
#[derive(Default)]
struct InputQueue {
    queue: Mutex<InputItems>,
    /// Signalled when a command is queued or the queue closes.
    ready: Condvar,
    /// Signalled when a command leaves the queue.
    space: Condvar,
}

#[derive(Default)]
struct InputItems {
    items: VecDeque<Input>,
    closed: bool,
}

/// What [`InputQueue::push`] did with a command.
#[derive(Debug, PartialEq, Eq)]
enum Queued {
    Yes,
    /// A motion report that the next one makes up for.
    DroppedMotion,
    /// Input the program will never see.
    Dropped,
}

impl InputQueue {
    /// Queues `input` without waiting.
    fn push(&self, input: Input) -> Queued {
        let mut queue = self.queue.lock().unwrap();
        if let RemoteCommand::WriteMotion(bytes) = &input.command {
            // Hovering over a busy pane queues one report, not hundreds. A
            // report never moves past other input.
            if let Some(Input {
                command: RemoteCommand::WriteMotion(waiting),
                reply: None,
            }) = queue.items.back_mut()
            {
                waiting.clone_from(bytes);
                return Queued::Yes;
            }
            if queue.items.len() >= INPUT_CAPACITY {
                return Queued::DroppedMotion;
            }
        } else if replaces_queued(&input.command) {
            let kind = std::mem::discriminant(&input.command);
            queue.items.retain(|queued| {
                queued.reply.is_some() || std::mem::discriminant(&queued.command) != kind
            });
        } else if queue.items.len() >= INPUT_CAPACITY {
            return Queued::Dropped;
        }
        queue.items.push_back(input);
        self.ready.notify_one();
        Queued::Yes
    }

    /// Queues `input`, waiting until `deadline` for room.
    fn push_until(&self, input: Input, deadline: Instant) -> anyhow::Result<()> {
        let mut queue = self.queue.lock().unwrap();
        while queue.items.len() >= INPUT_CAPACITY {
            let remaining = deadline.saturating_duration_since(Instant::now());
            ensure!(!remaining.is_zero(), "terminal input queue is full");
            queue = self.space.wait_timeout(queue, remaining).unwrap().0;
        }
        queue.items.push_back(input);
        self.ready.notify_one();
        Ok(())
    }

    /// The next command; `None` once the queue closed and is empty.
    fn pop(&self) -> Option<Input> {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if let Some(input) = queue.items.pop_front() {
                self.space.notify_all();
                return Some(input);
            }
            if queue.closed {
                return None;
            }
            queue = self.ready.wait(queue).unwrap();
        }
    }

    /// Lets the input thread send what is queued, then end.
    fn close(&self) {
        self.queue.lock().unwrap().closed = true;
        self.ready.notify_all();
    }
}

/// How to subscribe to a pane again after its stream was lost.
struct Subscription {
    root: PathBuf,
    pane: String,
    host_capable: bool,
    /// The token of the host that served the first subscription. A host
    /// with another token is a new one, which never had this pane.
    host: String,
}

impl Subscription {
    fn open(&self) -> anyhow::Result<TcpStream> {
        Self::open_on(&self.root, &self.pane, self.host_capable, Some(&self.host))
            .map(|(stream, _)| stream)
    }

    /// Subscribes to `pane`, on the host with token `host` when given.
    /// Errors are [`PaneGone`] or [`HostGone`] when retrying cannot help.
    fn open_on(
        root: &Path,
        pane: &str,
        host_capable: bool,
        host: Option<&str>,
    ) -> anyhow::Result<(TcpStream, String)> {
        let (mut stream, token) = discovery::connect_identified(root).map_err(|error| {
            if discovery::is_no_host(&error) || discovery::is_version_mismatch(&error) {
                HostGone(format!("{error:#}")).into()
            } else {
                error
            }
        })?;
        if host.is_some_and(|host| host != token) {
            return Err(HostGone("another host replaced it".into()).into());
        }
        let subscribe = Request::Subscribe {
            pane: pane.to_owned(),
            host_capable,
        };
        write_message(&mut stream, &subscribe)?;
        match read_message(&mut stream)? {
            Response::Ok => {}
            // The host says why (audit I11). Only a missing pane is final;
            // anything else, like a refresh that timed out, may pass.
            Response::PaneNotFound(error) => return Err(PaneGone(error).into()),
            Response::Error(error) if error.starts_with(HOST_STOPPING) => {
                return Err(HostGone(error).into());
            }
            Response::Error(error) => bail!("cannot subscribe to terminal session: {error}"),
            _ => bail!("invalid terminal subscription response"),
        }
        stream.set_read_timeout(None)?;
        Ok((stream, token))
    }
}

/// A handle on a subscription stream that writes [`Request::SetPaused`] and
/// [`Request::Resync`] on it, and shuts it down when the view closes.
fn control_handle(stream: &TcpStream) -> anyhow::Result<TcpStream> {
    let control = stream.try_clone()?;
    // Pausing writes a few bytes the host reads at once; never let it hold
    // up the UI for long.
    control.set_write_timeout(Some(Duration::from_secs(1)))?;
    Ok(control)
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
    /// Set while the subscription stream is lost and re-subscribing fails,
    /// and for good once the pane or its host is gone. Requests fail at
    /// once meanwhile.
    disconnected: AtomicBool,
    /// Set for good once the pane's host is gone (see
    /// [`RemoteTransport::host_lost`]).
    host_lost: AtomicBool,
    /// Set when the view drops the terminal: the stream's end is expected.
    closing: AtomicBool,
    /// Render damage since the renderer last read the state; locked only
    /// while `state` is held.
    damage: Mutex<crate::remote::frame::DamageTracker>,
    /// A handle on the current subscription stream (see
    /// [`control_handle`]), replaced when the client subscribes again;
    /// `None` once the view closed.
    stream: Mutex<Option<TcpStream>>,
    /// Paired with `stream`; signalled when the view closes, to end a
    /// pause between attempts to re-subscribe.
    closed: Condvar,
    /// Whether the view asked the host to pause frames (audit I7). Locked
    /// before `stream`.
    paused: Mutex<bool>,
    /// Whether the host confirmed that it stopped sending frames.
    frames_paused: AtomicBool,
    /// The latest of each command that [`replaces_queued`] (but nudges),
    /// sent again after re-subscribing: ones queued while the stream was
    /// lost were dropped.
    settings: Mutex<Vec<RemoteCommand>>,
}

impl ClientState {
    fn state(&self) -> Arc<RemoteState> {
        Arc::clone(&self.state.lock().unwrap())
    }

    fn send_control(&self, request: &Request) -> anyhow::Result<()> {
        let stream = self.stream.lock().unwrap();
        let Some(stream) = stream.as_ref() else {
            bail!("terminal session closed");
        };
        write_message(&mut &*stream, request)
    }

    fn publish_state(&self, state: Arc<RemoteState>, change: crate::remote::frame::FrameChange) {
        let mut claim = self.size_claim.lock().unwrap();
        claim.published(&state);
        let mut shown = self.state.lock().unwrap();
        // Recorded with the state, so a renderer always takes the damage
        // that leads to the state it reads.
        self.damage.lock().unwrap().record(change, &state);
        *shown = state;
        drop(shown);
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

    fn remember_setting(&self, command: &RemoteCommand) {
        if !replaces_queued(command) || matches!(command, RemoteCommand::NudgeResize) {
            return;
        }
        let kind = std::mem::discriminant(command);
        let mut settings = self.settings.lock().unwrap();
        settings.retain(|setting| std::mem::discriminant(setting) != kind);
        settings.push(command.clone());
    }

    /// A new subscription showed its first frame: the pane is back. Sends
    /// the view's size and options again, which the host may have missed.
    fn resubscribed(&self, input: &InputQueue) {
        self.reconnected();
        let settings = self.settings.lock().unwrap().clone();
        for command in settings {
            if let RemoteCommand::Resize(size) = &command {
                let mut claim = self.size_claim.lock().unwrap();
                claim.requested(*size, &self.state.lock().unwrap());
            }
            input.push(Input {
                command,
                reply: None,
            });
        }
    }

    fn input_dropped(&self) {
        let _ = self
            .pending
            .lock()
            .unwrap()
            .extend_events(vec![TerminalEvent::InputDropped]);
        self.notify();
    }

    /// Marks the stream lost, once per streak of failed attempts to
    /// re-subscribe, with a title on the tab that says so.
    fn disconnect(&self, error: &anyhow::Error) {
        if self.disconnected.swap(true, Ordering::AcqRel) {
            return;
        }
        log::error!("terminal session connection lost: {error:#}");
        self.wake_waiters();
        let _ = self
            .pending
            .lock()
            .unwrap()
            .extend_events(vec![TerminalEvent::Title(
                "Session disconnected — reconnecting…".into(),
            )]);
        self.notify_tab_chrome();
    }

    /// The pane's host is gone, so the pane ended with it, though its
    /// program did not exit. The view keeps the tab, marked as ended
    /// (audit B12): it takes no input and does not reconnect.
    fn host_lost(&self, error: &anyhow::Error) {
        log::warn!("terminal session ended with its host: {error:#}");
        self.disconnected.store(true, Ordering::Release);
        self.host_lost.store(true, Ordering::Release);
        self.wake_waiters();
        let _ = self
            .pending
            .lock()
            .unwrap()
            .extend_events(vec![TerminalEvent::Title(
                "Session ended — the multiplexer host stopped".into(),
            )]);
        self.notify_tab_chrome();
    }

    /// The pane will not come back: it was closed while the stream was
    /// lost. The view handles it like the pane's own exit.
    fn end(&self, error: &anyhow::Error) {
        log::warn!("terminal session ended: {error:#}");
        self.disconnected.store(true, Ordering::Release);
        self.wake_waiters();
        let _ = self
            .pending
            .lock()
            .unwrap()
            .extend_events(vec![TerminalEvent::Exit]);
        self.notify_tab_chrome();
    }

    fn reconnected(&self) {
        if !self.disconnected.swap(false, Ordering::AcqRel) {
            return;
        }
        log::info!("terminal session reconnected");
        // Clears the disconnected title. The new subscription's replay then
        // restores the pane's own title, which replaces this.
        let _ = self
            .pending
            .lock()
            .unwrap()
            .extend_events(vec![TerminalEvent::ResetTitle]);
        self.notify_tab_chrome();
    }

    /// Ends viewport waits early; they check `disconnected`.
    fn wake_waiters(&self) {
        // Pair the notification with the state mutex used by scroll waiters.
        let state = self.state.lock().unwrap();
        self.state_updated.notify_all();
        drop(state);
    }

    /// Subscribes again after the stream was lost (audit B9), retrying with
    /// a `delay` that grows up to `max_delay`. Returns the new stream, or
    /// `None` once the pane or its host is gone or the view closed.
    fn reattach(
        &self,
        subscription: &Subscription,
        error: &anyhow::Error,
        delay: &mut Duration,
        max_delay: Duration,
    ) -> Option<TcpStream> {
        log::warn!("terminal session stream lost, subscribing again: {error:#}");
        loop {
            if !self.pause_unless_closing(*delay) {
                return None;
            }
            *delay = (*delay * 2).clamp(REATTACH_MIN_DELAY, max_delay);
            let opened = subscription.open().and_then(|stream| {
                let handle = control_handle(&stream)?;
                Ok((stream, handle))
            });
            match opened {
                Ok((stream, handle)) => {
                    // A new subscription sends frames until told otherwise:
                    // a hidden view pauses it again (audit I7).
                    let paused = self.paused.lock().unwrap();
                    let mut slot = self.stream.lock().unwrap();
                    if self.closing.load(Ordering::Acquire) {
                        let _ = stream.shutdown(Shutdown::Both);
                        return None;
                    }
                    self.frames_paused.store(false, Ordering::Release);
                    if *paused
                        && let Err(error) = write_message(&mut &handle, &Request::SetPaused(true))
                    {
                        log::warn!("cannot pause terminal session frames: {error:#}");
                    }
                    *slot = Some(handle);
                    return Some(stream);
                }
                Err(error) if error.chain().any(|cause| cause.is::<PaneGone>()) => {
                    self.end(&error);
                    return None;
                }
                Err(error) if error.chain().any(|cause| cause.is::<HostGone>()) => {
                    self.host_lost(&error);
                    return None;
                }
                Err(error) => self.disconnect(&error),
            }
        }
    }

    /// Waits for `delay`; false when the view closed first.
    fn pause_unless_closing(&self, delay: Duration) -> bool {
        let slot = self.stream.lock().unwrap();
        let (_slot, _) = self
            .closed
            .wait_timeout_while(slot, delay, |_| !self.closing.load(Ordering::Acquire))
            .unwrap();
        !self.closing.load(Ordering::Acquire)
    }

    fn close(&self) {
        let mut slot = self.stream.lock().unwrap();
        self.closing.store(true, Ordering::Release);
        if let Some(stream) = slot.take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        drop(slot);
        self.closed.notify_all();
    }
}

struct ClientTerminal {
    id: String,
    rpc: Connection,
    host: Arc<Connection>,
    shared: Arc<ClientState>,
    input: Arc<InputQueue>,
}

impl Drop for ClientTerminal {
    fn drop(&mut self) {
        self.shared.close();
        self.input.close();
    }
}

impl ClientTerminal {
    // The host publishes the new frame before replying, so selection waits for
    // one subscription update rather than attaching for a second full state.
    fn viewport_request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
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
        // A paused view gets no frames until it resumes, which brings this
        // one too.
        if changed && !*self.shared.paused.lock().unwrap() {
            self.shared.wait_for_generation(generation)?;
        }
        Ok(RemoteReply::Changed(changed))
    }

    /// Requests fail at once while the stream is lost, rather than each
    /// reconnecting to a host that is not there (audit I3).
    fn ensure_connected(&self) -> anyhow::Result<()> {
        ensure!(
            !self.shared.disconnected.load(Ordering::Acquire),
            "terminal session disconnected"
        );
        Ok(())
    }
}

impl RemoteTransport for ClientTerminal {
    fn state(&self) -> Arc<RemoteState> {
        self.shared.state()
    }
    fn state_with_damage(&self) -> (Arc<RemoteState>, Option<RemoteDamage>) {
        let state = self.shared.state.lock().unwrap();
        let damage = self.shared.damage.lock().unwrap().take(&state);
        (Arc::clone(&state), damage)
    }
    // Errors fail only this request: the caller logs them, and the pane
    // stays usable (audit B9).
    fn request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply> {
        self.ensure_connected()?;
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
        })? {
            Response::Reply(reply) => Ok(reply),
            _ => bail!("invalid terminal response"),
        }
    }
    fn send(&self, command: RemoteCommand) {
        self.shared.remember_setting(&command);
        if let RemoteCommand::Resize(size) = &command {
            let mut claim = self.shared.size_claim.lock().unwrap();
            claim.requested(*size, &self.shared.state.lock().unwrap());
        }
        let input = Input {
            command,
            reply: None,
        };
        if self.input.push(input) == Queued::Dropped {
            self.shared.input_dropped();
        }
    }
    fn send_and_wait(&self, command: RemoteCommand) -> anyhow::Result<Option<RemoteReply>> {
        self.ensure_connected()?;
        let (tx, rx) = flume::bounded(1);
        let input = Input {
            command,
            reply: Some(tx),
        };
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        self.input.push_until(input, deadline)?;
        // Queued: the host may run it whatever happens next, so a failure
        // from here on is not a reason to send it another way. Its reply may
        // be `Written` or `InputRejected` for a write.
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
            // The program was or will be told it was denied.
            log::warn!("cannot answer a terminal clipboard request: {error:#}");
        }
    }
    fn has_pending_events(&self) -> bool {
        let pending = self.shared.pending.lock().unwrap();
        pending.wakeup || !pending.events.is_empty() || !pending.hosts.is_empty()
    }
    fn set_wakeup_enabled(&self, enabled: bool) {
        // A hidden view needs no frames, only events (audit I7). A new
        // subscription is paused again when it opens (see `reattach`).
        let mut paused = self.shared.paused.lock().unwrap();
        if *paused == enabled {
            *paused = !enabled;
            if !self.shared.disconnected.load(Ordering::Acquire)
                && let Err(error) = self.shared.send_control(&Request::SetPaused(!enabled))
            {
                log::warn!("cannot pause terminal session frames: {error:#}");
            }
        }
        drop(paused);
        self.shared.wakeup_enabled.store(enabled, Ordering::Release);
        if enabled && self.has_pending_events() {
            self.shared.notify();
        }
    }
    fn host_size_overridden(&self) -> bool {
        let claim = self.shared.size_claim.lock().unwrap();
        claim.overridden(&self.shared.state.lock().unwrap())
    }
    fn frames_paused(&self) -> bool {
        self.shared.frames_paused.load(Ordering::Acquire)
    }
    fn host_lost(&self) -> bool {
        self.shared.host_lost.load(Ordering::Acquire)
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

    /// One frame from `frames` and `images`, as the host sends it: images
    /// first.
    fn send(
        frames: &mut crate::remote::frame::FrameEncoder,
        images: &mut graphics::GraphicsEncoder,
        reader: &mut FrameReader,
        terminal: &Terminal,
    ) -> (crate::remote::frame::FrameUpdate, graphics::GraphicsUpdate) {
        let state = RemoteState::capture(terminal);
        let (uploads, placements) = images.encode(state.graphics.as_deref().unwrap_or_default());
        for upload in uploads {
            reader.graphics.insert(upload);
        }
        (frames.encode(&state), placements)
    }

    /// Audit I1: a frame that does not fit the client's frame makes it ask
    /// for a full one and ignore deltas until then, instead of failing.
    #[test]
    fn a_frame_that_does_not_fit_waits_for_a_full_one() {
        let terminal = Terminal::new_display(size(20, 4), None);
        terminal.feed_output(b"\x1b_Ga=T,i=1,f=32,s=1,v=1,C=1,q=2;AQID/w==\x1b\\");
        let mut frames = crate::remote::frame::FrameEncoder::default();
        let mut images = graphics::GraphicsEncoder::default();
        let mut reader = FrameReader::default();
        let initial = frame(20, 4);
        let (update, placements) = send(&mut frames, &mut images, &mut reader, &terminal);
        let (shown, _) = reader.frame(&initial, update, placements).unwrap().unwrap();
        assert_eq!(shown.graphics.as_ref().unwrap().len(), 1);

        // The client's frame is not the one the delta was made against.
        terminal.feed_output(b"a");
        let (update, placements) = send(&mut frames, &mut images, &mut reader, &terminal);
        let corrupted = frame(20, 4);
        assert!(reader.frame(&corrupted, update, placements).is_err());
        // Deltas sent before the host saw the resync are ignored.
        terminal.feed_output(b"b");
        let (update, placements) = send(&mut frames, &mut images, &mut reader, &terminal);
        assert!(reader.frame(&shown, update, placements).unwrap().is_none());

        // The host starts afresh: a full frame with its images again.
        frames.reset();
        images = graphics::GraphicsEncoder::default();
        terminal.feed_output(b"c");
        let (update, placements) = send(&mut frames, &mut images, &mut reader, &terminal);
        let (state, change) = reader.frame(&shown, update, placements).unwrap().unwrap();
        assert_eq!(change, crate::remote::frame::FrameChange::Full);
        assert!(state.same_frame(&RemoteState::capture(&terminal)));
        assert_eq!(state.graphics.as_ref().unwrap().len(), 1);
        // Deltas apply again.
        terminal.feed_output(b"d");
        let (update, placements) = send(&mut frames, &mut images, &mut reader, &terminal);
        assert!(reader.frame(&state, update, placements).unwrap().is_some());
    }

    /// Audit I2: a hidden view keeps only the images on its screen, and
    /// drops the rest of its cache when the host starts afresh on resume.
    #[test]
    fn a_paused_view_releases_images_it_does_not_show() {
        let terminal = Terminal::new_display(size(20, 4), None);
        let mut frames = crate::remote::frame::FrameEncoder::default();
        let mut images = graphics::GraphicsEncoder::default();
        let mut reader = FrameReader::default();
        let mut shown = frame(20, 4);
        for id in 1..=3 {
            terminal.feed_output(b"\x1b_Ga=d,d=A,q=2\x1b\\");
            terminal.feed_output(
                format!("\x1b_Ga=T,i={id},f=32,s=1,v=1,C=1,q=2;AQID/w==\x1b\\").as_bytes(),
            );
            let (update, placements) = send(&mut frames, &mut images, &mut reader, &terminal);
            shown = reader.frame(&shown, update, placements).unwrap().unwrap().0;
        }
        assert_eq!(reader.graphics.cached(), 3);
        reader.paused(true, &shown);
        assert_eq!(reader.graphics.cached(), 1);
        reader.paused(false, &shown);
        assert_eq!(reader.graphics.cached(), 0);
    }

    /// Audit B22: hovering queues one motion report behind the last one not
    /// yet sent, and never moves a report past other input.
    /// Audit B22: hovering queues one motion report behind the last one not
    /// yet sent, and never moves a report past other input.
    #[test]
    fn motion_reports_coalesce_only_at_the_end_of_the_queue() {
        let queue = InputQueue::default();
        let push = |command| {
            queue.push(Input {
                command,
                reply: None,
            })
        };
        let motion = |bytes: &[u8]| RemoteCommand::WriteMotion(bytes.to_vec());
        for bytes in [b"1", b"2", b"3"] {
            assert_eq!(push(motion(bytes)), Queued::Yes);
        }
        // A key behind it: the next report queues after the key.
        assert_eq!(push(RemoteCommand::Write(b"k".to_vec())), Queued::Yes);
        assert_eq!(push(motion(b"4")), Queued::Yes);
        let sent = |queue: &InputQueue| match queue.pop().unwrap().command {
            RemoteCommand::WriteMotion(bytes) | RemoteCommand::Write(bytes) => bytes,
            _ => unreachable!(),
        };
        assert_eq!(sent(&queue), b"3");
        assert_eq!(sent(&queue), b"k");
        assert_eq!(sent(&queue), b"4");
        // Once the input thread took a report, the next one queues anew.
        assert_eq!(push(motion(b"5")), Queued::Yes);
        assert_eq!(sent(&queue), b"5");
        // A full queue drops motion quietly, and typing with a report.
        for _ in 0..INPUT_CAPACITY {
            push(RemoteCommand::Write(b"x".to_vec()));
        }
        assert_eq!(push(motion(b"6")), Queued::DroppedMotion);
        assert_eq!(push(RemoteCommand::Write(b"y".to_vec())), Queued::Dropped);
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

    /// A host that authenticates every connection and hands it to the test,
    /// which then plays the host's part on it.
    struct FakeHost {
        root: tempfile::TempDir,
        connections: flume::Receiver<TcpStream>,
    }

    impl FakeHost {
        fn new() -> Self {
            use std::net::TcpListener;
            let root = crate::multiplexer::discovery::private_tempdir();
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            std::fs::write(
                root.path().join("endpoint.json"),
                serde_json::json!({ "version": VERSION, "port": port, "token": "token" })
                    .to_string(),
            )
            .unwrap();
            let (sender, connections) = flume::unbounded();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
                    let Request::Authenticate { client_nonce, .. } =
                        read_message(&mut stream).unwrap()
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
                    if sender.send(stream).is_err() {
                        return;
                    }
                }
            });
            Self { root, connections }
        }

        fn next(&self) -> TcpStream {
            let stream = self
                .connections
                .recv_timeout(Duration::from_secs(5))
                .expect("the client did not connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
        }

        /// Answers a subscription to `pane` and sends it `state`.
        fn subscribed(&self, state: RemoteState) -> TcpStream {
            let mut stream = self.next();
            let Request::Subscribe { pane, .. } = read_message(&mut stream).unwrap() else {
                panic!("expected a subscription");
            };
            assert_eq!(pane, "pane");
            write_message(&mut stream, &Response::Ok).unwrap();
            let frame = crate::remote::frame::FrameEncoder::default().encode(&state);
            let (_, images) = graphics::GraphicsEncoder::default().encode(&[]);
            write_message(&mut stream, &Update::Frame(Box::new(frame), images)).unwrap();
            stream
        }

        /// Attaches a view to "pane". Returns it with the host's ends of its
        /// request, input and subscription connections.
        fn attach(&self) -> (Arc<ClientTerminal>, TcpStream, TcpStream, TcpStream) {
            let client = SessionClient::connect(self.root.path()).unwrap();
            let _client_connection = self.next();
            let attaching = std::thread::spawn(move || {
                client
                    .attach_transport("pane", Arc::new(frame(80, 24)), None, true)
                    .unwrap()
            });
            let rpc = self.next();
            let input = self.next();
            let subscription = self.subscribed(frame(80, 24));
            (attaching.join().unwrap(), rpc, input, subscription)
        }
    }

    fn written(stream: &mut TcpStream) -> Vec<u8> {
        match read_message(stream).unwrap() {
            Request::Command {
                command: RemoteCommand::Write(bytes),
                ..
            } => bytes,
            _ => panic!("expected a write"),
        }
    }

    /// Events drained until `done` returns true for one of them.
    #[track_caller]
    fn events_until(
        terminal: &ClientTerminal,
        mut done: impl FnMut(&TerminalEvent) -> bool,
    ) -> Vec<TerminalEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        loop {
            for event in terminal.take_events() {
                if matches!(event, TerminalEvent::Wakeup) {
                    continue;
                }
                let finished = done(&event);
                seen.push(event);
                if finished {
                    return seen;
                }
            }
            assert!(Instant::now() < deadline, "events so far: {seen:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[track_caller]
    fn wait_for_cols(terminal: &ClientTerminal, cols: u16) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while terminal.state().size.cols != cols {
            assert!(Instant::now() < deadline, "the new state never arrived");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn is_disconnected_title(event: &TerminalEvent) -> bool {
        matches!(event, TerminalEvent::Title(title) if title.starts_with("Session disconnected"))
    }

    /// Audit B9: a request the host answers with an error fails alone. The
    /// pane stays connected, and later input still arrives.
    #[test]
    fn a_failed_request_leaves_the_pane_connected() {
        let host = FakeHost::new();
        let (terminal, mut rpc, mut input, _subscription) = host.attach();

        terminal.send(RemoteCommand::Write(b"one".to_vec()));
        assert_eq!(written(&mut input), b"one");
        write_message(&mut input, &Response::Error("busy".into())).unwrap();
        terminal.send(RemoteCommand::Write(b"two".to_vec()));
        assert_eq!(written(&mut input), b"two");
        write_message(&mut input, &Response::Reply(RemoteReply::Ok)).unwrap();

        let lines = || RemoteCommand::Lines { first: 0, last: 1 };
        std::thread::scope(|scope| {
            let request = scope.spawn(|| terminal.request(lines()));
            let _: Request = read_message(&mut rpc).unwrap();
            write_message(&mut rpc, &Response::Error("too slow".into())).unwrap();
            let error = request.join().unwrap().unwrap_err();
            assert!(!is_connection_error(&error), "{error:#}");

            let request = scope.spawn(|| terminal.request(lines()));
            let _: Request = read_message(&mut rpc).unwrap();
            write_message(&mut rpc, &Response::Reply(RemoteReply::Ok)).unwrap();
            assert!(matches!(request.join().unwrap(), Ok(RemoteReply::Ok)));
        });
        assert!(!terminal.shared.disconnected.load(Ordering::Acquire));
        assert!(!terminal.take_events().iter().any(is_disconnected_title));
    }

    /// Audit B9: a lost stream is subscribed again with a fresh state. The
    /// tab says it is disconnected only while that fails, and a pane the host
    /// no longer has ends with `Exit`.
    #[test]
    fn a_lost_stream_is_reattached_until_the_pane_is_gone() {
        let host = FakeHost::new();
        let (terminal, _rpc, _input, subscription) = host.attach();

        drop(subscription);
        let subscription = host.subscribed(frame(40, 10));
        wait_for_cols(&terminal, 40);
        assert!(!terminal.take_events().iter().any(is_disconnected_title));

        // The first attempt fails, here because the host's refresh of the
        // pane timed out: that may pass (review finding 1). The tab shows it
        // until the next attempt works.
        drop(subscription);
        let mut slow = host.next();
        let _: Request = read_message(&mut slow).unwrap();
        let timeout = Response::Error("refresh terminal session: timed out".into());
        write_message(&mut slow, &timeout).unwrap();
        events_until(&terminal, is_disconnected_title);
        assert!(terminal.request(RemoteCommand::Snapshot).is_err());
        let subscription = host.subscribed(frame(50, 10));
        wait_for_cols(&terminal, 50);
        events_until(&terminal, |event| {
            matches!(event, TerminalEvent::ResetTitle)
        });

        // The host closed the pane while the stream was down.
        drop(subscription);
        let mut refused = host.next();
        let _: Request = read_message(&mut refused).unwrap();
        let gone = Response::PaneNotFound("terminal session no longer exists: pane".into());
        write_message(&mut refused, &gone).unwrap();
        events_until(&terminal, |event| matches!(event, TerminalEvent::Exit));
        assert!(!terminal.host_lost());
        // Requests fail at once instead of reconnecting (audit I3), and
        // dropped input does not blame the program.
        assert!(terminal.request(RemoteCommand::Snapshot).is_err());
        terminal.send(RemoteCommand::Write(b"lost".to_vec()));
        std::thread::sleep(REATTACH_MIN_DELAY * 2);
        assert!(host.connections.is_empty());
        assert!(
            !terminal
                .take_events()
                .iter()
                .any(|event| matches!(event, TerminalEvent::InputDropped))
        );
    }

    fn is_ended_title(event: &TerminalEvent) -> bool {
        matches!(event, TerminalEvent::Title(title) if title.starts_with("Session ended"))
    }

    /// Review finding 2: a host that shuts down ends its panes without
    /// `Exit`, so views keep their tabs, marked as ended.
    #[test]
    fn a_host_shutdown_ends_the_pane_without_an_exit() {
        let host = FakeHost::new();
        let (terminal, _rpc, _input, mut subscription) = host.attach();
        write_message(&mut subscription, &Update::HostShutdown).unwrap();
        drop(subscription);
        let events = events_until(&terminal, is_ended_title);
        assert!(!events.iter().any(is_exit), "{events:?}");
        assert!(terminal.host_lost());
        std::thread::sleep(REATTACH_MIN_DELAY * 2);
        assert!(host.connections.is_empty(), "the view subscribed again");
        assert!(terminal.request(RemoteCommand::Snapshot).is_err());
    }

    /// Review finding 2: a host that another one replaced never had this
    /// pane; the view is not asked to subscribe on the new host.
    #[test]
    fn a_replaced_host_ends_the_pane_without_an_exit() {
        let host = FakeHost::new();
        let (terminal, _rpc, _input, subscription) = host.attach();
        let endpoint = host.root.path().join("endpoint.json");
        let replaced = std::fs::read_to_string(&endpoint)
            .unwrap()
            .replace("\"token\"", "\"other\"");
        std::fs::write(&endpoint, replaced).unwrap();
        drop(subscription);
        let events = events_until(&terminal, is_ended_title);
        assert!(!events.iter().any(is_exit), "{events:?}");
        assert!(terminal.host_lost());
    }

    /// Review finding 4: a host whose updates cannot be read is shown as
    /// disconnected at once, and not subscribed to again in a tight loop.
    #[test]
    fn an_unreadable_stream_is_shown_and_retried_slowly() {
        let host = FakeHost::new();
        let (terminal, _rpc, _input, mut subscription) = host.attach();
        // A host never sends its internal state update to a client.
        let state = Update::State(Arc::new(frame(80, 24)));
        write_message(&mut subscription, &state).unwrap();
        events_until(&terminal, is_disconnected_title);
        std::thread::sleep(Duration::from_millis(500));
        assert!(host.connections.is_empty(), "subscribed again at once");
        assert!(terminal.request(RemoteCommand::Snapshot).is_err());
    }

    /// Review finding 4: a host that accepts each new subscription and then
    /// drops it before a frame is shown as disconnected too.
    #[test]
    fn streams_that_never_show_a_frame_are_shown_as_disconnected() {
        let host = FakeHost::new();
        let (terminal, _rpc, _input, subscription) = host.attach();
        drop(subscription);
        for _ in 0..UNHEALTHY_STREAMS {
            let mut stream = host.next();
            let _: Request = read_message(&mut stream).unwrap();
            write_message(&mut stream, &Response::Ok).unwrap();
        }
        events_until(&terminal, is_disconnected_title);
    }

    /// Review finding 6: the size and options a view set while its stream
    /// was lost reach the host once it is back.
    #[test]
    fn settings_dropped_while_disconnected_are_sent_after_reattaching() {
        let host = FakeHost::new();
        let (terminal, _rpc, mut input, subscription) = host.attach();
        let resized = |input: &mut TcpStream| match read_message(input).unwrap() {
            Request::Command {
                command: RemoteCommand::Resize(size),
                ..
            } => (size.cols, size.rows),
            _ => panic!("expected a resize"),
        };
        terminal.send(RemoteCommand::Resize(size(100, 30)));
        assert_eq!(resized(&mut input), (100, 30));
        write_message(&mut input, &Response::Reply(RemoteReply::Ok)).unwrap();

        drop(subscription);
        drop(host.next());
        events_until(&terminal, is_disconnected_title);
        terminal.send(RemoteCommand::Resize(size(120, 40)));
        terminal.send(RemoteCommand::NudgeResize);
        let _subscription = host.subscribed(frame(100, 30));
        events_until(&terminal, |event| {
            matches!(event, TerminalEvent::ResetTitle)
        });
        assert_eq!(resized(&mut input), (120, 40));
        write_message(&mut input, &Response::Reply(RemoteReply::Ok)).unwrap();
        // The new size is the claim now, until a frame shows it.
        assert!(!terminal.host_size_overridden());
    }

    /// Review finding 9: a pane whose `Create` reply came after the client
    /// gave up is closed.
    #[test]
    fn a_pane_created_after_the_client_gave_up_is_closed() {
        let host = FakeHost::new();
        let client = SessionClient::connect(host.root.path()).unwrap();
        let mut connection = host.next();
        let stream = client.connection.stream.lock().unwrap();
        stream
            .as_ref()
            .unwrap()
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        drop(stream);
        let launch = PaneLaunch {
            size: TerminalSize::default(),
            working_directory: None,
            shell_integration: None,
            config: TerminalRuntimeConfig::default(),
            launch: None,
            environment: None,
        };
        assert!(client.create_detached(launch).is_err());
        let Request::Create(_) = read_message(&mut connection).unwrap() else {
            panic!("expected a create");
        };
        let info = PaneInfo {
            id: "late".into(),
            child_pid: None,
            title: None,
            working_directory: None,
            exited: false,
            shell: None,
        };
        let late = Response::Attached(info, Arc::new(frame(80, 24)));
        write_message(&mut connection, &late).unwrap();
        let mut close = host.next();
        match read_message(&mut close).unwrap() {
            Request::Close(id) => assert_eq!(id, "late"),
            _ => panic!("expected the late pane to be closed"),
        }
        write_message(&mut close, &Response::Ok).unwrap();
    }

    /// Input keeps its order; a newer size takes the place of the queued
    /// one, behind the input queued before it.
    #[test]
    fn coalesced_commands_keep_their_order_with_typing() {
        let queue = InputQueue::default();
        for command in [
            RemoteCommand::Write(b"a".to_vec()),
            RemoteCommand::Resize(size(80, 24)),
            RemoteCommand::Write(b"b".to_vec()),
            RemoteCommand::Resize(size(100, 30)),
            RemoteCommand::Write(b"c".to_vec()),
        ] {
            assert_eq!(
                queue.push(Input {
                    command,
                    reply: None,
                }),
                Queued::Yes
            );
        }
        queue.close();
        let order: Vec<String> = std::iter::from_fn(|| queue.pop())
            .map(|input| match input.command {
                RemoteCommand::Write(bytes) => String::from_utf8(bytes).unwrap(),
                RemoteCommand::Resize(size) => size.cols.to_string(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(order, ["a", "b", "100", "c"]);
    }

    /// Audit B10: a stream that ends after `Exit` is how a closed pane ends,
    /// not a lost connection.
    #[test]
    fn the_stream_of_a_pane_that_ended_closes_quietly() {
        let host = FakeHost::new();
        let (terminal, _rpc, _input, mut subscription) = host.attach();
        let exit = Update::Events(vec![TerminalEvent::Exit]);
        write_message(&mut subscription, &exit).unwrap();
        drop(subscription);
        let events = events_until(&terminal, |event| matches!(event, TerminalEvent::Exit));
        std::thread::sleep(REATTACH_MIN_DELAY * 2);
        assert!(host.connections.is_empty(), "the view subscribed again");
        assert!(!events.iter().any(is_disconnected_title));
        assert!(!terminal.take_events().iter().any(is_disconnected_title));
    }

    /// Audit B9: queuing input never blocks the UI. Sizes and options keep
    /// only their latest value; typed input beyond the queue is dropped.
    #[test]
    fn a_full_input_queue_keeps_the_latest_size_and_drops_typing() {
        let queue = InputQueue::default();
        let input = |command| Input {
            command,
            reply: None,
        };
        assert_eq!(
            queue.push(input(RemoteCommand::Resize(size(80, 24)))),
            Queued::Yes
        );
        for _ in 0..INPUT_CAPACITY - 1 {
            assert_eq!(
                queue.push(input(RemoteCommand::Write(b"x".to_vec()))),
                Queued::Yes
            );
        }
        assert_eq!(
            queue.push(input(RemoteCommand::Write(b"y".to_vec()))),
            Queued::Dropped
        );
        assert_eq!(
            queue.push(input(RemoteCommand::Resize(size(100, 30)))),
            Queued::Yes
        );
        assert_eq!(
            queue.push(input(RemoteCommand::Resize(size(120, 40)))),
            Queued::Yes
        );
        let waited = Instant::now();
        let (reply, _) = flume::bounded(1);
        let paste = Input {
            command: RemoteCommand::Write(b"z".to_vec()),
            reply: Some(reply),
        };
        assert!(
            queue
                .push_until(paste, Instant::now() + Duration::from_millis(50))
                .is_err()
        );
        assert!(waited.elapsed() >= Duration::from_millis(50));
        queue.close();
        let mut resizes = Vec::new();
        let mut writes = 0;
        while let Some(Input { command, .. }) = queue.pop() {
            match command {
                RemoteCommand::Resize(size) => resizes.push(size.cols),
                RemoteCommand::Write(_) => writes += 1,
                _ => unreachable!(),
            }
        }
        assert_eq!(resizes, [120]);
        assert_eq!(writes, INPUT_CAPACITY - 1);
    }
}
