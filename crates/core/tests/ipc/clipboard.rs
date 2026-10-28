//! Clipboard requests from pane programs, answered by an attached view.

use super::*;

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
                environment: None,
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
        environment: None,
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
