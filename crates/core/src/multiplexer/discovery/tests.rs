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
