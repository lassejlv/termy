use crate::multiplexer::{SessionClient, escape, protocol::*};
use anyhow::{Context, bail, ensure};
use fs4::fs_std::FileExt;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
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

pub(crate) fn no_host(root: &Path) -> anyhow::Error {
    NoHost(root.to_owned()).into()
}

/// Whether `error`, from [`connect_or_start`], says that a host held the
/// lock but did not answer: it may be stopping, or another client was
/// probing the lock.
pub(crate) fn is_unreachable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| cause.is::<Unreachable>())
}

/// Whether `error` says that no host runs, so the terminals of the last one
/// are gone with it.
pub(crate) fn is_no_host(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| cause.is::<NoHost>())
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

/// Whether `error` (from [`SessionClient::connect`] or a request) means that
/// no host runs in the session directory, or that the one there cannot be
/// reached. A host of another protocol version is [`is_version_mismatch`].
pub fn is_host_unavailable(error: &anyhow::Error) -> bool {
    !is_version_mismatch(error)
        && error.chain().any(|cause| {
            cause.is::<NoHost>()
                || cause.is::<Unreachable>()
                || cause.to_string().starts_with(HOST_STOPPING)
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
/// probing a directory no host ever used leaves no files behind. A directory
/// that is not private counts as having no host; [`running_host_endpoint`]
/// tells the two apart.
pub fn host_is_running(root: &Path) -> bool {
    Root::open(root).is_ok_and(|root| lock_is_held(&root))
}

fn lock_is_held(root: &Root) -> bool {
    root.open_file("host.lock", Access::ReadWrite)
        .is_ok_and(|lock| matches!(lock.try_lock_exclusive(), Ok(false)))
}

/// How a file in a [`Root`] is opened. Created files are private to this user.
#[derive(Clone, Copy)]
pub(crate) enum Access {
    Read,
    ReadWrite,
    CreateReadWrite,
    /// Fails if the file exists, so a file someone else placed there is
    /// never written to.
    CreateNew,
    CreateAppend,
}

/// Whether `left` and `right` are open handles of the same file; `None`
/// when that cannot be read.
pub(crate) fn same_file(left: &File, right: &File) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (left, right) = (left.metadata().ok()?, right.metadata().ok()?);
        Some(left.dev() == right.dev() && left.ino() == right.ino())
    }
    #[cfg(windows)]
    {
        use crate::multiplexer::windows_acl::file_id;
        Some(file_id(left)? == file_id(right)?)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (left, right);
        None
    }
}

/// A session directory that was checked to be private to this user. It
/// holds the endpoint token, which lets anyone who reads it run commands as
/// this user (audit L4).
///
/// On Unix the check is made on an open handle of the directory, and every
/// file is opened relative to that handle: a user who can write a directory
/// above it may rename or replace the path afterwards, but cannot redirect
/// the token into a directory of theirs. That is why no ancestor needs
/// checking, so `/tmp`, sticky or not, and group-writable homes are fine.
/// Only the directory itself must be this user's, mode 0700, and not a
/// symlink. ACLs are not checked: on macOS a directory created under a parent
/// whose inheritable ACL grants another user access passes the mode check.
///
/// On Windows the directory is held open for the life of the `Root` without
/// delete sharing, so it cannot be renamed or removed meanwhile, and it must
/// not be a junction or other reparse point. Its owner must be this user,
/// SYSTEM or the administrators, and its DACL must grant access to no one
/// else; when it does, a directory this user may change is given a
/// protected, owner-only DACL, and any other is refused. Files are opened by
/// path, not relative to the handle, without following reparse points, and
/// each must be owned by this user, SYSTEM or the administrators.
pub(crate) struct Root {
    path: PathBuf,
    #[cfg(any(unix, windows))]
    dir: File,
}

impl Root {
    /// Opens an existing session directory and checks that it is private.
    pub(crate) fn open(path: &Path) -> anyhow::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::{ffi::OsStrExt, io::FromRawFd};
            let name = std::ffi::CString::new(path.as_os_str().as_bytes())?;
            // SAFETY: `name` is NUL-terminated and outlives the call.
            let fd = unsafe {
                libc::open(
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                let error = std::io::Error::last_os_error();
                if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) {
                    bail!("invalid multiplexer directory: {}", path.display());
                }
                return Err(error).with_context(|| format!("open {}", path.display()));
            }
            let root = Self {
                path: path.to_owned(),
                // SAFETY: `fd` was just opened, and nothing else owns it.
                dir: unsafe { File::from_raw_fd(fd) },
            };
            root.check_private()?;
            Ok(root)
        }
        #[cfg(windows)]
        {
            use crate::multiplexer::windows_acl;
            let dir = windows_acl::open_directory(path)?;
            windows_acl::secure_directory(&dir, path)?;
            Ok(Self {
                path: path.to_owned(),
                dir,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            ensure!(
                fs::symlink_metadata(path)
                    .with_context(|| format!("open {}", path.display()))?
                    .is_dir(),
                "invalid multiplexer directory: {}",
                path.display()
            );
            Ok(Self {
                path: path.to_owned(),
            })
        }
    }

    #[cfg(unix)]
    fn check_private(&self) -> anyhow::Result<()> {
        use std::os::unix::fs::MetadataExt;
        let metadata = self.dir.metadata()?;
        // SAFETY: geteuid has no preconditions or side effects.
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
            "multiplexer directory must be private to the current user: {}",
            self.path.display()
        );
        Ok(())
    }

    /// Creates the session directory (and its parents) if it is missing,
    /// private to this user, then opens it like [`Self::open`].
    pub(crate) fn prepare(path: &Path) -> anyhow::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)?;
        }
        // `open` makes a new directory private on Windows.
        #[cfg(not(unix))]
        fs::create_dir_all(path)?;
        Self::open(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the path still names this directory. False after the
    /// directory was removed, renamed or replaced.
    pub(crate) fn is_current(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let (Ok(current), Ok(own)) = (fs::symlink_metadata(&self.path), self.dir.metadata())
            else {
                return false;
            };
            current.dev() == own.dev() && current.ino() == own.ino()
        }
        #[cfg(windows)]
        {
            crate::multiplexer::windows_acl::open_directory(&self.path)
                .is_ok_and(|current| same_file(&current, &self.dir) == Some(true))
        }
        #[cfg(not(any(unix, windows)))]
        self.path.is_dir()
    }

    /// Checks again that the path names this directory and that it is still
    /// private, before the token is written into it.
    pub(crate) fn verify(&self) -> anyhow::Result<()> {
        ensure!(
            self.is_current(),
            "the session directory {} was replaced",
            self.path.display()
        );
        #[cfg(unix)]
        self.check_private()?;
        #[cfg(windows)]
        crate::multiplexer::windows_acl::secure_directory(&self.dir, &self.path)?;
        Ok(())
    }

    pub(crate) fn open_file(&self, name: &str, access: Access) -> std::io::Result<File> {
        #[cfg(unix)]
        {
            use std::os::unix::io::{AsRawFd, FromRawFd};
            let flags = match access {
                Access::Read => libc::O_RDONLY,
                Access::ReadWrite => libc::O_RDWR,
                Access::CreateReadWrite => libc::O_RDWR | libc::O_CREAT,
                Access::CreateNew => libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                Access::CreateAppend => libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT,
            };
            let name = std::ffi::CString::new(name)?;
            // SAFETY: the directory handle is open for the life of `self`,
            // `name` is NUL-terminated, and the mode is passed as the
            // variadic argument `O_CREAT` expects.
            let fd = unsafe {
                libc::openat(
                    self.dir.as_raw_fd(),
                    name.as_ptr(),
                    flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    0o600 as libc::c_uint,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: `fd` was just opened, and nothing else owns it.
            Ok(unsafe { File::from_raw_fd(fd) })
        }
        #[cfg(not(unix))]
        {
            let mut options = fs::OpenOptions::new();
            match access {
                Access::Read => options.read(true),
                Access::ReadWrite => options.read(true).write(true),
                Access::CreateReadWrite => options.read(true).write(true).create(true),
                Access::CreateNew => options.read(true).write(true).create_new(true),
                Access::CreateAppend => options.append(true).create(true),
            };
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.custom_flags(
                    windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
                );
            }
            let file = options.open(self.path.join(name))?;
            #[cfg(windows)]
            crate::multiplexer::windows_acl::check_file(&file)
                .map_err(|error| std::io::Error::other(format!("{name}: {error:#}")))?;
            Ok(file)
        }
    }

    pub(crate) fn private_file(&self, name: &str) -> anyhow::Result<File> {
        Ok(self.open_file(name, Access::CreateReadWrite)?)
    }

    pub(crate) fn read(&self, name: &str) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.open_file(name, Access::Read)?
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub(crate) fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let (from, to) = (std::ffi::CString::new(from)?, std::ffi::CString::new(to)?);
            let dir = self.dir.as_raw_fd();
            // SAFETY: both names are NUL-terminated and the handle is open.
            if unsafe { libc::renameat(dir, from.as_ptr(), dir, to.as_ptr()) } < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        fs::rename(self.path.join(from), self.path.join(to))
    }

    pub(crate) fn remove(&self, name: &str) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let name = std::ffi::CString::new(name)?;
            // SAFETY: `name` is NUL-terminated and the handle is open.
            if unsafe { libc::unlinkat(self.dir.as_raw_fd(), name.as_ptr(), 0) } < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        fs::remove_file(self.path.join(name))
    }
}

