use crate::StartupArguments;
use crate::deeplink::new_tab_deeplink_for_dir;
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const PROTOCOL_PREFIX: &str = "TERMY1 ";
const CONNECT_ATTEMPTS: usize = 40;
const CONNECT_WAIT: Duration = Duration::from_millis(100);

#[derive(Debug, Serialize, Deserialize)]
struct Endpoint {
    port: u16,
}

pub(crate) enum InstanceClaim {
    Primary(InstanceGuard),
    Forwarded,
}

pub(crate) struct InstanceGuard {
    listener: TcpListener,
    home: PathBuf,
    _lock: File,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.home.join("instance.json"));
        // Keep the lock file: other launchers may already have it open. The OS
        // releases our lock when the handle closes, including on process exit.
    }
}

pub(crate) fn urls_to_forward(startup: &StartupArguments) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(dir) = &startup.working_dir {
        urls.push(new_tab_deeplink_for_dir(dir));
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
        let owns_lock = lock.try_lock_exclusive()?;
        // Also check once after acquiring the lock so launches still forward to
        // v0.2.61, which published an endpoint without holding an OS file lock.
        if forward_to_existing(home, urls)? {
            return Ok(InstanceClaim::Forwarded);
        }
        if owns_lock {
            return occupy_primary(home.to_path_buf(), lock);
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
    let payload = serde_json::to_vec(&Endpoint { port }).map_err(io::Error::other)?;
    fs::write(home.join("instance.json"), payload)?;
    Ok(InstanceClaim::Primary(InstanceGuard {
        listener,
        home,
        _lock: lock,
    }))
}

fn listen_for_handoffs(guard: InstanceGuard, tx: flume::Sender<Vec<String>>) {
    for stream in guard.listener.incoming() {
        match stream.and_then(read_forwarded_urls) {
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
    if let Some(port) = read_port(home) {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        if let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(150)) {
            write_forwarded_urls(&mut stream, urls)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_port(home: &Path) -> Option<u16> {
    let payload = fs::read(home.join("instance.json")).ok()?;
    serde_json::from_slice::<Endpoint>(&payload)
        .ok()
        .map(|endpoint| endpoint.port)
}

fn write_forwarded_urls(stream: &mut TcpStream, urls: &[String]) -> io::Result<()> {
    stream.write_all(encode_forward(urls).as_bytes())?;
    stream.flush()?;
    stream.shutdown(Shutdown::Write)
}

fn read_forwarded_urls(mut stream: TcpStream) -> io::Result<Vec<String>> {
    let mut buffer = String::new();
    stream.read_to_string(&mut buffer)?;
    decode_forward(&buffer)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid instance payload"))
}

pub(crate) fn encode_forward(urls: &[String]) -> String {
    let mut payload = String::from(PROTOCOL_PREFIX);
    payload.push_str(&urls.join("\n"));
    payload.push('\n');
    payload
}

pub(crate) fn decode_forward(payload: &str) -> Option<Vec<String>> {
    let body = payload.strip_prefix(PROTOCOL_PREFIX)?;
    let urls = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    Some(urls)
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
    fn secondary_forwards_to_legacy_primary_without_os_lock() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path();
        fs::write(home.join("instance.lock"), "").expect("legacy marker");
        let listener = TcpListener::bind("127.0.0.1:0").expect("legacy listener");
        let port = listener.local_addr().expect("listener address").port();
        fs::write(home.join("instance.json"), format!("{{\"port\":{port}}}"))
            .expect("legacy endpoint");

        let urls = vec!["termy://new?dir=%2Ftmp%2Fdemo".to_string()];
        let claim = claim_or_forward_in(home, &urls).expect("secondary claim");
        assert!(matches!(claim, InstanceClaim::Forwarded));
        let (stream, _) = listener.accept().expect("forwarded connection");
        assert_eq!(
            super::read_forwarded_urls(stream).expect("forwarded URLs"),
            urls
        );
        assert_eq!(super::read_port(home), Some(port));
    }

    #[test]
    fn encodes_and_decodes_forwarded_urls() {
        let encoded = encode_forward(&["termy://".to_string(), "termy://new".to_string()]);
        assert!(encoded.starts_with("TERMY1 "));
        assert_eq!(
            decode_forward(&encoded),
            Some(vec!["termy://".to_string(), "termy://new".to_string()])
        );
    }

    #[test]
    fn working_directory_forwards_as_new_tab_deeplink() {
        let startup = StartupArguments {
            working_dir: Some("/tmp/demo".to_string()),
            deeplinks: Vec::new(),
        };
        assert_eq!(
            urls_to_forward(&startup),
            vec!["termy://new?dir=%2Ftmp%2Fdemo".to_string()]
        );
    }

    #[test]
    fn empty_launch_forwards_an_activate_deeplink() {
        let startup = StartupArguments::default();
        assert_eq!(urls_to_forward(&startup), vec!["termy://".to_string()]);
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
