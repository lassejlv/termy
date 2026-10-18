use crate::multiplexer::{SessionClient, protocol::*};
use anyhow::{Context, bail, ensure};
use fs4::fs_std::FileExt;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
pub(crate) struct Endpoint {
    pub version: u32,
    pub port: u16,
    pub token: String,
    #[serde(default)]
    pub conditional_layout_updates: bool,
    #[serde(default)]
    pub graphics_stream: bool,
    #[serde(default)]
    pub viewport_replies: bool,
    /// The host proves it knows the token before the client sends anything.
    #[serde(default)]
    pub mutual_auth: bool,
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
#[cfg(windows)]
fn host_is_running(root: &Path) -> bool {
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

pub(crate) fn private_file(path: &Path) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    Ok(options.open(path)?)
}

pub(crate) fn connect_with_capabilities(root: &Path) -> anyhow::Result<(TcpStream, Endpoint)> {
    let endpoint: Endpoint = serde_json::from_slice(&fs::read(root.join("endpoint.json"))?)?;
    ensure!(
        endpoint.version == VERSION,
        "a different multiplexer protocol is running; use the matching Termy version to detach or close its sessions"
    );
    let mut stream = TcpStream::connect_timeout(
        &([127, 0, 0, 1], endpoint.port).into(),
        Duration::from_secs(2),
    )?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    if endpoint.mutual_auth {
        authenticate(&mut stream, &endpoint.token)?;
    } else {
        // Hosts from before mutual authentication only accept the token.
        write_message(
            &mut stream,
            &Request::Hello {
                version: VERSION,
                token: endpoint.token.clone(),
            },
        )?;
    }
    match read_message_limited(&mut stream, 4096)? {
        Response::Ok => Ok((stream, endpoint)),
        Response::Error(error) => bail!(error),
        _ => bail!("unexpected multiplexer greeting"),
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

pub fn connect_or_start(root: &Path, executable: &Path) -> anyhow::Result<SessionClient> {
    prepare_root(root)?;
    let launch_lock = private_file(&root.join("launch.lock"))?;
    launch_lock.lock_exclusive()?;
    if let Ok(client) = SessionClient::connect(root) {
        return Ok(client);
    }
    // A live host's lock is authoritative. Never replace an incompatible or
    // temporarily unresponsive host and abandon its running processes.
    let host_lock = private_file(&root.join("host.lock"))?;
    ensure!(
        host_lock.try_lock_exclusive()?,
        "the multiplexer is running but cannot be reached"
    );
    FileExt::unlock(&host_lock)?;

    let mut command = Command::new(executable);
    command
        .arg("--multiplexer-host")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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
            bail!("background terminal host exited: {status}");
        }
        if Instant::now() >= deadline {
            bail!("background terminal host did not become ready; it was left running");
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
    use std::net::TcpListener;

    #[cfg(windows)]
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

    /// After a host crash another local user can bind the freed port. The
    /// client must not hand that listener the token or accept it as the host.
    #[test]
    fn client_rejects_listener_that_does_not_know_the_token() {
        let temp = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "f".repeat(64);
        fs::write(
            temp.path().join("endpoint.json"),
            serde_json::json!({
                "version": VERSION,
                "port": port,
                "token": token,
                "mutual_auth": true,
            })
            .to_string(),
        )
        .unwrap();
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
        let result = connect_with_capabilities(temp.path());
        let greeting = impostor.join().unwrap();
        assert!(result.is_err(), "client accepted an impostor host");
        assert!(
            matches!(greeting, Request::Authenticate { .. }),
            "client must not send the token to an unauthenticated listener"
        );
    }
}
