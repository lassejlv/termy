use serde_json::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

/// A host in a temporary session directory, with a config directory of its
/// own so the user's Termy config does not apply.
struct Host(tempfile::TempDir);

/// How to run one command beyond its arguments.
#[derive(Default)]
struct Run<'a> {
    stdin: Option<&'a [u8]>,
    directory: Option<&'a Path>,
}

impl Host {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }

    fn sessions(&self) -> PathBuf {
        self.0.path().join("sessions")
    }

    fn config_home(&self) -> PathBuf {
        self.0.path().join("config")
    }

    #[cfg(unix)]
    fn write_config(&self, contents: &str) {
        let directory = self.config_home().join("termy");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("config.txt"), contents).unwrap();
    }

    fn call(&self, args: &[&str]) -> Output {
        self.call_with(args, Run::default())
    }

    fn call_with(&self, args: &[&str], run: Run) -> Output {
        // Files keep capture bounded even if a detached Windows child retains
        // an inherited output handle; pipe EOF would wait for that child too.
        let mut stdout = tempfile::tempfile().unwrap();
        let mut stderr = tempfile::tempfile().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_termy-cli"));
        command
            .args(["mux", "--session-dir"])
            .arg(self.sessions())
            .args(args)
            .env("XDG_CONFIG_HOME", self.config_home())
            // Where Windows looks first.
            .env("APPDATA", self.config_home())
            .stdout(stdout.try_clone().unwrap())
            .stderr(stderr.try_clone().unwrap());
        match run.stdin {
            Some(bytes) => {
                let mut input = tempfile::tempfile().unwrap();
                input.write_all(bytes).unwrap();
                input.seek(SeekFrom::Start(0)).unwrap();
                command.stdin(input);
            }
            None => {
                command.stdin(Stdio::null());
            }
        }
        if let Some(directory) = run.directory {
            command.current_dir(directory);
        }
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
        self.ok_with(args, Run::default())
    }

    fn ok_with(&self, args: &[&str], run: Run) -> Value {
        let output = self.call_with(args, run);
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
        assert_eq!(envelope["schema_version"], 2);
        assert_eq!(envelope["ok"], true);
        envelope["result"].clone()
    }

    /// Runs a command that must fail: its exit status and `error`.
    fn error(&self, args: &[&str]) -> (i32, Value) {
        self.error_with(args, Run::default())
    }

    fn error_with(&self, args: &[&str], run: Run) -> (i32, Value) {
        let output = self.call_with(args, run);
        assert!(!output.status.success(), "mux {args:?} succeeded");
        let envelope: Value = serde_json::from_slice(&output.stderr).unwrap_or_else(|error| {
            panic!(
                "mux {args:?} did not report JSON ({error}): {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(envelope["schema_version"], 2);
        assert_eq!(envelope["ok"], false);
        assert!(envelope["error"]["message"].is_string(), "{envelope}");
        (output.status.code().unwrap(), envelope["error"].clone())
    }

    /// Asserts that `args` fails with `code` and the exit status that goes
    /// with it.
    fn fails(&self, args: &[&str], code: &str) -> Value {
        let (status, error) = self.error(args);
        assert_eq!(error["code"], code, "mux {args:?}: {error}");
        assert_eq!(
            status,
            if code == "invalid_argument" { 2 } else { 1 },
            "mux {args:?}: {error}"
        );
        error
    }

    /// A new pane running the test shell, ready for input.
    fn shell(&self) -> String {
        let id = self.ok(&["create", "--shell", SHELL])["id"]
            .as_str()
            .unwrap()
            .to_owned();
        if cfg!(windows) {
            // PSReadLine initializes asynchronously and can discard input sent
            // before its first prompt. Wait for readiness instead of sleeping.
            self.ok(&["wait", &id, "PS ", "--timeout-ms", "15000"]);
        }
        id
    }

    /// Runs `command` in pane `id` and waits until it printed `marker`.
    fn run(&self, id: &str, command: &str, marker: &str) -> Value {
        self.ok(&["send", id, command, "--enter"]);
        self.ok(&["wait", id, marker, "--timeout-ms", "10000"])
    }

    fn client(&self) -> termy_core::multiplexer::SessionClient {
        termy_core::multiplexer::SessionClient::connect(&self.sessions()).unwrap()
    }

    /// Saves a layout with one window, `window-1`, whose workspaces hold the
    /// given panes, one tab each.
    fn set_layout(&self, workspaces: &[&[&str]], active_workspace: usize) {
        let workspaces: Vec<_> = workspaces
            .iter()
            .enumerate()
            .map(|(index, panes)| {
                let tabs: Vec<_> = panes
                    .iter()
                    .map(|id| {
                        json!({"pinned":false,"manual_title":null,"active_pane":0,"layout_tree_json":null,
                            "panes":[{"session_id":id,"left":0,"top":0,"width":80,"height":24,"buffer":null}]})
                    })
                    .collect();
                json!({"name":format!("Workspace {index}"),"pinned":false,"active_tab":0,"tabs":tabs})
            })
            .collect();
        self.client()
            .set_layout(
                json!({"version":1,"windows":[{"id":"window-1","session":{
                    "active_workspace":active_workspace,"workspaces":workspaces}}]})
                .to_string(),
            )
            .unwrap();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.call(&["shutdown"]);
    }
}

const SHELL: &str = if cfg!(windows) {
    "powershell.exe"
} else {
    "/bin/sh"
};

/// A command that prints `<left>_<right>` without that text appearing in
/// its echo, so `wait` matches the output and not the typed command (the
/// split-marker idiom).
fn print_marker(left: &str, right: &str) -> String {
    if cfg!(windows) {
        format!("Write-Output ('{left}_' + '{right}')")
    } else {
        format!("printf '{left}_%s\\n' {right}")
    }
}

/// Audit L13: every argument error is JSON with code `invalid_argument` and
/// exit status 2, and none starts a host.
#[test]
fn invalid_arguments_exit_2_with_a_json_error_and_start_nothing() {
    let host = Host::new();
    for args in [
        &["create", "--cols", "0"][..],
        &["create", "--cols", "70000"],
        &["create", "--rows", "201"],
        &["create", "--working-directory", "no-such-directory"],
        &["resize", "pane", "0", "10"],
        &["wait", "pane", ""],
        &["wait", "pane", "text", "--timeout-ms", "0"],
        &["wait", "pane"],
        &["send", "pane"],
        &["send", "pane", "x", "--timeout-ms", "10"],
        &["wait", "pane", "x", "--after", "1", "--start", "-5"],
        &["no-such-command"],
        &[],
        &["tab", "pane"],
    ] {
        host.fails(args, "invalid_argument");
    }
    assert!(!host.sessions().join("endpoint.json").exists());
    // Help is not an error.
    let help = host.call(&["send", "--help"]);
    assert!(help.status.success());
}

/// Audit I14: `status` reports whether a host runs without starting one;
/// other commands report that none runs.
#[test]
fn status_reports_the_host_without_starting_it() {
    let host = Host::new();
    let status = host.ok(&["status"]);
    assert_eq!(status["running"], false);
    assert!(!host.sessions().join("endpoint.json").exists());
    host.fails(&["list"], "host_unavailable");
    host.ok(&["start"]);
    let status = host.ok(&["status"]);
    assert_eq!(status["running"], true);
    assert_eq!(status["compatible"], true);
    assert_eq!(status["panes"], 0);
    assert_eq!(host.ok(&["list"]), json!([]));
}

#[test]
fn send_and_wait_drive_a_shell_across_processes() {
    let host = Host::new();
    let id = host.shell();
    let pid = host.ok(&["list"])[0]["child_pid"].clone();
    let found = host.run(&id, &print_marker("CLI", "PROOF"), "CLI_PROOF");
    assert_eq!(found["text"], "CLI_PROOF");
    assert!(found["line"].is_i64(), "{found}");
    assert_eq!(host.ok(&["list"])[0]["child_pid"], pid);
    let captured = host.ok(&["capture", &id]);
    assert!(captured["text"].as_str().unwrap().contains("CLI_PROOF"));
    assert_eq!(captured["cols"], 80);
    assert_eq!(captured["rows"], 24);
    host.fails(
        &["wait", &id, "NO_SUCH_OUTPUT", "--timeout-ms", "50"],
        "timeout",
    );
}

/// Audit L13: text that starts with `-` is text, not an option.
#[test]
fn text_arguments_may_start_with_a_hyphen() {
    let host = Host::new();
    let id = host.shell();
    let sent = host.ok(&["send", &id, "-n", "--enter"]);
    assert_eq!(sent["bytes"], 3);
    host.fails(
        &["wait", &id, "--no-such-text", "--timeout-ms", "50"],
        "timeout",
    );
}

/// Audit L12: `--stdin` sends bytes, which need not be UTF-8, and checks
/// the size limit before anything else.
#[test]
fn stdin_input_is_bytes_and_bounded() {
    let host = Host::new();
    let id = host.shell();
    let sent = host.ok_with(
        &["send", &id, "--stdin"],
        Run {
            stdin: Some(b"\xff\xfe"),
            ..Run::default()
        },
    );
    assert_eq!(sent["bytes"], 2);
    let oversized = "界".repeat(400_000);
    let (status, error) = host.error_with(
        &["send", &id, "--stdin"],
        Run {
            stdin: Some(oversized.as_bytes()),
            ..Run::default()
        },
    );
    assert_eq!(status, 2);
    assert_eq!(error["code"], "invalid_argument");
    assert!(
        error["message"].as_str().unwrap().contains("1 MiB"),
        "{error}"
    );
}

/// Audit B25 and I14: several keys go out in one write, in order.
#[cfg(unix)]
#[test]
fn keys_are_sent_in_order_and_interrupt() {
    let host = Host::new();
    let id = host.shell();
    host.ok(&["send", &id, "printf 'KEY_%s\\n'"]);
    let sent = host.ok(&["key", &id, "space", "O", "K", "Enter"]);
    assert_eq!(sent["keys"], 4);
    host.ok(&["wait", &id, "KEY_OK"]);
    host.run(&id, "printf 'RUNNING_%s\\n' NOW; sleep 60", "RUNNING_NOW");
    host.ok(&["key", &id, "c", "--control"]);
    host.run(&id, &print_marker("INTERRUPTED", "OK"), "INTERRUPTED_OK");
    host.fails(&["key", &id, "hyper+x"], "invalid_argument");
}

#[cfg(windows)]
#[test]
fn ctrl_c_interrupts_powershell() {
    let host = Host::new();
    let id = host.shell();
    host.run(
        &id,
        "Write-Output ('RUNNING_' + 'NOW'); Start-Sleep -Seconds 60",
        "RUNNING_NOW",
    );
    host.ok(&["key", &id, "ctrl+c"]);
    // PowerShell can discard queued input while cancelling a pipeline.
    // Require a fresh prompt after RUNNING_NOW before the next command.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = host.ok(&["capture", &id]);
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
    host.run(&id, &print_marker("INTERRUPTED", "OK"), "INTERRUPTED_OK");
}

/// Audit B18: `capture` and `wait` read the host's lines, not a view's
/// viewport, and join soft-wrapped rows.
#[cfg(unix)]
#[test]
fn capture_and_wait_ignore_a_scrolled_view_and_join_wrapped_rows() {
    let host = Host::new();
    let id = host.ok(&["create", "--shell", SHELL, "--cols", "20"])["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // 31 columns of output in a 20-column pane.
    let marker = "LONG_0123456789abcdefghijklmnop";
    let found = host.run(
        &id,
        "printf 'LONG_%s\\n' 0123456789abcdefghijklmnop",
        marker,
    );
    assert_eq!(found["text"], marker);
    host.run(&id, "seq 1 100; printf 'SEQ_%s\\n' DONE", "SEQ_DONE");
    // Scroll a view up, as a user reading history in the desktop would.
    let client = host.client();
    let view = client.attach(&id, None).unwrap();
    view.scroll_display(30);
    assert!(client.snapshot(&id).unwrap().render.metadata.display_offset > 0);
    let screen = host.ok(&["capture", &id]);
    let text = screen["text"].as_str().unwrap();
    assert!(text.lines().any(|line| line == "100"), "{text}");
    assert!(!text.contains(marker), "{text}");
    assert_eq!(screen["start"], 0);
    assert_eq!(screen["end"], 23);
    let history = host.ok(&["capture", &id, "--start", "-200", "--end", "-1"]);
    let history_text = history["text"].as_str().unwrap();
    assert!(
        history_text.lines().any(|line| line == marker),
        "{history_text}"
    );
    assert!(history["start"].as_i64().unwrap() < 0);
    // New output is still found while the view stays scrolled.
    host.run(&id, &print_marker("AFTER", "SCROLL"), "AFTER_SCROLL");
    drop(view);
}

/// Audit B18: `wait --after` searches the output from the line its input was
/// typed on, also what scrolled away before the wait began, and `send
/// --wait` does both in one step. A plain `wait` searches from the screen,
/// so older history only with `--start`.
#[cfg(unix)]
#[test]
fn waits_after_an_anchor_find_output_that_scrolled_away() {
    let host = Host::new();
    let id = host.shell();
    // One write: the marker and 300 lines after it arrive together.
    let sent = host.ok(&[
        "send",
        &id,
        "printf 'SCROLLED_%s\\n' AWAY $(seq 1 300); printf 'DONE_%s\\n' NOW",
        "--enter",
        "--wait",
        "DONE_NOW",
    ]);
    assert_eq!(sent["wait"]["text"], "DONE_NOW");
    let anchor = sent["anchor"].as_u64().unwrap().to_string();
    let found = host.ok(&["wait", &id, "SCROLLED_AWAY", "--after", &anchor]);
    assert_eq!(found["text"], "SCROLLED_AWAY");
    assert_eq!(found["truncated"], false);
    assert!(found["line"].as_i64().unwrap() < 0, "{found}");
    host.fails(
        &["wait", &id, "SCROLLED_AWAY", "--timeout-ms", "300"],
        "timeout",
    );
    host.ok(&["wait", &id, "SCROLLED_AWAY", "--start", "-1000"]);
}

/// Audit B18: a plain `wait` matches a marker still on screen from an earlier
/// round; waiting after the input's anchor does not.
#[cfg(unix)]
#[test]
fn waits_after_an_anchor_ignore_stale_markers() {
    let host = Host::new();
    let id = host.shell();
    host.run(&id, &print_marker("ROUND", "DONE"), "ROUND_DONE");
    let sent = host.ok(&["send", &id, "sleep 30", "--enter"]);
    let anchor = sent["anchor"].as_u64().unwrap().to_string();
    // The earlier marker is still on screen.
    host.ok(&["wait", &id, "ROUND_DONE", "--timeout-ms", "300"]);
    host.fails(
        &[
            "wait",
            &id,
            "ROUND_DONE",
            "--after",
            &anchor,
            "--timeout-ms",
            "300",
        ],
        "timeout",
    );
}

/// Audit L13: `--` passes text that looks like an option.
#[test]
fn text_after_a_double_dash_is_literal() {
    let host = Host::new();
    let id = host.shell();
    assert_eq!(host.ok(&["send", &id, "--", "-h"])["bytes"], 2);
}

/// Audit L10: a pane that exited and was closed, as a desktop window closes
/// it, reports `exited` rather than `not_found`.
#[cfg(unix)]
#[test]
fn panes_closed_after_their_exit_report_exited() {
    let host = Host::new();
    let id = host.shell();
    host.ok(&["send", &id, "exit", "--enter"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while host.ok(&["list"])[0]["exited"] != true {
        assert!(Instant::now() < deadline, "the shell never exited");
        std::thread::sleep(Duration::from_millis(20));
    }
    host.client().close(&id).unwrap();
    host.fails(&["send", &id, "x"], "exited");
    host.fails(&["wait", &id, "x"], "exited");
    host.fails(&["wait", &id, "--command-done"], "exited");
    host.fails(&["capture", &id], "exited");
    let other = host.shell();
    host.client().close(&other).unwrap();
    host.fails(&["send", &other, "x"], "not_found");
}

/// Audits B24 and L10: input for an exited pane fails, and `wait` fails at
/// once when its pane exits or closes, each with its own code.
#[cfg(unix)]
#[test]
fn exited_and_closed_panes_fail_fast_with_their_codes() {
    let host = Host::new();
    let id = host.shell();
    host.ok(&["send", &id, "exit", "--enter"]);
    let started = Instant::now();
    host.fails(&["wait", &id, "NEVER", "--timeout-ms", "60000"], "exited");
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(host.ok(&["list"])[0]["exited"], true);
    host.fails(&["send", &id, "echo"], "exited");
    host.fails(&["key", &id, "enter"], "exited");
    host.fails(&["wait", &id, "--command-done"], "exited");

    let other = host.shell();
    let waiting = {
        let sessions = host.sessions();
        let other = other.clone();
        std::thread::spawn(move || {
            Command::new(env!("CARGO_BIN_EXE_termy-cli"))
                .args(["mux", "--session-dir"])
                .arg(sessions)
                .args(["wait", &other, "NEVER", "--timeout-ms", "60000"])
                .stdin(Stdio::null())
                .output()
                .unwrap()
        })
    };
    std::thread::sleep(Duration::from_millis(500));
    host.ok(&["close", &other]);
    let output = waiting.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "not_found", "{error}");
    for args in [
        &["send", other.as_str(), "x"][..],
        &["capture", &other],
        &["resize", &other, "10", "10"],
        &["close", &other],
        &["close", "no-such-pane"],
    ] {
        host.fails(args, "not_found");
    }
}

/// A `wait` running when the host shuts down fails at once as
/// `host_unavailable`, not as a missing pane or a timeout.
#[test]
fn a_wait_ends_with_host_unavailable_when_the_host_shuts_down() {
    let host = Host::new();
    let id = host.shell();
    let waiting = {
        let sessions = host.sessions();
        std::thread::spawn(move || {
            Command::new(env!("CARGO_BIN_EXE_termy-cli"))
                .args(["mux", "--session-dir"])
                .arg(sessions)
                .args(["wait", &id, "NEVER", "--timeout-ms", "60000"])
                .stdin(Stdio::null())
                .output()
                .unwrap()
        })
    };
    std::thread::sleep(Duration::from_millis(500));
    let started = Instant::now();
    assert_eq!(host.ok(&["shutdown"]), json!({"stopped": true}));
    let output = waiting.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "host_unavailable", "{error}");
}

/// Audit I14: `wait --command-done` reports the exit code of the command
/// that the latest input started, from OSC 133 reports, which `list
/// --verbose` shows too.
#[cfg(unix)]
#[test]
fn wait_for_a_command_reports_its_exit_code() {
    let host = Host::new();
    let id = host.shell();
    let verbose = host.ok(&["list", "--verbose"]);
    assert_eq!(verbose[0]["cols"], 80);
    assert_eq!(verbose[0]["rows"], 24);
    assert_eq!(verbose[0]["shell"], Value::Null);
    // What shell integration prints around a command that fails.
    host.ok(&[
        "send",
        &id,
        "printf '\\033]133;C\\007'; sleep 0.3; printf '\\033]133;D;7\\007\\033]133;A\\007'",
        "--enter",
    ]);
    let done = host.ok(&["wait", &id, "--command-done", "--timeout-ms", "10000"]);
    assert_eq!(done["exit_code"], 7);
    let verbose = host.ok(&["list", "--verbose"]);
    assert_eq!(verbose[0]["shell"]["running"], false);
    assert_eq!(verbose[0]["shell"]["commands_finished"], 1);
    assert_eq!(verbose[0]["shell"]["exit_code"], 7);
    // New input waits for a new command.
    host.ok(&["send", &id, " "]);
    host.fails(
        &["wait", &id, "--command-done", "--timeout-ms", "300"],
        "timeout",
    );
}

/// Audits B26 and B27: a pane starts in the caller's directory with the
/// user's settings, a split pane in its source's directory with the source's
/// settings, and `list` shows the directory each one uses.
#[cfg(unix)]
#[test]
fn panes_start_with_the_callers_directory_and_the_users_settings() {
    let host = Host::new();
    host.write_config("term = xterm-termy-test\nno_such_setting = 1\n");
    let project = host.0.path().join("project");
    std::fs::create_dir_all(project.join("sub")).unwrap();
    let project = std::fs::canonicalize(project).unwrap();
    let in_project = Run {
        directory: Some(&project),
        ..Run::default()
    };
    let created = host.ok_with(&["create", "--shell", SHELL], in_project);
    // Invalid config lines are reported, and the rest applies.
    let warnings = created["warnings"].to_string();
    assert!(warnings.contains("line 2"), "{warnings}");
    let id = created["id"].as_str().unwrap().to_owned();
    // A symbolic link is kept, as a desktop tab keeps it.
    let link = host.0.path().join("link");
    std::os::unix::fs::symlink(project.join("sub"), &link).unwrap();
    let linked = host.ok(&[
        "create",
        "--shell",
        SHELL,
        "--working-directory",
        &link.to_string_lossy(),
    ])["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let sub = host.ok_with(
        &["create", "--shell", SHELL, "--working-directory", "sub"],
        Run {
            directory: Some(&project),
            ..Run::default()
        },
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, error) = host.error_with(
        &["create", "--working-directory", "typo"],
        Run {
            directory: Some(&project),
            ..Run::default()
        },
    );
    assert_eq!((status, &error["code"]), (2, &json!("invalid_argument")));
    let list = host.ok(&["list"]);
    let directory = |pane: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == pane)
            .unwrap()["working_directory"]
            .clone()
    };
    assert_eq!(directory(&id), json!(project.to_string_lossy()));
    assert_eq!(directory(&linked), json!(link.to_string_lossy()));
    assert_eq!(
        directory(&sub),
        json!(project.join("sub").to_string_lossy())
    );
    let environment = "printf 'ENV_%s_%s_%s\\n' \"$TERM\" \"$TERMY_SHELL_INTEGRATION\" \"$(pwd)\"";
    let expected = format!("ENV_xterm-termy-test_1_{}", project.join("sub").display());
    host.run(&sub, environment, &expected);

    // The split keeps the source's settings although the config changed.
    host.write_config("term = xterm-changed\n");
    host.set_layout(&[&[&sub]], 0);
    let split = host.ok(&["split", &sub])["id"].as_str().unwrap().to_owned();
    host.run(&split, environment, &expected);
}

/// Audit L8: `resize` keeps the pane's cell metrics and its saved size, and
/// refuses a pane that shares its tab.
#[test]
fn resize_keeps_cell_metrics_and_the_saved_size() {
    let host = Host::new();
    let id = host.shell();
    let client = host.client();
    let measured = termy_core::TerminalSize {
        cols: 80,
        rows: 24,
        cell_width: 7.5,
        cell_height: 15.0,
    };
    client.resize(&id, measured).unwrap();
    host.ok(&["resize", &id, "100", "30"]);
    let size = client.size(&id).unwrap();
    assert_eq!(
        (size.cols, size.rows, size.cell_width, size.cell_height),
        (100, 30, 7.5, 15.0)
    );
    let layout = host.ok(&["layout"]);
    let pane = &layout["windows"][0]["session"]["workspaces"][0]["tabs"][0]["panes"][0];
    assert_eq!((&pane["width"], &pane["height"]), (&json!(100), &json!(30)));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let output = host.ok(&["capture", &id]);
        if output["cols"] == 100 && output["rows"] == 30 {
            break;
        }
        assert!(Instant::now() < deadline, "resize was not applied");
        std::thread::sleep(Duration::from_millis(25));
    }
    host.ok(&["split", &id]);
    host.fails(&["resize", &id, "90", "30"], "unsupported");
}

