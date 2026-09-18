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

#[test]
fn scrolling_reply_updates_viewport_before_selection_reads() {
    assert_scrolling_reply_updates_viewport(false);
}

#[test]
fn legacy_scrolling_reply_updates_viewport_before_selection_reads() {
    assert_scrolling_reply_updates_viewport(true);
}

fn assert_scrolling_reply_updates_viewport(legacy_graphics: bool) {
    let host = Host::new();
    if legacy_graphics {
        let path = host.root.join("endpoint.json");
        let mut endpoint: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        endpoint.as_object_mut().unwrap().remove("graphics_stream");
        fs::write(path, serde_json::to_vec(&endpoint).unwrap()).unwrap();
    }
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
    eprintln!(
        "20 acknowledged viewport changes (legacy graphics: {legacy_graphics}): {:?}",
        started.elapsed()
    );
    // Once output and scrolling stop, read-only RPCs must not keep publishing
    // identical frames and waking the desktop (including legacy image reads).
    std::thread::sleep(Duration::from_millis(50));
    terminal.drain_events(&mut |_| None);
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
    wait_until(|| has_marker("A"));
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
        version: 1,
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
fn missing_layout_capability_does_not_disconnect_older_hosts() {
    let host = Host::new();
    let path = host.root.join("endpoint.json");
    let mut endpoint: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    endpoint
        .as_object_mut()
        .unwrap()
        .remove("conditional_layout_updates");
    fs::write(&path, serde_json::to_vec(&endpoint).unwrap()).unwrap();
    let client = SessionClient::connect(&host.root).unwrap();
    assert!(!client.supports_conditional_layout_updates());
    assert!(
        client
            .compare_and_set_layout(None, "unsupported".into())
            .is_err()
    );
    assert!(client.list().unwrap().is_empty());
    client.set_layout("legacy".into()).unwrap();
    assert_eq!(client.layout().unwrap().as_deref(), Some("legacy"));
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
