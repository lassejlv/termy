use serde_json::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Host(tempfile::TempDir);
impl Host {
    fn call(&self, args: &[&str]) -> Output {
        // Files keep capture bounded even if a detached Windows child retains
        // an inherited output handle; pipe EOF would wait for that child too.
        let mut stdout = tempfile::tempfile().unwrap();
        let mut stderr = tempfile::tempfile().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_termy-cli"));
        command
            .args(["mux", "--session-dir"])
            .arg(self.0.path().join("sessions"))
            .args(args)
            .stdin(Stdio::null())
            .stdout(stdout.try_clone().unwrap())
            .stderr(stderr.try_clone().unwrap());
        #[cfg(windows)]
        if args == ["start"] {
            use std::os::windows::process::CommandExt;
            // Reproduce launchers that disable Ctrl+C for their descendants.
            command.creation_flags(0x0000_0200); // CREATE_NEW_PROCESS_GROUP
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                child.kill().unwrap();
                break child.wait().unwrap();
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        stdout.seek(SeekFrom::Start(0)).unwrap();
        stderr.seek(SeekFrom::Start(0)).unwrap();
        let mut output = Output {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        stdout.read_to_end(&mut output.stdout).unwrap();
        stderr.read_to_end(&mut output.stderr).unwrap();
        assert!(
            !timed_out,
            "mux {args:?} timed out: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn ok(&self, args: &[&str]) -> Value {
        let output = self.call(args);
        if !output.status.success() {
            let capture = if args.first() == Some(&"wait") && args.len() > 1 {
                String::from_utf8_lossy(&self.call(&["capture", args[1]]).stdout).into_owned()
            } else {
                String::new()
            };
            panic!(
                "mux {args:?} failed: {}\n{capture}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(envelope["schema_version"], 1);
        assert_eq!(envelope["ok"], true);
        envelope["result"].clone()
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.call(&["shutdown"]);
    }
}

#[test]
fn agent_commands_control_a_persistent_terminal_across_processes() {
    let host = Host(tempfile::tempdir().unwrap());
    let shell = if cfg!(windows) {
        "powershell.exe"
    } else {
        "/bin/sh"
    };
    let invalid = host.call(&["create", "--cols", "0"]);
    assert_eq!(invalid.status.code(), Some(1));
    assert!(!host.0.path().join("sessions/endpoint.json").exists());
    host.ok(&["start"]);
    assert_eq!(host.ok(&["list"]), json!([]));
    let pane = host.ok(&["create", "--shell", shell]);
    let id = pane["id"].as_str().unwrap();
    if cfg!(windows) {
        // PSReadLine initializes asynchronously and can discard input sent
        // before its first prompt. Wait for readiness instead of sleeping.
        host.ok(&["wait", id, "PS ", "--timeout-ms", "15000"]);
    }
    let created_layout = host.ok(&["layout"]);
    assert_eq!(
        created_layout["windows"][0]["session"]["workspaces"][0]["tabs"][0]["panes"][0]["session_id"],
        id
    );
    let pid = host.ok(&["list"])[0]["child_pid"].clone();
    let client =
        termy_core::multiplexer::SessionClient::connect(&host.0.path().join("sessions")).unwrap();
    client.set_layout(json!({"version":1,"windows":[{"id":"window-1","session":{"active_workspace":0,"workspaces":[{
        "name":"Development","pinned":false,"active_tab":0,"tabs":[{"pinned":false,"manual_title":null,"active_pane":0,
        "layout_tree_json":null,"panes":[{"session_id":id,"left":0,"top":0,"width":80,"height":24,"buffer":null}]}]
    }]}}]}).to_string()).unwrap();
    host.ok(&["workspace", "window-1", "0", "rename", "Agent work"]);
    host.ok(&["workspace", "window-1", "0", "pin"]);
    let layout = host.ok(&["layout"]);
    assert_eq!(
        layout["windows"][0]["session"]["workspaces"][0]["name"],
        "Agent work"
    );
    assert_eq!(
        layout["windows"][0]["session"]["workspaces"][0]["pinned"],
        true
    );
    host.ok(&[
        "send",
        id,
        if cfg!(windows) {
            "Write-Output ('CLI_' + 'PROOF')"
        } else {
            "printf 'CLI_%s\\n' PROOF"
        },
        "--enter",
    ]);
    let captured = host.ok(&["wait", id, "CLI_PROOF"]);
    assert!(captured["text"].as_str().unwrap().contains("CLI_PROOF"));
    assert_eq!(host.ok(&["list"])[0]["child_pid"], pid);
    host.ok(&["resize", id, "100", "30"]);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let output = host.ok(&["capture", id]);
        if output["cols"] == 100 && output["rows"] == 30 {
            break;
        }
        assert!(Instant::now() < deadline, "resize was not applied");
        std::thread::sleep(Duration::from_millis(25));
    }
    host.ok(&[
        "send",
        id,
        if cfg!(windows) {
            "Write-Output ('RUNNING_' + 'NOW'); Start-Sleep -Seconds 60"
        } else {
            "printf 'RUNNING_%s\\n' NOW; sleep 60"
        },
        "--enter",
    ]);
    host.ok(&["wait", id, "RUNNING_NOW"]);
    host.ok(&["key", id, "c", "--control"]);
    if cfg!(windows) {
        // PowerShell can discard queued input while cancelling a pipeline.
        // Require a fresh prompt after RUNNING_NOW before the next command.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let output = host.ok(&["capture", id]);
            let text = output["text"].as_str().unwrap();
            if text
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .is_some_and(|line| line.starts_with("PS "))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Ctrl+C did not restore the PowerShell prompt: {text}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    host.ok(&[
        "send",
        id,
        if cfg!(windows) {
            "Write-Output ('INTERRUPTED_' + 'OK')"
        } else {
            "printf 'INTERRUPTED_%s\\n' OK"
        },
        "--enter",
    ]);
    host.ok(&["wait", id, "INTERRUPTED_OK"]);
    let timeout = host.call(&["wait", id, "NO_SUCH_OUTPUT", "--timeout-ms", "50"]);
    assert_eq!(timeout.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&timeout.stderr).unwrap()["ok"],
        false
    );
    host.ok(&["close", id]);
    assert_eq!(host.ok(&["list"]), json!([]));
    let pane = host.ok(&[
        "create",
        "--shell",
        shell,
        "--window",
        "window-1",
        "--workspace",
        "0",
    ]);
    let new_id = pane["id"].as_str().unwrap();
    let layout = host.ok(&["layout"]);
    let tabs = layout["windows"][0]["session"]["workspaces"][0]["tabs"]
        .as_array()
        .unwrap();
    assert_eq!(tabs.last().unwrap()["panes"][0]["session_id"], new_id);
    let invalid = host.call(&["create", "--window", "missing-window"]);
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(host.ok(&["list"]).as_array().unwrap().len(), 1);
    let split = host.ok(&["split", new_id, "--axis", "horizontal"]);
    let split_id = split["id"].as_str().unwrap();
    assert_eq!(host.ok(&["list"]).as_array().unwrap().len(), 2);
    let layout = host.ok(&["layout"]);
    let panes = layout["windows"][0]["session"]["workspaces"][0]["tabs"][0]["panes"]
        .as_array()
        .unwrap();
    assert_eq!(panes.len(), 2);
    assert_eq!(panes[0]["width"], 40);
    assert_eq!(panes[1]["left"], 40);
    host.ok(&["tab", new_id, "rename", "Build logs"]);
    host.ok(&["tab", new_id, "pin"]);
    host.ok(&["tab", new_id, "zoom"]);
    host.ok(&["tab", new_id, "focus", split_id]);
    let layout = host.ok(&["layout"]);
    let tab = &layout["windows"][0]["session"]["workspaces"][0]["tabs"][0];
    assert_eq!(tab["manual_title"], "Build logs");
    assert_eq!(tab["pinned"], true);
    assert_eq!(tab["zoomed"], true);
    assert_eq!(tab["active_pane"], 1);
    host.ok(&["tab", new_id, "unzoom"]);
    host.ok(&["tab", new_id, "resize-divider", "right", "8"]);
    let layout = host.ok(&["layout"]);
    let tab = &layout["windows"][0]["session"]["workspaces"][0]["tabs"][0];
    assert_eq!(tab["zoomed"], false);
    assert_eq!(tab["panes"][0]["width"], 48);
    assert_eq!(tab["panes"][1]["width"], 32);
    let invalid = host.call(&["tab", new_id, "resize-divider", "right", "500"]);
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(host.ok(&["layout"]), layout);
    host.ok(&["tab", new_id, "resize-divider", "right", "-8"]);
    host.ok(&["tab", new_id, "reset-title"]);
    host.ok(&["tab", new_id, "unpin"]);
    host.ok(&["close", split_id]);
    let layout = host.ok(&["layout"]);
    let panes = layout["windows"][0]["session"]["workspaces"][0]["tabs"][0]["panes"]
        .as_array()
        .unwrap();
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0]["width"], 80);
    assert_eq!(panes[0]["session_id"], new_id);
    host.ok(&["close", new_id]);
    host.ok(&["window", "window-1", "create-workspace", "Review"]);
    let layout = host.ok(&["layout"]);
    assert_eq!(layout["windows"][0]["session"]["active_workspace"], 1);
    assert_eq!(
        layout["windows"][0]["session"]["workspaces"][1]["name"],
        "Review"
    );
    let first = host.ok(&["create", "--shell", shell]);
    let second = host.ok(&["create", "--shell", shell]);
    let first = first["id"].as_str().unwrap();
    let second = second["id"].as_str().unwrap();
    host.ok(&["workspace", "window-1", "1", "select-tab", "0"]);
    host.ok(&["workspace", "window-1", "1", "move-tab", "0", "1"]);
    let layout = host.ok(&["layout"]);
    let workspace = &layout["windows"][0]["session"]["workspaces"][1];
    assert_eq!(workspace["active_tab"], 1);
    assert_eq!(workspace["tabs"][1]["panes"][0]["session_id"], first);
    host.ok(&["window", "window-1", "move-workspace", "1", "0"]);
    let layout = host.ok(&["layout"]);
    assert_eq!(layout["windows"][0]["session"]["active_workspace"], 0);
    assert_eq!(
        layout["windows"][0]["session"]["workspaces"][0]["name"],
        "Review"
    );
    let invalid = host.call(&["window", "window-1", "delete-empty-workspace", "0"]);
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(host.ok(&["layout"]), layout);
    host.ok(&["close", first]);
    host.ok(&["close", second]);
    host.ok(&["window", "window-1", "select-workspace", "1"]);
    host.ok(&["window", "window-1", "delete-empty-workspace", "0"]);
    let layout = host.ok(&["layout"]);
    assert_eq!(layout["windows"][0]["session"]["active_workspace"], 0);
    assert_eq!(
        layout["windows"][0]["session"]["workspaces"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn starting_a_host_releases_captured_output_while_host_stays_alive() {
    let host = Host(tempfile::tempdir().unwrap());
    let root = host.0.path().join("sessions");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let output = Command::new(env!("CARGO_BIN_EXE_termy-cli"))
            .args(["mux", "--session-dir"])
            .arg(root)
            .arg("start")
            .output();
        let _ = tx.send(output);
    });
    let output = rx.recv_timeout(Duration::from_secs(15));
    if output.is_err() {
        // Release leaked pipes before failing, rather than leaving the test
        // helper or background host running on the CI machine.
        let _ = host.call(&["shutdown"]);
    }
    let output = output
        .expect("mux start retained its captured output pipes")
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ready: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(ready["ok"], true);
    assert_eq!(host.ok(&["list"]), json!([]));
}

/// HMAC-SHA256 as an rc.8 (protocol 3) host computes its proofs.
fn hmac(key: &[u8], message: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut block = [0u8; 64];
    block[..key.len()].copy_from_slice(key);
    let mut inner = Sha256::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    for part in message {
        inner.update(part);
    }
    let mut outer = Sha256::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// After an upgrade, `mux shutdown` must still stop the host the previous
/// version left running (audit B1). The fake host speaks protocol 3 frames.
#[test]
fn shutdown_stops_a_host_of_an_older_protocol() {
    use std::io::Write;
    let host = Host(tempfile::tempdir().unwrap());
    let root = host.0.path().join("sessions");
    std::fs::create_dir(&root).unwrap();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let token = "rc8-token";
    std::fs::write(
        root.join("endpoint.json"),
        json!({
            "version": 3,
            "port": listener.local_addr().unwrap().port(),
            "token": token,
            "conditional_layout_updates": true,
            "graphics_stream": true,
            "viewport_replies": true,
            "mutual_auth": true,
        })
        .to_string(),
    )
    .unwrap();
    let lock = std::fs::File::create(root.join("host.lock")).unwrap();
    lock.try_lock().unwrap();

    let refused = host.call(&["list"]);
    assert_eq!(refused.status.code(), Some(1));
    let error = String::from_utf8_lossy(&refused.stderr);
    assert!(error.contains("termy mux shutdown"), "{error}");

    fn read_frame(stream: &mut std::net::TcpStream, length: usize) -> Vec<u8> {
        let mut frame = vec![0; length];
        stream.read_exact(&mut frame).unwrap();
        frame
    }
    let fake = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        // Authenticate { version: 3, client_nonce } is variant 14.
        let greeting = read_frame(&mut stream, 44);
        assert_eq!(greeting[..12], [40, 0, 0, 0, 14, 0, 0, 0, 3, 0, 0, 0]);
        let client_nonce = greeting[12..].to_vec();
        let server_nonce = [9u8; 32];
        let host_proof = hmac(
            token.as_bytes(),
            &[
                b"termy-multiplexer host proof v1",
                &client_nonce,
                &server_nonce,
            ],
        );
        // Challenge is variant 8, Ok 0, Proof 15 and Shutdown 10.
        let mut challenge = [68u32.to_le_bytes(), 8u32.to_le_bytes()].concat();
        challenge.extend(server_nonce);
        challenge.extend(host_proof);
        let client_proof = hmac(
            token.as_bytes(),
            &[
                b"termy-multiplexer client proof v1",
                &server_nonce,
                &client_nonce,
            ],
        );
        let ok = [4, 0, 0, 0, 0, 0, 0, 0];
        stream.write_all(&challenge).unwrap();
        let proof = read_frame(&mut stream, 40);
        assert_eq!(proof[..8], [36, 0, 0, 0, 15, 0, 0, 0]);
        assert_eq!(proof[8..], client_proof);
        stream.write_all(&ok).unwrap();
        assert_eq!(read_frame(&mut stream, 8), [4, 0, 0, 0, 10, 0, 0, 0]);
        stream.write_all(&ok).unwrap();
        drop(lock);
    });
    assert_eq!(host.ok(&["shutdown"]), json!({"stopped": true}));
    fake.join().unwrap();
}

/// `shutdown --force` stops the host process by its recorded PID, and later
/// commands report that no host runs instead of using its leftover endpoint.
#[test]
fn forced_shutdown_stops_the_host_process() {
    let host = Host(tempfile::tempdir().unwrap());
    let root = host.0.path().join("sessions");
    host.ok(&["start"]);
    let endpoint: Value =
        serde_json::from_slice(&std::fs::read(root.join("endpoint.json")).unwrap()).unwrap();
    assert!(
        endpoint["pid"].as_u64().is_some_and(|pid| pid > 0),
        "{endpoint}"
    );
    host.ok(&["create"]);

    assert_eq!(host.ok(&["shutdown", "--force"]), json!({"stopped": true}));
    assert!(!root.join("endpoint.json").exists());
    for args in [&["list"][..], &["shutdown"], &["shutdown", "--force"]] {
        let output = host.call(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("no session host is running"),
            "{args:?}: {error}"
        );
    }
}
