use crate::multiplexer::{SessionClient, protocol::*};
use anyhow::{Context, bail, ensure};
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    net::TcpStream,
    path::Path,
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

pub(crate) fn connect(root: &Path) -> anyhow::Result<TcpStream> {
    connect_with_capabilities(root).map(|(stream, _)| stream)
}

pub(crate) fn connect_with_capabilities(root: &Path) -> anyhow::Result<(TcpStream, bool)> {
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
    write_message(
        &mut stream,
        &Request::Hello {
            version: VERSION,
            token: endpoint.token,
        },
    )?;
    match read_message(&mut stream)? {
        Response::Ok => Ok((stream, endpoint.conditional_layout_updates)),
        Response::Error(error) => bail!(error),
        _ => bail!("unexpected multiplexer greeting"),
    }
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
        command.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
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
