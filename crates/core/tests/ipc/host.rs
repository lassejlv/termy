//! Host start-up, authentication, shutdown and recovery.

use super::*;

#[test]
fn authentication_and_pre_authentication_message_limit() {
    use std::os::unix::fs::PermissionsExt;
    let host = Host::new();
    let endpoint = host.root.join("endpoint.json");
    assert_eq!(
        fs::metadata(&endpoint).unwrap().permissions().mode() & 0o077,
        0
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(endpoint).unwrap()).unwrap();
    let port = value["port"].as_u64().unwrap() as u16;
    #[derive(serde::Serialize)]
    enum Greeting {
        Hello { version: u32, token: String },
    }
    let bad_greeting = bincode::serialize(&Greeting::Hello {
        version: value["version"].as_u64().unwrap() as u32,
        token: "wrong token".into(),
    })
    .unwrap();
    for bytes in [
        [
            (bad_greeting.len() as u32).to_le_bytes().as_slice(),
            &bad_greeting,
        ]
        .concat(),
        4097u32.to_le_bytes().to_vec(),
    ] {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        stream.write_all(&bytes).unwrap();
        match stream.read(&mut [0; 1]) {
            Ok(0) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            result => panic!("unauthenticated client was not disconnected: {result:?}"),
        }
    }
    assert!(host.client.list().unwrap().is_empty());
}

#[test]
fn host_records_its_build_and_logs_to_its_private_directory() {
    use std::os::unix::fs::PermissionsExt;
    let host = Host::new();
    let endpoint: serde_json::Value =
        serde_json::from_slice(&fs::read(host.root.join("endpoint.json")).unwrap()).unwrap();
    assert_eq!(endpoint["mutual_auth"], true);
    assert!(
        endpoint["build"]
            .as_str()
            .is_some_and(|build| build.contains("wire")),
        "{endpoint}"
    );
    let log = host.root.join("host.log");
    assert_eq!(
        fs::metadata(&log).unwrap().permissions().mode() & 0o777,
        0o600
    );
    wait_until(|| fs::read_to_string(&log).is_ok_and(|text| text.contains("session host started")));
}

#[test]
fn shutdown_host_stops_the_running_host() {
    let host = Host::new();
    termy_core::multiplexer::shutdown_host(&host.root).unwrap();
    assert!(!host.root.join("endpoint.json").exists());
    assert!(SessionClient::connect(&host.root).is_err());
    assert!(termy_core::multiplexer::shutdown_host(&host.root).is_err());
}

/// Audit I9: a host that fails to start explains why.
#[test]
fn failed_host_start_reports_its_output() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("host");
    let executable = temp.path().join("broken-host");
    fs::write(
        &executable,
        "#!/bin/sh\necho \"cannot open the session directory: $2\" >&2\nexit 3\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    // Another test's fork can briefly hold the script open for writing.
    let message = (0..20)
        .map(|attempt| {
            std::thread::sleep(Duration::from_millis(50) * attempt);
            match connect_or_start(&root, &executable) {
                Ok(_) => panic!("the broken host must not start"),
                Err(error) => format!("{error:#}"),
            }
        })
        .find(|message| !message.contains("Text file busy"))
        .unwrap();
    assert!(message.contains("exited"), "{message}");
    assert!(message.contains("host.log"), "{message}");
    assert!(
        message.contains("cannot open the session directory"),
        "{message}"
    );
}

fn host_executable() -> std::path::PathBuf {
    std::env::var_os("TERMY_MUX_TEST_HOST_BINARY").map_or_else(
        || Path::new(env!("CARGO_BIN_EXE_termy-session-host")).to_owned(),
        std::path::PathBuf::from,
    )
}

/// A pane that runs `script` with its output file as `$1`.
fn script_launch(
    script: &str,
    output: &Path,
    environment: Option<Vec<(String, String)>>,
) -> PaneLaunch {
    PaneLaunch {
        size: TerminalSize::default(),
        working_directory: None,
        shell_integration: None,
        config: TerminalRuntimeConfig {
            // The pane starts in the host's own directory.
            working_dir_fallback: WorkingDirFallback::Process,
            ..TerminalRuntimeConfig::default()
        },
        launch: Some(TerminalLaunch::Program {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                script.into(),
                "pane-test".into(),
                output.to_string_lossy().into_owned(),
            ],
        }),
        environment,
    }
}

/// Reads a file that a pane script writes with `mv`, once it is complete.
#[track_caller]
fn read_when_written(path: &Path) -> String {
    wait_until(|| path.exists());
    fs::read_to_string(path).unwrap()
}

/// Whether `pid` runs; a zombie that nobody reaps (as in some containers)
/// counts as ended.
fn process_running(pid: u32) -> bool {
    if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
        return stat
            .rsplit_once(')')
            .is_some_and(|(_, rest)| !rest.trim_start().starts_with('Z'));
    }
    // SAFETY: signal 0 only probes the process; it takes no pointers.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

// Started as a separate process by `panes_start_from_their_clients_environment`,
// like `termy mux create` run from a shell with variables of its own.
#[test]
fn launcher_process() {
    let Ok(root) = std::env::var("TERMY_MUX_TEST_LAUNCH_ROOT") else {
        return;
    };
    connect_or_start(Path::new(&root), &host_executable()).unwrap();
}

