use crate::multiplexer::{SessionClient, escape, protocol::*};
use anyhow::{Context, bail, ensure};
use fs4::fs_std::FileExt;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// `endpoint.json`. Every field added after VERSION 1 must stay
/// `#[serde(default)]`: [`shutdown_host`] reads the files of older hosts.
/// Fields of older hosts that are no longer read (`conditional_layout_updates`,
/// `graphics_stream`, `viewport_replies`) are ignored.
#[derive(Serialize, Deserialize)]
pub(crate) struct Endpoint {
    pub version: u32,
    pub port: u16,
    pub token: String,
    /// The host proves it knows the token before the client sends anything.
    /// Every VERSION 4 host does; VERSION 1 and early VERSION 2 hosts did not.
    #[serde(default)]
    pub mutual_auth: bool,
    /// The host's [`build_id`]; empty for hosts before VERSION 4.
    #[serde(default)]
    pub build: String,
    /// The host's process ID, for [`kill_host`]; 0 for hosts before VERSION 4.
    #[serde(default)]
    pub pid: u32,
}

/// No host holds `host.lock`, so any `endpoint.json` is left over from a host
/// that did not exit cleanly, and its port may belong to anyone now.
#[derive(Debug)]
struct NoHost(PathBuf);

impl std::fmt::Display for NoHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            formatter,
            "no session host is running in {}",
            self.0.display()
        )
    }
}

impl std::error::Error for NoHost {}

fn no_host(root: &Path) -> anyhow::Error {
    NoHost(root.to_owned()).into()
}

/// The running host speaks another protocol version. It cannot serve this
/// client, and it must not be replaced while its terminals run.
#[derive(Debug)]
pub struct VersionMismatch {
    pub running: u32,
}

impl std::fmt::Display for VersionMismatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            formatter,
            "an older or newer Termy session host is running (multiplexer protocol {}, this Termy uses {VERSION}); run `termy mux shutdown` to stop it, which ends its terminals",
            self.running
        )
    }
}

impl std::error::Error for VersionMismatch {}

/// A host holds `host.lock` but did not accept the connection. Displays its
/// cause inline, so it is not repeated when an error chain is printed.
#[derive(Debug)]
struct Unreachable(anyhow::Error);

impl std::fmt::Display for Unreachable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            formatter,
            "the multiplexer is running but cannot be reached: {:#}",
            self.0
        )
    }
}

impl std::error::Error for Unreachable {}

/// Whether `error` (from [`connect_or_start`] or [`SessionClient::connect`])
/// means that a host of another protocol version is running. Stopping it with
/// [`shutdown_host`] (`termy mux shutdown`) lets this version start its own.
pub fn is_version_mismatch(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.is::<VersionMismatch>()
            || cause
                .downcast_ref::<Unreachable>()
                .is_some_and(|unreachable| is_version_mismatch(&unreachable.0))
    })
}

/// The session host directory for this user: `multiplexer` next to the config
/// file. On Windows it is under `%LOCALAPPDATA%` instead, so the endpoint token
/// stays on this machine rather than roaming with the `%APPDATA%` profile; a
/// host that an older version started next to the config keeps being used
/// while it runs.
pub fn default_root() -> Option<PathBuf> {
    let legacy =
        crate::config_core::config_path().and_then(|path| Some(path.parent()?.join("multiplexer")));
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value).join("termy").join("multiplexer"))
    {
        return Some(match legacy {
            Some(legacy) if host_is_running(&legacy) => legacy,
            _ => local,
        });
    }
    legacy
}

/// Whether a session host holds the host lock in `root`. Creates nothing, so
/// probing a directory no host ever used leaves no files behind.
pub fn host_is_running(root: &Path) -> bool {
    let Ok(lock) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("host.lock"))
    else {
        return false;
    };
    matches!(lock.try_lock_exclusive(), Ok(false))
}

pub(crate) fn prepare_root(root: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(root)?;
        let metadata = fs::symlink_metadata(root)?;
        // SAFETY: geteuid has no preconditions or side effects.
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
            "multiplexer directory must be private to the current user: {}",
            root.display()
        );
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "invalid multiplexer directory"
        );
    }
    #[cfg(not(unix))]
    fs::create_dir_all(root)?;
    Ok(())
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
}

pub(crate) fn private_file(path: &Path) -> anyhow::Result<File> {
    Ok(private_options()
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?)
}

