use super::*;

#[test]
fn bounded_event_queue_retains_lifecycle_events_after_flooding() {
    let events = Arc::new(Mutex::new(VecDeque::from([Event::Wakeup, Event::Exit])));
    let wakeup_queued = AtomicBool::new(true);
    let wakeup_enabled = Arc::new(AtomicBool::new(false));
    let clipboard_request = ClipboardRequest {
        target: ClipboardTarget::Clipboard,
        selector: b'c',
        bell_terminated: false,
    };
    queue_events(
        &events,
        &wakeup_queued,
        &wakeup_enabled,
        None,
        [Event::ClipboardLoad(clipboard_request)]
            .into_iter()
            .chain([Event::ClipboardStore("preserved".to_string())])
            .chain([
                Event::Title("termy:tab:command:cargo test".to_string()),
                Event::ShellCommandStart,
                Event::ShellCommandExecuting,
                Event::ShellCommandFinished(Some(0)),
            ])
            .chain(std::iter::repeat_n(Event::Bell, MAX_QUEUED_EVENTS + 64))
            .chain([Event::Exit, Event::Wakeup]),
        true,
        false,
    );

    let queue = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(queue.len(), MAX_QUEUED_EVENTS);
    assert_eq!(
        queue
            .iter()
            .filter(|event| matches!(event, Event::Wakeup))
            .count(),
        1
    );
    assert_eq!(
        queue
            .iter()
            .filter(|event| matches!(event, Event::Exit))
            .count(),
        1
    );
    assert_eq!(
        queue
            .iter()
            .filter(|event| matches!(event, Event::ClipboardLoad(_)))
            .count(),
        1
    );
    assert_eq!(
        queue
            .iter()
            .filter(|event| matches!(event, Event::ClipboardStore(_)))
            .count(),
        1
    );
    assert!(queue.iter().any(|event| {
        matches!(event, Event::Title(title) if title == "termy:tab:command:cargo test")
    }));
    assert_eq!(
        queue
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    Event::ShellCommandStart
                        | Event::ShellCommandExecuting
                        | Event::ShellCommandFinished(_)
                )
            })
            .cloned()
            .collect::<Vec<_>>(),
        [
            Event::ShellCommandStart,
            Event::ShellCommandExecuting,
            Event::ShellCommandFinished(Some(0)),
        ]
    );
}

#[test]
fn bounded_event_queue_evicts_an_old_command_as_one_cycle() {
    let mut queue = VecDeque::with_capacity(MAX_QUEUED_EVENTS);
    for _ in 0..(MAX_QUEUED_EVENTS - 2) / 3 {
        queue.extend([
            Event::ShellCommandStart,
            Event::ShellCommandExecuting,
            Event::ShellCommandFinished(Some(0)),
        ]);
    }
    queue.extend([Event::Wakeup, Event::Exit]);
    assert_eq!(queue.len(), MAX_QUEUED_EVENTS);

    assert!(push_bounded_event(&mut queue, Event::ShellCommandStart));
    assert!(push_bounded_event(&mut queue, Event::ShellCommandExecuting));
    assert!(push_bounded_event(
        &mut queue,
        Event::ShellCommandFinished(Some(42))
    ));

    assert_eq!(queue.len(), MAX_QUEUED_EVENTS);
    let lifecycle = queue
        .iter()
        .filter(|event| event_priority(event) == EventPriority::Lifecycle)
        .collect::<Vec<_>>();
    let (cycles, remainder) = lifecycle.as_chunks::<3>();
    assert!(remainder.is_empty());
    assert!(cycles.iter().all(|cycle| {
        matches!(cycle[0], Event::ShellCommandStart)
            && matches!(cycle[1], Event::ShellCommandExecuting)
            && matches!(cycle[2], Event::ShellCommandFinished(_))
    }));
    assert!(matches!(
        lifecycle.last(),
        Some(Event::ShellCommandFinished(Some(42)))
    ));
}