/// Audit L15: `create --window` targets the window's active workspace.
#[test]
fn create_in_a_window_uses_its_active_workspace() {
    let host = Host::new();
    let first = host.shell();
    host.set_layout(&[&[&first], &[]], 1);
    let id = host.ok(&["create", "--shell", SHELL, "--window", "window-1"])["id"].clone();
    let layout = host.ok(&["layout"]);
    let workspaces = &layout["windows"][0]["session"]["workspaces"];
    assert_eq!(workspaces[1]["tabs"][0]["panes"][0]["session_id"], id);
    assert_eq!(workspaces[0]["tabs"].as_array().unwrap().len(), 1);
    host.fails(&["create", "--window", "missing-window"], "not_found");
    host.fails(
        &["create", "--window", "window-1", "--workspace", "5"],
        "not_found",
    );
    assert_eq!(host.ok(&["list"]).as_array().unwrap().len(), 2);
}

#[test]
fn workspaces_are_renamed_pinned_and_receive_new_tabs() {
    let host = Host::new();
    host.ok(&["start"]);
    let id = host.shell();
    let created_layout = host.ok(&["layout"]);
    assert_eq!(
        created_layout["windows"][0]["session"]["workspaces"][0]["tabs"][0]["panes"][0]["session_id"],
        id
    );
    host.set_layout(&[&[&id]], 0);
    host.ok(&["workspace", "window-1", "0", "rename", "Agent work"]);
    host.ok(&["workspace", "window-1", "0", "pin"]);
    let layout = host.ok(&["layout"]);
    let workspace = &layout["windows"][0]["session"]["workspaces"][0];
    assert_eq!(workspace["name"], "Agent work");
    assert_eq!(workspace["pinned"], true);
    host.fails(&["workspace", "window-1", "3", "pin"], "not_found");
    host.ok(&["close", &id]);
    assert_eq!(host.ok(&["list"]), json!([]));
    let pane = host.ok(&[
        "create",
        "--shell",
        SHELL,
        "--window",
        "window-1",
        "--workspace",
        "0",
    ]);
    let layout = host.ok(&["layout"]);
    let tabs = layout["windows"][0]["session"]["workspaces"][0]["tabs"]
        .as_array()
        .unwrap();
    assert_eq!(tabs.last().unwrap()["panes"][0]["session_id"], pane["id"]);
}

