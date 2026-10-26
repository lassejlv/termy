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

fn shell(script: &str) -> PaneLaunch {
    PaneLaunch {
        size: TerminalSize::default(),
        working_directory: None,
        shell_integration: None,
        config: TerminalRuntimeConfig::default(),
        launch: Some(TerminalLaunch::Program {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        }),
    }
}

fn saved_pane(id: &str, left: u16, width: u16) -> session_model::StoredPane {
    session_model::StoredPane {
        session_id: Some(id.into()),
        left,
        top: 0,
        width,
        height: 24,
        buffer: None,
        zoom_steps: 0,
    }
}

fn saved_tab(panes: Vec<session_model::StoredPane>) -> session_model::StoredTab {
    session_model::StoredTab {
        zoomed: false,
        presentation: None,
        pinned: false,
        manual_title: None,
        active_pane: 0,
        layout_tree_json: None,
        panes,
    }
}

/// One window, one workspace holding `tabs`, with the last tab active.
fn saved_layout(tabs: Vec<session_model::StoredTab>) -> String {
    use session_model::*;
    serde_json::to_string(&StoredMultiplexer {
        version: STORED_MULTIPLEXER_VERSION,
        windows: vec![StoredWindow {
            id: "window".into(),
            session: StoredSession {
                workspaces: vec![StoredWorkspace {
                    name: "Work".into(),
                    pinned: false,
                    active_tab: tabs.len().saturating_sub(1),
                    tabs,
                }],
                active_workspace: 0,
                zoom_steps: 0,
            },
        }],
    })
    .unwrap()
}

fn saved_tabs(client: &SessionClient) -> Vec<Vec<(String, u16)>> {
    let layout =
        session_model::StoredMultiplexer::parse(client.layout().unwrap().as_deref()).unwrap();
    layout.windows[0].session.workspaces[0]
        .tabs
        .iter()
        .map(|tab| {
            tab.panes
                .iter()
                .map(|pane| (pane.session_id.clone().unwrap(), pane.width))
                .collect()
        })
        .collect()
}

/// Audit I4: the host versions the layout, rejects layouts it cannot read and
/// tells watchers about every accepted write.
#[test]
fn layout_writes_are_revisioned_validated_and_announced() {
    let host = Host::new();
    let mut watch = host.client.watch_layout().unwrap();
    assert_eq!(watch.recv().unwrap(), 0);
    assert_eq!(host.client.layout_revision().unwrap(), (0, None));
    let initial = saved_layout(Vec::new());
    assert_eq!(
        host.client
            .compare_and_set_layout(0, initial.clone())
            .unwrap(),
        Some(1)
    );
    assert_eq!(watch.recv().unwrap(), 1);
    assert_eq!(
        host.client.compare_and_set_layout(0, initial).unwrap(),
        None,
        "a stale revision must not overwrite"
    );

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let writers: Vec<_> = ["desktop", "CLI"]
        .into_iter()
        .map(|name| {
            let client = SessionClient::connect(&host.root).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut tab = saved_tab(vec![saved_pane(name, 0, 80)]);
                tab.manual_title = Some(name.into());
                barrier.wait();
                let written = client
                    .compare_and_set_layout(1, saved_layout(vec![tab]))
                    .unwrap();
                (name, written)
            })
        })
        .collect();
    let results: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|(_, written)| written.is_some())
            .count(),
        1
    );
    let winner = results
        .iter()
        .find(|(_, written)| written.is_some())
        .unwrap()
        .0;
    assert_eq!(saved_tabs(&host.client), [[(winner.to_owned(), 80)]]);
    assert_eq!(watch.recv().unwrap(), 2);

    for invalid in [
        "legacy write",
        r#"{"tabs":[]}"#,
        r#"{"version":9,"windows":[]}"#,
    ] {
        assert!(host.client.set_layout(invalid.into()).is_err(), "{invalid}");
        assert!(
            host.client
                .compare_and_set_layout(2, invalid.into())
                .is_err()
        );
    }
    assert_eq!(host.client.layout_revision().unwrap().0, 2);
    // An unconditional write still gets a revision and a notification.
    assert_eq!(host.client.set_layout(saved_layout(Vec::new())).unwrap(), 3);
    assert_eq!(watch.recv().unwrap(), 3);
}

