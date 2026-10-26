use crate::multiplexer::{
    discovery::{self, Access, Endpoint, Root},
    layout_store::LayoutStore,
    pane::{Pane, PaneNotFound, lock_recovering},
    protocol::*,
};
use anyhow::{bail, ensure};
use fs4::fs_std::FileExt;
use rand_core::{OsRng, RngCore};
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// Connections that have not authenticated yet. Each holds a thread until it
/// authenticates or its `HANDSHAKE_TIMEOUT` ends. Beyond this, the oldest
/// pending connection is closed to make room: a local flood cannot exhaust
/// threads, and since a real handshake finishes in milliseconds, a client
/// that reconnects as soon as it is dropped cannot lock everyone else out.
const MAX_UNAUTHENTICATED: usize = 32;
/// The accept loop's pause after closing a pending connection. A client that
/// reconnects whenever it is closed turns each eviction into the next one;
/// the pause keeps that chain from cycling through every slot within a real
/// client's handshake. Accepts stay in arrival order, so the real client is
/// still admitted after at most a few pauses.
const EVICTION_PAUSE: Duration = Duration::from_millis(1);
/// The whole handshake must finish within this time, however the client
/// paces its bytes.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const ACCEPT_BACKOFF: Duration = Duration::from_millis(10);
const MAX_ACCEPT_BACKOFF: Duration = Duration::from_secs(1);
/// How long shutdown waits for the pane workers to drop their terminals.
const PANE_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long shutdown then waits for subscriptions to send their panes'
/// `Exit`, so views close their tabs rather than find the host gone.
const SUBSCRIPTION_FLUSH_TIMEOUT: Duration = Duration::from_secs(1);

struct Server {
    /// The session directory, exported to panes as `TERMY_MUX_DIR`.
    root: PathBuf,
    panes: Mutex<HashMap<String, Pane>>,
    /// Panes closed after their program exited, newest last, so a client
    /// learns that a pane another client closed had exited (a desktop view
    /// closes a pane whose shell exits). See [`EXITED_PANES`].
    exited: Mutex<VecDeque<(String, Instant)>>,
    layout: Arc<LayoutStore>,
    token: String,
    address: SocketAddr,
    stopping: AtomicBool,
    /// Connections still in the handshake, oldest first, each with a handle
    /// that can close it.
    pending: Mutex<VecDeque<(u64, TcpStream)>>,
    next_pending: AtomicU64,
    handshake_timeout: Duration,
    /// Subscriptions still sending updates.
    streaming: AtomicUsize,
}

impl Server {
    fn new(root: PathBuf, token: String, address: SocketAddr, handshake_timeout: Duration) -> Self {
        Self {
            root,
            panes: Mutex::new(HashMap::new()),
            exited: Mutex::new(VecDeque::new()),
            layout: Arc::default(),
            token,
            address,
            stopping: AtomicBool::new(false),
            pending: Mutex::new(VecDeque::new()),
            next_pending: AtomicU64::new(0),
            handshake_timeout,
            streaming: AtomicUsize::new(0),
        }
    }

    /// Waits until every subscription has sent its last update, or `deadline`.
    fn wait_for_subscriptions(&self, deadline: Instant) {
        while self.streaming.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Counts a subscription in [`Server::streaming`] until dropped.
struct Streaming<'a>(&'a AtomicUsize);

impl<'a> Streaming<'a> {
    fn new(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(count)
    }
}

impl Drop for Streaming<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A stream whose reads and writes fail once `deadline` passes. A timeout per
/// read is not enough: a client sending one byte every few seconds would
/// keep its pre-authentication slot for as long as it likes.
struct Handshake<'a> {
    stream: &'a TcpStream,
    deadline: Instant,
}

impl Handshake<'_> {
    fn remaining(&self) -> std::io::Result<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "session handshake timed out",
            ));
        }
        Ok(remaining)
    }
}

impl Read for Handshake<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        (&mut &*self.stream).read(buffer)
    }
}

impl Write for Handshake<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        (&mut &*self.stream).write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        (&mut &*self.stream).flush()
    }
}

/// Counts a connection against `MAX_UNAUTHENTICATED` until it is dropped.
struct PendingAuth {
    server: Arc<Server>,
    id: u64,
}

impl PendingAuth {
    /// Registers `stream`, closing the oldest pending connection when every
    /// slot is taken. Also returns whether one was closed.
    fn register(server: &Arc<Server>, stream: &TcpStream) -> std::io::Result<(Self, bool)> {
        let handle = stream.try_clone()?;
        let id = server.next_pending.fetch_add(1, Ordering::Relaxed);
        let mut pending = server.pending.lock().unwrap();
        let mut evicted = false;
        while pending.len() >= MAX_UNAUTHENTICATED {
            let Some((_, oldest)) = pending.pop_front() else {
                break;
            };
            // Its thread's next read fails at once, and the thread exits.
            let _ = oldest.shutdown(Shutdown::Both);
            evicted = true;
        }
        pending.push_back((id, handle));
        drop(pending);
        let server = Arc::clone(server);
        Ok((Self { server, id }, evicted))
    }
}

impl Drop for PendingAuth {
    fn drop(&mut self) {
        let mut pending = self.server.pending.lock().unwrap();
        if let Some(index) = pending.iter().position(|(id, _)| *id == self.id) {
            pending.remove(index);
        }
    }
}