#[test]
fn splits_and_tab_edits_follow_the_saved_tree() {
    let host = Host::new();
    let id = host.shell();
    host.set_layout(&[&[&id]], 0);
    let split = host.ok(&["split", &id, "--axis", "horizontal"]);
    let split_id = split["id"].as_str().unwrap();
    assert_eq!(host.ok(&["list"]).as_array().unwrap().len(), 2);
    let layout = host.ok(&["layout"]);
    let panes = layout["windows"][0]["session"]["workspaces"][0]["tabs"][0]["panes"]
        .as_array()
        .unwrap();
    assert_eq!(panes.len(), 2);
    assert_eq!(panes[0]["width"], 40);
    assert_eq!(panes[1]["left"], 40);
    host.ok(&["tab", &id, "rename", "Build logs"]);
    host.ok(&["tab", &id, "pin"]);
    host.ok(&["tab", &id, "zoom"]);
    host.ok(&["tab", &id, "focus", split_id]);
    let layout = host.ok(&["layout"]);
    let tab = &layout["windows"][0]["session"]["workspaces"][0]["tabs"][0];
    assert_eq!(tab["manual_title"], "Build logs");
    assert_eq!(tab["pinned"], true);
    assert_eq!(tab["zoomed"], true);
    assert_eq!(tab["active_pane"], 1);
    host.ok(&["tab", &id, "unzoom"]);
    host.ok(&["tab", &id, "resize-divider", "right", "8"]);
    let layout = host.ok(&["layout"]);
    let tab = &layout["windows"][0]["session"]["workspaces"][0]["tabs"][0];
    assert_eq!(tab["zoomed"], false);
    assert_eq!(tab["panes"][0]["width"], 48);
    assert_eq!(tab["panes"][1]["width"], 32);
    host.fails(&["tab", &id, "resize-divider", "right", "500"], "failed");
    assert_eq!(host.ok(&["layout"]), layout);
    host.ok(&["tab", &id, "resize-divider", "right", "-8"]);
    host.ok(&["tab", &id, "reset-title"]);
    host.ok(&["tab", &id, "unpin"]);
    host.ok(&["close", split_id]);
    let layout = host.ok(&["layout"]);
    let panes = layout["windows"][0]["session"]["workspaces"][0]["tabs"][0]["panes"]
        .as_array()
        .unwrap();
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0]["width"], 80);
    assert_eq!(panes[0]["session_id"], id);
    host.fails(&["tab", "no-such-pane", "pin"], "not_found");
    host.fails(&["split", "no-such-pane"], "not_found");
}

