use crate::deeplink::{new_tab_deeplink_for_dir, new_window_deeplink};
use crate::{LaunchTarget, StartupArguments};
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Handoff to primaries that predate token authentication.
const LEGACY_PROTOCOL_PREFIX: &str = "TERMY1 ";
/// `TERMY2 <token>\n<url>\n...`; the primary answers [`HANDOFF_ACK`].
const PROTOCOL_PREFIX: &str = "TERMY2 ";
const HANDOFF_ACK: &[u8] = b"TERMY2 OK\n";
const CONNECT_ATTEMPTS: usize = 40;
const CONNECT_WAIT: Duration = Duration::from_millis(100);
const HANDOFF_READ_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_HANDOFF_BYTES: usize = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct Endpoint {
    port: u16,
    /// A per-launch secret that proves a handoff reached this Termy and not
    /// whatever else now listens on a stale port. Missing in endpoints
    /// written by older versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}

pub(crate) enum InstanceClaim {
    Primary(InstanceGuard),
    Forwarded,
}

pub(crate) struct InstanceGuard {
    listener: TcpListener,
    home: PathBuf,
    token: String,
    _lock: File,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        remove_endpoint(&self.home, &self.token);
        // Keep the lock file: other launchers may already have it open. The OS
        // releases our lock when the handle closes, including on process exit.
    }
}

/// The endpoint this process published, so it can be removed at quit. The
/// guard lives in the listener thread, which never returns, so its `Drop`
/// does not run on a normal exit.
static PUBLISHED_ENDPOINT: std::sync::Mutex<Option<(PathBuf, String)>> =
    std::sync::Mutex::new(None);

/// Removes `instance.json` if it is still the one this process wrote.
pub(crate) fn remove_published_endpoint() {
    let published = PUBLISHED_ENDPOINT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some((home, token)) = published {
        remove_endpoint(&home, &token);
    }
}

fn remove_endpoint(home: &Path, token: &str) {
    if read_endpoint(home)
        .and_then(|endpoint| endpoint.token)
        .as_deref()
        == Some(token)
    {
        let _ = fs::remove_file(home.join("instance.json"));
    }
}

/// What a launch without `--new-window`, `--new-tab` or deeplinks asks the
/// running instance for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlainLaunch {
    /// macOS: focus the running app; a directory opens a tab.
    Activate,
    /// Linux: every launch, with or without a directory, opens a window.
    NewWindow,
    /// Windows: a bare launch (taskbar Shift/middle-click, Start menu) opens
    /// a window like Windows Terminal; a directory (Explorer verb) opens a tab.
    NewWindowWithoutDirectory,
}

impl PlainLaunch {
    pub(crate) const CURRENT: Self = if cfg!(target_os = "linux") {
        Self::NewWindow
    } else if cfg!(target_os = "windows") {
        Self::NewWindowWithoutDirectory
    } else {
        Self::Activate
    };
}

pub(crate) fn urls_to_forward(startup: &StartupArguments) -> Vec<String> {
    urls_to_forward_for_platform(startup, PlainLaunch::CURRENT)
}

pub(super) fn urls_to_forward_for_platform(
    startup: &StartupArguments,
    plain_launch: PlainLaunch,
) -> Vec<String> {
    let mut urls = Vec::new();
    let new_window = match startup.launch_target {
        LaunchTarget::Window => true,
        LaunchTarget::Tab => false,
        LaunchTarget::Default if !startup.deeplinks.is_empty() => false,
        LaunchTarget::Default => match plain_launch {
            PlainLaunch::Activate => false,
            PlainLaunch::NewWindow => true,
            PlainLaunch::NewWindowWithoutDirectory => startup.working_dir.is_none(),
        },
    };
    if new_window {
        urls.push(new_window_deeplink(startup.working_dir.as_deref()));
    } else if let Some(dir) = &startup.working_dir {
        urls.push(new_tab_deeplink_for_dir(dir));
    } else if startup.launch_target == LaunchTarget::Tab {
        urls.push("termy://new".to_string());
    }
    urls.extend(startup.deeplinks.iter().cloned());
    if urls.is_empty() {
        urls.push("termy://".to_string());
    }
    urls
}

pub(crate) fn claim_or_forward(urls: &[String]) -> io::Result<InstanceClaim> {
    claim_or_forward_in(&instance_home()?, urls)
}