#[test]
fn event_queue_coalesces_latest_state_updates() {
    let events = Arc::new(Mutex::new(VecDeque::new()));
    let wakeup_queued = AtomicBool::new(false);
    let wakeup_enabled = Arc::new(AtomicBool::new(false));
    queue_events(
        &events,
        &wakeup_queued,
        &wakeup_enabled,
        None,
        [
            Event::Title("old".to_string()),
            Event::Progress(Progress::InProgress(10)),
            Event::WorkingDirectory("/old".to_string()),
            Event::Title("new".to_string()),
            Event::ResetTitle,
            Event::Progress(Progress::InProgress(90)),
            Event::WorkingDirectory("/new".to_string()),
        ],
        false,
        false,
    );

    let queue = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        *queue,
        VecDeque::from([
            Event::ResetTitle,
            Event::Progress(Progress::InProgress(90)),
            Event::WorkingDirectory("/new".to_string()),
        ])
    );
}

#[test]
fn event_queue_preserves_state_updates_across_lifecycle_events() {
    let events = Arc::new(Mutex::new(VecDeque::new()));
    let wakeup_queued = AtomicBool::new(false);
    let wakeup_enabled = Arc::new(AtomicBool::new(false));
    queue_events(
        &events,
        &wakeup_queued,
        &wakeup_enabled,
        None,
        [
            Event::Title("termy:tab:command:cargo test".to_string()),
            Event::ShellCommandFinished(Some(0)),
            Event::Title("termy:tab:prompt:/repo".to_string()),
        ],
        false,
        false,
    );

    let queue = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        *queue,
        VecDeque::from([
            Event::Title("termy:tab:command:cargo test".to_string()),
            Event::ShellCommandFinished(Some(0)),
            Event::Title("termy:tab:prompt:/repo".to_string()),
        ])
    );
}

#[test]
fn event_queue_notifies_once_until_the_pending_wakeup_is_drained() {
    let events = Arc::new(Mutex::new(VecDeque::new()));
    let wakeup_queued = AtomicBool::new(false);
    let wakeup_enabled = Arc::new(AtomicBool::new(true));
    let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let notifier_notifications = notifications.clone();
    let notifier = WakeupNotifier::new(move || {
        notifier_notifications.fetch_add(1, Ordering::Relaxed);
    });

    for _ in 0..2 {
        queue_events(
            &events,
            &wakeup_queued,
            &wakeup_enabled,
            Some(&notifier),
            std::iter::empty(),
            true,
            true,
        );
    }
    assert_eq!(notifications.load(Ordering::Relaxed), 1);
    assert_eq!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        1
    );

    events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    wakeup_queued.store(false, Ordering::Release);
    queue_events(
        &events,
        &wakeup_queued,
        &wakeup_enabled,
        Some(&notifier),
        std::iter::empty(),
        true,
        true,
    );
    assert_eq!(notifications.load(Ordering::Relaxed), 2);
}

#[test]
fn hidden_terminal_notifies_once_for_exit() {
    let events = Arc::new(Mutex::new(VecDeque::new()));
    let wakeup_queued = AtomicBool::new(false);
    let wakeup_enabled = Arc::new(AtomicBool::new(false));
    let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let notifier_notifications = notifications.clone();
    let notifier = WakeupNotifier::new(move || {
        notifier_notifications.fetch_add(1, Ordering::Relaxed);
    });

    for _ in 0..2 {
        queue_events(
            &events,
            &wakeup_queued,
            &wakeup_enabled,
            Some(&notifier),
            [Event::Exit],
            false,
            true,
        );
    }

    assert_eq!(notifications.load(Ordering::Relaxed), 1);
    assert_eq!(
        *events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        VecDeque::from([Event::Exit])
    );
}

