//! What views receive: frames, images, shell events and pane endings.

use super::*;

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
            environment: None,
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
                environment: None,
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
                environment: None,
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
        environment: None,
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

/// Audit I1: frames arrive as row deltas. Replaying the damage the client
/// reports on the previous frame, as the renderer does, reproduces every
/// frame, and scrolling output is reported as a scroll.
#[test]
fn frame_deltas_report_damage_a_renderer_can_replay() {
    let host = Host::new();
    let script = r#"
stty raw -echo
printf READY
dd bs=1 count=1 >/dev/null 2>/dev/null
i=0
while [ $i -lt 150 ]; do
  printf 'line %d\r\n' $i
  i=$((i+1))
  sleep 0.002
done
printf DONE
sleep 30
"#;
    let (id, terminal) = script_pane(&host, script, &[]);
    wait_for_text(&terminal, "READY");
    let full_rows = |terminal: &Terminal| -> Vec<Vec<TerminalRenderCell>> {
        let read = terminal.render_read(true);
        read.cells
            .chunks(usize::from(read.metadata.cols))
            .map(<[_]>::to_vec)
            .collect()
    };
    let mut rows = full_rows(&terminal);
    terminal.write(b"x");
    let (mut scrolls, mut partial) = (0, 0);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (full, done) = terminal.with_render_damage(|damage, mut cells| {
            let TerminalDamageSnapshot::Partial(spans) = damage.damage else {
                return (true, false);
            };
            partial += 1;
            for scroll in &damage.scrolls {
                scrolls += 1;
                let region = &mut rows[scroll.top..=scroll.bottom];
                match scroll.direction {
                    TerminalViewportScrollDirection::Up => region.rotate_left(scroll.count),
                    TerminalViewportScrollDirection::Down => region.rotate_right(scroll.count),
                }
            }
            assert!(cells.visit(&spans, |row, _, _, col, cell| {
                rows[row][col] = cell.clone();
            }));
            // The same frame, read in full.
            let all: Vec<_> = (0..rows.len())
                .map(|row| TerminalDirtySpan {
                    row,
                    left_col: 0,
                    right_col: rows[0].len() - 1,
                })
                .collect();
            let mut expected = rows.clone();
            assert!(cells.visit(&all, |row, _, _, col, cell| {
                expected[row][col] = cell.clone();
            }));
            assert!(rows == expected, "replayed damage differs from the frame");
            let text: String = rows
                .iter()
                .flatten()
                .map(|cell| cell.text.as_str())
                .collect();
            (false, text.contains("DONE"))
        });
        if full {
            rows = full_rows(&terminal);
        }
        if done {
            break;
        }
        assert!(Instant::now() < deadline, "output did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        partial > 0 && scrolls > 0,
        "{partial} partial reads, {scrolls} scrolls"
    );
    host.client.close(&id).unwrap();
}

/// Audit I7: a hidden view still gets events, but no frames until it is
/// shown again; then it catches up at once.
#[test]
fn a_hidden_view_gets_events_but_no_frames_until_shown() {
    let host = Host::new();
    let script = r#"
stty raw -echo
printf READY
dd bs=1 count=1 >/dev/null 2>/dev/null
printf 'HIDDEN\033]2;hidden-title\007'
sleep 30
"#;
    let (id, terminal) = script_pane(&host, script, &[]);
    wait_for_text(&terminal, "READY");
    wait_until_quiet(&terminal);
    terminal.set_wakeup_enabled(false);
    // The pause travels on the subscription, the key on the input
    // connection: wait until the host confirmed the pause.
    wait_until(|| terminal.frames_paused());
    terminal.write(b"x");
    let mut title = None;
    wait_until(|| {
        let (events, _) = terminal.drain_events(&mut |_| None);
        title = title
            .take()
            .or(events.into_iter().find_map(|event| match event {
                TerminalEvent::Title(title) => Some(title),
                _ => None,
            }));
        title.is_some()
    });
    assert_eq!(title.as_deref(), Some("hidden-title"));
    // A frame published with the title would follow it at once; give one
    // sent by mistake time to arrive before checking that none did.
    std::thread::sleep(Duration::from_millis(300));
    let screen: String = terminal
        .render_read(true)
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    assert!(!screen.contains("HIDDEN"), "a paused view received a frame");
    terminal.set_wakeup_enabled(true);
    wait_for_text(&terminal, "HIDDEN");
    assert!(!terminal.frames_paused());
    host.client.close(&id).unwrap();
}

fn is_disconnected_title(event: &TerminalEvent) -> bool {
    matches!(event, TerminalEvent::Title(title) if title.starts_with("Session disconnected"))
}

/// Audit B10: a pane that another client closes ends in its views with
/// `Exit`, instead of leaving a disconnected tab that swallows typing.
#[test]
fn a_pane_closed_by_another_client_ends_with_exit() {
    let host = Host::new();
    let (id, terminal) = shell_pane(&host, "read line");
    let other = SessionClient::connect(&host.root).unwrap();
    other.close(&id).unwrap();
    let events = events_until(&terminal, |event| matches!(event, TerminalEvent::Exit));
    std::thread::sleep(Duration::from_millis(300));
    let later = terminal.drain_events(&mut |_| None).0;
    assert!(
        !events.iter().chain(&later).any(is_disconnected_title),
        "{events:?} {later:?}"
    );
    assert!(host.client.attach(&id, None).is_err());
}

/// Audit B11: a hidden view, whose render wakeups are off, still wakes when
/// a command finishes (workspace attention) and when its pane ends.
#[test]
fn command_finish_and_exit_wake_a_hidden_view() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let host = Host::new();
    let wakeups = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakeups);
    let notifier = TerminalWakeupNotifier::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let script = "stty -echo; printf READY; read line; printf '\\033]133;D;0\\007'; read line";
    let (id, terminal) = host.client.create(shell(script), Some(notifier)).unwrap();
    wait_for_text(&terminal, "READY");
    wait_until_quiet(&terminal);
    terminal.set_wakeup_enabled(false);

    wakeups.store(0, Ordering::SeqCst);
    terminal.write(b"\n");
    wait_until(|| wakeups.load(Ordering::SeqCst) > 0);
    events_until(&terminal, |event| {
        matches!(event, TerminalEvent::ShellCommandFinished(Some(0)))
    });

    wakeups.store(0, Ordering::SeqCst);
    terminal.write(b"\n");
    wait_until(|| wakeups.load(Ordering::SeqCst) > 0);
    events_until(&terminal, |event| matches!(event, TerminalEvent::Exit));
    drop(terminal);
    host.client.close(&id).unwrap();
}
