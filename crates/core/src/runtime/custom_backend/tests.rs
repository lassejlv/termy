use super::damage::{add_spans, normalize_spans};
use super::*;
use std::time::Duration;

fn display(cols: u16, rows: u16) -> CustomBackend {
    CustomBackend::new_display(
        TerminalSize {
            cols,
            rows,
            ..TerminalSize::default()
        },
        None,
    )
}

#[test]
fn facade_preserves_wide_combining_and_history_cells() {
    let terminal = display(6, 2);
    terminal.feed_output("a界e\u{301}\r\nsecond\r\nlast".as_bytes());
    assert_eq!(terminal.scroll_state().1, 1);
    let mut observed = Vec::new();
    terminal.visit_line_cells(-1, -1, |_, _, _, cell| observed.push(cell.clone()));
    assert_eq!(observed[1].text.as_str(), "界");
    assert!(observed[2].wide_character_spacer);
    assert_eq!(observed[3].text.as_str(), "e\u{301}");
    assert_eq!(terminal.search("last")[0].row, 2);
}

#[test]
fn quiet_history_compacts_without_changing_the_frame_or_generation() {
    let terminal = display(120, 4);
    for _ in 0..40 {
        terminal.feed_output("你好 👩🏽‍💻 a short history line\r\n".as_bytes());
    }
    let generation = terminal.shared.state().generation;
    assert!(terminal.shared.state().engine.needs_history_compaction());
    let deadline = Instant::now() + Duration::from_secs(2);
    while terminal.shared.state().engine.needs_history_compaction() {
        assert!(Instant::now() < deadline, "idle history timer did not run");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(terminal.shared.state().generation, generation);
    let mut cells = Vec::new();
    terminal.visit_line_cells(-1, -1, |_, _, _, cell| cells.push(cell.clone()));
    assert_eq!(cells[5].text.as_str(), "👩🏽‍💻");
    // Quiet output now packs rows as they scroll in: nothing to compact.
    terminal.feed_output(b"another line\r\n");
    assert!(!terminal.shared.state().engine.needs_history_compaction());
    // A burst keeps them dense for the idle timer to pack.
    terminal.feed_output(&b"a burst line\r\n".repeat(400));
    assert!(terminal.shared.state().engine.needs_history_compaction());
}

#[test]
fn generation_rejects_cells_changed_since_damage_read() {
    let terminal = display(6, 2);
    terminal.take_render_damage_snapshot();
    terminal.feed_output(b"x");
    let update = terminal.take_render_damage_snapshot();
    let TerminalDamageSnapshot::Partial(spans) = update.damage else {
        panic!("incremental damage");
    };
    let mut visited = 0;
    assert!(terminal.visit_viewport_ranges_at_generation(
        update.generation,
        &spans,
        |_, _, _, _, _| visited += 1
    ));
    assert!(visited > 0);
    terminal.feed_output(b"y");
    assert!(!terminal.visit_viewport_ranges_at_generation(
        update.generation,
        &spans,
        |_, _, _, _, _| panic!("stale read")
    ));
}

#[test]
fn forced_read_consumes_full_damage_once() {
    let terminal = display(6, 2);
    assert!(matches!(
        terminal.render_read(true).update.damage,
        TerminalDamageSnapshot::Full
    ));
    assert_eq!(
        terminal.take_damage_snapshot(),
        TerminalDamageSnapshot::Partial(Vec::new())
    );
}

#[test]
fn display_routes_cursor_and_clipboard_queries() {
    struct Host {
        replies: Vec<u8>,
    }
    impl TerminalReplyHost for Host {
        fn load_clipboard(&mut self, _: TerminalClipboardTarget) -> Option<String> {
            Some("hello".to_owned())
        }
        fn protocol_reply(&mut self, bytes: &[u8]) {
            self.replies.extend_from_slice(bytes);
        }
    }
    let terminal = display(6, 2);
    terminal.feed_output(b"hi\x1b[6n\x1b]52;c;?\x1b\\");
    let mut host = Host {
        replies: Vec::new(),
    };
    terminal.drain_events(&mut host);
    assert_eq!(host.replies, b"\x1b[1;3R\x1b]52;c;aGVsbG8=\x1b\\");
}

#[test]
fn image_uploads_decoded_outside_the_lock_keep_stream_order() {
    struct Host {
        replies: Vec<u8>,
    }
    impl TerminalReplyHost for Host {
        fn load_clipboard(&mut self, _: TerminalClipboardTarget) -> Option<String> {
            None
        }
        fn protocol_reply(&mut self, bytes: &[u8]) {
            self.replies.extend_from_slice(bytes);
        }
    }
    let terminal = display(20, 4);
    terminal.feed_output(
        b"hi\x1b[6n\x1b_Ga=t,f=32,s=1,v=1,i=5;AQID/w==\x1b\\\x1b_Ga=p,i=5,q=2\x1b\\\x1b[6n",
    );
    let mut host = Host {
        replies: Vec::new(),
    };
    terminal.drain_events(&mut host);
    // The one-cell image leaves the cursor right after it, as in kitty.
    assert_eq!(host.replies, b"\x1b[1;3R\x1b_Gi=5;OK\x1b\\\x1b[1;4R");
    assert_eq!(terminal.kitty_graphics_placements().len(), 1);
}

#[test]
fn clipboard_reply_echoes_only_known_selection_targets() {
    struct Host {
        replies: Vec<u8>,
    }
    impl TerminalReplyHost for Host {
        fn load_clipboard(&mut self, _: TerminalClipboardTarget) -> Option<String> {
            Some("hello".to_owned())
        }
        fn protocol_reply(&mut self, bytes: &[u8]) {
            self.replies.extend_from_slice(bytes);
        }
    }
    let terminal = display(6, 2);
    terminal.feed_output("\x1b]52;zc\u{e9}31mcp;?\x1b\\".as_bytes());
    let mut host = Host {
        replies: Vec::new(),
    };
    terminal.drain_events(&mut host);
    assert_eq!(host.replies, b"\x1b]52;c31p;aGVsbG8=\x1b\\");
}

#[test]
fn event_payload_budget_counts_all_osc_payloads_and_reopens_after_drain() {
    const PAYLOAD_BYTES: usize = 64 * 1024;
    let terminal = display(6, 2);
    let packet_body = vec![b'x'; PAYLOAD_BYTES];
    {
        let mut state = terminal.shared.state();
        for index in 0..512 {
            // Spare capacity must count too: tiny strings can retain a
            // large allocation after an upstream parser trims their text.
            let mut text = String::with_capacity(PAYLOAD_BYTES);
            text.push('x');
            let event = match index % 5 {
                0 => PendingEvent::Terminal(TerminalEvent::Title(text)),
                1 => PendingEvent::Terminal(TerminalEvent::WorkingDirectory(text)),
                2 => PendingEvent::Terminal(TerminalEvent::ClipboardStore(text)),
                3 => PendingEvent::ClipboardLoad(text),
                _ => PendingEvent::Kitty(KittyClipboardOsc::from_body(
                    &packet_body,
                    crate::KittyClipboardOscTerminator::StringTerminator,
                )),
            };
            state.queue(event);
            assert!(state.event_payload_bytes <= MAX_EVENT_PAYLOAD_BYTES);
        }
        assert!(
            state.events.len() < 512,
            "overload must discard excess payloads"
        );
        assert_eq!(
            state.event_payload_bytes,
            state
                .events
                .iter()
                .map(PendingEvent::payload_bytes)
                .sum::<usize>()
        );
        let bytes = state.event_payload_bytes;
        state.queue(PendingEvent::Terminal(TerminalEvent::Title(
            String::with_capacity(MAX_EVENT_PAYLOAD_BYTES + 1),
        )));
        assert_eq!(state.event_payload_bytes, bytes);
    }
    let (_, more) = terminal.drain_events(&mut |_| None);
    assert!(!more);
    let mut state = terminal.shared.state();
    assert_eq!(state.event_payload_bytes, 0);
    assert!(state.events.is_empty());
    state.queue(PendingEvent::Terminal(TerminalEvent::Title(
        "accepted".into(),
    )));
    assert_eq!(state.events.len(), 1);
    assert_eq!(state.event_payload_bytes, "accepted".len());
}

#[test]
fn dropped_clipboard_chunk_cannot_commit_a_truncated_write() {
    #[derive(Default)]
    struct Host(Vec<crate::TerminalClipboardWriteRequest>);
    impl TerminalReplyHost for Host {
        fn load_clipboard(&mut self, _: TerminalClipboardTarget) -> Option<String> {
            None
        }
        fn write_clipboard(
            &mut self,
            request: crate::TerminalClipboardWriteRequest,
        ) -> crate::TerminalClipboardWriteResult {
            self.0.push(request);
            crate::TerminalClipboardWriteResult::Success {
                remember_permission: false,
            }
        }
    }
    for pressure in ["payload", "count", "compaction"] {
        let terminal = display(6, 2);
        let mut host = Host::default();
        terminal.feed_output(b"\x1b[?5522h\x1b]5522;type=write:id=partial\x1b\\\x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;aGVsbG8=\x1b\\");
        terminal.drain_events(&mut host);
        const MISSING_CHUNK: &[u8] = b"\x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;d29ybGQ=\x1b\\";
        if pressure == "compaction" {
            terminal.feed_output(MISSING_CHUNK);
        }
        {
            let mut state = terminal.shared.state();
            if pressure == "payload" {
                state.queue(PendingEvent::Terminal(TerminalEvent::Title(
                    String::with_capacity(MAX_EVENT_PAYLOAD_BYTES),
                )));
            } else {
                for _ in state.events.len()..MAX_EVENTS {
                    state.queue(PendingEvent::Terminal(TerminalEvent::Bell));
                }
                if pressure == "compaction" {
                    state.queue(PendingEvent::Terminal(TerminalEvent::Exit));
                }
            }
        }
        if pressure != "compaction" {
            terminal.feed_output(MISSING_CHUNK);
        }
        while terminal.drain_events(&mut host).1 {}
        terminal.feed_output(b"\x1b]5522;type=wdata;\x1b\\");
        terminal.drain_events(&mut host);
        assert!(
            host.0.is_empty(),
            "missing world chunk must abort the hello write under {pressure} pressure"
        );
        assert!(
            terminal
                .shared
                .clipboard
                .lock()
                .unwrap()
                .paste_notification(TerminalClipboardLocation::Clipboard, &["text/plain".into()])
                .is_some(),
            "overflow reset must preserve the host's paste mode"
        );
        terminal.feed_output(b"\x1b]5522;type=write:id=fresh\x1b\\\x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;ZnJlc2g=\x1b\\\x1b]5522;type=wdata;\x1b\\");
        terminal.drain_events(&mut host);
        assert_eq!(host.0.len(), 1);
        assert_eq!(host.0[0].contents[0].data, b"fresh");
    }
}

#[test]
fn saturated_event_queue_preserves_exit_and_clipboard_invalidation() {
    for invalidation in [
        KittyClipboardControl::Reset,
        KittyClipboardControl::Set(false),
    ] {
        let mut state = State::new(TerminalSize::default(), &TerminalRuntimeConfig::default());
        state.queue(PendingEvent::KittyControl(invalidation));
        state.queue(PendingEvent::KittyControl(KittyClipboardControl::Set(true)));
        for _ in state.events.len()..MAX_EVENTS {
            state.queue(PendingEvent::Terminal(TerminalEvent::Bell));
        }
        state.queue(PendingEvent::Terminal(TerminalEvent::Exit));
        assert_eq!(state.events.len(), 3);
        assert!(
            matches!(state.pop_event(), Some(PendingEvent::KittyControl(control)) if control == invalidation)
        );
        assert!(matches!(
            state.pop_event(),
            Some(PendingEvent::KittyControl(KittyClipboardControl::Set(true)))
        ));
        // Leave Exit queued and saturate again: a later reset must not
        // discard the process exit that the host has yet to observe.
        for _ in state.events.len()..MAX_EVENTS {
            state.queue(PendingEvent::Terminal(TerminalEvent::Bell));
        }
        state.queue(PendingEvent::KittyControl(KittyClipboardControl::Reset));
        assert!(matches!(
            state.pop_event(),
            Some(PendingEvent::Terminal(TerminalEvent::Exit))
        ));
        assert!(matches!(
            state.pop_event(),
            Some(PendingEvent::KittyControl(KittyClipboardControl::Reset))
        ));
        assert!(state.events.is_empty());
        assert_eq!(state.event_payload_bytes, 0);
    }
}

#[test]
fn display_reply_is_ready_when_the_wakeup_callback_runs() {
    struct Host(Arc<Mutex<Vec<u8>>>);
    impl TerminalReplyHost for Host {
        fn load_clipboard(&mut self, _: TerminalClipboardTarget) -> Option<String> {
            None
        }
        fn protocol_reply(&mut self, bytes: &[u8]) {
            self.0.lock().unwrap().extend_from_slice(bytes);
        }
    }
    let slot = Arc::new(Mutex::new(Weak::<CustomBackend>::new()));
    let replies = Arc::new(Mutex::new(Vec::new()));
    let callback_slot = slot.clone();
    let callback_replies = replies.clone();
    let terminal = Arc::new(CustomBackend::new_display_with_wakeup_notifier(
        TerminalSize::default(),
        None,
        Some(TerminalWakeupNotifier::new(move || {
            let terminal = callback_slot.lock().unwrap().upgrade().unwrap();
            terminal.drain_events(&mut Host(callback_replies.clone()));
        })),
    ));
    *slot.lock().unwrap() = Arc::downgrade(&terminal);
    terminal.feed_output(b"hi\x1b[6n");
    assert_eq!(*replies.lock().unwrap(), b"\x1b[1;3R");
    assert!(!terminal.has_pending_events());
}

#[test]
fn synchronized_output_commits_after_deadline_without_more_input() {
    let (send, receive) = flume::unbounded();
    let terminal = CustomBackend::new_display_with_wakeup_notifier(
        TerminalSize::default(),
        None,
        Some(TerminalWakeupNotifier::new(move || {
            let _ = send.send(());
        })),
    );
    terminal.render_read(true);
    terminal.feed_output(b"\x1b[?2026hdeadline");
    assert_eq!(terminal.snapshot().cells[0].char, ' ');
    receive
        .recv_timeout(Duration::from_secs(2))
        .expect("watchdog commits output");
    assert_eq!(terminal.snapshot().cells[0].char, 'd');
}

#[cfg(unix)]
#[test]
fn native_exit_commits_synchronized_tail_before_exit_event() {
    let terminal = CustomBackend::new_with_launch_and_wakeup_notifier(
        TerminalSize::default(),
        None,
        None,
        None,
        None,
        Some(&TerminalLaunch::Program {
            program: "/bin/sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "printf '\\033[?2026hfinal-tail'".to_owned(),
            ],
        }),
        None,
        ChildEnvironment::Process,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (events, _) = terminal.drain_events(&mut |_| None);
        if events
            .iter()
            .any(|event| matches!(event, TerminalEvent::Exit))
        {
            let text: String = terminal
                .snapshot()
                .cells
                .into_iter()
                .map(|cell| cell.char)
                .collect();
            assert!(text.starts_with("final-tail"));
            break;
        }
        assert!(Instant::now() < deadline, "child exited");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
#[test]
fn unqueueable_input_reports_one_dropped_input_event() {
    let terminal = CustomBackend::new_with_launch_and_wakeup_notifier(
        TerminalSize::default(),
        None,
        None,
        None,
        None,
        Some(&TerminalLaunch::Program {
            program: "/bin/sleep".to_owned(),
            args: vec!["5".to_owned()],
        }),
        None,
        ChildEnvironment::Process,
    )
    .unwrap();
    terminal.drain_events(&mut |_| None);
    // Larger than the whole input backlog, so it can never queue.
    let paste = vec![b'x'; 9 * 1024 * 1024];
    terminal.write(&paste);
    terminal.write_owned(paste);
    // Motion is dropped silently.
    terminal.write_mouse_motion(vec![b'm'; 9 * 1024 * 1024]);
    assert!(terminal.has_pending_events());
    let (events, _) = terminal.drain_events(&mut |_| None);
    let dropped = events
        .iter()
        .filter(|event| matches!(event, TerminalEvent::InputDropped))
        .count();
    assert_eq!(dropped, 1);
    let (events, _) = terminal.drain_events(&mut |_| None);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, TerminalEvent::InputDropped))
    );
}