pub fn serve(root: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        // Launchers can pass down the inheritable "ignore Ctrl+C" attribute.
        // Clear it before creating ConPTY shells so their foreground commands
        // can be interrupted even when this host started from a background job.
        // SAFETY: a null handler changes only the current process attribute;
        // no callback or pointer is registered.
        if unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 0) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    // Panes run in the host's directory, the home directory, and see this
    // path as `TERMY_MUX_DIR`.
    let root = Root::prepare(&std::path::absolute(root)?)?;
    let lock = root.private_file("host.lock")?;
    // Clients probe the lock by taking it for a moment; retry briefly so a
    // probe is not mistaken for a running host.
    let mut attempts = 0;
    while !lock.try_lock_exclusive()? {
        attempts += 1;
        ensure!(attempts < 10, "a session host is already running");
        std::thread::sleep(Duration::from_millis(20));
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    let mut bytes = [0; 32];
    OsRng.fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let endpoint = Endpoint {
        version: VERSION,
        port: address.port(),
        token: token.clone(),
        mutual_auth: true,
        build: build_id(),
        pid: std::process::id(),
    };
    discovery::write_endpoint(&root, &endpoint)?;
    let server = Arc::new(Server::new(
        root.path().to_owned(),
        token,
        address,
        HANDSHAKE_TIMEOUT,
    ));
    let watchdog = Watchdog {
        root,
        endpoint,
        lock,
        superseded: false,
    }
    .start(&server)?;
    accept_connections(&server, listener.incoming());
    let watchdog = watchdog.stop();
    let panes: Vec<Pane> = server.panes().drain().map(|(_, pane)| pane).collect();
    close_panes(panes);
    server.wait_for_subscriptions(Instant::now() + SUBSCRIPTION_FLUSH_TIMEOUT);
    // The lock is released last, when `watchdog` drops.
    watchdog.remove_endpoint()
}

/// Ends every pane before the host exits (audit L1). Dropping a terminal
/// hangs up its process group and schedules a SIGKILL for a shell that
/// ignores the hang-up, as when a tab closes, but that timer would die with
/// this process. Each worker therefore ends its program and waits for it.
fn close_panes(panes: Vec<Pane>) {
    for pane in &panes {
        pane.shutdown();
    }
    let deadline = Instant::now() + PANE_CLOSE_TIMEOUT;
    for pane in &panes {
        if !pane.wait_closed(deadline) {
            log::warn!("terminal session {} did not close in time", pane.info().id);
        }
    }
}

/// Puts `endpoint.json` and `host.lock` back while the host runs when
/// something removes or replaces them, or the whole session directory
/// (audit I17). Without them no client could reach this host's sessions,
/// and the next client would start a second host beside it.
struct Watchdog {
    root: Root,
    endpoint: Endpoint,
    /// This host's lock on `host.lock`.
    lock: File,
    /// Another host took over the directory. This host leaves it alone, takes
    /// it back once that host has released its lock, and stops when its own
    /// terminals have all ended.
    superseded: bool,
}

/// A [`Watchdog`] checking in the background.
struct RunningWatchdog {
    stop: flume::Sender<()>,
    thread: std::thread::JoinHandle<Watchdog>,
}

impl Watchdog {
    /// How long the watchdog waits between checks: often at first, when a
    /// script that cleans up the directory is most likely to run, then
    /// rarely, since each check opens and reads files.
    fn interval(&self, running: Duration) -> Duration {
        if self.superseded || running >= Duration::from_secs(60) {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(1)
        }
    }

    fn start(mut self, server: &Arc<Server>) -> anyhow::Result<RunningWatchdog> {
        let (stop, stopped) = flume::bounded(0);
        let server = Arc::downgrade(server);
        let started = Instant::now();
        let thread = std::thread::Builder::new()
            .name("termy-session-watchdog".into())
            .spawn(move || {
                while let Err(flume::RecvTimeoutError::Timeout) =
                    stopped.recv_timeout(self.interval(started.elapsed()))
                {
                    self.check();
                    let Some(server) = server.upgrade() else {
                        break;
                    };
                    if self.superseded && server.panes_have_ended() {
                        log::warn!(
                            "this session host stops: another host uses its directory and its own terminals have ended"
                        );
                        server.stop();
                        break;
                    }
                }
                self
            })?;
        Ok(RunningWatchdog { stop, thread })
    }

    fn check(&mut self) {
        if let Err(error) = self.repair() {
            log::warn!(
                "session host could not restore its files in {}: {error:#}",
                self.root.path().display()
            );
        }
    }

