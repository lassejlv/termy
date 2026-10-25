#![cfg(unix)]

use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::Path,
    time::{Duration, Instant},
};
use termy_core::multiplexer::{PaneLaunch, SessionClient, connect_or_start};
use termy_core::*;

struct Host {
    client: SessionClient,
    root: std::path::PathBuf,
    _temp: tempfile::TempDir,
}

impl Host {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("host");
        let executable = std::env::var_os("TERMY_MUX_TEST_HOST_BINARY").map_or_else(
            || Path::new(env!("CARGO_BIN_EXE_termy-session-host")).to_owned(),
            std::path::PathBuf::from,
        );
        let client = connect_or_start(&root, &executable).unwrap();
        Self {
            client,
            root,
            _temp: temp,
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.client.shutdown();
    }
}

#[track_caller]
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "condition did not become true");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Drains events until the terminal stays quiet for a full window, so late
/// frames from earlier output or scrolls on a slow host are not mistaken for
/// wakeups caused by whatever the test does next.
#[track_caller]
fn wait_until_quiet(terminal: &Terminal) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        terminal.drain_events(&mut |_| None);
        std::thread::sleep(Duration::from_millis(100));
        if !terminal.has_pending_events() {
            return;
        }
        assert!(Instant::now() < deadline, "terminal did not go quiet");
    }
}

#[test]
fn scrolling_reply_updates_viewport_before_selection_reads() {
    let host = Host::new();
    let (id, terminal) = host.client.create(
        PaneLaunch {
            size: TerminalSize { cols: 40, rows: 6, ..TerminalSize::default() },
            working_directory: None,
            shell_integration: None,
            config: TerminalRuntimeConfig::default(),
            launch: Some(TerminalLaunch::Program {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "stty -echo; i=0; while [ $i -lt 50 ]; do printf 'line-%s\\r\\n' \"$i\"; i=$((i+1)); done; printf READY; read done".into()],
            }),
        },
        None,
    ).unwrap();
    wait_until(|| terminal.scroll_state().1 >= 45);
    let started = Instant::now();
    for _ in 0..10 {
        assert!(terminal.scroll_display(3));
        assert_eq!(
            terminal.scroll_state().0,
            3,
            "selection must see the acknowledged scroll before recording its baseline"
        );
        assert_eq!(terminal.render_read(true).metadata.display_offset, 3);
        assert!(terminal.scroll_to_bottom());
        assert_eq!(terminal.scroll_state().0, 0);
    }
    eprintln!("20 acknowledged viewport changes: {:?}", started.elapsed());
    // Once output and scrolling stop, read-only RPCs must not keep publishing
    // identical frames and waking the desktop.
    wait_until_quiet(&terminal);
    terminal.snapshot();
    terminal.search("line");
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        !terminal.has_pending_events(),
        "read-only queries woke the terminal"
    );
    host.client.close(&id).unwrap();
}

#[test]
fn kitty_images_follow_pty_scrolling_through_the_session_host() {
    let host = Host::new();
    // Wait for input between frames so the client caches each placement before
    // the next scroll. The image revision need not change when history grows.
    let script = r#"
stty raw -echo
printf '\033[2J\033[3;1H\033_Ga=T,i=1,f=32,s=1,v=1,c=2,r=2,C=1,q=2;AQID/w==\033\\'
printf '\033[1;1HA'
dd bs=1 count=1 >/dev/null 2>/dev/null
printf '\033[6;1H\r\nB'
dd bs=1 count=1 >/dev/null 2>/dev/null
printf '\r\n\r\n\r\n\r\nC'
dd bs=1 count=1 >/dev/null 2>/dev/null
"#;
    let (id, terminal) = host
        .client
        .create(
            PaneLaunch {
                size: TerminalSize {
                    cols: 20,
                    rows: 6,
                    cell_width: 10.0,
                    cell_height: 20.0,
                },
                working_directory: None,
                shell_integration: None,
                config: TerminalRuntimeConfig::default(),
                launch: Some(TerminalLaunch::Program {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), script.into()],
                }),
            },
            None,
        )
        .unwrap();
    let has_marker = |marker| {
        terminal
            .render_read(true)
            .cells
            .iter()
            .any(|cell| cell.text == marker)
    };
    // The Create snapshot can already contain text before the asynchronous
    // graphics stream delivers its first placement.
    wait_until(|| {
        has_marker("A")
            && terminal
                .kitty_graphics_placements()
                .first()
                .is_some_and(|placement| placement.viewport_row == 2)
    });
    assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 2);
    terminal.write(b"x");
    wait_until(|| has_marker("B"));
    assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);
    terminal.write(b"x");
    wait_until(|| has_marker("C"));
    assert!(terminal.kitty_graphics_placements().is_empty());

    assert!(terminal.scroll_display(4));
    wait_until(|| terminal.scroll_state().0 == 4);
    assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);
    assert!(terminal.scroll_to_bottom());
    wait_until(|| terminal.scroll_state().0 == 0);
    assert!(terminal.kitty_graphics_placements().is_empty());

    drop(terminal);
    host.client.close(&id).unwrap();
}

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