#[test]
fn windows_create_reorder_and_delete_workspaces() {
    let host = Host::new();
    let id = host.shell();
    host.set_layout(&[&[&id]], 0);
    host.ok(&["close", &id]);
    host.ok(&["window", "window-1", "create-workspace", "Review"]);
    let layout = host.ok(&["layout"]);
    assert_eq!(layout["windows"][0]["session"]["active_workspace"], 1);
    assert_eq!(
        layout["windows"][0]["session"]["workspaces"][1]["name"],
        "Review"
    );
    let first = host.shell();
    let second = host.shell();
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
    host.fails(
        &["window", "window-1", "delete-empty-workspace", "0"],
        "failed",
    );
    assert_eq!(host.ok(&["layout"]), layout);
    host.ok(&["close", &first]);
    host.ok(&["close", &second]);
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
    host.fails(
        &["window", "no-such-window", "select-workspace", "0"],
        "not_found",
    );
}

#[test]
fn starting_a_host_releases_captured_output_while_host_stays_alive() {
    let host = Host::new();
    let root = host.sessions();
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
    let host = Host::new();
    let root = host.sessions();
    // Private, as every host since VERSION 1 created it.
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut std::fs::DirBuilder::new(), 0o700)
        .create(&root)
        .unwrap();
    #[cfg(not(unix))]
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

    let error = host.fails(&["list"], "version_mismatch");
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("termy mux shutdown"), "{message}");
    assert_eq!(
        host.ok(&["status"]),
        json!({"session_dir": root, "running": true, "compatible": false})
    );

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
    let host = Host::new();
    let root = host.sessions();
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
        let (status, error) = host.error(args);
        assert_eq!(status, 1, "{args:?}");
        let message = error["message"].as_str().unwrap();
        assert!(
            message.contains("no session host is running"),
            "{args:?}: {message}"
        );
    }
    host.fails(&["list"], "host_unavailable");
}

/// Audit I16: a pane learns its host and ID, and `termy mux` run inside it
/// finds that host through `TERMY_MUX_DIR`, whatever config directory it
/// would resolve.
#[cfg(unix)]
#[test]
fn commands_in_a_pane_reach_the_panes_host() {
    let host = Host::new();
    let id = host.shell();
    // The echoed command shows `<%s>`; only the output shows the ID.
    host.run(
        &id,
        "printf '<%s>\\n' \"$TERMY_MUX_PANE\"",
        &format!("<{id}>"),
    );
    let other_home = host.0.path().join("other-home");
    let output = Command::new(env!("CARGO_BIN_EXE_termy-cli"))
        .args(["mux", "list"])
        .env("TERMY_MUX_DIR", host.sessions())
        .env("HOME", &other_home)
        .env("XDG_CONFIG_HOME", other_home.join(".config"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["result"][0]["id"], id.as_str());
}