/// Audit I4 and B2: the host prunes panes it closes and panes that exit while
/// no client shows them. An attached client still gets `Exit` and decides.
#[test]
fn closed_and_exited_panes_leave_the_layout() {
    let host = Host::new();
    let (kept, kept_terminal) = host.client.create(shell("read line"), None).unwrap();
    let (closed, closed_terminal) = host.client.create(shell("read line"), None).unwrap();
    // Never attached, so no client shows it when it exits.
    let exited = host
        .client
        .create_detached(shell("read line; exit 0"))
        .unwrap();
    let (watched, watched_terminal) = host
        .client
        .create(shell("read line; exit 0"), None)
        .unwrap();
    host.client
        .set_layout(saved_layout(vec![
            saved_tab(vec![saved_pane(&watched, 0, 80)]),
            saved_tab(vec![saved_pane(&kept, 0, 40), saved_pane(&closed, 40, 40)]),
            saved_tab(vec![saved_pane(&exited, 0, 80)]),
        ]))
        .unwrap();
    let mut watch = host.client.watch_layout().unwrap();
    let revision = watch.recv().unwrap();

    drop(closed_terminal);
    host.client.close(&closed).unwrap();
    assert!(watch.recv().unwrap() > revision);
    assert_eq!(
        saved_tabs(&host.client),
        [
            vec![(watched.clone(), 80)],
            vec![(kept.clone(), 80)],
            vec![(exited.clone(), 80)]
        ]
    );

    // Detached: the host removes the pane once its shell exits.
    host.client.write(&exited, b"\n".to_vec()).unwrap();
    wait_until(|| saved_tabs(&host.client).len() == 2);
    let layout =
        session_model::StoredMultiplexer::parse(host.client.layout().unwrap().as_deref()).unwrap();
    assert_eq!(
        layout.windows[0].session.workspaces[0].active_tab, 1,
        "the remaining neighbour of the closed active tab is selected"
    );

    // Attached: the client sees `Exit`; the layout keeps the pane for it.
    host.client.write(&watched, b"\n".to_vec()).unwrap();
    wait_until(|| {
        watched_terminal
            .drain_events(&mut |_| None)
            .0
            .iter()
            .any(|event| matches!(event, TerminalEvent::Exit))
    });
    assert_eq!(saved_tabs(&host.client).len(), 2);
    assert!(
        host.client
            .list()
            .unwrap()
            .iter()
            .any(|pane| pane.id == watched && pane.exited)
    );
    drop((kept_terminal, watched_terminal));
    for id in [kept, watched, exited] {
        host.client.close(&id).unwrap();
    }
    assert!(saved_tabs(&host.client).is_empty());
}