#[test]
fn render_damage_reads_stay_coherent_while_output_streams() {
    fn replay(rows: &mut [Vec<TerminalRenderCell>], scrolls: &[TerminalViewportScroll]) -> bool {
        scrolls.iter().all(|scroll| {
            let valid = scroll.top <= scroll.bottom
                && scroll.bottom < rows.len()
                && (1..=scroll.bottom - scroll.top + 1).contains(&scroll.count);
            if valid {
                let region = &mut rows[scroll.top..=scroll.bottom];
                match scroll.direction {
                    TerminalViewportScrollDirection::Up => region.rotate_left(scroll.count),
                    TerminalViewportScrollDirection::Down => region.rotate_right(scroll.count),
                }
            }
            valid
        })
    }
    // A full read takes the damage with the cells, like the desktop's rebuild.
    fn full_rows(terminal: &CustomBackend) -> Vec<Vec<TerminalRenderCell>> {
        let read = terminal.render_read(true);
        read.cells
            .chunks(usize::from(read.metadata.cols))
            .map(<[TerminalRenderCell]>::to_vec)
            .collect()
    }

    let terminal = Arc::new(display(40, 8));
    let mut incremental = full_rows(&terminal);
    let feeder = {
        let terminal = Arc::clone(&terminal);
        std::thread::spawn(move || {
            for line in 0..1500 {
                // Pace the output so reads interleave with it.
                if line % 4 == 0 {
                    std::thread::sleep(Duration::from_micros(100));
                }
                let edit = if line % 7 == 0 {
                    "\x1b[3;1Hedit\x1b[8;1H"
                } else {
                    ""
                };
                terminal.feed_output(
                    format!("{edit}line {line} {}\r\n", "x".repeat(line % 30)).as_bytes(),
                );
            }
        })
    };
    let mut partial_reads = 0;
    loop {
        let finished = feeder.is_finished();
        let full = terminal.with_render_damage(|update, mut cells| match update.damage {
            TerminalDamageSnapshot::Full => true,
            TerminalDamageSnapshot::Partial(spans) => {
                assert!(replay(&mut incremental, &update.scrolls));
                assert!(cells.visit(&spans, |row, _, _, col, cell| {
                    incremental[row][col] = cell.clone();
                }));
                partial_reads += 1;
                false
            }
        });
        if full {
            incremental = full_rows(&terminal);
        }
        if finished {
            break;
        }
    }
    feeder.join().unwrap();
    assert!(partial_reads > 0);
    let mut fresh = Vec::new();
    terminal.visit_viewport_cells(|_, _, _, cell| fresh.push(cell.clone()));
    assert_eq!(incremental.concat(), fresh);
}