/// Audit B28 and I16: a pane gets the environment of the client that
/// creates it, not that of whoever started the host, and learns its host and
/// its pane ID.
#[test]
fn panes_start_from_their_clients_environment() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("host");
    let launcher_directory = temp.path().join("launcher");
    fs::create_dir(&launcher_directory).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "host::launcher_process", "--nocapture"])
        .env("TERMY_MUX_TEST_LAUNCH_ROOT", &root)
        .env("MUX_LAUNCHER_SECRET", "launcher-token")
        .current_dir(&launcher_directory)
        .status()
        .unwrap();
    assert!(status.success());
    let client = SessionClient::connect(&root).unwrap();
    struct Shutdown<'a>(&'a SessionClient);
    impl Drop for Shutdown<'_> {
        fn drop(&mut self) {
            let _ = self.0.shutdown();
        }
    }
    let _shutdown = Shutdown(&client);
    let script = r#"printf '%s|%s|%s|%s|%s' "${MUX_LAUNCHER_SECRET-unset}" "${MUX_CLIENT_ONLY-unset}" "$TERMY_MUX_DIR" "$TERMY_MUX_PANE" "$PWD" > "$1.tmp"; mv "$1.tmp" "$1"; exec sleep 30"#;
    let fields = |output: &Path| -> Vec<String> {
        read_when_written(output)
            .split('|')
            .map(str::to_owned)
            .collect()
    };

    // Without an environment of its own, a pane gets the host's.
    let output = temp.path().join("host-environment");
    let (id, _host_pane) = client
        .create(script_launch(script, &output, None), None)
        .unwrap();
    let host_fields = fields(&output);
    assert_eq!(
        host_fields[..4],
        ["unset", "unset", root.to_str().unwrap(), id.as_str()]
    );
    assert_ne!(
        Path::new(&host_fields[4]),
        launcher_directory,
        "the pane runs in the launcher's directory"
    );

    let output = temp.path().join("client-environment");
    let environment = vec![
        ("MUX_CLIENT_ONLY".to_string(), "client-value".to_string()),
        ("TERMY_MUX_PANE".to_string(), "stale-outer-pane".to_string()),
    ];
    let (id, _client_pane) = client
        .create(script_launch(script, &output, Some(environment)), None)
        .unwrap();
    assert_eq!(
        fields(&output)[..4],
        ["unset", "client-value", root.to_str().unwrap(), id.as_str()]
    );
}

/// Audit L1: shutdown must not leave behind a program that ignores SIGHUP,
/// which closing its pane would have killed.
#[test]
fn shutdown_ends_pane_programs_that_ignore_hangup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("host");
    let client = connect_or_start(&root, &host_executable()).unwrap();
    let output = temp.path().join("sleeper");
    let script = r#"trap '' HUP; sleep 1000 & printf '%s' "$!" > "$1.tmp"; mv "$1.tmp" "$1"; wait"#;
    let (_, terminal) = client
        .create(script_launch(script, &output, None), None)
        .unwrap();
    let sleeper: u32 = read_when_written(&output).parse().unwrap();
    let shell = terminal.child_pid().unwrap();
    assert!(process_running(sleeper) && process_running(shell));
    drop(terminal);
    termy_core::multiplexer::shutdown_host(&root).unwrap();
    for pid in [shell, sleeper] {
        wait_until(|| !process_running(pid));
    }
}

/// Audit I17: removing the session directory under a running host must not
/// orphan its sessions or let a second host start beside it.
#[test]
fn host_restores_its_files_after_its_directory_is_removed() {
    use std::os::unix::io::AsRawFd;
    let host = Host::new();
    let output = host._temp.path().join("ready");
    let (id, _terminal) = host
        .client
        .create(
            script_launch(r#": > "$1"; exec sleep 30"#, &output, None),
            None,
        )
        .unwrap();
    read_when_written(&output);
    fs::remove_dir_all(&host.root).unwrap();
    wait_until(|| SessionClient::connect(&host.root).is_ok());
    let client = SessionClient::connect(&host.root).unwrap();
    assert!(client.list().unwrap().iter().any(|pane| pane.id == id));
    let lock = fs::File::open(host.root.join("host.lock")).unwrap();
    // SAFETY: the descriptor is open; LOCK_NB returns at once.
    let locked = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(locked, -1, "the host must hold the restored host.lock");
}

fn is_ended_title(event: &TerminalEvent) -> bool {
    matches!(event, TerminalEvent::Title(title) if title.starts_with("Session ended"))
}

/// Audit B12: when the host dies, its panes end in their views, not as an
/// exit of their programs (so a window keeps its tabs), and a client that
/// can start hosts gets a new one.
#[test]
fn a_dead_hosts_panes_end_and_the_client_starts_a_new_host() {
    let host = Host::new();
    let (id, terminal) = shell_pane(&host, "read line");
    termy_core::multiplexer::kill_host(&host.root).unwrap();
    let events = events_until(&terminal, is_ended_title);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, TerminalEvent::Exit))
    );
    assert!(terminal.session_host_lost());
    assert!(host.client.list().is_err());
    host.client.reconnect().unwrap();
    assert!(host.client.list().unwrap().is_empty());
    let error = host.client.attach(&id, None).err().unwrap();
    assert!(!termy_core::multiplexer::is_connection_error(&error));
}

/// Review finding 2: `termy mux shutdown` ends the panes in their views as
/// a lost host, not as exits of their programs.
#[test]
fn a_host_shutdown_ends_views_as_a_lost_host() {
    let host = Host::new();
    let (_id, terminal) = shell_pane(&host, "read line");
    host.client.shutdown().unwrap();
    let events = events_until(&terminal, is_ended_title);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, TerminalEvent::Exit))
    );
    assert!(terminal.session_host_lost());
}