    fn repair(&mut self) -> anyhow::Result<()> {
        if !self.root.is_current() {
            let path = self.root.path().to_owned();
            // Recreate only what a cleanup removed: the directory itself, or
            // the whole default location, never a custom one's parents.
            if !path.parent().is_some_and(Path::is_dir)
                && discovery::default_root().as_deref() != Some(path.as_path())
            {
                log::warn!(
                    "the session directory {} and its parent were removed; this host's terminals cannot be reached until it is recreated",
                    path.display()
                );
                return Ok(());
            }
            log::warn!(
                "the session directory {} was removed or replaced; recreating it",
                path.display()
            );
            self.root = Root::prepare(&path)?;
            crate::multiplexer::host::reopen_log();
        }
        match self.root.open_file("host.lock", Access::ReadWrite) {
            Ok(file)
                if !self.superseded && discovery::same_file(&file, &self.lock).unwrap_or(false) => {
            }
            Ok(file) => {
                if file.try_lock_exclusive()? {
                    // Nobody held it, so it is not the file this host locked,
                    // or the host that superseded this one has exited.
                    log::warn!("host.lock was replaced or released; this session host locked it");
                    self.lock = file;
                    self.superseded = false;
                } else {
                    if !self.superseded
                        && discovery::read_endpoint(&self.root)
                            .is_ok_and(|endpoint| endpoint.token != self.endpoint.token)
                    {
                        log::warn!(
                            "another session host now uses {}; this host's terminals can no longer be reached, and it stops once they have ended",
                            self.root.path().display()
                        );
                        self.superseded = true;
                    }
                    // Someone else holds the lock: another host, one still
                    // starting, or a client probing it for a moment. Never
                    // write the endpoint then.
                    return Ok(());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let file = self.root.private_file("host.lock")?;
                if !file.try_lock_exclusive()? {
                    // A new host locked it first; the next check sees it.
                    return Ok(());
                }
                log::warn!("host.lock was removed; this session host created it again");
                self.lock = file;
                self.superseded = false;
            }
            Err(error) => return Err(error.into()),
        }
        if !discovery::read_endpoint(&self.root)
            .is_ok_and(|endpoint| endpoint.token == self.endpoint.token)
        {
            // The token goes only into the directory that was checked, and
            // only while it is still private.
            self.root.verify()?;
            log::warn!("endpoint.json was removed or replaced; this session host wrote it again");
            discovery::write_endpoint(&self.root, &self.endpoint)?;
        }
        Ok(())
    }

    /// Removes `endpoint.json` at shutdown, unless another host wrote it.
    fn remove_endpoint(&self) -> anyhow::Result<()> {
        if discovery::read_endpoint(&self.root)
            .is_ok_and(|endpoint| endpoint.token == self.endpoint.token)
        {
            self.root.remove("endpoint.json")?;
        }
        Ok(())
    }
}

impl RunningWatchdog {
    fn stop(self) -> Watchdog {
        drop(self.stop);
        match self.thread.join() {
            Ok(watchdog) => watchdog,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}

/// Serves connections until shutdown. Accept and thread-spawn failures (for
/// example running out of file descriptors) are logged and retried: returning
/// an error here would exit the host and kill every session.
fn accept_connections(
    server: &Arc<Server>,
    incoming: impl Iterator<Item = std::io::Result<TcpStream>>,
) {
    let mut backoff = Duration::ZERO;
    for stream in incoming {
        if server.stopping.load(Ordering::Acquire) {
            break;
        }
        let stream = match stream {
            Ok(stream) => {
                backoff = Duration::ZERO;
                stream
            }
            Err(error) => {
                // Persistent errors return immediately; don't spin on them.
                backoff = (backoff * 2).clamp(ACCEPT_BACKOFF, MAX_ACCEPT_BACKOFF);
                log::warn!("session host failed to accept a connection: {error}");
                std::thread::sleep(backoff);
                continue;
            }
        };
        let pending_auth = match PendingAuth::register(server, &stream) {
            Ok((pending_auth, evicted)) => {
                if evicted {
                    log::warn!("too many unauthenticated session connections; closed the oldest");
                    std::thread::sleep(EVICTION_PAUSE);
                }
                pending_auth
            }
            Err(error) => {
                log::warn!("session host failed to track a connection: {error}");
                continue;
            }
        };
        let server = Arc::clone(server);
        let spawned = std::thread::Builder::new()
            .name("termy-session-client".into())
            .spawn(move || {
                if let Err(error) = serve_client(stream, &server, pending_auth) {
                    log::debug!("session client disconnected: {error}");
                }
            });
        if let Err(error) = spawned {
            log::warn!("session host failed to start a client thread: {error}");
        }
    }
}

impl Server {
    /// Ends the accept loop, as a `Shutdown` request does.
    fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
    }

    /// Whether every pane's program has exited.
    fn panes_have_ended(&self) -> bool {
        self.panes().values().all(|pane| pane.info().exited)
    }

    fn pane(&self, id: &str) -> anyhow::Result<Pane> {
        if self.stopping.load(Ordering::Acquire) {
            bail!(HOST_STOPPING);
        }
        if let Some(pane) = self.panes().get(id) {
            return Ok(pane.clone());
        }
        let mut exited = lock_recovering(&self.exited);
        exited.retain(|(_, closed)| closed.elapsed() < EXITED_PANES_FOR);
        if exited.iter().any(|(closed, _)| closed == id) {
            return Err(PaneNotFound(format!("{EXITED_PANE}: {id}")).into());
        }
        Err(PaneNotFound(format!("{MISSING_PANE}: {id}")).into())
    }

    /// Remembers that pane `id`, closed now, had exited.
    fn remember_exited(&self, id: String) {
        let mut exited = lock_recovering(&self.exited);
        if exited.len() >= EXITED_PANES {
            exited.pop_front();
        }
        exited.push_back((id, Instant::now()));
    }

    /// The pane map. A panic in one client's thread must not stop every
    /// other client from finding its panes.
    fn panes(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pane>> {
        lock_recovering(&self.panes)
    }

    /// Starts a pane and registers it once `refresh` read its first state.
    /// A pane that cannot be registered is closed again, so its shell does
    /// not run on unseen (audit L3).
    fn create(
        &self,
        launch: PaneLaunch,
        refresh: impl FnOnce(&Pane) -> anyhow::Result<Arc<crate::remote::RemoteState>>,
    ) -> anyhow::Result<Response> {
        let pane = Pane::create(launch, &self.root)?;
        let info = pane.info();
        let state = match refresh(&pane) {
            Ok(state) => state,
            Err(error) => {
                close_unregistered(&pane);
                return Err(error);
            }
        };
        // A pane that exits while no client shows it leaves the layout at
        // once. An attached client gets `Exit` and closes the pane itself,
        // which prunes it then.
        let layout = Arc::downgrade(&self.layout);
        let id = info.id.clone();
        pane.on_exit(move |attached| {
            if !attached && let Some(layout) = layout.upgrade() {
                layout.remove_pane(&id);
            }
        });
        self.panes().insert(info.id.clone(), pane);
        Ok(Response::Attached(info, state))
    }

    /// Closes a pane that `Create` started for a client that will never
    /// learn its ID.
    fn abandon_created(&self, id: &str) {
        let pane = self.panes().remove(id);
        if let Some(pane) = pane {
            log::warn!("closing terminal session {id}: its client left before the reply");
            close_unregistered(&pane);
        }
        self.layout.remove_pane(id);
    }

    fn execute(&self, request: Request) -> anyhow::Result<Response> {
        Ok(match request {
            Request::Create(launch) => return self.create(*launch, Pane::refresh),
            Request::Attach(id) => {
                let pane = self.pane(&id)?;
                Response::Attached(pane.info(), pane.refresh()?)
            }
            Request::Command { pane, command } => {
                Response::Reply(self.pane(&pane)?.command(command)?)
            }
            Request::ViewportCommand { pane, command } => {
                let (changed, generation) = self.pane(&pane)?.viewport_command(command)?;
                Response::Viewport {
                    changed,
                    generation,
                }
            }
            Request::HostReply { pane, id, reply } => {
                self.pane(&pane)?.host_reply(id, reply)?;
                Response::Ok
            }
            Request::Snapshot(id) => {
                let pane = self.pane(&id)?;
                Response::Attached(pane.info(), pane.snapshot()?)
            }
            Request::Size(id) => Response::Size(self.pane(&id)?.size()?),
            Request::LaunchConfig(id) => {
                Response::LaunchConfig(Box::new(self.pane(&id)?.launch_config()))
            }
            Request::List => Response::Panes(self.panes().values().map(Pane::info).collect()),
            Request::Close(id) => {
                // Closing can wait on the pane worker; don't block every other
                // client's pane lookups meanwhile.
                let pane = self.panes().get(&id).cloned();
                if let Some(pane) = pane {
                    if pane.info().exited {
                        self.remember_exited(id.clone());
                    }
                    // A pane whose worker did not take the request still
                    // runs: keep it listed, so a later close can end it.
                    pane.close().inspect_err(|error| {
                        log::warn!("cannot close terminal session {id}: {error:#}");
                    })?;
                    self.panes().remove(&id);
                }
                self.layout.remove_pane(&id);
                Response::Ok
            }
            Request::GetLayout => {
                let (revision, layout) = self.layout.get();
                Response::Layout { revision, layout }
            }
            Request::SetLayout(layout) => Response::LayoutUpdated {
                updated: true,
                revision: self.layout.set(layout)?,
            },
            Request::CompareAndSetLayout {
                expected,
                replacement,
            } => {
                let (updated, revision) = self.layout.compare_and_set(expected, replacement)?;
                Response::LayoutUpdated { updated, revision }
            }
            Request::Shutdown
            | Request::RetiredHello
            | Request::RetiredSubscribe
            | Request::Authenticate { .. }
            | Request::Proof(_)
            | Request::WatchLayout
            | Request::RetiredSubscribeGraphics
            | Request::Subscribe { .. }
            | Request::SetPaused(_)
            | Request::Resync => {
                bail!("invalid session request")
            }
        })
    }
}

/// How the host's error for an unknown pane ID begins; see
/// [`is_missing_pane`].
const MISSING_PANE: &str = "terminal session no longer exists";
/// How the host's error for a pane closed after its program exited begins;
/// see [`is_exited_pane`].
const EXITED_PANE: &str = "terminal session exited and was closed";
/// The most closed panes the host remembers as exited, and for how long.
const EXITED_PANES: usize = 256;
const EXITED_PANES_FOR: Duration = Duration::from_secs(10 * 60);

/// Whether `error`, from a request about one pane, means the host has no
/// pane with that ID: it never existed, or it was closed.
pub fn is_missing_pane(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().starts_with(MISSING_PANE))
}

/// Whether `error`, from a request about one pane, means the pane's program
/// exited and the pane was closed since, within the last 10 minutes (and
/// the last 256 such panes).
pub fn is_exited_pane(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().starts_with(EXITED_PANE))
}

fn serve_client(
    mut stream: TcpStream,
    server: &Server,
    pending_auth: PendingAuth,
) -> anyhow::Result<()> {
    stream.set_nodelay(true)?;
    let mut handshake = Handshake {
        stream: &stream,
        deadline: Instant::now() + server.handshake_timeout,
    };
    authenticate(&mut handshake, &server.token)?;
    write_message(&mut handshake, &Response::Ok)?;
    drop(pending_auth);
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    loop {
        let request = read_message(&mut stream)?;
        if matches!(request, Request::Shutdown) {
            write_message(&mut stream, &Response::Ok)?;
            server.stopping.store(true, Ordering::Release);
            let _ = TcpStream::connect(server.address);
            return Ok(());
        }
        // The host is closing its panes: say so, rather than report each
        // pane as missing as they go (`is_host_unavailable`).
        if server.stopping.load(Ordering::Acquire) {
            write_message(&mut stream, &Response::Error(HOST_STOPPING.into()))?;
            return Ok(());
        }
        if let Request::Subscribe { pane, host_capable } = request {
            let subscription = match server
                .pane(&pane)
                .and_then(|pane| pane.subscribe(host_capable))
            {
                Ok(subscription) => subscription,
                Err(error) => {
                    // Say why, rather than only closing the connection
                    // (audit I11). Only a pane that is gone ends the
                    // client's view; any other failure may pass.
                    let refusal = if error.is::<PaneNotFound>() {
                        Response::PaneNotFound(error.to_string())
                    } else {
                        Response::Error(format!("{error:#}"))
                    };
                    write_message(&mut stream, &refusal)?;
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return Ok(());
                }
            };
            let _streaming = Streaming::new(&server.streaming);
            write_message(&mut stream, &Response::Ok)?;
            let mut reader = stream.try_clone()?;
            let monitor = Arc::clone(&subscription);
            std::thread::Builder::new()
                .name("termy-session-detach".into())
                .spawn(move || {
                    // The client only pauses and resumes the stream; anything
                    // else, or the connection closing, detaches it.
                    loop {
                        match read_message_limited(&mut reader, 64) {
                            Ok(Request::SetPaused(paused)) => monitor.set_paused(paused),
                            Ok(Request::Resync) => monitor.resync(),
                            _ => break,
                        }
                    }
                    monitor.close();
                })?;
            // Each subscription has its own base frame and image cache.
            let mut frames = crate::remote::frame::FrameEncoder::default();
            let mut graphics = crate::remote::graphics::GraphicsEncoder::default();
            while let Some(update) = subscription.next() {
                let mut updates = Vec::new();
                match update {
                    Update::State(state) => {
                        if subscription.take_resync() {
                            frames.reset();
                            graphics = Default::default();
                        }
                        let (images, placements) =
                            graphics.encode(state.graphics.as_deref().unwrap_or_default());
                        updates.extend(images.into_iter().map(Update::Image));
                        updates.push(Update::Frame(Box::new(frames.encode(&state)), placements));
                    }
                    Update::Paused(false) => {
                        // The client drops its image cache when it reads this.
                        graphics = Default::default();
                        updates.push(Update::Paused(false));
                    }
                    update => updates.push(update),
                }
                for update in updates {
                    if let Err(error) = write_message(&mut stream, &update) {
                        subscription.close();
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        return Err(error);
                    }
                }
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return Ok(());
        }
        if let Request::WatchLayout = request {
            return watch_layout(stream, &server.layout);
        }
        let creates = matches!(request, Request::Create(_));
        let response = server
            .execute(request)
            .unwrap_or_else(|error| Response::Error(error.to_string()));
        // A client that gave up waiting for its new pane never learns its
        // ID, so nothing would ever close it (audit L3).
        let created = match &response {
            Response::Attached(info, _) if creates => Some(info.id.as_str()),
            _ => None,
        };
        if let Some(id) = created
            && peer_closed(&stream)
        {
            server.abandon_created(id);
            return Ok(());
        }
        if let Err(error) = write_response(&mut stream, &response) {
            if let Some(id) = created {
                server.abandon_created(id);
            }
            return Err(error);
        }
    }
}

/// Closes a pane that no client knows of. The pane is not in the map, so a
/// failed close can only be logged.
fn close_unregistered(pane: &Pane) {
    if let Err(error) = pane.close() {
        log::error!(
            "cannot close unregistered terminal session {}: {error:#}",
            pane.info().id
        );
    }
}

/// Whether the client already closed its end of `stream`. A client sends
/// nothing while it waits for a reply, so readable data means it is there.
fn peer_closed(stream: &TcpStream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return false;
    }
    let result = stream.peek(&mut [0u8]);
    let _ = stream.set_nonblocking(false);
    match result {
        Ok(read) => read == 0,
        Err(error) => matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
        ),
    }
}