pub(crate) fn claim_or_forward_in(home: &Path, urls: &[String]) -> io::Result<InstanceClaim> {
    fs::create_dir_all(home)?;
    let lock_path = home.join("instance.lock");

    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;

    for _ in 0..CONNECT_ATTEMPTS {
        // The primary holds the lock for its whole life, so once we hold it
        // any instance.json is stale (left by a crash or a forced quit) and its
        // port may now belong to an unrelated app. Never forward then.
        if lock.try_lock_exclusive()? {
            return occupy_primary(home.to_path_buf(), lock);
        }
        if forward_to_existing(home, urls)? {
            return Ok(InstanceClaim::Forwarded);
        }
        // Only wait while another process actually owns the lock. Retrying the
        // lock also recovers promptly if that process exits during startup.
        std::thread::sleep(CONNECT_WAIT);
    }
    Err(io::Error::other(
        "another Termy instance is starting; try again",
    ))
}

pub(crate) fn spawn_listener(guard: InstanceGuard, tx: flume::Sender<Vec<String>>) {
    if let Err(error) = std::thread::Builder::new()
        .name("termy-instance".to_string())
        .spawn(move || listen_for_handoffs(guard, tx))
    {
        log::warn!("Failed to start Termy instance listener: {error}");
    }
}

fn occupy_primary(home: PathBuf, lock: File) -> io::Result<InstanceClaim> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let payload = serde_json::to_vec(&Endpoint {
        port,
        token: Some(token.clone()),
    })
    .map_err(io::Error::other)?;
    // Replace the stale file in one step so a waiting launcher never reads a
    // half-written endpoint.
    let mut temp = tempfile::NamedTempFile::new_in(&home)?;
    temp.write_all(&payload)?;
    temp.persist(home.join("instance.json"))
        .map_err(|error| error.error)?;
    *PUBLISHED_ENDPOINT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((home.clone(), token.clone()));
    Ok(InstanceClaim::Primary(InstanceGuard {
        listener,
        home,
        token,
        _lock: lock,
    }))
}

fn listen_for_handoffs(guard: InstanceGuard, tx: flume::Sender<Vec<String>>) {
    for stream in guard.listener.incoming() {
        match stream.and_then(|stream| accept_handoff(stream, &guard.token)) {
            Ok(urls) if !urls.is_empty() => {
                if let Err(error) = tx.send(urls) {
                    log::error!("Failed to enqueue forwarded Termy request: {error}");
                }
            }
            Ok(_) => {}
            Err(error) => {
                log::warn!("Failed to read forwarded Termy request: {error}");
            }
        }
    }
}

fn forward_to_existing(home: &Path, urls: &[String]) -> io::Result<bool> {
    let Some(endpoint) = read_endpoint(home) else {
        return Ok(false);
    };
    let address = SocketAddr::from(([127, 0, 0, 1], endpoint.port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(150)) else {
        return Ok(false);
    };
    let Some(token) = endpoint.token else {
        // A primary from before token authentication: it sends no reply.
        write_payload(&mut stream, &encode_forward(urls))?;
        return Ok(true);
    };
    match write_payload(&mut stream, &encode_authenticated_forward(&token, urls)) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => return Err(error),
        Err(_) => return Ok(false),
    }
    // Only a Termy that knows the token acknowledges. Anything else means
    // the endpoint is stale or still being replaced, so keep waiting.
    Ok(read_handoff_ack(&mut stream))
}

fn read_endpoint(home: &Path) -> Option<Endpoint> {
    let payload = fs::read(home.join("instance.json")).ok()?;
    serde_json::from_slice::<Endpoint>(&payload).ok()
}

#[cfg(test)]
fn read_port(home: &Path) -> Option<u16> {
    read_endpoint(home).map(|endpoint| endpoint.port)
}

fn read_handoff_ack(stream: &mut TcpStream) -> bool {
    let mut reply = Vec::new();
    let mut chunk = [0u8; 64];
    let deadline = Instant::now() + HANDOFF_READ_TIMEOUT;
    while reply.len() < HANDOFF_ACK.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || stream.set_read_timeout(Some(remaining)).is_err() {
            return false;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => reply.extend_from_slice(&chunk[..count]),
            // Retry reads interrupted by signals, as the primary does.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
    reply == HANDOFF_ACK
}

/// Reads one handoff, checks its token and acknowledges it.
fn accept_handoff(mut stream: TcpStream, token: &str) -> io::Result<Vec<String>> {
    let payload = read_handoff_payload(&mut stream)?;
    let urls = decode_authenticated_forward(&payload, token).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid or unauthenticated instance payload",
        )
    })?;
    stream.write_all(HANDOFF_ACK)?;
    stream.flush()?;
    Ok(urls)
}

#[cfg(test)]
fn write_forwarded_urls(stream: &mut TcpStream, urls: &[String]) -> io::Result<()> {
    write_payload(stream, &encode_forward(urls))
}