/// The size at which `host.log` is rotated to `host.log.1`: at start-up, and
/// by the host's logger while it runs.
pub(crate) const HOST_LOG_LIMIT: u64 = 1024 * 1024;

/// Opens `host.log` for a new host and returns it with its current length,
/// where the new host's output will start.
fn open_host_log(root: &Path) -> anyhow::Result<(File, u64)> {
    let path = root.join("host.log");
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.len() > HOST_LOG_LIMIT) {
        // Best effort: on Windows a file still open elsewhere cannot be renamed.
        let _ = fs::rename(&path, root.join("host.log.1"));
    }
    let file = private_options()
        .append(true)
        .open(&path)
        .context("open the session host log")?;
    let start = file.metadata()?.len();
    Ok((file, start))
}

/// The last lines a host wrote to `host.log` after `start`, formatted to
/// follow a start-up error.
fn host_log_tail(root: &Path, start: u64) -> String {
    const TAIL_BYTES: u64 = 4096;
    const TAIL_LINES: usize = 12;
    let path = root.join("host.log");
    let mut output = String::new();
    if let Ok(mut file) = File::open(&path) {
        let length = file.metadata().map_or(0, |metadata| metadata.len());
        let offset = start.max(length.saturating_sub(TAIL_BYTES));
        let mut bytes = Vec::new();
        if file.seek(SeekFrom::Start(offset)).is_ok()
            && file.take(TAIL_BYTES).read_to_end(&mut bytes).is_ok()
        {
            output = String::from_utf8_lossy(&bytes).into_owned();
        }
    }
    let lines: Vec<&str> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return format!(" (no output in {})", path.display());
    }
    let tail = &lines[lines.len().saturating_sub(TAIL_LINES)..];
    format!("\nLast lines of {}:\n{}", path.display(), tail.join("\n"))
}

pub(crate) fn read_endpoint(root: &Path) -> anyhow::Result<Endpoint> {
    Ok(serde_json::from_slice(&fs::read(
        root.join("endpoint.json"),
    )?)?)
}

fn open_stream(port: u16) -> anyhow::Result<TcpStream> {
    let stream =
        TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(stream)
}

/// An authenticated connection to the host of this protocol version. Errors
/// say "no session host is running" when no host holds `host.lock`, rather
/// than blaming a leftover `endpoint.json`. The lock is probed only on
/// failure: a probe holds it for a moment, and mutual authentication already
/// keeps a stale port's new owner from learning anything.
pub(crate) fn connect(root: &Path) -> anyhow::Result<TcpStream> {
    let endpoint = match read_endpoint(root) {
        Ok(endpoint) => endpoint,
        Err(_) if !host_is_running(root) => return Err(no_host(root)),
        Err(error) => return Err(error),
    };
    if endpoint.version != VERSION {
        if !host_is_running(root) {
            return Err(no_host(root));
        }
        return Err(VersionMismatch {
            running: endpoint.version,
        }
        .into());
    }
    warn_about_build_skew(&endpoint.build);
    let result = (|| {
        let mut stream = open_stream(endpoint.port)?;
        authenticate(&mut stream, &endpoint.token)?;
        match read_message_limited(&mut stream, 4096)? {
            Response::Ok => Ok(stream),
            Response::Error(error) => bail!(error),
            _ => bail!("unexpected multiplexer greeting"),
        }
    })();
    result.map_err(|error| {
        if host_is_running(root) {
            error
        } else {
            no_host(root)
        }
    })
}

/// A host of the same protocol but another build works, unless a wire change
/// missed its VERSION bump. Say so once, in case decoding fails later.
fn warn_about_build_skew(build: &str) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    let own = build_id();
    if build != own && !WARNED.swap(true, Ordering::Relaxed) {
        log::warn!(
            "the session host was built as {build:?} and this client as {own:?}; both use multiplexer protocol {VERSION}"
        );
    }
}