/// The size at which `host.log` is rotated to `host.log.1`: at start-up, and
/// by the host's logger while it runs.
pub(crate) const HOST_LOG_LIMIT: u64 = 1024 * 1024;

/// Opens `host.log` for a new host and returns it with its current length,
/// where the new host's output will start.
fn open_host_log(root: &Root) -> anyhow::Result<(File, u64)> {
    if root
        .open_file("host.log", Access::Read)
        .and_then(|log| log.metadata())
        .is_ok_and(|metadata| metadata.len() > HOST_LOG_LIMIT)
    {
        // Best effort: on Windows a file still open elsewhere cannot be renamed.
        let _ = root.rename("host.log", "host.log.1");
    }
    let file = root
        .open_file("host.log", Access::CreateAppend)
        .context("open the session host log")?;
    let start = file.metadata()?.len();
    Ok((file, start))
}

/// The last lines a host wrote to `host.log` after `start`, formatted to
/// follow a start-up error.
fn host_log_tail(root: &Root, start: u64) -> String {
    const TAIL_BYTES: u64 = 4096;
    const TAIL_LINES: usize = 12;
    let path = root.path().join("host.log");
    let mut output = String::new();
    if let Ok(mut file) = root.open_file("host.log", Access::Read) {
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

pub(crate) fn read_endpoint(root: &Root) -> anyhow::Result<Endpoint> {
    Ok(serde_json::from_slice(&root.read("endpoint.json")?)?)
}

/// Replaces `endpoint.json` in one step, so a client never reads half of it.
/// The token goes into a file created here, never into one that was left in
/// the directory.
pub(crate) fn write_endpoint(root: &Root, endpoint: &Endpoint) -> anyhow::Result<()> {
    match root.remove("endpoint.tmp") {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    let mut file = root.open_file("endpoint.tmp", Access::CreateNew)?;
    file.write_all(&serde_json::to_vec(endpoint)?)?;
    file.sync_all()?;
    root.rename("endpoint.tmp", "endpoint.json")?;
    Ok(())
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
    connect_identified(root).map(|(stream, _)| stream)
}

/// Like [`connect`], also returning the host's token, which no other host
/// shares: a client can tell that the host it reached is a new one.
pub(crate) fn connect_identified(root: &Path) -> anyhow::Result<(TcpStream, String)> {
    if fs::symlink_metadata(root).is_err() {
        return Err(no_host(root));
    }
    let endpoint = match read_endpoint(&Root::open(root)?) {
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
            Response::Ok => Ok((stream, endpoint.token)),
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
    if let Ok(root) = Root::open(root) {
        clear_stale_endpoint(&root);
    }
    Ok(())
}

/// The endpoint of the host holding `host.lock`. Without one, removes a
/// leftover `endpoint.json` and reports that no host is running.
fn running_host_endpoint(root: &Path) -> anyhow::Result<Endpoint> {
    // A directory that exists but is not private says so, rather than that
    // no host is running.
    let directory = match Root::open(root) {
        Ok(directory) => directory,
        Err(error) if fs::symlink_metadata(root).is_ok() => return Err(error),
        Err(_) => return Err(no_host(root)),
    };
    if !lock_is_held(&directory) {
        clear_stale_endpoint(&directory);
        return Err(no_host(root));
    }
    read_endpoint(&directory).with_context(|| {
        format!(
            "the session host in {} has not published its endpoint",
            root.display()
        )
    })
}

/// Waits until no host holds `host.lock`; false if one still does at `timeout`.
pub(crate) fn wait_for_exit(root: &Path, timeout: Duration) -> bool {
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
fn clear_stale_endpoint(root: &Root) {
    if root.open_file("endpoint.json", Access::Read).is_err() {
        return;
    }
    let Ok(lock) = root.private_file("host.lock") else {
        return;
    };
    if matches!(lock.try_lock_exclusive(), Ok(true)) {
        let _ = root.remove("endpoint.json");
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

/// Whether the session host keeps the variable `name` from the process that
/// starts it. The host outlives that process and serves every later client,
/// so a virtualenv's PATH, a direnv token or an agent sandbox's variables
/// must not reach the panes of other clients (audit B28): each client sends
/// its own environment with every pane instead ([`PaneLaunch::environment`]).
/// What stays describes the user, the login session, the locale and the
/// machine, which the host needs for itself and for panes created without an
/// environment (by clients that send none): identity and the default shell,
/// temporary and XDG base directories, the display, SSH agent and session bus
/// of the login session, the log level, `TERMY_*` overrides, and on Windows
/// the system and profile folders that programs and the shell lookup expect.
/// One-shot launch tokens are left out. PATH is replaced by the login
/// default.
fn host_keeps_variable(name: &str) -> bool {
    #[cfg(windows)]
    const NAMES: &[&str] = &[
        "ALLUSERSPROFILE",
        "APPDATA",
        "COMMONPROGRAMFILES",
        "COMMONPROGRAMFILES(X86)",
        "COMMONPROGRAMW6432",
        "COMPUTERNAME",
        "COMSPEC",
        "HOMEDRIVE",
        "HOMEPATH",
        "LOCALAPPDATA",
        "NUMBER_OF_PROCESSORS",
        "OS",
        "PATHEXT",
        "PROCESSOR_ARCHITECTURE",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PROGRAMW6432",
        "PUBLIC",
        "RUST_BACKTRACE",
        "RUST_LOG",
        "SYSTEMDRIVE",
        "SYSTEMROOT",
        "TEMP",
        "TMP",
        "USERDOMAIN",
        "USERNAME",
        "USERPROFILE",
        "WINDIR",
    ];
    #[cfg(not(windows))]
    const NAMES: &[&str] = &[
        "DBUS_SESSION_BUS_ADDRESS",
        "DISPLAY",
        "HOME",
        "LANG",
        "LANGUAGE",
        "LOGNAME",
        "RUST_BACKTRACE",
        "RUST_LOG",
        "SHELL",
        "SSH_AUTH_SOCK",
        "TMPDIR",
        "TZ",
        "USER",
        "WAYLAND_DISPLAY",
    ];
    // Windows names are case-insensitive.
    let kept = if cfg!(windows) {
        NAMES.iter().any(|known| known.eq_ignore_ascii_case(name))
    } else {
        NAMES.contains(&name)
            || name.starts_with("LC_")
            || (name.starts_with("XDG_") && name != "XDG_ACTIVATION_TOKEN")
    };
    let termy = name
        .get(.."TERMY_".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("TERMY_"));
    (kept || termy)
        && !crate::terminal_engine::transport::belongs_to_parent_terminal(std::ffi::OsStr::new(
            name,
        ))
}

/// The environment a new session host starts with; see [`host_keeps_variable`].
fn host_environment(
    variables: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<_> = variables
        .into_iter()
        .filter(|(name, _)| name.to_str().is_some_and(host_keeps_variable))
        .collect();
    if let Some(path) = crate::normalized_path_env(None) {
        environment.push(("PATH".into(), path.into()));
    }
    environment
}

/// Starts the host, or connects to the one already running in `root`. A
/// relative `root` is resolved against this process's directory.
pub fn connect_or_start(root: &Path, executable: &Path) -> anyhow::Result<SessionClient> {
    // The host runs in the home directory, so it must get an absolute path.
    let root = Root::prepare(&std::path::absolute(root)?)?;
    let launch_lock = root.private_file("launch.lock")?;
    launch_lock.lock_exclusive()?;
    let first_error = match SessionClient::connect(root.path()) {
        Ok(client) => return Ok(client.starting_hosts_with(executable)),
        Err(error) => error,
    };
    // A live host's lock is authoritative. Never replace an incompatible or
    // temporarily unresponsive host and abandon its running processes.
    let host_lock = root.private_file("host.lock")?;
    if !host_lock.try_lock_exclusive()? {
        return Err(Unreachable(first_error).into());
    }
    FileExt::unlock(&host_lock)?;

    let (log, log_start) = open_host_log(&root)?;
    let mut command = Command::new(executable);
    command
        .arg("--multiplexer-host")
        .arg(root.path())
        .env_clear()
        .envs(host_environment(std::env::vars_os()))
        // Not this process's directory: the host would keep it busy (it
        // could not be unmounted) and hand it to panes.
        .current_dir(
            dirs::home_dir()
                .filter(|home| home.is_dir())
                .unwrap_or_else(|| root.path().to_owned()),
        )
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
    let mut child = spawn_detached(&mut command)?;
    #[cfg(not(windows))]
    let mut child = command.spawn().context("start background terminal host")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(client) = SessionClient::connect(root.path()) {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(client.starting_hosts_with(executable));
        }
        if let Some(status) = child.try_wait()? {
            bail!(
                "background terminal host exited: {status}{}",
                host_log_tail(&root, log_start)
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "background terminal host did not become ready; it was left running{}",
                host_log_tail(&root, log_start)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Starts the host outside the caller's console and job (audit B8). A Termy
/// tab puts its shell in a job that ends the tab's console programs when it
/// closes; a host started from that shell (`termy mux create`) would end with
/// it, taking every persistent session along. Tab jobs allow breakaway; a job
/// that does not (some CI and service hosts) fails the spawn with
/// `ERROR_ACCESS_DENIED`, and the host then starts inside it.
#[cfg(windows)]
fn spawn_detached(command: &mut Command) -> anyhow::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::{
        Foundation::ERROR_ACCESS_DENIED,
        System::Threading::{CREATE_BREAKAWAY_FROM_JOB, DETACHED_PROCESS},
    };
    prevent_standard_handle_inheritance()?;
    // Detaching already isolates the host from the caller's console.
    // CREATE_NEW_PROCESS_GROUP would also disable Ctrl+C in the host
    // and its descendants, including shells attached to ConPTY.
    command.creation_flags(DETACHED_PROCESS | CREATE_BREAKAWAY_FROM_JOB);
    match command.spawn() {
        Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
            log::warn!(
                "the session host cannot leave the job this process runs in (a job, or one it is nested in, forbids breakaway), so it ends when that job does"
            );
            command.creation_flags(DETACHED_PROCESS);
            command.spawn()
        }
        result => result,
    }
    .context("start background terminal host")
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

/// A temporary directory that passes [`Root::open`]: `tempfile` creates
/// directories with the umask's permissions, usually 0755.
#[cfg(test)]
pub(crate) fn private_tempdir() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    temp
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn host_lock_probe_detects_a_running_host() {
        let temp = private_tempdir();
        assert!(!host_is_running(temp.path()));
        assert!(!temp.path().join("host.lock").exists());

        let held = Root::open(temp.path())
            .unwrap()
            .private_file("host.lock")
            .unwrap();
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
        let lock = Root::open(root).unwrap().private_file("host.lock").unwrap();
        assert!(lock.try_lock_exclusive().unwrap());
        lock
    }

    #[test]
    fn client_rejects_a_host_of_another_protocol_version() {
        let temp = private_tempdir();
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
            let temp = private_tempdir();
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
        let temp = private_tempdir();
        let root = temp.path().join("host");
        Root::prepare(&root).unwrap();
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
        let temp = private_tempdir();
        let host = fake_host(temp.path(), 3, true, "rc8-token");
        shutdown_host(temp.path()).unwrap();
        assert_eq!(host.join().unwrap(), [14, 15, 10]);
        assert!(!host_is_running(temp.path()));
    }

    #[test]
    fn shutdown_host_stops_a_host_without_mutual_authentication() {
        let temp = private_tempdir();
        let host = fake_host(temp.path(), 2, false, "rc4-token");
        shutdown_host(temp.path()).unwrap();
        assert_eq!(host.join().unwrap(), [0, 10]);
    }

    /// The plaintext greeting reveals the token, so it is sent only while
    /// the host that wrote it still holds its lock.
    #[test]
    fn shutdown_host_does_not_send_the_token_to_a_stale_endpoint() {
        let temp = private_tempdir();
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
        let temp = private_tempdir();
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
        let temp = private_tempdir();
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
        while read_endpoint(&Root::open(temp.path()).unwrap()).is_err() {
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
        let temp = private_tempdir();
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
        let temp = private_tempdir();
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
        let temp = private_tempdir();
        fs::write(temp.path().join("host.log"), "an earlier host\n").unwrap();
        let root = Root::open(temp.path()).unwrap();
        let (mut log, start) = open_host_log(&root).unwrap();
        writeln!(log, "session host panicked: boom").unwrap();
        let tail = host_log_tail(&root, start);
        assert!(tail.contains("session host panicked: boom"), "{tail}");
        assert!(!tail.contains("an earlier host"), "{tail}");
        assert!(host_log_tail(&root, start + 100).contains("no output"));
    }

    #[test]
    fn host_log_is_private_and_rotated_past_its_limit() {
        let temp = private_tempdir();
        let path = temp.path().join("host.log");
        fs::write(&path, vec![b'x'; HOST_LOG_LIMIT as usize + 1]).unwrap();
        let (_, start) = open_host_log(&Root::open(temp.path()).unwrap()).unwrap();
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

    /// Audit L4: another user who can write a directory above the session
    /// directory may swap it after the privacy check. The token must still
    /// land in the directory that was checked.
    #[cfg(unix)]
    #[test]
    fn session_files_stay_in_the_checked_directory_when_its_path_is_swapped() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = private_tempdir();
        let shared = temp.path().join("shared");
        fs::create_dir(&shared).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
        let path = shared.join("mux");
        let root = Root::prepare(&path).unwrap();
        // The swap: the checked directory moves away, and a directory the
        // other user controls takes its place.
        fs::rename(&path, shared.join("moved")).unwrap();
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(!root.is_current());
        super::write_endpoint(
            &root,
            &Endpoint {
                version: VERSION,
                port: 1,
                token: "secret".into(),
                mutual_auth: true,
                build: String::new(),
                pid: 0,
            },
        )
        .unwrap();
        assert!(shared.join("moved/endpoint.json").exists());
        assert!(!path.join("endpoint.json").exists());
        // Clients and hosts refuse the replacement, and a symlinked one.
        let error = Root::open(&path).err().unwrap().to_string();
        assert!(error.contains("must be private"), "{error}");
        fs::remove_dir(&path).unwrap();
        symlink(shared.join("moved"), &path).unwrap();
        let error = Root::prepare(&path).err().unwrap().to_string();
        assert!(error.contains("invalid multiplexer directory"), "{error}");
    }

    /// Audit L4 on Windows: a session directory of this user that grants
    /// others access (one an older Termy created under `C:\`) is made
    /// private, a file someone left in it is not reused for the token, and a
    /// junction is refused.
    #[cfg(windows)]
    #[test]
    fn windows_session_directory_is_made_private() {
        use crate::multiplexer::windows_acl;
        let temp = private_tempdir();
        let shared = temp.path().join("shared");
        fs::create_dir(&shared).unwrap();
        windows_acl::set_dacl(&shared, "D:P(A;OICI;FA;;;WD)").unwrap();
        assert!(!windows_acl::is_private(&shared));
        let root = Root::prepare(&shared).unwrap();
        assert!(windows_acl::is_private(&shared));
        fs::write(shared.join("endpoint.tmp"), "planted").unwrap();
        super::write_endpoint(
            &root,
            &Endpoint {
                version: VERSION,
                port: 1,
                token: "secret".into(),
                mutual_auth: true,
                build: String::new(),
                pid: 0,
            },
        )
        .unwrap();
        assert!(read_endpoint(&root).unwrap().token == "secret");
        drop(root);

        let junction = temp.path().join("junction");
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&shared)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        let error = Root::open(&junction).err().unwrap().to_string();
        assert!(error.contains("invalid multiplexer directory"), "{error}");
    }

    /// Audit B28: the host keeps only what describes the user and machine,
    /// not the launching shell's virtualenv, tokens or terminal identity.
    #[test]
    fn host_starts_with_only_login_variables() {
        let variables = [
            ("HOME", "/home/user"),
            ("LC_ALL", "C.UTF-8"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("XDG_ACTIVATION_TOKEN", "one-shot"),
            ("DISPLAY", ":0"),
            ("TERMY_INSTANCE_HOME", "/tmp/instance"),
            ("TERMY_MUX_PANE", "outer-pane"),
            ("PATH", "/home/user/venv/bin"),
            ("VIRTUAL_ENV", "/home/user/venv"),
            ("GITHUB_TOKEN", "secret"),
            ("TMUX", "/tmp/tmux-1000/default"),
        ]
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let environment = host_environment(variables);
        let names: Vec<_> = environment
            .iter()
            .map(|(name, _)| name.to_str().unwrap())
            .collect();
        #[cfg(unix)]
        assert_eq!(
            names,
            [
                "HOME",
                "LC_ALL",
                "XDG_RUNTIME_DIR",
                "DISPLAY",
                "TERMY_INSTANCE_HOME",
                "PATH"
            ]
        );
        #[cfg(windows)]
        assert_eq!(names, ["TERMY_INSTANCE_HOME", "PATH"]);
        let path = &environment.last().unwrap().1;
        assert!(
            !path.to_string_lossy().contains("venv"),
            "PATH must be the login default: {path:?}"
        );
    }
}