fn write_payload(stream: &mut TcpStream, payload: &str) -> io::Result<()> {
    if payload.len() > MAX_HANDOFF_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "instance payload exceeds size limit",
        ));
    }
    stream.write_all(payload.as_bytes())?;
    stream.flush()?;
    stream.shutdown(Shutdown::Write)
}

fn read_handoff_payload(stream: &mut TcpStream) -> io::Result<String> {
    let deadline = Instant::now() + HANDOFF_READ_TIMEOUT;
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "instance read timed out",
            ));
        }
        stream.set_read_timeout(Some(remaining))?;
        let mut chunk = [0u8; 4096];
        // Linux returns EINTR for timed socket reads even with SA_RESTART, and
        // PTY runtimes can install SIGCHLD handlers, so a PTY exit must not
        // abort the handoff.
        let count = match stream.read(&mut chunk) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if count == 0 {
            break;
        }
        if bytes.len() + count > MAX_HANDOFF_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "instance payload exceeds size limit",
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub(crate) fn encode_forward(urls: &[String]) -> String {
    let mut payload = String::from(LEGACY_PROTOCOL_PREFIX);
    payload.push_str(&urls.join("\n"));
    payload.push('\n');
    payload
}

fn encode_authenticated_forward(token: &str, urls: &[String]) -> String {
    let mut payload = format!("{PROTOCOL_PREFIX}{token}\n");
    payload.push_str(&urls.join("\n"));
    payload.push('\n');
    payload
}

fn decode_authenticated_forward(payload: &str, token: &str) -> Option<Vec<String>> {
    let (header, body) = payload.strip_prefix(PROTOCOL_PREFIX)?.split_once('\n')?;
    if header.trim() != token {
        return None;
    }
    Some(decode_urls(body))
}

#[cfg(test)]
pub(crate) fn decode_forward(payload: &str) -> Option<Vec<String>> {
    Some(decode_urls(payload.strip_prefix(LEGACY_PROTOCOL_PREFIX)?))
}