/// Audit B17: creates that resolve their own workspace must not fail, or kill
/// their new pane, because another client saved in between.
#[test]
fn concurrent_creates_and_saves_all_succeed() {
    let host = Host::new();
    host.client.set_layout(saved_layout(Vec::new())).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    // One creator resolves the default workspace, the other names it like
    // `termy mux create --window window --workspace 0`.
    let creators: Vec<_> = [false, true]
        .into_iter()
        .map(|named| {
            let client = SessionClient::connect(&host.root).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                (0..3)
                    .map(|_| {
                        if named {
                            client.create_tab_in_workspace(shell("read line"), "window", 0)
                        } else {
                            client.create_tab(shell("read line"), None)
                        }
                        .unwrap()
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let saver = {
        let client = SessionClient::connect(&host.root).unwrap();
        std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..40 {
                // A desktop save: selection and presentation change.
                client
                    .update_layout(|layout| {
                        let workspace = &mut layout.windows[0].session.workspaces[0];
                        workspace.pinned = !workspace.pinned;
                        workspace.active_tab = 0;
                        Ok(())
                    })
                    .unwrap();
            }
        })
    };
    let mut created: Vec<String> = creators
        .into_iter()
        .flat_map(|creator| creator.join().unwrap())
        .collect();
    saver.join().unwrap();
    let mut saved: Vec<String> = saved_tabs(&host.client)
        .into_iter()
        .map(|tab| tab[0].0.clone())
        .collect();
    created.sort();
    saved.sort();
    assert_eq!(saved, created);
    assert_eq!(host.client.list().unwrap().len(), 6);
    for id in created {
        host.client.close(&id).unwrap();
    }
}

/// Audit L6: closing a pane whose tab cannot be inferred repairs the layout
/// instead of failing after the process was killed. Audit L7: once the
/// layout change is committed, resize failures are warnings.
#[test]
fn close_and_split_commit_the_layout_and_warn_about_resizes() {
    let host = Host::new();
    let (left, left_terminal) = host.client.create(shell("read line"), None).unwrap();
    let (right, right_terminal) = host.client.create(shell("read line"), None).unwrap();
    drop((left_terminal, right_terminal));
    // Overlapping rects: no split tree can be inferred.
    host.client
        .set_layout(saved_layout(vec![saved_tab(vec![
            saved_pane(&left, 0, 60),
            saved_pane(&right, 30, 50),
            saved_pane("ghost", 0, 80),
        ])]))
        .unwrap();
    let warnings = host.client.close_saved_pane(&right).unwrap();
    assert_eq!(
        saved_tabs(&host.client),
        [[(left.clone(), 40), ("ghost".to_owned(), 40)]]
    );
    assert!(
        host.client
            .list()
            .unwrap()
            .iter()
            .all(|pane| pane.id != right)
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("ghost"), "{warnings:?}");

    let tab = session_model::StoredMultiplexer::parse(host.client.layout().unwrap().as_deref())
        .unwrap()
        .windows[0]
        .session
        .workspaces[0]
        .tabs[0]
        .clone();
    let (new_id, warnings) = host
        .client
        .split_pane(
            &left,
            session_model::PaneResizeAxis::Horizontal,
            &tab,
            shell("read line"),
        )
        .unwrap();
    assert_eq!(saved_tabs(&host.client)[0].len(), 3);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("ghost"), "{warnings:?}");
    for id in [left, new_id] {
        host.client.close(&id).unwrap();
    }
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
    let (new_id, warnings) = host
        .client
        .split_pane(
            &id,
            PaneResizeAxis::Horizontal,
            &tab,
            launch(TerminalSize::default()),
        )
        .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    for pane in [&new_id, &id] {
        let size = host.client.snapshot(pane).unwrap().size;
        assert_eq!((size.cell_width, size.cell_height), (11.0, 23.0));
        assert!(size.cols < 80, "pane was not resized: {size:?}");
    }
    drop(terminal);
    host.client.close(&new_id).unwrap();
    host.client.close(&id).unwrap();
}

/// Audit B20: a view learns that another client resized its pane, also when
/// it never resized the pane itself and when nobody read the size while the
/// frame that confirmed its own resize was current.
#[test]
fn another_clients_resize_overrides_the_size_this_client_claimed() {
    let host = Host::new();
    let size = |cols, rows| TerminalSize {
        cols,
        rows,
        ..TerminalSize::default()
    };
    let grid = |size: TerminalSize| (size.cols, size.rows);
    let (id, mut terminal) = host
        .client
        .create(
            PaneLaunch {
                size: size(100, 30),
                working_directory: None,
                shell_integration: None,
                config: TerminalRuntimeConfig::default(),
                launch: Some(TerminalLaunch::Program {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), "read done".into()],
                }),
            },
            None,
        )
        .unwrap();
    assert!(!terminal.host_size_overridden());

    // Created at the view's size: this client never sent a resize.
    host.client.resize(&id, size(40, 10)).unwrap();
    wait_until(|| grid(terminal.size()) == (40, 10));
    assert!(terminal.host_size_overridden());
    assert_eq!(grid(terminal.requested_size()), (40, 10));

    // Re-asserting clears the override.
    terminal.resize(size(100, 30));
    assert!(!terminal.host_size_overridden());
    wait_until(|| grid(terminal.size()) == (100, 30));
    assert!(!terminal.host_size_overridden());

    // `requested_size` is not read while the 90x20 frame is current.
    terminal.resize(size(90, 20));
    wait_until(|| grid(terminal.size()) == (90, 20));
    host.client.resize(&id, size(30, 8)).unwrap();
    wait_until(|| grid(terminal.size()) == (30, 8));
    assert!(terminal.host_size_overridden());
    assert_eq!(
        grid(terminal.requested_size()),
        (30, 8),
        "a stale request must not hide the other client's size"
    );
    drop(terminal);
    host.client.close(&id).unwrap();
}

