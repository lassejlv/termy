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