#[test]
fn hidden_terminal_notifies_for_clipboard_load() {
    let events = Arc::new(Mutex::new(VecDeque::new()));
    let wakeup_queued = AtomicBool::new(false);
    let wakeup_enabled = Arc::new(AtomicBool::new(false));
    let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let notifier_notifications = notifications.clone();
    let notifier = WakeupNotifier::new(move || {
        notifier_notifications.fetch_add(1, Ordering::Relaxed);
    });
    let request = ClipboardRequest {
        target: ClipboardTarget::Clipboard,
        selector: b'c',
        bell_terminated: false,
    };

    queue_events(
        &events,
        &wakeup_queued,
        &wakeup_enabled,
        Some(&notifier),
        [Event::ClipboardLoad(request)],
        false,
        true,
    );

    assert_eq!(notifications.load(Ordering::Relaxed), 1);
    assert!(matches!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .front(),
        Some(Event::ClipboardLoad(_))
    ));
}

#[test]
fn hidden_content_wakeup_is_signalled_once_when_reenabled() {
    let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let notifier_notifications = notifications.clone();
    let mut terminal = Terminal::new_display(Size::default(), Config::default());
    terminal.wakeup_notifier = Some(WakeupNotifier::new(move || {
        notifier_notifications.fetch_add(1, Ordering::Relaxed);
    }));

    terminal.set_wakeup_enabled(false);
    terminal.feed_output(b"content");
    assert_eq!(notifications.load(Ordering::Relaxed), 0);

    terminal.set_wakeup_enabled(true);
    terminal.set_wakeup_enabled(true);
    assert_eq!(notifications.load(Ordering::Relaxed), 1);
    assert_eq!(terminal.drain_events().0, vec![Event::Wakeup]);
}

#[test]
fn kitty_graphics_are_placed_at_the_cursor_when_the_command_arrives() {
    let terminal = Terminal::new_display(
        Size {
            cols: 8,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );

    terminal.feed_output(b"A\x1b_Ga=T,f=32,s=1,v=1,i=7,C=1;/wAA/w==\x1b\\B");

    let placements = terminal.kitty_graphics_placements();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].image_id, 7);
    assert_eq!(placements[0].col, 1);
    assert_eq!(placements[0].viewport_row, 0);
    let frame = terminal.snapshot();
    assert_eq!(frame.cells[0].character, 'A');
    assert_eq!(frame.cells[1].character, 'B');
}

#[test]
fn kitty_graphics_revision_does_not_churn_without_graphics() {
    let terminal = graphics_test_terminal(8);
    let revision = terminal.kitty_graphics_revision();

    terminal.feed_output(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
    terminal.resize(Size {
        cols: 6,
        rows: 3,
        ..terminal.size()
    });
    assert!(terminal.scroll_display(1));
    assert!(terminal.scroll_to_bottom());

    assert_eq!(terminal.kitty_graphics_revision(), revision);
}

#[test]
fn kitty_graphics_revision_bumps_once_for_an_effect_batch() {
    let terminal = graphics_test_terminal(8);
    terminal.feed_output(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=30,c=2,r=1,C=1;AQID/w==\x1b\\");
    let revision = terminal.kitty_graphics_revision();

    terminal.feed_output(b"\x1b[4;1H\n\n");

    assert_eq!(terminal.kitty_graphics_revision(), revision.wrapping_add(1));
}

#[test]
fn kitty_graphics_revision_and_visible_placements_are_atomic() {
    let terminal = graphics_test_terminal(8);
    assert_eq!(terminal.kitty_graphics_snapshot(), (0, Vec::new()));

    terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=31,c=2,r=1,C=1;AQID/w==\x1b\\");
    let (placed_revision, placed) = terminal.kitty_graphics_snapshot();
    assert!(placed_revision > 0);
    assert_eq!(placed.len(), 1);
    assert_eq!(placed_revision, terminal.kitty_graphics_revision());

    terminal.feed_output(b"\x1b[?1049h");
    let (alternate_revision, alternate) = terminal.kitty_graphics_snapshot();
    assert!(alternate_revision > placed_revision);
    assert!(alternate.is_empty());

    terminal.feed_output(b"\x1b[?1049l");
    let (restored_revision, restored) = terminal.kitty_graphics_snapshot();
    assert!(restored_revision > alternate_revision);
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].placement_serial, placed[0].placement_serial);

    terminal.feed_output(b"\x1b_Ga=d,d=a,q=1\x1b\\");
    let (deleted_revision, deleted) = terminal.kitty_graphics_snapshot();
    assert!(deleted_revision > restored_revision);
    assert!(deleted.is_empty());
}