#[cfg(unix)]
#[test]
fn hidden_terminal_wakes_the_host_when_its_child_exits() {
    let (send, receive) = flume::unbounded();
    let terminal = CustomBackend::new_with_launch_and_wakeup_notifier(
        TerminalSize::default(),
        None,
        Some(TerminalWakeupNotifier::new(move || {
            let _ = send.send(());
        })),
        None,
        None,
        Some(&TerminalLaunch::Program {
            program: "/bin/sh".to_owned(),
            args: vec!["-c".to_owned(), "sleep 0.3; exit 3".to_owned()],
        }),
        None,
        ChildEnvironment::Process,
    )
    .unwrap();
    terminal.set_wakeup_enabled(false);
    let _ = terminal.drain_events(&mut |_| None);
    while receive.try_recv().is_ok() {}
    receive
        .recv_timeout(Duration::from_secs(5))
        .expect("the exit wakes a hidden terminal's host");
    let (events, _) = terminal.drain_events(&mut |_| None);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, TerminalEvent::Exit))
    );
}

#[cfg(unix)]
#[test]
fn pty_replies_reach_the_child_in_order() {
    let terminal = CustomBackend::new_with_launch_and_wakeup_notifier(
        TerminalSize::default(),
        None,
        None,
        None,
        None,
        Some(&TerminalLaunch::Program {
            program: "/bin/sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                // Replies are queued per feed slice straight to the transport.
                "stty raw -echo; printf '\\033[6nx\\033[6n'; dd bs=1 count=12 2>/dev/null | od -An -c | tr -d ' \\n'; printf '|done'; sleep 1".to_owned(),
            ],
        }),
        None,
        ChildEnvironment::Process,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let text: String = terminal
            .snapshot()
            .cells
            .into_iter()
            .map(|cell| cell.char)
            .collect();
        if text.contains("|done") {
            assert!(text.contains(r"033[1;1R033[1;2R|done"), "{text:?}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "child read its replies: {text:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn damage_cells_reject_spans_outside_the_viewport() {
    let terminal = CustomBackend::new_display(
        TerminalSize {
            cols: 5,
            rows: 3,
            ..TerminalSize::default()
        },
        None,
    );
    terminal.feed_output(b"abcde");
    let span = |row, left_col, right_col| TerminalDirtySpan {
        row,
        left_col,
        right_col,
    };
    terminal.with_render_damage(|_, mut cells| {
        let mut visited = 0;
        assert!(cells.visit(&[span(0, 1, 3)], |_, _, _, _, _| visited += 1));
        assert_eq!(visited, 3);
        for outside in [span(3, 0, 1), span(0, 2, 5), span(0, 3, 2)] {
            assert!(!cells.visit(&[span(0, 0, 0), outside], |_, _, _, _, _| visited += 1));
        }
        assert_eq!(visited, 3, "a rejected batch visits nothing");
    });
}

#[cfg(unix)]
#[test]
fn restored_output_precedes_the_new_sessions_first_output() {
    let terminal = CustomBackend::new_with_launch_and_wakeup_notifier(
        TerminalSize::default(),
        None,
        None,
        None,
        None,
        Some(&TerminalLaunch::Program {
            program: "/bin/sh".to_owned(),
            args: vec!["-c".to_owned(), "printf live; sleep 1".to_owned()],
        }),
        Some(b"saved\r\n".as_slice()),
        ChildEnvironment::Process,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = terminal.snapshot();
        let cols = usize::from(snapshot.cols);
        let row = |index: usize| -> String {
            snapshot.cells[index * cols..(index + 1) * cols]
                .iter()
                .map(|cell| cell.char)
                .collect::<String>()
                .trim_end()
                .to_owned()
        };
        if row(1) == "live" {
            assert_eq!(row(0), "saved");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "child printed: {:?}",
            (row(0), row(1))
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn reverse_video_swaps_the_resolved_default_colors() {
    let mut terminal = CustomBackend::new_display(TerminalSize::default(), None);
    let colors = TerminalQueryColors {
        foreground: TerminalColor { r: 1, g: 2, b: 3 },
        background: TerminalColor { r: 9, g: 8, b: 7 },
        ..TerminalQueryColors::default()
    };
    terminal.set_query_colors(colors);
    let before = terminal.palette();
    assert_eq!((before.foreground, before.background), (None, None));
    terminal.feed_output(b"\x1b[?5h");
    let reversed = terminal.palette();
    assert_eq!(reversed.foreground, Some(colors.background));
    assert_eq!(reversed.background, Some(colors.foreground));
    assert_ne!(reversed.revision, before.revision);
    // A program-set foreground is swapped as well.
    terminal.feed_output(b"\x1b]10;#abcdef\x07");
    assert_eq!(
        terminal.palette().background,
        Some(TerminalColor {
            r: 0xab,
            g: 0xcd,
            b: 0xef
        })
    );
    terminal.feed_output(b"\x1b[?5l");
    assert_eq!(terminal.palette().background, None);
}

// Spans left out because existing damage covers them must not change the
// normalized result.
#[test]
fn covered_additions_normalize_like_every_addition() {
    let span = |row, left_col, right_col| TerminalDirtySpan {
        row,
        left_col,
        right_col,
    };
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = |limit: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % limit as u64) as usize
    };
    for case in 0..2000 {
        let random = |next: &mut dyn FnMut(usize) -> usize, count| {
            (0..count)
                .map(|_| {
                    let left = next(12);
                    span(next(6), left, left + next(8))
                })
                .collect::<Vec<_>>()
        };
        let count = next(10);
        let mut spans = random(&mut next, count);
        // Half the cases start from normalized damage, as `take_damage`'s
        // usually is; the rest exercise the unsorted fallback.
        if case % 2 == 0 {
            normalize_spans(&mut spans);
        }
        let count = next(10);
        let extra = random(&mut next, count);
        let mut expected = spans.clone();
        expected.extend(extra.iter().copied());
        normalize_spans(&mut expected);
        add_spans(&mut spans, extra.iter().copied());
        assert_eq!(spans, expected, "case {case}");
    }
}

#[test]
fn restore_snapshot_drops_events_and_deadlines_from_the_replaced_state() {
    let source = display(10, 2);
    source.feed_output(b"restored");
    let snapshot = source.encode_snapshot();

    let terminal = display(10, 2);
    terminal.feed_output(b"\x07\x1b]2;stale\x07\x1b]52;c;aGk=\x07");
    terminal.shared.state().history_deadline = Some(Instant::now());
    terminal
        .restore_snapshot(&snapshot)
        .expect("restore snapshot");

    assert_eq!(terminal.shared.state().history_deadline, None);
    let (events, _) = terminal.drain_events(&mut |_| None);
    assert!(
        events.iter().all(|event| !matches!(
            event,
            TerminalEvent::Bell | TerminalEvent::Title(_) | TerminalEvent::ClipboardStore(_)
        )),
        "{events:?}"
    );
    assert_eq!(terminal.snapshot().cells[0].char, 'r');
}
