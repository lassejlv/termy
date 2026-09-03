//! Per-user terminal multiplexer.
//!
//! The daemon owns PTYs and shadow terminal emulators. GUI clients receive a complete emulator
//! snapshot when they attach, then mirror the daemon by consuming the same output and resize
//! stream. Closing the GUI only disconnects this client; it never drops daemon-owned PTYs.

#![cfg(unix)]

use std::{
    ffi::OsString,
    fs,
    io::{ErrorKind, Read, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use engine::{
    DynamicColor, MousePointerShape, Terminal, TerminalConfig, TerminalEvent,
    pty::{PtyCommand, PtyEvent, PtySession, PtySize},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const DAEMON_ARGUMENT: &str = "--tmon-multiplexer";

const PROTOCOL_VERSION: u16 = 2;
const MAX_FRAME_BYTES: usize = 1024 * 1024 * 1024;
const MAX_PENDING_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const CONNECT_RETRY: Duration = Duration::from_millis(20);
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalSize {
    pub rows: u16,
    pub columns: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl TerminalSize {
    #[must_use]
    pub const fn new(rows: u16, columns: u16, pixel_width: u16, pixel_height: u16) -> Self {
        Self {
            rows,
            columns,
            pixel_width,
            pixel_height,
        }
    }

    const fn pty(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.columns,
            pixel_width: self.pixel_width,
            pixel_height: self.pixel_height,
        }
    }
}

impl From<PtySize> for TerminalSize {
    fn from(size: PtySize) -> Self {
        Self::new(size.rows, size.cols, size.pixel_width, size.pixel_height)
    }
}

#[derive(Clone, Debug)]
pub struct TabRestore {
    pub id: u64,
    pub index: usize,
    pub terminal_snapshot: Vec<u8>,
    pub title: String,
    pub pointer_shape: MousePointerShape,
    pub dynamic_colors: [Option<[u8; 3]>; 3],
    pub current_directory: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Restore {
    pub active_tab_id: u64,
    pub tabs: Vec<TabRestore>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabOutput {
    pub tab_id: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct DrainBatch {
    pub outputs: Vec<TabOutput>,
    pub resynchronized_tabs: Vec<TabRestore>,
}

pub struct Client {
    stream: UnixStream,
    socket_path: PathBuf,
    generation: u64,
    epoch: u64,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Client")
            .field("socket_path", &self.socket_path)
            .field("generation", &self.generation)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Connects to the per-user daemon, starting it when no live listener exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot be reached before the startup timeout.
    pub fn connect_or_spawn(socket_path: &Path, executable: &Path) -> Result<Self> {
        prepare_socket_directory(socket_path)?;
        match UnixStream::connect(socket_path) {
            Ok(stream) => {
                return Ok(Self {
                    stream,
                    socket_path: socket_path.to_owned(),
                    generation: 0,
                    epoch: 0,
                });
            }
            Err(error)
                if !matches!(
                    error.kind(),
                    ErrorKind::NotFound | ErrorKind::ConnectionRefused
                ) =>
            {
                return Err(error).with_context(|| {
                    format!(
                        "connecting to Tmon multiplexer at {}",
                        socket_path.display()
                    )
                });
            }
            Err(_) => {}
        }

        let mut child = Command::new(executable);
        child
            .arg(DAEMON_ARGUMENT)
            .arg(socket_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let mut child = child
            .spawn()
            .with_context(|| format!("starting Tmon multiplexer via {}", executable.display()))?;

        let deadline = Instant::now() + CONNECT_TIMEOUT;
        loop {
            match UnixStream::connect(socket_path) {
                Ok(stream) => {
                    return Ok(Self {
                        stream,
                        socket_path: socket_path.to_owned(),
                        generation: 0,
                        epoch: 0,
                    });
                }
                Err(_) if Instant::now() < deadline => {
                    if let Some(status) = child
                        .try_wait()
                        .context("checking Tmon multiplexer startup")?
                    {
                        bail!("Tmon multiplexer exited during startup with {status}");
                    }
                    thread::sleep(CONNECT_RETRY);
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("waiting for Tmon multiplexer at {}", socket_path.display())
                    });
                }
            }
        }
    }

    /// Attaches this GUI generation and restores every daemon-owned tab.
    ///
    /// # Errors
    ///
    /// Returns an error for a protocol mismatch, invalid command, or failed first PTY spawn.
    pub fn attach(
        &mut self,
        command: &PtyCommand,
        size: TerminalSize,
        scrollback_limit: usize,
        inactive_scrollback_limit: usize,
    ) -> Result<Restore> {
        let response = self.request(&Request::Attach {
            protocol_version: PROTOCOL_VERSION,
            command: WireCommand::from(command),
            size,
            scrollback_limit,
            inactive_scrollback_limit,
        })?;
        let Response::Attached {
            generation,
            epoch,
            restore,
        } = response
        else {
            return unexpected_response(&response);
        };
        self.generation = generation;
        self.epoch = epoch;
        Ok(restore.into())
    }

    /// Spawns a new daemon-owned tab.
    ///
    /// # Errors
    ///
    /// Returns an error when the PTY cannot be created or this client is stale.
    pub fn new_tab(
        &mut self,
        command: &PtyCommand,
        size: TerminalSize,
        scrollback_limit: usize,
    ) -> Result<TabRestore> {
        let response = self.request(&Request::NewTab {
            generation: self.generation,
            command: WireCommand::from(command),
            size,
            scrollback_limit,
        })?;
        match response {
            Response::Tab(tab) => Ok(tab.into()),
            response => unexpected_response(&response),
        }
    }

    /// Permanently closes one tab and its process group.
    ///
    /// # Errors
    ///
    /// Returns an error for the final tab, an unknown tab, or a stale client.
    pub fn close_tab(&mut self, tab_id: u64) -> Result<()> {
        self.expect_ok(&Request::CloseTab {
            generation: self.generation,
            tab_id,
        })
    }

    /// Records which daemon tab is selected by the native app.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown tab or stale client.
    pub fn activate_tab(&mut self, tab_id: u64) -> Result<()> {
        self.expect_ok(&Request::ActivateTab {
            generation: self.generation,
            tab_id,
        })
    }

    /// Writes already encoded terminal input to a tab's PTY.
    ///
    /// # Errors
    ///
    /// Returns an error for a failed PTY write, unknown tab, or stale client.
    pub fn write(&mut self, tab_id: u64, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.expect_ok(&Request::Write {
            generation: self.generation,
            tab_id,
            bytes: bytes.to_vec(),
        })
    }

    /// Applies one geometry to every tab, matching Tmon's shared renderer geometry.
    ///
    /// # Errors
    ///
    /// Returns an error for a failed PTY resize or stale client.
    pub fn resize_all(&mut self, size: TerminalSize) -> Result<()> {
        self.expect_ok(&Request::ResizeAll {
            generation: self.generation,
            size,
        })
    }

    /// Drains output accumulated since the last GUI drain.
    ///
    /// A tab is returned in `resynchronized_tabs` instead of `outputs` if the GUI fell more than
    /// the bounded pending-output budget behind. The snapshot includes every byte because the
    /// daemon continuously feeds its own emulator even while detached.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon disconnects or this client is stale.
    pub fn drain(&mut self) -> Result<DrainBatch> {
        let response = self.request(&Request::Drain {
            generation: self.generation,
        })?;
        let Response::Drained {
            epoch,
            outputs,
            resynchronized_tabs,
        } = response
        else {
            return unexpected_response(&response);
        };
        self.epoch = epoch;
        Ok(DrainBatch {
            outputs: outputs
                .into_iter()
                .map(|output| TabOutput {
                    tab_id: output.tab_id,
                    bytes: output.bytes,
                })
                .collect(),
            resynchronized_tabs: resynchronized_tabs
                .into_iter()
                .map(TabRestore::from)
                .collect(),
        })
    }

    /// Starts a blocking watcher that calls `wake` whenever daemon state changes.
    ///
    /// # Errors
    ///
    /// Returns an error if the watcher thread cannot be spawned.
    pub fn watch(&self, wake: impl Fn() + Send + 'static) -> Result<()> {
        let socket_path = self.socket_path.clone();
        let generation = self.generation;
        let mut epoch = self.epoch;
        thread::Builder::new()
            .name("tmon-mux-watch".to_owned())
            .spawn(move || {
                let Ok(mut stream) = UnixStream::connect(socket_path) else {
                    return;
                };
                loop {
                    if send_message(
                        &mut stream,
                        &Request::Wait {
                            generation,
                            since_epoch: epoch,
                        },
                    )
                    .is_err()
                    {
                        return;
                    }
                    let Ok(response) = receive_message::<Response>(&mut stream) else {
                        return;
                    };
                    let Response::Wake {
                        generation: current_generation,
                        epoch: current_epoch,
                    } = response
                    else {
                        return;
                    };
                    if current_generation != generation {
                        return;
                    }
                    if current_epoch != epoch {
                        epoch = current_epoch;
                        wake();
                    }
                }
            })
            .context("spawning a local multiplexer watcher thread")?;
        Ok(())
    }

    #[doc(hidden)]
    pub fn shutdown_daemon(&mut self) -> Result<()> {
        self.expect_ok(&Request::Shutdown {
            generation: self.generation,
        })
    }

    fn expect_ok(&mut self, request: &Request) -> Result<()> {
        let response = self.request(request)?;
        match response {
            Response::Ok => Ok(()),
            response => unexpected_response(&response),
        }
    }

    fn request(&mut self, request: &Request) -> Result<Response> {
        send_message(&mut self.stream, request).context("sending multiplexer request")?;
        match receive_message(&mut self.stream).context("receiving multiplexer response")? {
            Response::Error(message) => Err(anyhow!(message)),
            response => Ok(response),
        }
    }
}

/// Returns the default private Unix-socket location for the current user.
///
/// The filename includes the protocol version so incompatible daemons can coexist without
/// terminating the PTYs owned by an older process.
///
/// `TMON_MUX_SOCKET` is accepted for isolated integration tests and development instances.
///
/// # Errors
///
/// Returns an error if neither an override nor the user's home directory is available.
pub fn default_socket_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("TMON_MUX_SOCKET") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(default_socket_path_for_home(&PathBuf::from(home)))
}

fn default_socket_path_for_home(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Tmon/runtime")
        .join(format!("multiplexer-v{PROTOCOL_VERSION}.sock"))
}

/// Runs the daemon listener until explicitly stopped.
///
/// # Errors
///
/// Returns an error when the socket cannot be secured, bound, or accepted.
pub fn serve(socket_path: &Path) -> Result<()> {
    prepare_socket_directory(socket_path)?;
    remove_stale_socket(socket_path)?;
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("binding multiplexer socket {}", socket_path.display()))?;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))
        .context("securing multiplexer socket")?;
    let _socket_guard = SocketGuard(socket_path.to_owned());
    listener
        .set_nonblocking(true)
        .context("configuring multiplexer listener")?;

    let (notices_tx, notices_rx) = mpsc::channel();
    let shared = Arc::new(Shared {
        state: Mutex::new(ServerState::new(notices_tx)),
        notifier: Notifier::default(),
        shutdown: AtomicBool::new(false),
    });
    spawn_notice_loop(Arc::clone(&shared), notices_rx);

    while !shared.shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .context("configuring multiplexer client stream")?;
                let connection_shared = Arc::clone(&shared);
                thread::Builder::new()
                    .name("tmon-mux-client".to_owned())
                    .spawn(move || handle_connection(stream, &connection_shared))
                    .context("spawning multiplexer client handler")?;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error).context("accepting multiplexer client"),
        }
    }
    Ok(())
}