fn graphics_test_terminal(scrollback_history: usize) -> Terminal {
    Terminal::new_display(
        Size {
            cols: 8,
            rows: 4,
            ..Size::default()
        },
        Config {
            scrollback_history,
            ..Config::default()
        },
    )
}

#[test]
fn kitty_graphics_follow_zero_and_full_scrollback() {
    for history_limit in [0, 1] {
        let terminal = graphics_test_terminal(history_limit);
        if history_limit == 1 {
            terminal.feed_output(b"\x1b[4;1H\n");
            assert_eq!(terminal.scroll_state().1, 1);
        }
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=10,c=2,r=1,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);

        terminal.feed_output(b"\x1b[4;1H\n");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);
        terminal.feed_output(b"\n");
        assert!(terminal.kitty_graphics_placements().is_empty());
    }
}

#[test]
fn kitty_graphics_follow_alternate_and_partial_region_scrolls() {
    let terminal = graphics_test_terminal(0);
    terminal
        .feed_output(b"\x1b[?1049h\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=11,c=2,r=1,C=1;AQID/w==\x1b\\");
    assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);
    terminal.feed_output(b"\x1b[4;1H\n");
    assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);
    terminal.feed_output(b"\n");
    assert!(terminal.kitty_graphics_placements().is_empty());

    terminal
        .feed_output(b"\x1b[?1049l\x1b[1;1H\x1b_Ga=T,f=32,s=1,v=1,i=12,c=2,r=1,C=1;AQID/w==\x1b\\");
    terminal.feed_output(b"\x1b[2;3r\x1b[3;1H\n");
    assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);
}

#[test]
fn kitty_graphics_clear_with_viewport_alt_entry_and_terminal_reset() {
    let terminal = graphics_test_terminal(8);
    terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=13,c=2,r=1,C=1;AQID/w==\x1b\\");
    terminal.feed_output(b"\x1b[?1049h");
    assert!(terminal.kitty_graphics_placements().is_empty());
    terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=14,c=2,r=1,C=1;AQID/w==\x1b\\");
    assert_eq!(terminal.kitty_graphics_placements().len(), 1);
    terminal.feed_output(b"\x1b[2J");
    assert!(terminal.kitty_graphics_placements().is_empty());

    terminal.feed_output(b"\x1b[?1049l");
    assert_eq!(terminal.kitty_graphics_placements()[0].image_id, 13);
    terminal.feed_output(b"\x1bc");
    assert!(terminal.kitty_graphics_placements().is_empty());
}