#[test]
fn conditional_layout_updates_reject_stale_and_concurrent_writers() {
    let host = Host::new();
    assert!(
        host.client
            .compare_and_set_layout(None, "initial".into())
            .unwrap()
    );
    assert!(
        !host
            .client
            .compare_and_set_layout(None, "stale".into())
            .unwrap()
    );
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let writers: Vec<_> = ["desktop", "CLI"]
        .into_iter()
        .map(|value| {
            let client = SessionClient::connect(&host.root).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                (
                    value,
                    client
                        .compare_and_set_layout(Some("initial".into()), value.into())
                        .unwrap(),
                )
            })
        })
        .collect();
    let results: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|(_, updated)| *updated).count(), 1);
    let winner = results.iter().find(|(_, updated)| *updated).unwrap().0;
    assert_eq!(host.client.layout().unwrap().as_deref(), Some(winner));
    host.client.set_layout("legacy write".into()).unwrap();
    assert!(
        !host
            .client
            .compare_and_set_layout(Some(winner.into()), "stale again".into())
            .unwrap()
    );
    assert_eq!(
        host.client.layout().unwrap().as_deref(),
        Some("legacy write")
    );
}

#[test]
fn clipboard_requests_use_the_attached_host_and_queries_work_detached() {
    let host = Host::new();
    let expected = b"\x1b]5522;type=read:status=OK:id=clip\x1b\\\x1b]5522;type=read:status=DATA:id=clip:mime=dGV4dC9wbGFpbg==;cmVwbHk=\x1b\\\x1b]5522;type=read:status=DONE:id=clip\x1b\\";
    struct ClipboardHost(bool);
    impl TerminalReplyHost for ClipboardHost {
        fn load_clipboard(&mut self, _: TerminalClipboardTarget) -> Option<String> {
            None
        }
        fn read_clipboard(
            &mut self,
            request: TerminalClipboardReadRequest,
        ) -> TerminalClipboardReadResult {
            assert_eq!(request.mime_types, vec!["text/plain"]);
            self.0 = true;
            TerminalClipboardReadResult::Success {
                available_formats: vec!["text/plain".into()],
                contents: vec![TerminalClipboardContent {
                    mime_type: "text/plain".into(),
                    data: b"reply".to_vec(),
                }],
                remember_permission: false,
            }
        }
    }
    let mut clipboard_host = ClipboardHost(false);
    let script = r#"
stty raw -echo
printf READY
dd bs=1 count=1 >/dev/null 2>/dev/null
printf '\033]5522;type=read:id=clip;dGV4dC9wbGFpbg==\033\\'
dd bs=1 count="$2" > "$1/clipboard" 2>/dev/null
while [ ! -f "$1/detached" ]; do sleep 0.01; done
printf '\033[H\033[6n'
dd bs=1 count=6 > "$1/cursor" 2>/dev/null
printf DONE
sleep 30
"#;
    let (id, terminal) = host
        .client
        .create(
            PaneLaunch {
                size: TerminalSize::default(),
                working_directory: None,
                shell_integration: None,
                config: TerminalRuntimeConfig::default(),
                launch: Some(TerminalLaunch::Program {
                    program: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        script.into(),
                        "protocol-test".into(),
                        host.root.to_string_lossy().into_owned(),
                        expected.len().to_string(),
                    ],
                }),
            },
            None,
        )
        .unwrap();
    wait_until(|| {
        terminal
            .snapshot()
            .cells
            .iter()
            .any(|cell| cell.char == 'Y')
    });
    terminal.write(b"x");
    wait_until(|| {
        terminal.drain_events(&mut clipboard_host);
        fs::read(host.root.join("clipboard")).is_ok_and(|bytes| bytes.len() == expected.len())
    });
    assert!(clipboard_host.0);
    assert_eq!(fs::read(host.root.join("clipboard")).unwrap(), expected);
    drop(terminal);
    fs::write(host.root.join("detached"), b"").unwrap();
    wait_until(|| fs::read(host.root.join("cursor")).is_ok_and(|bytes| bytes.len() == 6));
    assert_eq!(fs::read(host.root.join("cursor")).unwrap(), b"\x1b[1;1R");
    host.client.close(&id).unwrap();
}