/// Challenge-response over the endpoint port. Another local user can bind the
/// port after a host crash, so the host must prove it knows the token before
/// the client sends its own proof, and later its keystrokes.
pub(crate) fn authenticate(stream: &mut TcpStream, token: &str) -> anyhow::Result<()> {
    let mut client_nonce = [0; 32];
    OsRng.fill_bytes(&mut client_nonce);
    write_message(
        stream,
        &Request::Authenticate {
            version: VERSION,
            client_nonce,
        },
    )?;
    let (server_nonce, server_proof) = match read_message_limited(stream, 4096)? {
        Response::Challenge {
            server_nonce,
            server_proof,
        } => (server_nonce, server_proof),
        Response::Error(error) => bail!(error),
        _ => bail!("unexpected multiplexer greeting"),
    };
    ensure!(
        constant_time_eq(
            &server_proof,
            &host_proof(token, &client_nonce, &server_nonce)
        ),
        "the multiplexer endpoint failed authentication; it is not this user's session host"
    );
    write_message(
        stream,
        &Request::Proof(client_proof(token, &client_nonce, &server_nonce)),
    )
}

/// Stops the session host in `root` and ends all of its terminals. Works for
/// a host of any protocol version, so a user can stop the host an older Termy
/// left running before upgrading. Returns once the host has exited.
pub fn shutdown_host(root: &Path) -> anyhow::Result<()> {
    // Checked first: without a live host, `endpoint.json` is stale and its
    // port may belong to another process, which must not get the token (the
    // retired greeting sends it in plain text).
    let endpoint = running_host_endpoint(root)?;
    const HUNG: &str =
        "if the session host is hung, `termy mux shutdown --force` stops its process";
    if endpoint.version == VERSION {
        SessionClient::connect(root)
            .and_then(|client| client.shutdown())
            .context(HUNG)?;
    } else {
        let mutual_auth = endpoint.mutual_auth || endpoint.version >= 4;
        let mut stream = open_stream(endpoint.port).context(HUNG)?;
        escape::shutdown_host(&mut stream, endpoint.version, &endpoint.token, mutual_auth)
            .with_context(|| {
                format!(
                    "stop the multiplexer protocol {} session host",
                    endpoint.version
                )
            })?;
    }
    // The host closes its terminals before it releases the lock and exits.
    ensure!(
        wait_for_exit(root, Duration::from_secs(10)),
        "the session host accepted the shutdown but is still running; {HUNG}"
    );
    Ok(())
}

/// Stops the session host in `root` by signalling its process: for a host
/// that no longer answers [`shutdown_host`]. Its terminals end with it. On
/// Unix the host gets SIGTERM, then SIGKILL if it is still running after a
/// grace period; on Windows its process is terminated.
pub fn kill_host(root: &Path) -> anyhow::Result<()> {
    const GRACE: Duration = Duration::from_secs(5);
    let endpoint = running_host_endpoint(root)?;
    ensure!(
        endpoint.pid != 0,
        "the running session host did not record its process ID (an older Termy started it); stop it with `termy mux shutdown` or your system's process manager"
    );
    signal_host(endpoint.pid, false)?;
    if !wait_for_exit(root, GRACE) {
        #[cfg(unix)]
        signal_host(endpoint.pid, true)?;
        ensure!(
            wait_for_exit(root, GRACE),
            "the session host (process {}) is still running",
            endpoint.pid
        );
    }
    // A killed host cannot remove its own endpoint.
    clear_stale_endpoint(root);
    Ok(())
}

/// The endpoint of the host holding `host.lock`. Without one, removes a
/// leftover `endpoint.json` and reports that no host is running.
fn running_host_endpoint(root: &Path) -> anyhow::Result<Endpoint> {
    if !host_is_running(root) {
        clear_stale_endpoint(root);
        return Err(no_host(root));
    }
    read_endpoint(root).with_context(|| {
        format!(
            "the session host in {} has not published its endpoint",
            root.display()
        )
    })
}

/// Waits until no host holds `host.lock`; false if one still does at `timeout`.
fn wait_for_exit(root: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while host_is_running(root) {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

/// Removes `endpoint.json` if no host holds `host.lock`. Holds the lock while
/// it does, so a host starting meanwhile cannot lose its fresh endpoint.
fn clear_stale_endpoint(root: &Path) {
    let endpoint = root.join("endpoint.json");
    if fs::symlink_metadata(&endpoint).is_err() {
        return;
    }
    let Ok(lock) = private_file(&root.join("host.lock")) else {
        return;
    };
    if matches!(lock.try_lock_exclusive(), Ok(true)) {
        let _ = fs::remove_file(endpoint);
        let _ = FileExt::unlock(&lock);
    }
}

/// Asks the host process to stop (SIGTERM), or with `kill`, forces it.
#[cfg(unix)]
fn signal_host(pid: u32, kill: bool) -> anyhow::Result<()> {
    let pid = libc::pid_t::try_from(pid)
        .ok()
        .filter(|pid| *pid > 1)
        .context("invalid session host process ID")?;
    let signal = if kill { libc::SIGKILL } else { libc::SIGTERM };
    // SAFETY: kill only sends a signal. `pid` is positive, so it names one
    // process, never a process group.
    if unsafe { libc::kill(pid, signal) } != 0 {
        let error = std::io::Error::last_os_error();
        // It exited meanwhile.
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error).context("signal the session host process");
        }
    }
    Ok(())
}