#[test]
fn kitty_graphics_cursor_advance_tracks_bottom_scrolls() {
    for history_limit in [0, 2, 8] {
        let terminal = graphics_test_terminal(history_limit);
        if history_limit == 2 {
            terminal.feed_output(b"\x1b[4;1H\n\n");
        }
        terminal.feed_output(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=15,c=2,r=3;AQID/w==\x1b\\");
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
        assert_eq!(terminal.cursor_position(), (2, 3));
    }
}

#[test]
fn kitty_graphics_revision_tracks_resize_geometry_and_visibility() {
    let terminal = graphics_test_terminal(8);
    terminal.feed_output(b"\x1b[2;7H\x1b_Ga=T,f=32,s=1,v=1,i=18,C=1;AQID/w==\x1b\\");
    let initial_revision = terminal.kitty_graphics_revision();

    terminal.resize(Size {
        cols: 6,
        ..terminal.size()
    });
    let width_revision = terminal.kitty_graphics_revision();
    assert_eq!(width_revision, initial_revision.wrapping_add(1));
    assert!(terminal.kitty_graphics_placements().is_empty());

    terminal.resize(Size {
        cols: 8,
        ..terminal.size()
    });
    let revealed_revision = terminal.kitty_graphics_revision();
    assert_eq!(revealed_revision, width_revision.wrapping_add(1));
    assert_eq!(terminal.kitty_graphics_placements().len(), 1);

    terminal.resize(Size {
        cell_width: 20.0,
        cell_height: 40.0,
        ..terminal.size()
    });
    assert_eq!(
        terminal.kitty_graphics_revision(),
        revealed_revision.wrapping_add(1)
    );
}

#[test]
fn kitty_graphics_revision_tracks_viewport_scrolling() {
    let terminal = graphics_test_terminal(8);
    terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=19,c=2,r=1,C=1;AQID/w==\x1b\\");
    terminal.feed_output(b"\x1b[4;1H\n\n");
    let revision = terminal.kitty_graphics_revision();
    assert!(terminal.kitty_graphics_placements().is_empty());

    assert!(terminal.scroll_display(1));
    let scrolled_revision = terminal.kitty_graphics_revision();
    assert_eq!(scrolled_revision, revision.wrapping_add(1));
    assert_eq!(terminal.kitty_graphics_placements().len(), 1);

    assert!(terminal.scroll_to_bottom());
    assert_eq!(
        terminal.kitty_graphics_revision(),
        scrolled_revision.wrapping_add(1)
    );
    assert!(terminal.kitty_graphics_placements().is_empty());
}

#[test]
fn kitty_graphics_follow_history_during_row_only_resizes() {
    let terminal = graphics_test_terminal(8);
    terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=16,c=2,r=1,C=1;AQID/w==\x1b\\");
    terminal.feed_output(b"\x1b[4;1H\n\n");
    assert_eq!(terminal.scroll_state().1, 2);
    assert!(terminal.kitty_graphics_placements().is_empty());

    terminal.resize(Size {
        rows: 6,
        ..terminal.size()
    });
    assert_eq!(terminal.scroll_state().1, 0);
    assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);

    terminal.resize(Size {
        rows: 2,
        ..terminal.size()
    });
    assert_eq!(terminal.scroll_state().1, 4);
    assert!(terminal.kitty_graphics_placements().is_empty());
}