#[test]
fn program_status_survives_reattach_and_expires_active_records_on_exit() {
    let host = Host::new();
    let (id, terminal) = host
        .client
        .create(
            PaneLaunch {
                size: TerminalSize::default(),
                working_directory: None,
                shell_integration: None,
                config: TerminalRuntimeConfig::default(),
                launch: Some(TerminalLaunch::Program {
                    program: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        r#"stty -echo
printf '\033]7501;state=working:app=deploy\007\033]7501;state=done:id=child:msg=SGk=\007'
read finish
"#
                        .into(),
                    ],
                }),
            },
            None,
        )
        .unwrap();
    let mut latest = vec![];
    wait_until(|| {
        for event in terminal.drain_events(&mut |_| None).0 {
            if let TerminalEvent::ProgramStatus(records) = event {
                latest = records;
            }
        }
        latest.len() == 2
    });
    assert_eq!(latest[1].app.as_deref(), Some("deploy"));
    drop(terminal);
    let terminal = host.client.attach(&id, None).unwrap();
    let mut attached = vec![];
    wait_until(|| {
        for event in terminal.drain_events(&mut |_| None).0 {
            if let TerminalEvent::ProgramStatus(records) = event {
                attached = records;
            }
        }
        attached.len() == 2
    });
    assert_eq!(attached, latest);
    terminal.write(b"done\n");
    let mut exited = false;
    wait_until(|| {
        for event in terminal.drain_events(&mut |_| None).0 {
            match event {
                TerminalEvent::ProgramStatus(records) => latest = records,
                TerminalEvent::Exit => exited = true,
                _ => {}
            }
        }
        exited
    });
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].state, ProgramState::Done);
    assert_eq!(latest[0].msg.as_deref(), Some("Hi"));
    assert_eq!(latest[0].app, None); // removed parent no longer supplies app
    host.client.close(&id).unwrap();
}

fn shell_pane(host: &Host, script: &str) -> (String, Terminal) {
    host.client
        .create(
            PaneLaunch {
                size: TerminalSize::default(),
                working_directory: None,
                shell_integration: None,
                config: TerminalRuntimeConfig::default(),
                launch: Some(TerminalLaunch::Program {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), script.into()],
                }),
            },
            None,
        )
        .unwrap()
}

/// Drains `terminal` until a drained batch has an event for which `done`
/// returns true, and returns every event drained, without wakeups.
#[track_caller]
fn events_until(
    terminal: &Terminal,
    mut done: impl FnMut(&TerminalEvent) -> bool,
) -> Vec<TerminalEvent> {
    let mut seen = Vec::new();
    wait_until(|| {
        let mut finished = false;
        for event in terminal.drain_events(&mut |_| None).0 {
            if !matches!(event, TerminalEvent::Wakeup) {
                finished |= done(&event);
                seen.push(event);
            }
        }
        finished
    });
    seen
}

fn is_shell_event(event: &TerminalEvent) -> bool {
    matches!(
        event,
        TerminalEvent::ShellPromptStart
            | TerminalEvent::ShellCommandStart
            | TerminalEvent::ShellCommandExecuting
            | TerminalEvent::ShellCommandFinished(_)
    )
}