/// Terminates the host process; Windows has no gentler signal for a detached
/// process without a console.
#[cfg(windows)]
fn signal_host(pid: u32, _kill: bool) -> anyhow::Result<()> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess},
    };
    // SAFETY: OpenProcess takes no pointers; it returns null on failure.
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error()).context("open the session host process");
    }
    // SAFETY: `handle` is the process handle opened above with
    // PROCESS_TERMINATE; it is closed exactly once, after this call.
    let terminated = unsafe { TerminateProcess(handle, 1) };
    let error = std::io::Error::last_os_error();
    // SAFETY: as above; the handle is not used afterwards.
    unsafe { CloseHandle(handle) };
    if terminated == 0 {
        return Err(error).context("terminate the session host process");
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn signal_host(_pid: u32, _kill: bool) -> anyhow::Result<()> {
    bail!("stopping a session host by process ID is not supported on this platform")
}

pub fn connect_or_start(root: &Path, executable: &Path) -> anyhow::Result<SessionClient> {
    prepare_root(root)?;
    let launch_lock = private_file(&root.join("launch.lock"))?;
    launch_lock.lock_exclusive()?;
    let first_error = match SessionClient::connect(root) {
        Ok(client) => return Ok(client),
        Err(error) => error,
    };
    // A live host's lock is authoritative. Never replace an incompatible or
    // temporarily unresponsive host and abandon its running processes.
    let host_lock = private_file(&root.join("host.lock"))?;
    if !host_lock.try_lock_exclusive()? {
        return Err(Unreachable(first_error).into());
    }
    FileExt::unlock(&host_lock)?;

    let (log, log_start) = open_host_log(root)?;
    let mut command = Command::new(executable);
    command
        .arg("--multiplexer-host")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: only the async-signal-safe setsid syscall runs between fork
        // and exec. A separate session keeps the host independent of app quit.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        prevent_standard_handle_inheritance()?;
        // Detaching already isolates the host from the caller's console.
        // CREATE_NEW_PROCESS_GROUP would also disable Ctrl+C in the host
        // and its descendants, including shells attached to ConPTY.
        command.creation_flags(0x0000_0008); // DETACHED_PROCESS
    }
    let mut child = command.spawn().context("start background terminal host")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(client) = SessionClient::connect(root) {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(client);
        }
        if let Some(status) = child.try_wait()? {
            bail!(
                "background terminal host exited: {status}{}",
                host_log_tail(root, log_start)
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "background terminal host did not become ready; it was left running{}",
                host_log_tail(root, log_start)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
fn prevent_standard_handle_inheritance() -> anyhow::Result<()> {
    use windows_sys::Win32::{
        Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation},
        System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
    };
    // Stdio::null() replaces the child's standard handles, but CreateProcess
    // also inherits other inheritable handles in this process. In a piped CLI
    // invocation those include our original stdout/stderr, keeping pipe EOF
    // pending until the detached host exits. Clear inheritance without closing
    // or redirecting the caller's streams. Rust duplicates explicitly inherited
    // stdio when spawning other commands, so those commands still work normally.
    for stream in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: the constants select existing process standard handles.
        let handle = unsafe { GetStdHandle(stream) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            continue;
        }
        // SAFETY: this changes only the inheritance flag of the existing
        // process handle; it neither closes the handle nor accesses its data.
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(std::io::Error::last_os_error())
                .context("isolate background host standard handles");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn host_lock_probe_detects_a_running_host() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!host_is_running(temp.path()));
        assert!(!temp.path().join("host.lock").exists());

        let held = private_file(&temp.path().join("host.lock")).unwrap();
        assert!(!host_is_running(temp.path()));
        assert!(held.try_lock_exclusive().unwrap());
        assert!(host_is_running(temp.path()));
        FileExt::unlock(&held).unwrap();
        assert!(!host_is_running(temp.path()));
    }

    fn write_endpoint(root: &Path, endpoint: serde_json::Value) {
        fs::write(root.join("endpoint.json"), endpoint.to_string()).unwrap();
    }

    fn hold_host_lock(root: &Path) -> File {
        let lock = private_file(&root.join("host.lock")).unwrap();
        assert!(lock.try_lock_exclusive().unwrap());
        lock
    }

    #[test]
    fn client_rejects_a_host_of_another_protocol_version() {
        let temp = tempfile::tempdir().unwrap();
        write_endpoint(
            temp.path(),
            serde_json::json!({ "version": 3, "port": 0, "token": "older", "mutual_auth": true }),
        );
        // Without a live host the file is stale: nothing to shut down.
        let Err(error) = connect(temp.path()) else {
            panic!("a stale endpoint must be rejected");
        };
        assert!(!is_version_mismatch(&error));
        assert!(
            error.to_string().starts_with("no session host is running"),
            "{error:#}"
        );

        let _lock = hold_host_lock(temp.path());
        let Err(error) = connect(temp.path()) else {
            panic!("a host of another version must be rejected before connecting");
        };
        assert!(is_version_mismatch(&error));
        assert!(error.to_string().contains("termy mux shutdown"));
    }

    /// Finding 5: a leftover `endpoint.json` of any version, whose port now
    /// belongs to another process, is reported and removed, never contacted.
    #[test]
    fn shutdown_host_reports_a_stale_endpoint_without_connecting() {
        for (version, mutual_auth) in [(2, false), (3, true), (VERSION, true)] {
            let temp = tempfile::tempdir().unwrap();
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            listener.set_nonblocking(true).unwrap();
            write_endpoint(
                temp.path(),
                serde_json::json!({
                    "version": version,
                    "port": listener.local_addr().unwrap().port(),
                    "token": "secret",
                    "mutual_auth": mutual_auth,
                }),
            );
            for result in [shutdown_host(temp.path()), kill_host(temp.path())] {
                let error = result.unwrap_err();
                assert!(
                    error.to_string().starts_with("no session host is running"),
                    "version {version}: {error:#}"
                );
            }
            assert!(listener.accept().is_err(), "version {version}: connected");
            assert!(!temp.path().join("endpoint.json").exists());
        }
    }

    /// Audit B1: after an upgrade the old host still holds `host.lock`. The
    /// error must say why it cannot be used and how to stop it.
    #[test]
    fn connect_or_start_keeps_the_cause_when_a_live_host_is_unreachable() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("host");
        prepare_root(&root).unwrap();
        write_endpoint(
            &root,
            serde_json::json!({ "version": 3, "port": 1, "token": "older", "mutual_auth": true }),
        );
        let _lock = hold_host_lock(&root);
        let Err(error) = connect_or_start(&root, Path::new("/nonexistent/termy")) else {
            panic!("a live host of another version must not be replaced");
        };
        let message = format!("{error:#}");
        assert!(
            message.starts_with("the multiplexer is running but cannot be reached: "),
            "{message}"
        );
        assert!(message.contains("multiplexer protocol 3"), "{message}");
        assert!(message.contains("termy mux shutdown"), "{message}");
        // The cause is shown once, also by `{:#}` chains in the CLI.
        assert_eq!(message.matches("termy mux shutdown").count(), 1);
        assert!(is_version_mismatch(&error));
        assert!(!root.join("host.log").exists(), "no host may be started");
    }

    /// Emulates an older host speaking only the frozen frames, decoded byte
    /// by byte, as a host of `version` would.
    fn fake_host(
        root: &Path,
        version: u32,
        mutual_auth: bool,
        token: &'static str,
    ) -> std::thread::JoinHandle<Vec<u32>> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut endpoint = serde_json::json!({
            "version": version,
            "port": listener.local_addr().unwrap().port(),
            "token": token,
            "conditional_layout_updates": true,
        });
        if mutual_auth {
            endpoint["mutual_auth"] = true.into();
            endpoint["graphics_stream"] = true.into();
        }
        write_endpoint(root, endpoint);
        let lock = hold_host_lock(root);
        let root = root.to_owned();
        // Returns the variant index and the fields of the next request.
        fn read_request(stream: &mut TcpStream) -> (u32, Vec<u8>) {
            let mut length = [0; 4];
            stream.read_exact(&mut length).unwrap();
            let mut body = vec![0; u32::from_le_bytes(length) as usize];
            stream.read_exact(&mut body).unwrap();
            let fields = body.split_off(4);
            (u32::from_le_bytes(body.try_into().unwrap()), fields)
        }
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut requests = Vec::new();
            let (variant, greeting) = read_request(&mut stream);
            requests.push(variant);
            assert_eq!(
                u32::from_le_bytes(greeting[..4].try_into().unwrap()),
                version
            );
            if mutual_auth {
                assert_eq!(variant, 14);
                let client_nonce: [u8; 32] = greeting[4..].try_into().unwrap();
                let server_nonce = [5; 32];
                let server_proof = host_proof(token, &client_nonce, &server_nonce);
                stream
                    .write_all(&escape::challenge(&server_nonce, &server_proof))
                    .unwrap();
                let (variant, proof) = read_request(&mut stream);
                requests.push(variant);
                assert_eq!(proof, client_proof(token, &client_nonce, &server_nonce));
            } else {
                assert_eq!(variant, 0);
                assert_eq!(greeting[4..12], (token.len() as u64).to_le_bytes());
                assert_eq!(&greeting[12..], token.as_bytes());
            }
            stream.write_all(&escape::ok()).unwrap();
            let (variant, fields) = read_request(&mut stream);
            assert!(fields.is_empty());
            requests.push(variant);
            stream.write_all(&escape::ok()).unwrap();
            // Like a real host: remove the endpoint, then exit.
            fs::remove_file(root.join("endpoint.json")).unwrap();
            drop(lock);
            requests
        })
    }

    #[test]
    fn shutdown_host_stops_a_version_three_host() {
        let temp = tempfile::tempdir().unwrap();
        let host = fake_host(temp.path(), 3, true, "rc8-token");
        shutdown_host(temp.path()).unwrap();
        assert_eq!(host.join().unwrap(), [14, 15, 10]);
        assert!(!host_is_running(temp.path()));
    }

    #[test]
    fn shutdown_host_stops_a_host_without_mutual_authentication() {
        let temp = tempfile::tempdir().unwrap();
        let host = fake_host(temp.path(), 2, false, "rc4-token");
        shutdown_host(temp.path()).unwrap();
        assert_eq!(host.join().unwrap(), [0, 10]);
    }

    /// The plaintext greeting reveals the token, so it is sent only while
    /// the host that wrote it still holds its lock.
    #[test]
    fn shutdown_host_does_not_send_the_token_to_a_stale_endpoint() {
        let temp = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        write_endpoint(
            temp.path(),
            serde_json::json!({
                "version": 2,
                "port": listener.local_addr().unwrap().port(),
                "token": "secret",
            }),
        );
        let error = shutdown_host(temp.path()).unwrap_err();
        assert!(error.to_string().contains("no session host is running"));
        assert!(listener.accept().is_err(), "the client connected anyway");
    }

    /// With the lock held by something that is not the host that wrote the
    /// endpoint, a mutually authenticating client still refuses to proceed.
    #[test]
    fn shutdown_host_rejects_an_older_impostor() {
        let temp = tempfile::tempdir().unwrap();
        let _lock = hold_host_lock(temp.path());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        write_endpoint(
            temp.path(),
            serde_json::json!({
                "version": 3,
                "port": listener.local_addr().unwrap().port(),
                "token": "f".repeat(64),
                "mutual_auth": true,
            }),
        );
        let impostor = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut greeting = [0; 44];
            stream.read_exact(&mut greeting).unwrap();
            stream
                .write_all(&escape::challenge(&[7; 32], &[0; 32]))
                .unwrap();
            // The client must hang up instead of proving the token.
            let mut rest = Vec::new();
            stream.read_to_end(&mut rest).unwrap();
            rest
        });
        let error = shutdown_host(temp.path()).unwrap_err();
        assert!(format!("{error:#}").contains("failed authentication"));
        assert!(impostor.join().unwrap().is_empty());
    }

    const HUNG_HOST_ENV: &str = "TERMY_TEST_HUNG_SESSION_HOST";

    /// Not a test by itself: `kill_host_stops_a_hung_host` runs this binary
    /// again with [`HUNG_HOST_ENV`] set, and this then acts as a current host
    /// that holds its lock but never answers.
    #[test]
    fn hung_host_process() {
        let Some(root) = std::env::var_os(HUNG_HOST_ENV) else {
            return;
        };
        let root = PathBuf::from(root);
        let _lock = hold_host_lock(&root);
        // Connections queue in the backlog and are never accepted.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        write_endpoint(
            &root,
            serde_json::json!({
                "version": VERSION,
                "port": listener.local_addr().unwrap().port(),
                "token": "hung",
                "mutual_auth": true,
                "pid": std::process::id(),
            }),
        );
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    /// Finding 4: a host that holds its lock but never answers cannot be
    /// stopped with a request; `kill_host` stops its process instead.
    #[test]
    fn kill_host_stops_a_hung_host() {
        let temp = tempfile::tempdir().unwrap();
        let mut host = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "multiplexer::discovery::tests::hung_host_process",
                "--nocapture",
            ])
            .env(HUNG_HOST_ENV, temp.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while read_endpoint(temp.path()).is_err() {
            assert!(Instant::now() < deadline, "the hung host did not start");
            assert!(host.try_wait().unwrap().is_none(), "the hung host exited");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(host_is_running(temp.path()));

        let started = Instant::now();
        let error = shutdown_host(temp.path()).unwrap_err();
        assert!(
            format!("{error:#}").contains("termy mux shutdown --force"),
            "{error:#}"
        );
        assert!(started.elapsed() < Duration::from_secs(15));

        kill_host(temp.path()).unwrap();
        let status = host.wait().unwrap();
        assert!(!status.success());
        assert!(!host_is_running(temp.path()));
        assert!(!temp.path().join("endpoint.json").exists());
    }

    #[test]
    fn kill_host_requires_a_recorded_process_id() {
        let temp = tempfile::tempdir().unwrap();
        let _lock = hold_host_lock(temp.path());
        write_endpoint(
            temp.path(),
            serde_json::json!({ "version": 3, "port": 1, "token": "old", "mutual_auth": true }),
        );
        let error = kill_host(temp.path()).unwrap_err();
        assert!(error.to_string().contains("process ID"), "{error:#}");
        assert!(temp.path().join("endpoint.json").exists());
    }

    /// After a host crash another local user can bind the freed port. The
    /// client must not hand that listener the token or accept it as the host.
    #[test]
    fn client_rejects_listener_that_does_not_know_the_token() {
        let temp = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "f".repeat(64);
        write_endpoint(
            temp.path(),
            serde_json::json!({
                "version": VERSION,
                "port": port,
                "token": token,
                "mutual_auth": true,
            }),
        );
        let impostor = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let greeting: Request = read_message_limited(&mut stream, 4096).unwrap();
            // Pretend to be the host without knowing the token.
            let reply = match &greeting {
                Request::Authenticate { .. } => Response::Challenge {
                    server_nonce: [7; 32],
                    server_proof: [0; 32],
                },
                _ => Response::Ok,
            };
            write_message(&mut stream, &reply).unwrap();
            greeting
        });
        let result = connect(temp.path());
        let greeting = impostor.join().unwrap();
        assert!(result.is_err(), "client accepted an impostor host");
        assert!(
            matches!(greeting, Request::Authenticate { .. }),
            "client must not send the token to an unauthenticated listener"
        );
    }

    #[test]
    fn start_up_errors_include_the_new_host_output() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("host.log"), "an earlier host\n").unwrap();
        let (mut log, start) = open_host_log(temp.path()).unwrap();
        writeln!(log, "session host panicked: boom").unwrap();
        let tail = host_log_tail(temp.path(), start);
        assert!(tail.contains("session host panicked: boom"), "{tail}");
        assert!(!tail.contains("an earlier host"), "{tail}");
        assert!(host_log_tail(temp.path(), start + 100).contains("no output"));
    }

    #[test]
    fn host_log_is_private_and_rotated_past_its_limit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("host.log");
        fs::write(&path, vec![b'x'; HOST_LOG_LIMIT as usize + 1]).unwrap();
        let (_, start) = open_host_log(temp.path()).unwrap();
        assert_eq!(start, 0);
        assert_eq!(
            fs::metadata(temp.path().join("host.log.1")).unwrap().len(),
            HOST_LOG_LIMIT + 1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