#[test]
fn width_reflow_preserves_placements_and_reusable_image_data() {
    let terminal = graphics_test_terminal(8);
    terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=17,c=2,r=1,C=1,q=1;AQID/w==\x1b\\");
    let original = terminal.kitty_graphics_placements();
    assert_eq!(original.len(), 1);
    let original_serial = original[0].placement_serial;

    terminal.resize(Size {
        cols: 5,
        ..terminal.size()
    });
    let resized = terminal.kitty_graphics_placements();
    assert_eq!(resized.len(), 1);
    assert_eq!(resized[0].placement_serial, original_serial);

    terminal.feed_output(b"\x1b_Ga=p,i=17,c=1,r=1,C=1,q=1\x1b\\");
    let placements = terminal.kitty_graphics_placements();
    assert_eq!(placements.len(), 2);
    assert!(placements.iter().all(|placement| placement.image_id == 17));
    assert!(
        placements
            .iter()
            .any(|placement| placement.placement_serial == original_serial)
    );
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
#[test]
fn unix_pty_streams_child_output_into_the_tmon_grid() {
    use std::{sync::mpsc, time::Duration};

    let (notify_tx, notify_rx) = mpsc::channel();
    let terminal = Terminal::new(
        Size {
            cols: 16,
            rows: 2,
            ..Size::default()
        },
        Config {
            launch: Some(Launch::ShellCommand("printf tmon-pty".to_string())),
            ..Config::default()
        },
        Some(WakeupNotifier::new(move || {
            let _ = notify_tx.send(());
        })),
    )
    .expect("tmon should start a Unix PTY");

    let mut exited = false;
    for _ in 0..20 {
        let _ = notify_rx.recv_timeout(Duration::from_millis(100));
        let (events, _) = terminal.drain_events();
        exited |= events.contains(&Event::Exit);
        let text = terminal
            .snapshot()
            .cells
            .iter()
            .map(|cell| cell.character)
            .collect::<String>();
        if exited && text.contains("tmon-pty") {
            return;
        }
    }
    panic!("PTY output or exit event did not arrive in time");
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
#[test]
fn unix_pty_processes_owned_input_through_the_writer_thread() {
    use std::{sync::mpsc, time::Duration};

    let (notify_tx, notify_rx) = mpsc::channel();
    let terminal = Terminal::new(
        Size {
            cols: 32,
            rows: 3,
            ..Size::default()
        },
        Config {
            launch: Some(Launch::ShellCommand(
                "IFS= read -r line; printf 'reply:%s' \"$line\"".to_string(),
            )),
            ..Config::default()
        },
        Some(WakeupNotifier::new(move || {
            let _ = notify_tx.send(());
        })),
    )
    .expect("tmon should start an interactive Unix PTY");

    terminal.write_owned(b"hello\n".to_vec());
    for _ in 0..20 {
        let _ = notify_rx.recv_timeout(Duration::from_millis(100));
        let (events, _) = terminal.drain_events();
        let text = terminal
            .snapshot()
            .cells
            .iter()
            .map(|cell| cell.character)
            .collect::<String>();
        if events.contains(&Event::Exit) {
            assert!(
                text.contains("reply:hello"),
                "PTY reply was missing: {text:?}"
            );
            return;
        }
    }
    panic!("PTY input reply or exit event did not arrive in time");
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
#[test]
fn unix_pty_honors_the_configured_working_directory() {
    use std::{sync::mpsc, time::Duration};

    let working_directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("manifest directory should exist");
    let (notify_tx, notify_rx) = mpsc::channel();
    let terminal = Terminal::new(
        Size {
            cols: 256,
            rows: 2,
            ..Size::default()
        },
        Config {
            working_directory: Some(working_directory.clone()),
            launch: Some(Launch::ShellCommand("pwd".to_string())),
            ..Config::default()
        },
        Some(WakeupNotifier::new(move || {
            let _ = notify_tx.send(());
        })),
    )
    .expect("tmon should start a Unix PTY");

    for _ in 0..20 {
        let _ = notify_rx.recv_timeout(Duration::from_millis(100));
        let (events, _) = terminal.drain_events();
        let text = terminal
            .snapshot()
            .cells
            .iter()
            .map(|cell| cell.character)
            .collect::<String>();
        if events.contains(&Event::Exit) {
            assert!(
                text.contains(&working_directory.to_string_lossy().to_string()),
                "PTY reported a different working directory: {text:?}"
            );
            return;
        }
    }
    panic!("PTY working-directory output or exit event did not arrive in time");
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
#[test]
fn unix_pty_reports_chdir_failure_synchronously() {
    let missing_directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!(".missing-tmon-cwd-{}", std::process::id()));
    assert!(!missing_directory.exists());
    let error = Terminal::new(
        Size::default(),
        Config {
            working_directory: Some(missing_directory),
            launch: Some(Launch::ShellCommand("printf should-not-run".to_string())),
            ..Config::default()
        },
        None,
    )
    .err()
    .expect("a missing child working directory should fail the spawn handshake");
    assert!(
        error
            .to_string()
            .contains("failed to change terminal working directory"),
        "unexpected PTY spawn error: {error}"
    );
}