#[test]
fn command_finish_reaches_the_view_and_is_not_replayed_on_reattach() {
    let host = Host::new();
    // The shell integrations print the finish and the next prompt in one
    // write. Output before the view subscribes would only be replayed.
    let (id, terminal) = shell_pane(
        &host,
        r#"stty -echo
read start
printf '\033]2;build\007\033]9;4;3;0\007\007\033]133;C\007\033]133;D;1\007\033]133;A\007'
read go
printf '\033]133;C\007\033]133;D;0\007\033]133;A\007'
read finish
"#,
    );
    terminal.write(b"start\n");
    let events = events_until(&terminal, |event| {
        matches!(event, TerminalEvent::ShellPromptStart)
    });
    let shell: Vec<_> = events
        .iter()
        .filter(|event| is_shell_event(event))
        .collect();
    assert!(
        matches!(
            shell.as_slice(),
            [
                TerminalEvent::ShellCommandExecuting,
                TerminalEvent::ShellCommandFinished(Some(1)),
                TerminalEvent::ShellPromptStart,
            ]
        ),
        "{events:?}"
    );
    wait_until_quiet(&terminal);
    drop(terminal);

    // The replay restores the title, but not the finished command, the bell
    // or the progress of the program that the prompt replaced.
    let terminal = host.client.attach(&id, None).unwrap();
    let replay = events_until(
        &terminal,
        |event| matches!(event, TerminalEvent::Title(title) if title == "build"),
    );
    wait_until_quiet(&terminal);
    assert!(
        !replay.iter().any(|event| is_shell_event(event)
            || matches!(event, TerminalEvent::Bell | TerminalEvent::Progress(_))),
        "{replay:?}"
    );

    terminal.write(b"go\n");
    let events = events_until(&terminal, |event| {
        matches!(event, TerminalEvent::ShellPromptStart)
    });
    let shell: Vec<_> = events
        .iter()
        .filter(|event| is_shell_event(event))
        .collect();
    assert!(
        matches!(
            shell.as_slice(),
            [
                TerminalEvent::ShellCommandExecuting,
                TerminalEvent::ShellCommandFinished(Some(0)),
                TerminalEvent::ShellPromptStart,
            ]
        ),
        "{events:?}"
    );
    host.client.close(&id).unwrap();
}

#[test]
fn layout_resizes_keep_the_live_cell_metrics() {
    use termy_core::session_model::*;
    let host = Host::new();
    let launch = |size| PaneLaunch {
        size,
        working_directory: None,
        shell_integration: None,
        config: TerminalRuntimeConfig::default(),
        launch: Some(TerminalLaunch::Program {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "read done".into()],
        }),
    };
    // The desktop created this pane with its real font metrics.
    let live = TerminalSize {
        cols: 80,
        rows: 24,
        cell_width: 11.0,
        cell_height: 23.0,
    };
    let (id, terminal) = host.client.create(launch(live), None).unwrap();
    let tab = StoredTab {
        zoomed: false,
        presentation: None,
        pinned: false,
        manual_title: None,
        active_pane: 0,
        layout_tree_json: None,
        panes: vec![StoredPane {
            session_id: Some(id.clone()),
            left: 0,
            top: 0,
            width: 80,
            height: 24,
            buffer: None,
            zoom_steps: 0,
        }],
    };
    let layout = StoredMultiplexer {
        version: 1,
        windows: vec![StoredWindow {
            id: "window".into(),
            session: StoredSession {
                workspaces: vec![StoredWorkspace {
                    name: "main".into(),
                    pinned: false,
                    active_tab: 0,
                    tabs: vec![tab.clone()],
                }],
                active_workspace: 0,
                zoom_steps: 0,
            },
        }],
    };
    host.client
        .set_layout(serde_json::to_string(&layout).unwrap())
        .unwrap();

    // The CLI splits without knowing the font; both panes keep 11x23 cells.
    let (new_id, split) = host
        .client
        .split_pane(
            &id,
            PaneResizeAxis::Horizontal,
            &tab,
            launch(TerminalSize::default()),
        )
        .unwrap();
    for pane in [&new_id, &id] {
        let size = host.client.attach(pane, None).unwrap().size();
        assert_eq!((size.cell_width, size.cell_height), (11.0, 23.0));
        assert!(size.cols < 80, "pane was not resized: {size:?}");
    }
    drop((terminal, split));
    host.client.close(&new_id).unwrap();
    host.client.close(&id).unwrap();
}