fn decode_urls(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn instance_home() -> io::Result<PathBuf> {
    if let Ok(path) = std::env::var("TERMY_INSTANCE_HOME") {
        return Ok(PathBuf::from(path));
    }
    // Prefer the per-user data dir so GUI launches (file managers) and
    // terminal launches share the same instance lock. XDG_RUNTIME_DIR is
    // often unset when Explorer/Nemo/Finder spawn Termy.
    let base = dirs::data_local_dir()
        .or_else(dirs::runtime_dir)
        .ok_or_else(|| io::Error::other("could not resolve Termy instance directory"))?;
    Ok(base.join("termy").join("instance"))
}

#[cfg(test)]
mod tests {
    use super::{
        InstanceClaim, claim_or_forward_in, decode_forward, encode_forward, spawn_listener,
        urls_to_forward,
    };
    use crate::StartupArguments;
    use fs4::fs_std::FileExt;
    use std::fs;
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    #[test]
    fn stale_instance_files_do_not_delay_startup() {
        for has_endpoint in [false, true] {
            let temp = tempfile::tempdir().expect("tempdir");
            let home = temp.path();
            fs::write(home.join("instance.lock"), "").expect("stale lock");
            if has_endpoint {
                let listener = TcpListener::bind("127.0.0.1:0").expect("unused port");
                let port = listener.local_addr().expect("listener address").port();
                fs::write(home.join("instance.json"), format!("{{\"port\":{port}}}"))
                    .expect("stale endpoint");
                drop(listener);
            }

            let started = Instant::now();
            let claim = claim_or_forward_in(home, &["termy://".to_string()])
                .expect("stale files should not prevent a primary claim");
            assert!(matches!(claim, InstanceClaim::Primary(_)));
            assert!(
                started.elapsed() < Duration::from_millis(500),
                "stale instance files delayed startup by {:?} (endpoint: {has_endpoint})",
                started.elapsed()
            );
        }
    }

    #[test]
    fn stale_endpoint_is_never_forwarded_to_once_the_lock_is_acquired() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path();
        // Another app now listens on the port a crashed primary left behind.
        let unrelated = TcpListener::bind("127.0.0.1:0").expect("unrelated listener");
        unrelated.set_nonblocking(true).expect("nonblocking");
        let port = unrelated.local_addr().expect("listener address").port();
        fs::write(home.join("instance.json"), format!("{{\"port\":{port}}}"))
            .expect("stale endpoint");

        let claim = claim_or_forward_in(home, &["termy://".to_string()]).expect("claim");
        assert!(
            matches!(claim, InstanceClaim::Primary(_)),
            "a launch holding the lock must not forward to a stale endpoint"
        );
        assert!(
            unrelated.accept().is_err(),
            "the unrelated app must not receive a handoff"
        );
        assert_ne!(super::read_port(home), Some(port));
    }

    #[test]
    fn primary_cleanup_keeps_waiters_on_the_same_lock_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path();
        let first = claim_or_forward_in(home, &[]).expect("first primary");
        assert!(matches!(first, InstanceClaim::Primary(_)));
        let waiter = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(home.join("instance.lock"))
            .expect("waiter lock handle");
        assert!(!waiter.try_lock_exclusive().expect("primary holds lock"));

        drop(first);
        assert!(home.join("instance.lock").exists());
        assert!(!home.join("instance.json").exists());
        let second = claim_or_forward_in(home, &[]).expect("replacement primary");
        assert!(matches!(second, InstanceClaim::Primary(_)));
        assert!(
            !waiter.try_lock_exclusive().expect("replacement holds lock"),
            "a waiter must not acquire a separate, unlinked lock file"
        );
    }

    #[test]
    fn secondary_forwards_to_legacy_primary_holding_the_lock() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path();
        // Versions before token authentication hold the lock and publish a
        // port without a token; they read TERMY1 and send no reply.
        let legacy_lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(home.join("instance.lock"))
            .expect("legacy lock");
        assert!(
            legacy_lock
                .try_lock_exclusive()
                .expect("legacy primary lock")
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("legacy listener");
        let port = listener.local_addr().expect("listener address").port();
        fs::write(home.join("instance.json"), format!("{{\"port\":{port}}}"))
            .expect("legacy endpoint");

        let urls = vec!["termy://new?dir=%2Ftmp%2Fdemo".to_string()];
        let claim = claim_or_forward_in(home, &urls).expect("secondary claim");
        assert!(matches!(claim, InstanceClaim::Forwarded));
        let (mut stream, _) = listener.accept().expect("forwarded connection");
        let payload = super::read_handoff_payload(&mut stream).expect("forwarded payload");
        assert_eq!(decode_forward(&payload), Some(urls));
    }

    #[test]
    fn handoff_without_acknowledgement_is_not_treated_as_forwarded() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path();
        let unrelated = TcpListener::bind("127.0.0.1:0").expect("unrelated listener");
        let port = unrelated.local_addr().expect("listener address").port();
        fs::write(
            home.join("instance.json"),
            format!("{{\"port\":{port},\"token\":\"secret\"}}"),
        )
        .expect("endpoint");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = unrelated.accept().expect("connection");
            let _ = super::read_handoff_payload(&mut stream);
            // Close without replying, like any non-Termy service would.
        });

        assert!(!super::forward_to_existing(home, &["termy://".to_string()]).expect("forward"));
        server.join().expect("unrelated server");
    }

    #[test]
    fn primary_rejects_handoffs_with_the_wrong_token() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let mut client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        std::io::Write::write_all(
            &mut client,
            super::encode_authenticated_forward("wrong", &["termy://".to_string()]).as_bytes(),
        )
        .unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let error = super::accept_handoff(stream, "right").expect_err("wrong token");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(!super::read_handoff_ack(&mut client));
    }

    #[test]
    fn endpoint_is_only_removed_by_the_instance_that_wrote_it() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path();
        fs::write(
            home.join("instance.json"),
            "{\"port\":1,\"token\":\"newer\"}",
        )
        .expect("endpoint");
        super::remove_endpoint(home, "older");
        assert!(home.join("instance.json").exists());
        super::remove_endpoint(home, "newer");
        assert!(!home.join("instance.json").exists());
    }

    #[test]
    fn incomplete_handoff_does_not_block_the_listener_indefinitely() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let started = Instant::now();
        let error = super::read_handoff_payload(&mut stream).expect_err("incomplete handoff");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "incomplete client held the listener too long: {error}"
        );
        drop(client);
    }

    #[test]
    fn slow_handoff_has_a_total_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let mut client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let writer = std::thread::spawn(move || {
            for _ in 0..8 {
                if std::io::Write::write_all(&mut client, b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        let started = Instant::now();
        let error = super::read_handoff_payload(&mut stream).expect_err("slow handoff");
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
        assert!(started.elapsed() < Duration::from_secs(3));
        writer.join().expect("slow client");
    }

    #[test]
    fn oversized_handoff_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let mut client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let urls = ["a".repeat(super::MAX_HANDOFF_BYTES)];
        let write_error = super::write_forwarded_urls(&mut client, &urls)
            .expect_err("sender must reject oversized handoff");
        assert_eq!(write_error.kind(), std::io::ErrorKind::InvalidInput);
        let payload = super::encode_forward(&urls);
        std::io::Write::write_all(&mut client, payload.as_bytes()).unwrap();
        drop(client);
        let error = super::read_handoff_payload(&mut stream).expect_err("oversized handoff");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn encodes_and_decodes_forwarded_urls() {
        let encoded = encode_forward(&["termy://".to_string(), "termy://new".to_string()]);
        assert!(encoded.starts_with("TERMY1 "));
        assert_eq!(
            decode_forward(&encoded),
            Some(vec!["termy://".to_string(), "termy://new".to_string()])
        );

        let urls = vec!["termy://new".to_string()];
        let authenticated = super::encode_authenticated_forward("t0k", &urls);
        assert_eq!(
            super::decode_authenticated_forward(&authenticated, "t0k"),
            Some(urls)
        );
        assert_eq!(
            super::decode_authenticated_forward(&authenticated, "other"),
            None
        );
        assert_eq!(super::decode_authenticated_forward(&encoded, "t0k"), None);
    }

    #[test]
    fn launch_routing_preserves_explicit_window_tab_and_deeplink_intent() {
        use super::PlainLaunch::{Activate, NewWindow, NewWindowWithoutDirectory};

        for (args, plain_launch, expected) in [
            (vec![], NewWindow, "termy://window"),
            (
                vec!["--working-directory", "/tmp/demo"],
                NewWindow,
                "termy://window?dir=%2Ftmp%2Fdemo",
            ),
            (
                vec!["/tmp/demo"],
                NewWindow,
                "termy://window?dir=%2Ftmp%2Fdemo",
            ),
            (vec![], Activate, "termy://"),
            (vec!["/tmp/demo"], Activate, "termy://new?dir=%2Ftmp%2Fdemo"),
            (vec!["--new-window"], Activate, "termy://window"),
            (
                vec!["--new-window", "/tmp/demo"],
                Activate,
                "termy://window?dir=%2Ftmp%2Fdemo",
            ),
            (vec![], NewWindowWithoutDirectory, "termy://window"),
            (
                vec!["--working-directory", "/tmp/demo"],
                NewWindowWithoutDirectory,
                "termy://new?dir=%2Ftmp%2Fdemo",
            ),
            (vec!["--dock-action", "0"], Activate, "termy://window"),
            (
                vec!["termy://settings"],
                NewWindowWithoutDirectory,
                "termy://settings",
            ),
            (vec!["--new-tab"], NewWindow, "termy://new"),
            (
                vec!["--new-tab", "/tmp/demo"],
                NewWindow,
                "termy://new?dir=%2Ftmp%2Fdemo",
            ),
            (vec!["termy://settings"], NewWindow, "termy://settings"),
            (vec!["termy://new"], NewWindow, "termy://new"),
            (vec!["termy://"], NewWindow, "termy://"),
        ] {
            let startup = crate::parse_startup_arguments(args.clone());
            assert_eq!(
                super::urls_to_forward_for_platform(&startup, plain_launch),
                vec![expected],
                "{args:?}, {plain_launch:?}"
            );
        }
    }

    #[test]
    fn working_directory_forwards_as_new_tab_deeplink() {
        let startup = StartupArguments {
            working_dir: Some("/tmp/demo".to_string()),
            deeplinks: Vec::new(),
            launch_target: crate::LaunchTarget::Tab,
        };
        assert_eq!(
            urls_to_forward(&startup),
            vec!["termy://new?dir=%2Ftmp%2Fdemo".to_string()]
        );
    }

    #[test]
    fn empty_launch_forwards_an_activate_deeplink() {
        let startup = StartupArguments::default();
        assert_eq!(
            super::urls_to_forward_for_platform(&startup, super::PlainLaunch::Activate),
            vec!["termy://".to_string()]
        );
    }

    #[test]
    fn secondary_forwards_working_directory_to_primary() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path();
        let InstanceClaim::Primary(guard) =
            claim_or_forward_in(home, &["termy://".to_string()]).expect("primary claim")
        else {
            panic!("first claim should occupy the instance");
        };
        let (tx, rx) = flume::unbounded();
        spawn_listener(guard, tx);

        let forwarded = claim_or_forward_in(home, &["termy://new?dir=%2Ftmp%2Fdemo".to_string()])
            .expect("secondary claim");
        assert!(matches!(forwarded, InstanceClaim::Forwarded));

        let urls = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("primary should receive the forwarded tab");
        assert_eq!(urls, vec!["termy://new?dir=%2Ftmp%2Fdemo".to_string()]);
    }
}
