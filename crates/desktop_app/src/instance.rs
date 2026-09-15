use crate::StartupArguments;
use crate::deeplink::new_tab_deeplink_for_dir;
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
        let _ = fs::remove_file(self.home.join("instance.lock"));
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

    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(lock) => occupy_primary(home.to_path_buf(), lock),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if forward_to_existing(home, urls)? {
                Ok(InstanceClaim::Forwarded)
            } else {
                let _ = fs::remove_file(&lock_path);
                let _ = fs::remove_file(home.join("instance.json"));
                match fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&lock_path)
                {
                    Ok(lock) => occupy_primary(home.to_path_buf(), lock),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        if forward_to_existing(home, urls)? {
                            Ok(InstanceClaim::Forwarded)
                        } else {
                            Err(io::Error::other(
                                "another Termy instance is starting; try again",
                            ))
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        }
        Err(error) => Err(error),
    }
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
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => {
            drop(lock);
            let _ = fs::remove_file(home.join("instance.lock"));
            return Err(error);
        }
    };
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(error) => {
            drop(lock);
            let _ = fs::remove_file(home.join("instance.lock"));
            return Err(error);
        }
    };
    let payload = match serde_json::to_vec(&Endpoint { port }) {
        Ok(payload) => payload,
        Err(error) => {
            drop(lock);
            let _ = fs::remove_file(home.join("instance.lock"));
            return Err(io::Error::other(error.to_string()));
        }
    };
    if let Err(error) = fs::write(home.join("instance.json"), payload) {
        drop(lock);
        let _ = fs::remove_file(home.join("instance.lock"));
        return Err(error);
    }
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
    for _ in 0..CONNECT_ATTEMPTS {
        if let Some(port) = read_port(home) {
            let address = SocketAddr::from(([127, 0, 0, 1], port));
            match TcpStream::connect_timeout(&address, Duration::from_millis(150)) {
                Ok(mut stream) => {
                    write_forwarded_urls(&mut stream, urls)?;
                    return Ok(true);
                }
                Err(_) => std::thread::sleep(CONNECT_WAIT),
            }
        } else {
            std::thread::sleep(CONNECT_WAIT);
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
    let base = dirs::runtime_dir()
        .or_else(dirs::data_local_dir)
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
    use std::time::Duration;

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
