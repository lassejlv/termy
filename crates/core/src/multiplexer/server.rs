use crate::multiplexer::{
    discovery::{self, Endpoint},
    layout_store::LayoutStore,
    pane::Pane,
    protocol::*,
};
use anyhow::{bail, ensure};
use fs4::fs_std::FileExt;
use rand_core::{OsRng, RngCore};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
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

struct Server {
    panes: Mutex<HashMap<String, Pane>>,
    layout: Arc<LayoutStore>,
    token: String,
    address: SocketAddr,
    stopping: AtomicBool,
    /// Connections still in the handshake, oldest first, each with a handle
    /// that can close it.
    pending: Mutex<VecDeque<(u64, TcpStream)>>,
    next_pending: AtomicU64,
    handshake_timeout: Duration,
}

impl Server {
    fn new(token: String, address: SocketAddr, handshake_timeout: Duration) -> Self {
        Self {
            panes: Mutex::new(HashMap::new()),
            layout: Arc::default(),
            token,
            address,
            stopping: AtomicBool::new(false),
            pending: Mutex::new(VecDeque::new()),
            next_pending: AtomicU64::new(0),
            handshake_timeout,
        }
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
    discovery::prepare_root(root)?;
    let lock = discovery::private_file(&root.join("host.lock"))?;
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
    let path = root.join("endpoint.json");
    let temp = root.join("endpoint.tmp");
    let mut file = discovery::private_file(&temp)?;
    file.set_len(0)?;
    file.write_all(&serde_json::to_vec(&endpoint)?)?;
    file.sync_all()?;
    std::fs::rename(temp, &path)?;
    let server = Arc::new(Server::new(token, address, HANDSHAKE_TIMEOUT));
    accept_connections(&server, listener.incoming());
    let panes: Vec<Pane> = server
        .panes
        .lock()
        .unwrap()
        .drain()
        .map(|(_, pane)| pane)
        .collect();
    for pane in panes {
        pane.close();
    }
    std::fs::remove_file(path)?;
    Ok(())
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
    fn pane(&self, id: &str) -> anyhow::Result<Pane> {
        self.panes
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("terminal session no longer exists: {id}"))
    }

    fn execute(&self, request: Request) -> anyhow::Result<Response> {
        Ok(match request {
            Request::Create(launch) => {
                let pane = Pane::create(*launch)?;
                let info = pane.info();
                let state = pane.refresh()?;
                // A pane that exits while no client shows it leaves the
                // layout at once. An attached client gets `Exit` and closes
                // the pane itself, which prunes it then.
                let layout = Arc::downgrade(&self.layout);
                let id = info.id.clone();
                pane.on_exit(move |attached| {
                    if !attached && let Some(layout) = layout.upgrade() {
                        layout.remove_pane(&id);
                    }
                });
                self.panes.lock().unwrap().insert(info.id.clone(), pane);
                Response::Attached(info, state)
            }
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
                self.pane(&pane)?.host_reply(id, reply);
                Response::Ok
            }
            Request::List => Response::Panes(
                self.panes
                    .lock()
                    .unwrap()
                    .values()
                    .map(Pane::info)
                    .collect(),
            ),
            Request::Close(id) => {
                // Closing can wait on the pane worker; don't block every other
                // client's pane lookups meanwhile.
                let pane = self.panes.lock().unwrap().remove(&id);
                if let Some(pane) = pane {
                    pane.close();
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
            | Request::SubscribeGraphics(_)
            | Request::WatchLayout => {
                bail!("invalid session request")
            }
        })
    }
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
        if let Request::SubscribeGraphics(id) = request {
            let subscription = server.pane(&id)?.subscribe()?;
            write_message(&mut stream, &Response::Ok)?;
            let mut reader = stream.try_clone()?;
            let monitor = Arc::clone(&subscription);
            std::thread::Builder::new()
                .name("termy-session-detach".into())
                .spawn(move || {
                    let _ = reader.read(&mut [0u8]);
                    monitor.close();
                })?;
            let mut graphics = crate::remote::graphics::GraphicsEncoder::default();
            while let Some(mut update) = subscription.next() {
                if let Update::State(state) = update {
                    let images = graphics.encode(state.graphics.as_deref().unwrap_or_default());
                    update = Update::GraphicsState(state, images);
                }
                if let Err(error) = write_message(&mut stream, &update) {
                    subscription.close();
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return Err(error);
                }
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return Ok(());
        }
        if let Request::WatchLayout = request {
            return watch_layout(stream, &server.layout);
        }
        let response = server
            .execute(request)
            .unwrap_or_else(|error| Response::Error(error.to_string()));
        write_response(&mut stream, &response)?;
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
        let server = Arc::new(Server::new("token".into(), address, handshake_timeout));
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
}