/// A pane running `script` with `/bin/sh -c`; `$1` is the host directory.
fn script_pane(host: &Host, script: &str, args: &[&str]) -> (String, Terminal) {
    let mut argv = vec![
        "-c".into(),
        script.into(),
        "ipc-test".into(),
        host.root.to_string_lossy().into_owned(),
    ];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    host.client
        .create(
            PaneLaunch {
                size: TerminalSize::default(),
                working_directory: None,
                shell_integration: None,
                config: TerminalRuntimeConfig::default(),
                launch: Some(TerminalLaunch::Program {
                    program: "/bin/sh".into(),
                    args: argv,
                }),
            },
            None,
        )
        .unwrap()
}

#[track_caller]
fn wait_for_text(terminal: &Terminal, text: &str) {
    wait_until(|| {
        let render = terminal.render_read(true);
        let screen: String = render.cells.iter().map(|cell| cell.text.as_str()).collect();
        screen.contains(text)
    });
}

/// Answers kitty clipboard reads with the name of the requested type and
/// records the order of the requests.
#[derive(Default)]
struct RecordingClipboard {
    reads: Vec<String>,
}

impl TerminalReplyHost for RecordingClipboard {
    fn load_clipboard(&mut self, _: TerminalClipboardTarget) -> Option<String> {
        None
    }
    fn read_clipboard(
        &mut self,
        request: TerminalClipboardReadRequest,
    ) -> TerminalClipboardReadResult {
        let mime_type = request.mime_types[0].clone();
        self.reads.push(mime_type.clone());
        TerminalClipboardReadResult::Success {
            available_formats: vec![mime_type.clone()],
            contents: vec![TerminalClipboardContent {
                data: mime_type.as_bytes().to_vec(),
                mime_type,
            }],
            remember_permission: false,
        }
    }
}

/// The reply host of a view that must never be asked.
struct NoClipboard;

impl TerminalReplyHost for NoClipboard {
    fn load_clipboard(&mut self, _: TerminalClipboardTarget) -> Option<String> {
        panic!("this view must not receive clipboard requests")
    }
    fn read_clipboard(&mut self, _: TerminalClipboardReadRequest) -> TerminalClipboardReadResult {
        panic!("this view must not receive clipboard requests")
    }
}

/// What [`RecordingClipboard`] makes the program read for request `id`.
fn kitty_read_reply(id: &str, mime_type: &str) -> Vec<u8> {
    use base64::Engine as _;
    let mime = base64::engine::general_purpose::STANDARD.encode(mime_type);
    format!(
        "\x1b]5522;type=read:status=OK:id={id}\x1b\\\
         \x1b]5522;type=read:status=DATA:id={id}:mime={mime};{mime}\x1b\\\
         \x1b]5522;type=read:status=DONE:id={id}\x1b\\"
    )
    .into_bytes()
}

/// Audit B7: the user may take seconds to answer the permission prompt. An
/// answer long after the old 500 ms deadline must still reach the program,
/// and the pane keeps working while the requests wait: the old host held
/// output for 500 ms per request.
#[test]
fn a_late_clipboard_answer_still_reaches_the_program() {
    let host = Host::new();
    let mut expected = Vec::new();
    for id in ["a", "b", "c"] {
        expected.extend(kitty_read_reply(id, "text/plain"));
    }
    let script = r#"
stty raw -echo
printf READY
dd bs=1 count=1 >/dev/null 2>/dev/null
for id in a b c; do printf '\033]5522;type=read:id=%s;dGV4dC9wbGFpbg==\033\\' "$id"; done
printf WAITING
dd bs=1 count="$2" > "$1/clipboard" 2>/dev/null
sleep 30
"#;
    let length = expected.len().to_string();
    let (id, terminal) = script_pane(&host, script, &[&length]);
    wait_for_text(&terminal, "READY");
    let requested = Instant::now();
    terminal.write(b"x");
    wait_for_text(&terminal, "WAITING");
    assert!(
        requested.elapsed() < Duration::from_secs(1),
        "output behind the requests was held for {:?}",
        requested.elapsed()
    );
    std::thread::sleep(Duration::from_millis(800));
    assert!(fs::read(host.root.join("clipboard")).map_or(true, |bytes| bytes.is_empty()));
    let mut clipboard = RecordingClipboard::default();
    wait_until(|| {
        terminal.drain_events(&mut clipboard);
        fs::read(host.root.join("clipboard")).is_ok_and(|bytes| bytes.len() == expected.len())
    });
    assert_eq!(clipboard.reads.len(), 3);
    assert_eq!(fs::read(host.root.join("clipboard")).unwrap(), expected);
    host.client.close(&id).unwrap();
}