fn handle_connection(mut stream: UnixStream, shared: &Shared) {
    while let Ok(request) = receive_message::<Request>(&mut stream) {
        let response = match request {
            Request::Wait {
                generation,
                since_epoch,
            } => match shared.wait(generation, since_epoch) {
                Ok((current_generation, epoch)) => Response::Wake {
                    generation: current_generation,
                    epoch,
                },
                Err(error) => Response::Error(format!("{error:#}")),
            },
            request => match shared.handle(request) {
                Ok(response) => response,
                Err(error) => Response::Error(format!("{error:#}")),
            },
        };
        if send_message(&mut stream, &response).is_err() {
            return;
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
enum Request {
    Attach {
        protocol_version: u16,
        command: WireCommand,
        size: TerminalSize,
        scrollback_limit: usize,
        inactive_scrollback_limit: usize,
    },
    NewTab {
        generation: u64,
        command: WireCommand,
        size: TerminalSize,
        scrollback_limit: usize,
    },
    CloseTab {
        generation: u64,
        tab_id: u64,
    },
    ActivateTab {
        generation: u64,
        tab_id: u64,
    },
    Write {
        generation: u64,
        tab_id: u64,
        bytes: Vec<u8>,
    },
    ResizeAll {
        generation: u64,
        size: TerminalSize,
    },
    Drain {
        generation: u64,
    },
    Wait {
        generation: u64,
        since_epoch: u64,
    },
    Shutdown {
        generation: u64,
    },
}

#[derive(Debug, Deserialize, Serialize)]
enum Response {
    Attached {
        generation: u64,
        epoch: u64,
        restore: WireRestore,
    },
    Tab(WireTabRestore),
    Drained {
        epoch: u64,
        outputs: Vec<WireTabOutput>,
        resynchronized_tabs: Vec<WireTabRestore>,
    },
    Wake {
        generation: u64,
        epoch: u64,
    },
    Ok,
    Error(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WireCommand {
    program: Vec<u8>,
    arguments: Vec<Vec<u8>>,
    working_directory: Option<Vec<u8>>,
}

impl From<&PtyCommand> for WireCommand {
    fn from(command: &PtyCommand) -> Self {
        Self {
            program: command.program.as_os_str().as_bytes().to_vec(),
            arguments: command
                .arguments
                .iter()
                .map(|argument| argument.as_os_str().as_bytes().to_vec())
                .collect(),
            working_directory: command
                .working_directory
                .as_ref()
                .map(|directory| directory.as_os_str().as_bytes().to_vec()),
        }
    }
}

impl From<WireCommand> for PtyCommand {
    fn from(command: WireCommand) -> Self {
        Self {
            program: OsString::from_vec(command.program),
            arguments: command
                .arguments
                .into_iter()
                .map(OsString::from_vec)
                .collect(),
            working_directory: command
                .working_directory
                .map(|directory| PathBuf::from(OsString::from_vec(directory))),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct WireRestore {
    active_tab_id: u64,
    tabs: Vec<WireTabRestore>,
}

impl From<WireRestore> for Restore {
    fn from(restore: WireRestore) -> Self {
        Self {
            active_tab_id: restore.active_tab_id,
            tabs: restore.tabs.into_iter().map(TabRestore::from).collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WireTabRestore {
    id: u64,
    index: usize,
    terminal_snapshot: Vec<u8>,
    title: String,
    pointer_shape: MousePointerShape,
    dynamic_colors: [Option<[u8; 3]>; 3],
    current_directory: Option<Vec<u8>>,
}

impl From<WireTabRestore> for TabRestore {
    fn from(tab: WireTabRestore) -> Self {
        Self {
            id: tab.id,
            index: tab.index,
            terminal_snapshot: tab.terminal_snapshot,
            title: tab.title,
            pointer_shape: tab.pointer_shape,
            dynamic_colors: tab.dynamic_colors,
            current_directory: tab
                .current_directory
                .map(|directory| PathBuf::from(OsString::from_vec(directory))),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct WireTabOutput {
    tab_id: u64,
    bytes: Vec<u8>,
}

struct Shared {
    state: Mutex<ServerState>,
    notifier: Notifier,
    shutdown: AtomicBool,
}

impl Shared {
    fn handle(&self, request: Request) -> Result<Response> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("multiplexer state lock poisoned"))?;
        match request {
            Request::Attach {
                protocol_version,
                command,
                size,
                scrollback_limit,
                inactive_scrollback_limit,
            } => {
                if protocol_version != PROTOCOL_VERSION {
                    bail!(
                        "multiplexer protocol mismatch: daemon {PROTOCOL_VERSION}, client {protocol_version}"
                    );
                }
                state.drain_all_available();
                if state.tabs.is_empty() {
                    state.spawn_tab(command.into(), size, scrollback_limit)?;
                }
                // A reattached window may have different geometry from the window that detached.
                // Resize before snapshotting so the client and daemon resume from the same grid.
                state.resize_all(size)?;
                state.scrollback_limit = scrollback_limit;
                state.inactive_scrollback_limit = inactive_scrollback_limit;
                state.apply_scrollback_limits();
                state.generation = state.generation.wrapping_add(1).max(1);
                for tab in &mut state.tabs {
                    tab.pending.clear();
                    tab.needs_resync = false;
                }
                let epoch = self.notifier.bump();
                Ok(Response::Attached {
                    generation: state.generation,
                    epoch,
                    restore: state.restore()?,
                })
            }
            Request::NewTab {
                generation,
                command,
                size,
                scrollback_limit,
            } => {
                state.require_generation(generation)?;
                let id = state.spawn_tab(command.into(), size, scrollback_limit)?;
                let tab = state
                    .tabs
                    .iter()
                    .find(|tab| tab.id == id)
                    .context("new multiplexer tab disappeared")?
                    .restore()?;
                self.notifier.bump();
                Ok(Response::Tab(tab))
            }
            Request::CloseTab { generation, tab_id } => {
                state.require_generation(generation)?;
                if state.tabs.len() == 1 {
                    bail!("the final tab detaches instead of closing");
                }
                let position = state
                    .tabs
                    .iter()
                    .position(|tab| tab.id == tab_id)
                    .with_context(|| format!("unknown multiplexer tab {tab_id}"))?;
                let closed = state.tabs.remove(position);
                for (index, tab) in state.tabs.iter_mut().enumerate() {
                    tab.index = index;
                }
                if state.active_tab_id == tab_id {
                    state.active_tab_id = state.tabs[position.min(state.tabs.len() - 1)].id;
                }
                state.apply_scrollback_limits();
                thread::spawn(move || drop(closed));
                self.notifier.bump();
                Ok(Response::Ok)
            }
            Request::ActivateTab { generation, tab_id } => {
                state.require_generation(generation)?;
                if !state.tabs.iter().any(|tab| tab.id == tab_id) {
                    bail!("unknown multiplexer tab {tab_id}");
                }
                state.active_tab_id = tab_id;
                state.apply_scrollback_limits();
                self.notifier.bump();
                Ok(Response::Ok)
            }
            Request::Write {
                generation,
                tab_id,
                bytes,
            } => {
                state.require_generation(generation)?;
                state.tab(tab_id)?.pty.write(&bytes)?;
                Ok(Response::Ok)
            }
            Request::ResizeAll { generation, size } => {
                state.require_generation(generation)?;
                state.resize_all(size)?;
                Ok(Response::Ok)
            }
            Request::Drain { generation } => {
                state.require_generation(generation)?;
                state.drain_all_available();
                let mut outputs = Vec::new();
                let mut resynchronized_tabs = Vec::new();
                for tab in &mut state.tabs {
                    if tab.needs_resync {
                        resynchronized_tabs.push(tab.restore()?);
                        tab.needs_resync = false;
                        tab.pending.clear();
                    } else if !tab.pending.is_empty() {
                        outputs.push(WireTabOutput {
                            tab_id: tab.id,
                            bytes: std::mem::take(&mut tab.pending),
                        });
                    }
                }
                Ok(Response::Drained {
                    epoch: self.notifier.epoch(),
                    outputs,
                    resynchronized_tabs,
                })
            }
            Request::Shutdown { generation } => {
                state.require_generation(generation)?;
                self.shutdown.store(true, Ordering::Release);
                self.notifier.bump();
                Ok(Response::Ok)
            }
            Request::Wait { .. } => unreachable!("wait requests do not take the server-state lock"),
        }
    }

    fn wait(&self, generation: u64, since_epoch: u64) -> Result<(u64, u64)> {
        let current_generation = self
            .state
            .lock()
            .map_err(|_| anyhow!("multiplexer state lock poisoned"))?
            .generation;
        if current_generation != generation {
            bail!("this Tmon window was replaced by a newer attachment");
        }
        let epoch = self.notifier.wait_since(since_epoch)?;
        let current_generation = self
            .state
            .lock()
            .map_err(|_| anyhow!("multiplexer state lock poisoned"))?
            .generation;
        Ok((current_generation, epoch))
    }
}

struct ServerState {
    tabs: Vec<ServerTab>,
    active_tab_id: u64,
    next_tab_id: u64,
    generation: u64,
    notices: mpsc::Sender<TabNotice>,
    scrollback_limit: usize,
    inactive_scrollback_limit: usize,
}

impl ServerState {
    fn new(notices: mpsc::Sender<TabNotice>) -> Self {
        Self {
            tabs: Vec::new(),
            active_tab_id: 0,
            next_tab_id: 1,
            generation: 0,
            notices,
            scrollback_limit: 5_000,
            inactive_scrollback_limit: 1_000,
        }
    }

    fn require_generation(&self, generation: u64) -> Result<()> {
        if self.generation != generation {
            bail!("this Tmon window was replaced by a newer attachment");
        }
        Ok(())
    }

    fn resize_all(&mut self, size: TerminalSize) -> Result<()> {
        for tab in &mut self.tabs {
            tab.terminal
                .resize(usize::from(size.columns), usize::from(size.rows));
            tab.terminal
                .set_pixel_size(u32::from(size.pixel_width), u32::from(size.pixel_height));
            tab.pty.resize(size.pty())?;
        }
        Ok(())
    }

    fn spawn_tab(
        &mut self,
        command: PtyCommand,
        size: TerminalSize,
        scrollback_limit: usize,
    ) -> Result<u64> {
        let id = self.next_tab_id;
        self.next_tab_id = self.next_tab_id.wrapping_add(1).max(1);
        let mut terminal = Terminal::new(TerminalConfig {
            columns: usize::from(size.columns),
            rows: usize::from(size.rows),
            scrollback_limit,
        });
        terminal.set_pixel_size(u32::from(size.pixel_width), u32::from(size.pixel_height));
        let notices = self.notices.clone();
        let pty = PtySession::spawn(&command, size.pty(), move |event| {
            let _ = notices.send(TabNotice { tab_id: id, event });
        })?;
        let index = self.tabs.len();
        self.tabs.push(ServerTab {
            id,
            index,
            terminal,
            pty,
            scratch: Vec::new(),
            pending: Vec::new(),
            needs_resync: false,
            title: "Tmon".to_owned(),
            pointer_shape: MousePointerShape::Text,
            dynamic_colors: [None; 3],
            current_directory: command.working_directory,
        });
        if self.active_tab_id == 0 {
            self.active_tab_id = id;
        }
        Ok(id)
    }

    fn apply_scrollback_limits(&mut self) {
        for tab in &mut self.tabs {
            let limit = if tab.id == self.active_tab_id {
                self.scrollback_limit
            } else {
                self.inactive_scrollback_limit
            };
            tab.terminal.set_scrollback_limit(limit);
        }
    }

    fn tab(&self, tab_id: u64) -> Result<&ServerTab> {
        self.tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .with_context(|| format!("unknown multiplexer tab {tab_id}"))
    }

    fn restore(&self) -> Result<WireRestore> {
        let mut tabs = self
            .tabs
            .iter()
            .map(ServerTab::restore)
            .collect::<Result<Vec<_>>>()?;
        tabs.sort_by_key(|tab| tab.index);
        Ok(WireRestore {
            active_tab_id: self.active_tab_id,
            tabs,
        })
    }

    fn drain_all_available(&mut self) {
        for tab in &mut self.tabs {
            tab.drain_available();
        }
    }

    fn process_notice(&mut self, notice: TabNotice) -> bool {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == notice.tab_id) else {
            return false;
        };
        match notice.event {
            PtyEvent::Wake => tab.drain_available(),
            PtyEvent::Exit { code, signal } => {
                tab.drain_available();
                let detail = signal.unwrap_or_else(|| code.to_string());
                tab.feed(format!("\r\n\x1b[90m[process exited: {detail}]\x1b[0m\r\n").as_bytes());
            }
            PtyEvent::ReadError(error) => {
                tab.drain_available();
                tab.feed(format!("\r\n\x1b[31m[PTY error: {error}]\x1b[0m\r\n").as_bytes());
            }
        }
        true
    }
}

struct ServerTab {
    id: u64,
    index: usize,
    terminal: Terminal,
    pty: PtySession,
    scratch: Vec<u8>,
    pending: Vec<u8>,
    needs_resync: bool,
    title: String,
    pointer_shape: MousePointerShape,
    dynamic_colors: [Option<[u8; 3]>; 3],
    current_directory: Option<PathBuf>,
}

impl ServerTab {
    fn drain_available(&mut self) {
        let mut scratch = std::mem::take(&mut self.scratch);
        match self.pty.drain_output_into(&mut scratch) {
            Ok(length) if length > 0 => self.feed(&scratch),
            Ok(_) => {}
            Err(error) => {
                self.feed(format!("\r\n\x1b[31m[PTY error: {error}]\x1b[0m\r\n").as_bytes());
            }
        }
        scratch.clear();
        self.scratch = scratch;
    }

    fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.terminal.feed(bytes);
        let mut replies = Vec::new();
        for event in self.terminal.drain_events() {
            match event {
                TerminalEvent::Title(title) => {
                    self.title = if title.is_empty() {
                        "Tmon".to_owned()
                    } else {
                        title
                    };
                }
                TerminalEvent::ResetTitle => "Tmon".clone_into(&mut self.title),
                TerminalEvent::CurrentDirectory(directory) => {
                    self.current_directory = directory_from_osc7(&directory);
                }
                TerminalEvent::MousePointerShape(shape) => self.pointer_shape = shape,
                TerminalEvent::SetDynamicColor { target, color } => {
                    self.dynamic_colors[dynamic_color_index(target)] = Some(color);
                }
                TerminalEvent::ResetDynamicColor { target } => {
                    self.dynamic_colors[dynamic_color_index(target)] = None;
                }
                TerminalEvent::Reply(bytes) => replies.push(bytes),
                TerminalEvent::Bell | TerminalEvent::ClipboardStore { .. } => {}
            }
        }
        for reply in replies {
            let _ = self.pty.write(&reply);
        }

        if self.needs_resync {
            return;
        }
        if self.pending.len().saturating_add(bytes.len()) > MAX_PENDING_OUTPUT_BYTES {
            self.pending.clear();
            self.needs_resync = true;
        } else {
            self.pending.extend_from_slice(bytes);
        }
    }

    fn restore(&self) -> Result<WireTabRestore> {
        Ok(WireTabRestore {
            id: self.id,
            index: self.index,
            terminal_snapshot: self.terminal.snapshot()?,
            title: self.title.clone(),
            pointer_shape: self.pointer_shape,
            dynamic_colors: self.dynamic_colors,
            current_directory: self
                .current_directory
                .as_ref()
                .map(|directory| directory.as_os_str().as_bytes().to_vec()),
        })
    }
}

struct TabNotice {
    tab_id: u64,
    event: PtyEvent,
}

fn spawn_notice_loop(shared: Arc<Shared>, notices: mpsc::Receiver<TabNotice>) {
    thread::Builder::new()
        .name("tmon-mux-pty-events".to_owned())
        .spawn(move || {
            while !shared.shutdown.load(Ordering::Acquire) {
                match notices.recv_timeout(Duration::from_millis(100)) {
                    Ok(notice) => {
                        let changed = shared
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .process_notice(notice);
                        if changed {
                            shared.notifier.bump();
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .expect("spawning the multiplexer PTY event loop should succeed");
}

#[derive(Default)]
struct Notifier {
    epoch: Mutex<u64>,
    changed: Condvar,
}

impl Notifier {
    fn bump(&self) -> u64 {
        let mut epoch = self
            .epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *epoch = epoch.wrapping_add(1);
        self.changed.notify_all();
        *epoch
    }

    fn epoch(&self) -> u64 {
        *self
            .epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn wait_since(&self, since: u64) -> Result<u64> {
        let epoch = self
            .epoch
            .lock()
            .map_err(|_| anyhow!("multiplexer notification lock poisoned"))?;
        if *epoch != since {
            return Ok(*epoch);
        }
        let (epoch, _) = self
            .changed
            .wait_timeout(epoch, WAIT_TIMEOUT)
            .map_err(|_| anyhow!("multiplexer notification lock poisoned"))?;
        Ok(*epoch)
    }
}

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn prepare_socket_directory(socket_path: &Path) -> Result<()> {
    let parent = socket_path
        .parent()
        .context("multiplexer socket has no parent")?;
    if !parent.exists() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating multiplexer directory {}", parent.display()))?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("securing multiplexer directory {}", parent.display()))?;
        return Ok(());
    }
    let metadata = fs::metadata(parent)
        .with_context(|| format!("inspecting multiplexer directory {}", parent.display()))?;
    if !metadata.is_dir() {
        bail!(
            "multiplexer socket parent is not a directory: {}",
            parent.display()
        );
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        bail!(
            "multiplexer directory must not be accessible by other users: {}",
            parent.display()
        );
    }
    Ok(())
}

fn remove_stale_socket(socket_path: &Path) -> Result<()> {
    if !socket_path.exists() {
        return Ok(());
    }
    if UnixStream::connect(socket_path).is_ok() {
        bail!(
            "a Tmon multiplexer is already listening at {}",
            socket_path.display()
        );
    }
    fs::remove_file(socket_path).with_context(|| {
        format!(
            "removing stale multiplexer socket {}",
            socket_path.display()
        )
    })
}

fn send_message<T: Serialize>(stream: &mut UnixStream, message: &T) -> Result<()> {
    let encoded = bincode::serde::encode_to_vec(message, bincode::config::standard())
        .context("encoding multiplexer message")?;
    if encoded.len() > MAX_FRAME_BYTES {
        bail!("multiplexer message exceeds the maximum frame size");
    }
    let length = u64::try_from(encoded.len()).context("multiplexer message is too large")?;
    stream
        .write_all(&length.to_be_bytes())
        .context("writing multiplexer frame length")?;
    stream
        .write_all(&encoded)
        .context("writing multiplexer frame")
}

fn receive_message<T: DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let mut length = [0_u8; 8];
    stream
        .read_exact(&mut length)
        .context("reading multiplexer frame length")?;
    let length = usize::try_from(u64::from_be_bytes(length))
        .context("multiplexer frame does not fit in memory")?;
    if length > MAX_FRAME_BYTES {
        bail!("multiplexer frame exceeds the maximum size");
    }
    let mut encoded = vec![0; length];
    stream
        .read_exact(&mut encoded)
        .context("reading multiplexer frame")?;
    let (message, consumed): (T, usize) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
            .context("decoding multiplexer message")?;
    if consumed != encoded.len() {
        bail!("multiplexer frame contains trailing data");
    }
    Ok(message)
}

fn unexpected_response<T>(response: &Response) -> Result<T> {
    Err(anyhow!("unexpected multiplexer response: {response:?}"))
}

const fn dynamic_color_index(target: DynamicColor) -> usize {
    match target {
        DynamicColor::Foreground => 0,
        DynamicColor::Background => 1,
        DynamicColor::Cursor => 2,
    }
}

fn directory_from_osc7(uri: &str) -> Option<PathBuf> {
    let authority_and_path = uri.strip_prefix("file://")?;
    let path_start = authority_and_path.find('/')?;
    let encoded_path = &authority_and_path[path_start..];
    let decoded = percent_decode(encoded_path)?;
    let path = PathBuf::from(decoded);
    path.is_absolute().then_some(path)
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex(*bytes.get(index + 1)?)?;
            let low = hex(*bytes.get(index + 2)?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

const fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{PROTOCOL_VERSION, default_socket_path_for_home};

    #[test]
    fn default_socket_is_namespaced_by_protocol_version() {
        let path = default_socket_path_for_home(Path::new("/Users/example"));
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(format!("multiplexer-v{PROTOCOL_VERSION}.sock").as_str()),
        );
    }
}