/// Streams the layout revision until the client disconnects: the current one
/// at once, then each new one. A watcher that falls behind skips revisions;
/// it only ever needs the latest.
fn watch_layout(mut stream: TcpStream, layout: &Arc<LayoutStore>) -> anyhow::Result<()> {
    let closed = Arc::new(AtomicBool::new(false));
    let mut reader = stream.try_clone()?;
    let monitor = Arc::clone(&closed);
    let monitor_layout = Arc::clone(layout);
    std::thread::Builder::new()
        .name("termy-layout-watch".into())
        .spawn(move || {
            let _ = reader.read(&mut [0u8]);
            monitor.store(true, Ordering::Release);
            monitor_layout.wake_watchers();
        })?;
    let mut revision = layout.revision();
    let result = (|| -> anyhow::Result<()> {
        loop {
            write_message(&mut stream, &Response::LayoutChanged { revision })?;
            match layout.wait_for_change(revision, &closed) {
                Some(next) => revision = next,
                None => return Ok(()),
            }
        }
    })();
    let _ = stream.shutdown(std::net::Shutdown::Both);
    result
}

/// The host side of `discovery::authenticate`. Its frames are frozen: any
/// later client must still be able to stop this host (see `escape`).
fn authenticate(stream: &mut (impl Read + Write), token: &str) -> anyhow::Result<()> {
    let Request::Authenticate {
        version,
        client_nonce,
    } = read_message_limited(stream, 4096)?
    else {
        bail!("session authentication required");
    };
    ensure!(version == VERSION, "invalid session authentication");
    let mut server_nonce = [0; 32];
    OsRng.fill_bytes(&mut server_nonce);
    write_message(
        stream,
        &Response::Challenge {
            server_nonce,
            server_proof: host_proof(token, &client_nonce, &server_nonce),
        },
    )?;
    let Request::Proof(proof) = read_message_limited(stream, 4096)? else {
        bail!("session authentication required");
    };
    ensure!(
        constant_time_eq(&proof, &client_proof(token, &client_nonce, &server_nonce)),
        "invalid session authentication"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the accept loop on a fresh listener; `stop` ends it.
    fn start(
        handshake_timeout: Duration,
        failures: usize,
    ) -> (Arc<Server>, SocketAddr, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = Arc::new(Server::new(
            PathBuf::new(),
            "token".into(),
            address,
            handshake_timeout,
        ));
        let accept_server = Arc::clone(&server);
        let host = std::thread::spawn(move || {
            let failures =
                (0..failures).map(|_| Err(std::io::Error::other("simulated accept failure")));
            accept_connections(&accept_server, failures.chain(listener.incoming()));
        });
        (server, address, host)
    }

    fn stop(server: &Server, host: std::thread::JoinHandle<()>) {
        server.stopping.store(true, Ordering::Release);
        let _ = TcpStream::connect(server.address);
        host.join().unwrap();
    }

    fn pending(server: &Server) -> usize {
        server.pending.lock().unwrap().len()
    }

    #[track_caller]
    fn wait_for_unauthenticated(server: &Server, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while pending(server) != count {
            assert!(
                Instant::now() < deadline,
                "expected {count} pending connections"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Whether the host closed `stream`. Windows reports a closed socket as
    /// reset or aborted rather than as end of file.
    fn closed_by_host(stream: &mut TcpStream) -> Result<(), String> {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        match stream.read(&mut [0; 1]) {
            Ok(0) => Ok(()),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ) =>
            {
                Ok(())
            }
            result => Err(format!("{result:?}")),
        }
    }

    /// A real client's exchange. Like `discovery::open_stream` it disables
    /// Nagle's algorithm, which would hold a frame's body for a delayed ACK.
    fn list_panes(address: SocketAddr) -> anyhow::Result<()> {
        let mut client = TcpStream::connect(address)?;
        client.set_nodelay(true)?;
        client.set_read_timeout(Some(Duration::from_secs(5)))?;
        discovery::authenticate(&mut client, "token")?;
        ensure!(matches!(
            read_message_limited(&mut client, 4096)?,
            Response::Ok
        ));
        write_message(&mut client, &Request::List)?;
        ensure!(matches!(read_message(&mut client)?, Response::Panes(panes) if panes.is_empty()));
        Ok(())
    }

    #[test]
    fn accept_errors_and_unauthenticated_floods_do_not_stop_the_host() {
        let (server, address, host) = start(HANDSHAKE_TIMEOUT, 3);

        // Silent connections fill every pre-authentication slot; one more
        // closes the oldest instead of holding another thread.
        let mut idle: Vec<_> = (0..MAX_UNAUTHENTICATED)
            .map(|_| TcpStream::connect(address).unwrap())
            .collect();
        wait_for_unauthenticated(&server, MAX_UNAUTHENTICATED);
        let extra = TcpStream::connect(address).unwrap();
        if let Err(result) = closed_by_host(&mut idle[0]) {
            panic!("the oldest pending connection was kept: {result}");
        }
        wait_for_unauthenticated(&server, MAX_UNAUTHENTICATED);
        drop(idle);
        drop(extra);
        wait_for_unauthenticated(&server, 0);

        // A wrong proof is rejected; the real client still gets in.
        let mut forged = TcpStream::connect(address).unwrap();
        let client_nonce = [1; 32];
        let greeting = Request::Authenticate {
            version: VERSION,
            client_nonce,
        };
        write_message(&mut forged, &greeting).unwrap();
        let Response::Challenge { server_nonce, .. } =
            read_message_limited(&mut forged, 4096).unwrap()
        else {
            panic!("expected a challenge");
        };
        let wrong = client_proof("wrong token", &client_nonce, &server_nonce);
        write_message(&mut forged, &Request::Proof(wrong)).unwrap();
        assert!(read_message_limited::<Response>(&mut forged, 4096).is_err());
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        discovery::authenticate(&mut client, "token").unwrap();
        assert!(matches!(
            read_message_limited(&mut client, 4096).unwrap(),
            Response::Ok
        ));
        write_message(&mut client, &Request::List).unwrap();
        assert!(
            matches!(read_message(&mut client).unwrap(), Response::Panes(panes) if panes.is_empty())
        );

        stop(&server, host);
    }

    /// Audit L2: an attacker that holds every pre-authentication slot and
    /// reconnects each socket as soon as the host drops it cannot keep a
    /// real client out.
    #[test]
    fn reconnecting_flood_cannot_lock_out_real_clients() {
        let (server, address, host) = start(HANDSHAKE_TIMEOUT, 0);
        let running = Arc::new(AtomicBool::new(true));
        let attacker = {
            let running = Arc::clone(&running);
            std::thread::spawn(move || {
                let open = || {
                    let stream = TcpStream::connect(address).unwrap();
                    stream.set_nonblocking(true).unwrap();
                    stream
                };
                let mut sockets: Vec<_> = (0..MAX_UNAUTHENTICATED).map(|_| open()).collect();
                let mut reconnects = 0;
                while running.load(Ordering::Acquire) {
                    for socket in &mut sockets {
                        match socket.read(&mut [0; 1]) {
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                            _ => {
                                *socket = open();
                                reconnects += 1;
                            }
                        }
                    }
                    std::thread::yield_now();
                }
                reconnects
            })
        };
        wait_for_unauthenticated(&server, MAX_UNAUTHENTICATED);
        for attempt in 0..5 {
            if let Err(error) = list_panes(address) {
                panic!("real client {attempt} was locked out: {error:#}");
            }
        }
        running.store(false, Ordering::Release);
        let reconnects = attacker.join().unwrap();
        assert!(
            reconnects >= 5,
            "the attacker reconnected {reconnects} times"
        );
        assert!(pending(&server) <= MAX_UNAUTHENTICATED);
        stop(&server, host);
    }

    /// Audit L2: a client that sends a byte now and then must still lose its
    /// pre-authentication slot when the handshake deadline passes.
    #[test]
    fn trickling_client_is_dropped_at_the_handshake_deadline() {
        let timeout = Duration::from_secs(1);
        let (server, address, host) = start(timeout, 0);
        let mut client = TcpStream::connect(address).unwrap();
        let started = Instant::now();
        wait_for_unauthenticated(&server, 1);
        // An `Authenticate` frame is 44 bytes; send it far slower than the
        // deadline allows, but well within any per-read timeout.
        let frame = crate::multiplexer::escape::authenticate(VERSION, &[1; 32]);
        for byte in &frame[..frame.len() - 1] {
            if pending(&server) == 0 {
                break;
            }
            if client.write_all(std::slice::from_ref(byte)).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        wait_for_unauthenticated(&server, 0);
        let elapsed = started.elapsed();
        assert!(elapsed >= timeout, "dropped early: {elapsed:?}");
        assert!(elapsed < timeout * 3, "the slot was held for {elapsed:?}");
        if let Err(result) = closed_by_host(&mut client) {
            panic!("trickling client was not disconnected: {result}");
        }
        stop(&server, host);
    }

    /// A shell that writes its process ID to `pid_file`, then waits.
    #[cfg(unix)]
    fn pid_writing_shell(pid_file: &Path) -> PaneLaunch {
        PaneLaunch {
            size: crate::TerminalSize::default(),
            working_directory: None,
            shell_integration: None,
            config: crate::TerminalRuntimeConfig::default(),
            launch: Some(crate::TerminalLaunch::Program {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    format!("echo $$ > '{}'; exec sleep 30", pid_file.display()),
                ],
            }),
            environment: None,
        }
    }

    #[cfg(unix)]
    #[track_caller]
    fn wait_for_exit(pid: i32) {
        let deadline = Instant::now() + Duration::from_secs(5);
        // SAFETY: signal 0 only checks that the process exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(Instant::now() < deadline, "process {pid} kept running");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Audit I11: a subscription the host refuses is answered with the
    /// reason, not only a closed connection.
    #[test]
    fn a_refused_subscription_says_why() {
        let (server, address, host) = start(HANDSHAKE_TIMEOUT, 0);
        let mut client = TcpStream::connect(address).unwrap();
        client.set_nodelay(true).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        discovery::authenticate(&mut client, "token").unwrap();
        assert!(matches!(
            read_message_limited(&mut client, 4096).unwrap(),
            Response::Ok
        ));
        let subscribe = Request::Subscribe {
            pane: "missing".into(),
            host_capable: true,
        };
        write_message(&mut client, &subscribe).unwrap();
        match read_message(&mut client).unwrap() {
            Response::PaneNotFound(error) => assert!(error.contains("missing"), "{error}"),
            _ => panic!("expected a refusal naming the pane"),
        }
        stop(&server, host);
    }

    /// Audit L3: a pane whose first state cannot be read is closed, not
    /// left running where no client can see or close it.
    #[cfg(unix)]
    #[test]
    fn a_create_whose_refresh_fails_ends_its_shell() {
        let temp = tempfile::tempdir().unwrap();
        let server = Server::new(
            temp.path().to_owned(),
            "token".into(),
            ([127, 0, 0, 1], 0).into(),
            HANDSHAKE_TIMEOUT,
        );
        let mut pid = None;
        let result = server.create(pid_writing_shell(&temp.path().join("pid")), |pane| {
            pid = pane.info().child_pid;
            bail!("injected refresh failure")
        });
        assert!(result.is_err());
        assert!(server.panes().is_empty());
        wait_for_exit(pid.expect("the pane started a process") as i32);
    }

    /// Audit L3: a client that leaves before reading its `Create` reply
    /// never learns the pane ID; the host closes that pane.
    #[cfg(unix)]
    #[test]
    fn a_pane_created_for_a_client_that_left_is_closed() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("pid");
        let (server, address, host) = start(HANDSHAKE_TIMEOUT, 0);
        let mut client = TcpStream::connect(address).unwrap();
        client.set_nodelay(true).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        discovery::authenticate(&mut client, "token").unwrap();
        assert!(matches!(
            read_message_limited(&mut client, 4096).unwrap(),
            Response::Ok
        ));
        let create = Request::Create(Box::new(pid_writing_shell(&pid_file)));
        write_message(&mut client, &create).unwrap();
        drop(client);
        // The host may end the shell before it writes its ID; a leaked shell
        // would write it at once and keep running.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|pid| pid.trim().parse::<i32>().ok())
            {
                wait_for_exit(pid);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(server.panes().is_empty());
        stop(&server, host);
    }

    /// Hosts no longer accept the plaintext-token greeting (audit I12).
    #[test]
    fn retired_plaintext_greeting_is_rejected() {
        let (server, address, host) = start(HANDSHAKE_TIMEOUT, 0);
        let mut client = TcpStream::connect(address).unwrap();
        client
            .write_all(&crate::multiplexer::escape::hello(VERSION, "token"))
            .unwrap();
        if let Err(result) = closed_by_host(&mut client) {
            panic!("plaintext greeting was answered: {result}");
        }
        stop(&server, host);
    }

    fn endpoint(token: &str) -> Endpoint {
        Endpoint {
            version: VERSION,
            port: 1,
            token: token.into(),
            mutual_auth: true,
            build: String::new(),
            pid: 0,
        }
    }

    /// A watchdog for a host that has published `own` in a new directory.
    fn watchdog(parent: &Path) -> Watchdog {
        let root = Root::prepare(&parent.join("host")).unwrap();
        let lock = root.private_file("host.lock").unwrap();
        assert!(lock.try_lock_exclusive().unwrap());
        discovery::write_endpoint(&root, &endpoint("own")).unwrap();
        Watchdog {
            root,
            endpoint: endpoint("own"),
            lock,
            superseded: false,
        }
    }

    fn published(root: &Root) -> Option<String> {
        discovery::read_endpoint(root)
            .ok()
            .map(|endpoint| endpoint.token)
    }

    fn lock_is_held(root: &Root) -> bool {
        let lock = root.open_file("host.lock", Access::ReadWrite).unwrap();
        if lock.try_lock_exclusive().unwrap() {
            FileExt::unlock(&lock).unwrap();
            return false;
        }
        true
    }

    /// Audit I17: a `host.lock` replaced by a new file is locked again, so a
    /// second host cannot start beside this one.
    #[test]
    fn watchdog_locks_a_replaced_lock_file() {
        let temp = discovery::private_tempdir();
        let mut watchdog = watchdog(temp.path());
        let root = Root::open(watchdog.root.path()).unwrap();
        root.remove("host.lock").unwrap();
        drop(root.private_file("host.lock").unwrap());
        root.remove("endpoint.json").unwrap();
        assert!(!lock_is_held(&root));
        watchdog.check();
        assert!(lock_is_held(&root));
        assert_eq!(published(&root).as_deref(), Some("own"));
        assert!(!watchdog.superseded);
    }

    /// Audit I17: once another host holds the lock and has published its
    /// endpoint, this one leaves the directory alone, and takes it back when
    /// that host has exited.
    #[test]
    fn watchdog_yields_to_another_host_and_reclaims_the_directory_afterwards() {
        let temp = discovery::private_tempdir();
        let mut watchdog = watchdog(temp.path());
        let root = Root::open(watchdog.root.path()).unwrap();
        root.remove("host.lock").unwrap();
        let other = root.private_file("host.lock").unwrap();
        assert!(other.try_lock_exclusive().unwrap());
        // Still starting: the other host holds the lock but has no endpoint.
        root.remove("endpoint.json").unwrap();
        watchdog.check();
        assert_eq!(
            published(&root),
            None,
            "written while another host held the lock"
        );
        assert!(!watchdog.superseded);

        discovery::write_endpoint(&root, &endpoint("other")).unwrap();
        watchdog.check();
        assert!(watchdog.superseded);
        watchdog.check();
        assert_eq!(published(&root).as_deref(), Some("other"));

        // The other host exits.
        root.remove("endpoint.json").unwrap();
        drop(other);
        watchdog.check();
        assert!(!watchdog.superseded);
        assert_eq!(published(&root).as_deref(), Some("own"));
        assert!(lock_is_held(&root));
    }
}