/// Audit B7: a script watching the pane (like `termy mux wait` used to)
/// must not take the view's clipboard requests, and the view runs requests
/// queued together in the order the program made them.
#[test]
fn clipboard_requests_skip_observers_and_keep_their_order() {
    let host = Host::new();
    let mime_types = ["text/plain", "text/html", "image/png", "text/uri-list"];
    let mut expected = Vec::new();
    for (index, mime_type) in mime_types.iter().enumerate() {
        expected.extend(kitty_read_reply(&index.to_string(), mime_type));
    }
    let script = r#"
stty raw -echo
printf READY
dd bs=1 count=1 >/dev/null 2>/dev/null
index=0
for mime in dGV4dC9wbGFpbg== dGV4dC9odG1s aW1hZ2UvcG5n dGV4dC91cmktbGlzdA==; do
  printf '\033]5522;type=read:id=%s;%s\033\\' "$index" "$mime"
  index=$((index + 1))
done
printf QUEUED
dd bs=1 count="$2" > "$1/clipboard" 2>/dev/null
sleep 30
"#;
    let length = expected.len().to_string();
    let (id, terminal) = script_pane(&host, script, &[&length]);
    // Newer than the view, so the old routing would have picked it.
    let observer = host.client.observe(&id, None).unwrap();
    wait_for_text(&terminal, "READY");
    terminal.write(b"x");
    // All requests reach the view before it drains any of them.
    wait_for_text(&terminal, "QUEUED");
    let queued = Instant::now() + Duration::from_millis(300);
    while Instant::now() < queued {
        observer.drain_events(&mut NoClipboard);
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut clipboard = RecordingClipboard::default();
    terminal.drain_events(&mut clipboard);
    assert_eq!(clipboard.reads, mime_types, "drained in one pass, in order");
    wait_until(|| {
        observer.drain_events(&mut NoClipboard);
        fs::read(host.root.join("clipboard")).is_ok_and(|bytes| bytes.len() == expected.len())
    });
    assert_eq!(fs::read(host.root.join("clipboard")).unwrap(), expected);
    drop(observer);
    host.client.close(&id).unwrap();
}

/// Audit B7: a request held by a view that goes away is denied, not kept.
#[test]
fn a_detached_view_denies_its_waiting_clipboard_request() {
    let host = Host::new();
    let expected = b"\x1b]5522;type=read:status=EPERM:id=gone\x1b\\";
    let script = r#"
stty raw -echo
printf READY
dd bs=1 count=1 >/dev/null 2>/dev/null
printf '\033]5522;type=read:id=gone;dGV4dC9wbGFpbg==\033\\'
printf WAITING
dd bs=1 count="$2" > "$1/clipboard" 2>/dev/null
sleep 30
"#;
    let length = expected.len().to_string();
    let (id, terminal) = script_pane(&host, script, &[&length]);
    wait_for_text(&terminal, "READY");
    terminal.write(b"x");
    // The view leaves without answering.
    wait_for_text(&terminal, "WAITING");
    std::thread::sleep(Duration::from_millis(100));
    assert!(fs::read(host.root.join("clipboard")).map_or(true, |bytes| bytes.is_empty()));
    drop(terminal);
    wait_until(|| {
        fs::read(host.root.join("clipboard")).is_ok_and(|bytes| bytes.len() == expected.len())
    });
    assert_eq!(fs::read(host.root.join("clipboard")).unwrap(), expected);
    host.client.close(&id).unwrap();
}

/// Audit B7: a hidden tab suspends render wakeups, but must still be woken
/// to answer a clipboard request before the host gives up on it.
#[test]
fn clipboard_requests_wake_a_hidden_view() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let host = Host::new();
    let script = r#"
stty raw -echo
printf READY
dd bs=1 count=1 >/dev/null 2>/dev/null
printf '\033]5522;type=read:id=hidden;dGV4dC9wbGFpbg==\033\\'
sleep 30
"#;
    let wakeups = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakeups);
    let notifier = TerminalWakeupNotifier::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let launch = PaneLaunch {
        size: TerminalSize::default(),
        working_directory: None,
        shell_integration: None,
        config: TerminalRuntimeConfig::default(),
        launch: Some(TerminalLaunch::Program {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        }),
    };
    let (id, terminal) = host.client.create(launch, Some(notifier)).unwrap();
    wait_for_text(&terminal, "READY");
    wait_until_quiet(&terminal);
    terminal.set_wakeup_enabled(false);
    wakeups.store(0, Ordering::SeqCst);
    terminal.write(b"x");
    wait_until(|| wakeups.load(Ordering::SeqCst) > 0);
    let mut clipboard = RecordingClipboard::default();
    terminal.drain_events(&mut clipboard);
    assert_eq!(clipboard.reads, ["text/plain"]);
    host.client.close(&id).unwrap();
}

/// Audit L18: a kitty paste notification goes behind the keys typed before
/// it. The program reads nothing until both were sent, so the keys wait in
/// the host's input backlog, where the old notification, a protocol reply,
/// jumped ahead of them.
#[test]
fn paste_notification_follows_the_keys_queued_before_it() {
    let host = Host::new();
    let script = r#"
stty raw -echo
printf '\033[?5522h'
printf READY
while [ ! -f "$1/go" ]; do sleep 0.01; done
exec cat > "$1/input"
"#;
    let (id, terminal) = script_pane(&host, script, &[]);
    wait_for_text(&terminal, "READY");
    wait_until(|| terminal.kitty_clipboard_paste_events_enabled());
    // Far more than a PTY buffers for a program that is not reading.
    let keys = vec![b'k'; 2048];
    for _ in 0..128 {
        terminal.write(&keys);
    }
    assert!(terminal.send_kitty_clipboard_paste_event(
        TerminalClipboardLocation::Clipboard,
        &["text/plain".into()]
    ));
    fs::write(host.root.join("go"), b"").unwrap();
    let typed = keys.len() * 128;
    wait_until(|| {
        fs::read(host.root.join("input")).is_ok_and(|bytes| {
            bytes.len() > typed && String::from_utf8_lossy(&bytes).contains("status=DONE")
        })
    });
    let input = fs::read(host.root.join("input")).unwrap();
    let first_other = input.iter().position(|byte| *byte != b'k');
    assert_eq!(
        first_other,
        Some(typed),
        "the notification overtook typed keys"
    );
    assert!(input[typed..].starts_with(b"\x1b]5522;type=read:status=OK"));
    host.client.close(&id).unwrap();
}

/// Audit I13: scripts create and read panes without subscribing to them,
/// and reading publishes no frame that would wake the views.
#[test]
fn detached_panes_are_read_without_subscribing() {
    let host = Host::new();
    let launch = PaneLaunch {
        size: TerminalSize {
            cols: 30,
            rows: 5,
            cell_width: 7.0,
            cell_height: 15.0,
        },
        working_directory: None,
        shell_integration: None,
        config: TerminalRuntimeConfig::default(),
        launch: Some(TerminalLaunch::Program {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "printf SNAPSHOT; sleep 30".into()],
        }),
    };
    let id = host.client.create_detached(launch).unwrap();
    let view = host.client.observe(&id, None).unwrap();
    wait_for_text(&view, "SNAPSHOT");
    wait_until_quiet(&view);
    for _ in 0..3 {
        let state = host.client.snapshot(&id).unwrap();
        let screen: String = state
            .render
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        assert!(screen.contains("SNAPSHOT"));
        assert_eq!((state.size.cols, state.size.rows), (30, 5));
        assert_eq!((state.size.cell_width, state.size.cell_height), (7.0, 15.0));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !view.has_pending_events(),
        "a snapshot published a frame to the view"
    );
    drop(view);
    host.client.close(&id).unwrap();
}
