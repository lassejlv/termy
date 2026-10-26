use super::*;

#[test]
fn subscribing_wakes_a_worker_that_refreshed_while_detached() {
    let terminal = Terminal::new_display(TerminalSize::default(), None);
    let state = Arc::new(RemoteState::capture(&terminal));
    let shared = Arc::new(Mutex::new(Shared {
        info: PaneInfo {
            id: "test-pane".into(),
            child_pid: None,
            title: None,
            working_directory: None,
            exited: false,
            shell: None,
        },
        launch: LaunchConfig {
            config: TerminalRuntimeConfig::default(),
            shell_integration: None,
        },
        input: InputTracker::default(),
        state: Arc::clone(&state),
        sticky_events: Vec::new(),
        program_status: None,
        subscribers: Vec::new(),
        alive: true,
    }));
    let (tx, rx) = flume::bounded(256);
    let pane = Pane {
        tx,
        shared,
        exit_hook: Arc::default(),
        worker: Arc::new(Mutex::new(None)),
    };
    let worker = std::thread::spawn(move || {
        let Work::Refresh(reply) = rx.recv_timeout(REQUEST_TIMEOUT).unwrap() else {
            panic!("subscription must first refresh the pane");
        };
        reply.send(state).unwrap();
        // The worker has no subscribers yet and can go idle. Registration
        // must wake it even if no more terminal output arrives afterward.
        assert!(matches!(rx.recv_timeout(REQUEST_TIMEOUT), Ok(Work::Wake)));
    });
    let subscription = pane.subscribe(true).unwrap();
    worker.join().unwrap();
    assert!(matches!(subscription.next(), Some(Update::State(_))));
}

#[test]
fn bursts_of_bells_and_clipboard_writes_coalesce_for_slow_clients() {
    let terminal = Terminal::new_display(TerminalSize::default(), None);
    let shared = Arc::new(Mutex::new(Shared {
        info: PaneInfo {
            id: "test-pane".into(),
            child_pid: None,
            title: None,
            working_directory: None,
            exited: false,
            shell: None,
        },
        launch: LaunchConfig {
            config: TerminalRuntimeConfig::default(),
            shell_integration: None,
        },
        input: InputTracker::default(),
        state: Arc::new(RemoteState::capture(&terminal)),
        sticky_events: Vec::new(),
        program_status: None,
        subscribers: Vec::new(),
        alive: true,
    }));
    let subscription = Arc::new(Subscription {
        pending: Mutex::new(Pending::default()),
        ready: Condvar::new(),
        shared: Arc::downgrade(&shared),
        host_capable: true,
        worker: flume::bounded(1).0.downgrade(),
    });
    let mut state = shared.lock().unwrap();
    state.subscribers.push(Arc::downgrade(&subscription));

    // `printf '\a%.0s' {1..300}` arrives as one drain; a client that has
    // not read yet then sees more bursts, clipboard writes and dropped input.
    publish(&mut state, None, vec![TerminalEvent::Bell; 300]);
    for round in 0..300 {
        publish(
            &mut state,
            None,
            vec![
                TerminalEvent::Bell,
                TerminalEvent::InputDropped,
                TerminalEvent::ClipboardStore(format!("copy {round}")),
            ],
        );
    }
    assert_eq!(state.subscribers.len(), 1, "subscriber must stay attached");
    drop(state);

    let Some(Update::Events(events)) = subscription.next() else {
        panic!("expected coalesced events");
    };
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(matches!(events[0], TerminalEvent::Bell));
    assert!(matches!(events[1], TerminalEvent::InputDropped));
    assert!(matches!(&events[2], TerminalEvent::ClipboardStore(text) if text == "copy 299"));
}

/// A pane whose worker answers refreshes until the pane is dropped.
fn detached_pane() -> Pane {
    let terminal = Terminal::new_display(TerminalSize::default(), None);
    let state = Arc::new(RemoteState::capture(&terminal));
    let shared = Arc::new(Mutex::new(Shared {
        info: PaneInfo {
            id: "test-pane".into(),
            child_pid: None,
            title: None,
            working_directory: None,
            exited: false,
            shell: None,
        },
        launch: LaunchConfig {
            config: TerminalRuntimeConfig::default(),
            shell_integration: None,
        },
        input: InputTracker::default(),
        state: Arc::clone(&state),
        sticky_events: Vec::new(),
        program_status: None,
        subscribers: Vec::new(),
        alive: true,
    }));
    let (tx, rx) = flume::bounded(256);
    std::thread::spawn(move || {
        while let Ok(work) = rx.recv() {
            if let Work::Refresh(reply) = work {
                let _ = reply.send(Arc::clone(&state));
            }
        }
    });
    Pane {
        tx,
        shared,
        exit_hook: Arc::default(),
        worker: Arc::new(Mutex::new(None)),
    }
}

fn publish_to(pane: &Pane, events: Vec<TerminalEvent>) {
    publish(&mut pane.shared.lock().unwrap(), None, events);
}

/// The next update other than a frame.
fn next_events(subscription: &Subscription) -> Update {
    loop {
        match subscription.next() {
            Some(Update::State(_)) => {}
            Some(update) => return update,
            None => panic!("subscription closed"),
        }
    }
}

#[test]
fn command_finish_and_prompt_in_one_drain_reach_a_subscriber_in_order() {
    let pane = detached_pane();
    let subscription = pane.subscribe(true).unwrap();
    publish_to(
        &pane,
        vec![
            TerminalEvent::ShellCommandFinished(Some(1)),
            TerminalEvent::ShellPromptStart,
        ],
    );
    let Update::Events(events) = next_events(&subscription) else {
        panic!("expected live events");
    };
    assert!(
        matches!(
            events.as_slice(),
            [
                TerminalEvent::ShellCommandFinished(Some(1)),
                TerminalEvent::ShellPromptStart
            ]
        ),
        "{events:?}"
    );
}

#[test]
fn a_hidden_view_stays_subscribed_through_a_flood_of_prompts() {
    let pane = detached_pane();
    let subscription = pane.subscribe(true).unwrap();
    for code in 0..10_000 {
        publish_to(
            &pane,
            vec![
                TerminalEvent::ShellCommandExecuting,
                TerminalEvent::ShellCommandFinished(Some(code)),
                TerminalEvent::ShellPromptStart,
            ],
        );
    }
    assert_eq!(pane.shared.lock().unwrap().subscribers.len(), 1);
    let Update::Events(events) = next_events(&subscription) else {
        panic!("expected live events");
    };
    assert!(events.len() <= 64, "{}", events.len());
    assert!(matches!(
        &events[events.len() - 2..],
        [
            TerminalEvent::ShellCommandFinished(Some(9_999)),
            TerminalEvent::ShellPromptStart
        ]
    ));
}

#[test]
fn reattach_replays_current_state_as_a_marked_seed() {
    let pane = detached_pane();
    publish_to(
        &pane,
        vec![
            TerminalEvent::Title("vim".into()),
            TerminalEvent::Bell,
            TerminalEvent::ShellCommandExecuting,
            TerminalEvent::ShellCommandFinished(Some(2)),
            TerminalEvent::ShellPromptStart,
            TerminalEvent::ShellCommandStart,
            TerminalEvent::ShellCommandExecuting,
            TerminalEvent::Progress(ProgressState::InProgress(40)),
        ],
    );
    let subscription = pane.subscribe(true).unwrap();
    publish_to(&pane, vec![TerminalEvent::Bell]);
    let Update::Replay(replay) = next_events(&subscription) else {
        panic!("the seed must be marked as a replay");
    };
    assert!(
        matches!(
            replay.as_slice(),
            [
                TerminalEvent::Title(_),
                TerminalEvent::ShellCommandExecuting,
                TerminalEvent::Progress(ProgressState::InProgress(40)),
            ]
        ),
        "{replay:?}"
    );
    let Update::Events(live) = next_events(&subscription) else {
        panic!("expected live events after the replay");
    };
    assert!(matches!(live.as_slice(), [TerminalEvent::Bell]));
}

#[test]
fn prompt_start_clears_the_replayed_progress_and_shell_phase_follows_finish() {
    let pane = detached_pane();
    publish_to(
        &pane,
        vec![
            TerminalEvent::ShellCommandExecuting,
            TerminalEvent::Progress(ProgressState::Indeterminate),
            TerminalEvent::ShellCommandFinished(None),
        ],
    );
    let replay = seed(&pane, SystemTime::now());
    // The command finished: neither it nor its start is replayed.
    assert!(
        matches!(
            replay.as_slice(),
            [TerminalEvent::Progress(ProgressState::Indeterminate)]
        ),
        "{replay:?}"
    );
    publish_to(&pane, vec![TerminalEvent::ShellPromptStart]);
    let replay = seed(&pane, SystemTime::now());
    assert!(
        matches!(replay.as_slice(), [TerminalEvent::ShellPromptStart]),
        "{replay:?}"
    );
}

fn host_requests(subscription: &Subscription, count: usize) -> Vec<u64> {
    (0..count)
        .map(|_| match next_events(subscription) {
            Update::HostRequest { id, .. } => id,
            _ => panic!("expected a clipboard request"),
        })
        .collect()
}

#[test]
fn clipboard_requests_go_to_the_newest_view_and_are_denied_when_it_leaves() {
    let pane = detached_pane();
    let older_view = pane.subscribe(true).unwrap();
    let view = pane.subscribe(true).unwrap();
    // A script subscribes last; it cannot ask the user.
    let script = pane.subscribe(false).unwrap();
    let mut bridge = HostBridge {
        shared: Arc::clone(&pane.shared),
        timeout: HOST_REQUEST_TIMEOUT,
        parked: Vec::new(),
    };
    let load = || RemoteHostRequest::Load(TerminalClipboardTarget::Clipboard);
    for id in [7, 8] {
        assert!(bridge.defer_clipboard_request(id, load()).is_ok());
    }
    assert!(script.pending.lock().unwrap().host.is_empty());
    assert!(older_view.pending.lock().unwrap().host.is_empty());
    // First in, first out.
    assert_eq!(host_requests(&view, 2), [7, 8]);
    assert!(bridge.expired().is_empty());
    assert!(bridge.next_check() <= HOST_CHECK_INTERVAL);

    // The view leaves: its requests are denied, and new ones go to the
    // older view that is still open.
    view.close();
    assert_eq!(bridge.expired(), [7, 8]);
    assert!(!bridge.unpark(7), "a late answer must not be written");
    assert!(bridge.defer_clipboard_request(9, load()).is_ok());
    assert_eq!(host_requests(&older_view, 1), [9]);
    older_view.close();
    assert!(
        bridge.defer_clipboard_request(10, load()).is_err(),
        "with no view to ask, the request is denied at once"
    );
    assert!(bridge.unpark(9));
    assert_eq!(bridge.next_check(), Duration::MAX);
}

#[test]
fn expired_clipboard_requests_are_not_sent() {
    let pane = detached_pane();
    let view = pane.subscribe(true).unwrap();
    let now = Instant::now();
    let load = RemoteHostRequest::Load(TerminalClipboardTarget::Clipboard);
    let mut pending = view.pending.lock().unwrap();
    for (id, deadline) in [
        (1, now + HOST_REQUEST_TIMEOUT),
        (2, now),
        (3, now + HOST_REQUEST_TIMEOUT),
    ] {
        pending.host.push_back((id, load.clone(), deadline));
    }
    drop(pending);
    assert_eq!(host_requests(&view, 2), [1, 3]);
}

/// Audit B7: the worker keeps running while a clipboard request waits,
/// and denies it at its deadline when nobody answers.
#[cfg(unix)]
#[test]
fn an_unanswered_clipboard_request_is_denied_at_its_deadline() {
    let temp = tempfile::tempdir().unwrap();
    let reply = temp.path().join("reply");
    let expected = b"\x1b]5522;type=read:status=EPERM:id=late\x1b\\";
    let script = format!(
        "stty raw -echo; dd bs=1 count=1 >/dev/null 2>/dev/null; \
             printf '\\033]5522;type=read:id=late;dGV4dC9wbGFpbg==\\033\\\\'; \
             dd bs=1 count={} > '{}' 2>/dev/null; sleep 30",
        expected.len(),
        reply.display()
    );
    let timeout = Duration::from_millis(500);
    let pane = Pane::spawn(
        PaneLaunch {
            size: TerminalSize::default(),
            working_directory: None,
            shell_integration: None,
            config: TerminalRuntimeConfig::default(),
            launch: Some(TerminalLaunch::Program {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), script],
            }),
            environment: None,
        },
        &std::env::temp_dir(),
        timeout,
    )
    .unwrap();
    let view = pane.subscribe(true).unwrap();
    // Before the request exists, so its deadline cannot precede this.
    let sent = Instant::now();
    pane.command(RemoteCommand::Write(b"x".to_vec())).unwrap();
    let [id] = host_requests(&view, 1)[..] else {
        unreachable!()
    };
    // Other work is still answered meanwhile.
    pane.snapshot().unwrap();
    let deadline = sent + Duration::from_secs(5);
    while std::fs::read(&reply).map_or(true, |bytes| bytes.len() < expected.len()) {
        assert!(Instant::now() < deadline, "the request was never denied");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(sent.elapsed() >= timeout);
    assert_eq!(std::fs::read(&reply).unwrap(), expected);
    // A late answer is dropped.
    let late = RemoteHostReply::Read(TerminalClipboardReadResult::Denied);
    pane.host_reply(id, late).unwrap();
    pane.close().unwrap();
}

#[test]
fn a_closed_subscription_still_delivers_its_queued_events() {
    let pane = detached_pane();
    let subscription = pane.subscribe(true).unwrap();
    publish_to(&pane, vec![TerminalEvent::Exit]);
    subscription.close();
    assert!(matches!(
        subscription.next(),
        Some(Update::Events(events)) if matches!(events.as_slice(), [TerminalEvent::Exit])
    ));
    assert!(subscription.next().is_none());
}

#[cfg(unix)]
fn sleeping_pane() -> Pane {
    Pane::create(
        PaneLaunch {
            size: TerminalSize::default(),
            working_directory: None,
            shell_integration: None,
            config: TerminalRuntimeConfig::default(),
            launch: Some(TerminalLaunch::Program {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "sleep 30".into()],
            }),
            environment: None,
        },
        &std::env::temp_dir(),
    )
    .unwrap()
}

/// Updates until the subscription ends, without frames.
#[cfg(unix)]
fn remaining_events(subscription: &Subscription) -> Vec<TerminalEvent> {
    let mut events = Vec::new();
    while let Some(update) = subscription.next() {
        if let Update::Events(batch) | Update::Replay(batch) = update {
            events.extend(batch);
        }
    }
    events
}

/// Audit B10: a pane closed by another client, or at host shutdown,
/// tells its subscribers it ended, and cannot be subscribed to again.
#[cfg(unix)]
#[test]
fn closing_a_pane_sends_exit_and_refuses_new_subscriptions() {
    let pane = sleeping_pane();
    let subscription = pane.subscribe(true).unwrap();
    pane.close().unwrap();
    let events = remaining_events(&subscription);
    assert!(
        matches!(events.last(), Some(TerminalEvent::Exit)),
        "{events:?}"
    );
    assert!(pane.info().exited);
    let error = pane
        .subscribe(true)
        .err()
        .expect("a closed pane was subscribed");
    assert!(error.to_string().contains("no longer exists"), "{error:#}");
    // A pane that is already gone needs no second close.
    pane.close().unwrap();
}

/// Review finding 2: at host shutdown a pane tells its subscribers the
/// host is stopping instead of reporting that its program exited.
#[cfg(unix)]
#[test]
fn a_host_shutdown_is_not_reported_as_an_exit() {
    let pane = sleeping_pane();
    let subscription = pane.subscribe(true).unwrap();
    pane.shutdown();
    let mut updates = Vec::new();
    while let Some(update) = subscription.next() {
        match update {
            Update::Events(events) => assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, TerminalEvent::Exit))
            ),
            Update::HostShutdown => updates.push("shutdown"),
            _ => {}
        }
    }
    assert_eq!(updates, ["shutdown"]);
    assert!(pane.info().exited);
    assert!(pane.subscribe(true).is_err());
}

/// Audit I10: a panic in the worker ends the pane like an exit of its
/// process, instead of leaving it listed as alive with no `Exit`.
#[cfg(unix)]
#[test]
fn a_panicking_worker_ends_the_pane() {
    let pane = sleeping_pane();
    let (exited, exit) = flume::bounded(1);
    pane.on_exit(move |attached| {
        let _ = exited.send(attached);
    });
    let subscription = pane.subscribe(true).unwrap();
    pane.tx.send(Work::Panic).unwrap();
    let events = remaining_events(&subscription);
    assert!(
        matches!(events.last(), Some(TerminalEvent::Exit)),
        "{events:?}"
    );
    assert_eq!(exit.recv_timeout(REQUEST_TIMEOUT), Ok(true));
    assert!(pane.info().exited);
    assert!(pane.subscribe(true).is_err());
    assert!(pane.refresh().is_err());
}

/// Audit B14: an animation deadline inside the frame interval, or one
/// already past while the interval runs, must not make the worker poll.
#[test]
fn animation_deadlines_inside_the_frame_interval_do_not_spin_the_worker() {
    let now = Instant::now();
    let just_captured = now;
    for deadline in [
        now - Duration::from_millis(1),
        now + Duration::from_millis(2),
    ] {
        assert_eq!(
            worker_timeout(false, false, Some(deadline), just_captured, now),
            FRAME_INTERVAL
        );
    }
    // A later deadline is kept, and a due frame is not delayed.
    let later = now + FRAME_INTERVAL * 3;
    assert_eq!(
        worker_timeout(false, false, Some(later), just_captured, now),
        FRAME_INTERVAL * 3
    );
    let long_ago = now - FRAME_INTERVAL * 2;
    assert_eq!(
        worker_timeout(false, false, Some(now), long_ago, now),
        Duration::ZERO
    );
    assert_eq!(
        worker_timeout(false, true, None, just_captured, now),
        FRAME_INTERVAL
    );
    assert_eq!(
        worker_timeout(true, true, None, just_captured, now),
        Duration::ZERO
    );
}

/// A pane running `script` with `/bin/sh -c`.
#[cfg(unix)]
fn script_pane(script: &str) -> Pane {
    Pane::spawn(
        PaneLaunch {
            size: TerminalSize::default(),
            working_directory: None,
            shell_integration: None,
            config: TerminalRuntimeConfig::default(),
            launch: Some(TerminalLaunch::Program {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), script.into()],
            }),
            environment: None,
        },
        &std::env::temp_dir(),
        HOST_REQUEST_TIMEOUT,
    )
    .unwrap()
}

#[cfg(unix)]
fn screen(state: &RemoteState) -> String {
    state
        .render
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect()
}

/// The next frame, skipping events.
#[cfg(unix)]
fn next_frame(view: &Subscription) -> Arc<RemoteState> {
    loop {
        match view.next() {
            Some(Update::State(state)) => return state,
            Some(_) => {}
            None => panic!("subscription closed"),
        }
    }
}

/// Audit I7: a paused view gets events but no frames; on resume it gets
/// one frame captured then, never the one from before it was hidden.
#[cfg(unix)]
#[test]
fn a_paused_view_gets_events_and_only_a_fresh_frame_on_resume() {
    let pane = script_pane("stty -echo; printf A; read x; printf 'B\\033]2;hidden\\007'; sleep 30");
    let view = pane.subscribe(true).unwrap();
    while !screen(&next_frame(&view)).contains('A') {}
    view.set_paused(true);
    assert!(matches!(view.next(), Some(Update::Paused(true))));
    assert!(!pane.shared.lock().unwrap().wants_frames());
    pane.command(RemoteCommand::Write(b"x\n".to_vec())).unwrap();
    loop {
        match view.next() {
            Some(Update::Events(events))
                if events
                    .iter()
                    .any(|event| matches!(event, TerminalEvent::Title(_))) =>
            {
                break;
            }
            Some(Update::State(_)) => panic!("a paused view received a frame"),
            Some(_) => {}
            None => panic!("subscription closed"),
        }
    }
    assert!(!view.pending.lock().unwrap().state);
    view.set_paused(false);
    assert!(matches!(view.next(), Some(Update::Paused(false))));
    let Some(Update::State(state)) = view.next() else {
        panic!("expected the frame captured on resume");
    };
    assert!(screen(&state).contains("AB"), "{}", screen(&state));
    pane.close().unwrap();
}

/// Audit I1: a view that lost track of its frames gets the next one, and
/// the server loop learns to send it in full.
#[test]
fn a_resync_sends_a_frame_and_is_reported_once() {
    let pane = detached_pane();
    let view = pane.subscribe(true).unwrap();
    assert!(matches!(view.next(), Some(Update::State(_))));
    assert!(!view.take_resync());
    view.resync();
    assert!(matches!(view.next(), Some(Update::State(_))));
    assert!(view.take_resync());
    assert!(!view.take_resync());
}

/// Audit B14: a worker pass with no frame and no events must not wake
/// the subscriber threads.
#[test]
fn publishing_nothing_leaves_subscribers_alone() {
    let pane = detached_pane();
    let subscription = pane.subscribe(true).unwrap();
    // Holding a subscriber's queue blocks any publish that touches it.
    let held = subscription.pending.lock().unwrap();
    let shared = Arc::clone(&pane.shared);
    let (done, published) = flume::bounded(1);
    std::thread::spawn(move || {
        publish(
            &mut shared.lock().unwrap(),
            None,
            vec![TerminalEvent::Wakeup],
        );
        let _ = done.send(());
    });
    let result = published.recv_timeout(Duration::from_secs(5));
    drop(held);
    assert!(result.is_ok(), "an empty publish woke the subscribers");
}

/// Audit I8: a capture identical to the published state is not sent.
#[test]
fn an_unchanged_capture_is_not_published_again() {
    let terminal = Terminal::new_display(TerminalSize::default(), None);
    let pane = detached_pane();
    pane.shared.lock().unwrap().state = Arc::new(RemoteState::capture(&terminal));
    let subscription = pane.subscribe(true).unwrap();
    assert!(matches!(subscription.next(), Some(Update::State(_))));
    let republish = |terminal: &Terminal| {
        publish(
            &mut pane.shared.lock().unwrap(),
            Some(Arc::new(RemoteState::capture(terminal))),
            Vec::new(),
        );
        std::mem::take(&mut subscription.pending.lock().unwrap().state)
    };
    assert!(!republish(&terminal), "an identical frame was published");
    terminal.feed_output(b"x");
    assert!(republish(&terminal));
    assert!(!republish(&terminal));
}

/// Audit B22: hovering over a program that stopped reading fills at most
/// half of the input backlog, so a key typed afterwards still queues and
/// nothing reports dropped input.
#[cfg(unix)]
#[test]
fn motion_over_a_stalled_program_leaves_room_for_keys() {
    // Raw mode, so a full input queue blocks the writer instead of the
    // line discipline discarding characters; READY says it is set.
    let stalled = || {
        let pane = script_pane("stty raw -echo; printf READY; exec sleep 30");
        let view = pane.subscribe(true).unwrap();
        while !screen(&next_frame(&view)).contains("READY") {}
        (pane, view)
    };
    // 6 MB in 6000 reports: far more than the PTY buffers, and more
    // entries than the whole input backlog holds (4096).
    let flood = |pane: &Pane, command: fn(Vec<u8>) -> RemoteCommand| {
        for _ in 0..6000 {
            pane.command(command(vec![b'm'; 1024])).unwrap();
        }
    };
    let dropped = |pane: &Pane, view: &Subscription| {
        // The worker drains events after each command; one more round
        // trip makes sure it published those of the last one.
        pane.snapshot().unwrap();
        let pending = view.pending.lock().unwrap();
        pending
            .events
            .iter()
            .any(|event| matches!(event, TerminalEvent::InputDropped))
    };
    let key = RemoteCommand::Write(b"q".to_vec());

    let (pane, view) = stalled();
    flood(&pane, RemoteCommand::WriteMotion);
    pane.command(key.clone()).unwrap();
    assert!(!dropped(&pane, &view), "motion crowded out a key");
    // The program really stopped reading: plain input past the backlog
    // is refused.
    flood(&pane, RemoteCommand::Write);
    assert!(dropped(&pane, &view), "the input backlog never filled");
    pane.close().unwrap();

    // The same hover sent as plain input fills the backlog and drops
    // the key (the bug).
    let (pane, view) = stalled();
    flood(&pane, RemoteCommand::Write);
    pane.command(key).unwrap();
    assert!(dropped(&pane, &view));
    pane.close().unwrap();
}

fn seed(pane: &Pane, now: SystemTime) -> Vec<TerminalEvent> {
    let shared = pane.shared.lock().unwrap();
    replay_seed(&shared.sticky_events, shared.program_status.as_deref(), now)
}

fn record(id: &str, state: ProgramState) -> ProgramStatusRecord {
    ProgramStatusRecord {
        id: id.into(),
        state,
        kind: None,
        progress: None,
        app: None,
        title: None,
        msg: None,
    }
}

fn states(records: &[ProgramStatusRecord]) -> Vec<ProgramState> {
    records.iter().map(|record| record.state).collect()
}

#[test]
fn stale_progress_and_working_records_expire_only_outside_a_running_command() {
    let since = SystemTime::now();
    let mut sticky = vec![
        (TerminalEvent::Title("build".into()), since),
        (TerminalEvent::Progress(ProgressState::Indeterminate), since),
    ];
    let status = [
        (record("", ProgramState::Working), since),
        (record("ask", ProgramState::Blocked), since),
        (record("done", ProgramState::Done), since),
    ];
    let fresh = replay_seed(&sticky, Some(&status), since + STALE_ACTIVITY_AFTER / 2);
    assert!(
        matches!(
            fresh.as_slice(),
            [TerminalEvent::Title(_), TerminalEvent::Progress(_), TerminalEvent::ProgramStatus(records)]
                if records.len() == 3
        ),
        "{fresh:?}"
    );
    // A clock set back does not expire anything.
    let earlier = since - STALE_ACTIVITY_AFTER;
    assert_eq!(
        format!("{:?}", replay_seed(&sticky, Some(&status), earlier)),
        format!("{fresh:?}")
    );

    // No command runs (no shell integration, or it finished): expire the
    // progress and the working record, but never the blocked one.
    let stale = replay_seed(&sticky, Some(&status), since + STALE_ACTIVITY_AFTER);
    let [
        TerminalEvent::Title(_),
        TerminalEvent::ProgramStatus(records),
    ] = stale.as_slice()
    else {
        panic!("unexpected replay {stale:?}");
    };
    assert_eq!(states(records), [ProgramState::Blocked, ProgramState::Done]);

    // A 45-minute build keeps its spinner and status.
    sticky.push((TerminalEvent::ShellCommandExecuting, since));
    let running = replay_seed(&sticky, Some(&status), since + STALE_ACTIVITY_AFTER * 2);
    assert!(
        matches!(
            running.as_slice(),
            [
                TerminalEvent::Title(_),
                TerminalEvent::Progress(_),
                TerminalEvent::ShellCommandExecuting,
                TerminalEvent::ProgramStatus(records),
            ] if records.len() == 3
        ),
        "{running:?}"
    );
}

#[test]
fn status_records_keep_their_times_until_reported_again() {
    let (then, now) = (SystemTime::UNIX_EPOCH, SystemTime::now());
    let old = vec![
        (record("a", ProgramState::Working), then),
        (record("b", ProgramState::Working), then),
        (record("c", ProgramState::Done), then),
    ];
    let times = |new: Vec<ProgramStatusRecord>| -> Vec<(String, SystemTime)> {
        status_times(&old, new, now)
            .into_iter()
            .map(|(record, time)| (record.id, time))
            .collect()
    };
    let a = record("a", ProgramState::Working);
    let b = record("b", ProgramState::Working);
    let c = record("c", ProgramState::Done);
    // `b` reported unchanged moves to the end.
    assert_eq!(
        times(vec![a.clone(), c.clone(), b.clone()]),
        [("a".into(), then), ("c".into(), then), ("b".into(), now)]
    );
    // `b` reported with a new state.
    let blocked = record("b", ProgramState::Blocked);
    assert_eq!(
        times(vec![a.clone(), c.clone(), blocked]),
        [("a".into(), then), ("c".into(), then), ("b".into(), now)]
    );
    // `a` cleared: nothing was reported.
    assert_eq!(
        times(vec![b.clone(), c.clone()]),
        [("b".into(), then), ("c".into(), then)]
    );
    // The last record reported again unchanged.
    assert_eq!(
        times(vec![a, b, c]),
        [("a".into(), then), ("b".into(), then), ("c".into(), now)]
    );
}

#[test]
fn stale_working_records_stay_hidden_when_the_engine_resends_the_snapshot() {
    let pane = detached_pane();
    let subscription = pane.subscribe(true).unwrap();
    let stuck = record("", ProgramState::Working);
    let long_ago = SystemTime::now() - STALE_ACTIVITY_AFTER * 2;
    pane.shared.lock().unwrap().program_status = Some(vec![(stuck.clone(), long_ago)]);
    // Another program reports; the snapshot still holds the stuck record.
    publish_to(
        &pane,
        vec![TerminalEvent::ProgramStatus(vec![
            stuck,
            record("tests", ProgramState::Done),
        ])],
    );
    let Update::Events(events) = next_events(&subscription) else {
        panic!("expected live events");
    };
    let [TerminalEvent::ProgramStatus(records)] = events.as_slice() else {
        panic!("unexpected events {events:?}");
    };
    assert_eq!(states(records), [ProgramState::Done]);
    let replay = seed(&pane, SystemTime::now());
    assert!(
        matches!(replay.as_slice(), [TerminalEvent::ProgramStatus(records)] if records.len() == 1),
        "{replay:?}"
    );
}

#[cfg(unix)]
fn launched_pane(script: &str, working_directory: Option<&str>) -> Pane {
    Pane::spawn(
        PaneLaunch {
            size: TerminalSize::default(),
            working_directory: working_directory.map(Into::into),
            shell_integration: Some(TabTitleShellIntegration {
                enabled: true,
                explicit_prefix: String::new(),
            }),
            config: TerminalRuntimeConfig {
                scrollback_history: 1234,
                working_dir_fallback: WorkingDirFallback::Process,
                environment: [("SSH_ASKPASS".into(), "helper".into())].into(),
                ..TerminalRuntimeConfig::default()
            },
            launch: Some(TerminalLaunch::Program {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), script.into()],
            }),
            environment: None,
        },
        &std::env::temp_dir(),
        HOST_REQUEST_TIMEOUT,
    )
    .unwrap()
}

#[cfg(unix)]
fn until(pane: &Pane, what: &str, done: impl Fn(&PaneInfo) -> bool) -> PaneInfo {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let info = pane.info();
        if done(&info) {
            return info;
        }
        assert!(Instant::now() < deadline, "{what}: {info:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Audit B24: input for a program that exited, or one that stopped
/// reading until the backlog filled, is rejected instead of accepted.
#[cfg(unix)]
#[test]
fn input_that_cannot_reach_the_program_is_rejected() {
    let exited = launched_pane("exit 0", None);
    until(&exited, "the program never exited", |info| info.exited);
    assert!(matches!(
        exited.command(RemoteCommand::Write(b"x".to_vec())).unwrap(),
        RemoteReply::InputRejected(InputRejection::Exited)
    ));
    exited.close().unwrap();

    // Raw mode: a full input queue blocks the writer instead of
    // discarding characters, as for a stalled full-screen program;
    // READY says it is set.
    let stalled = launched_pane("stty raw -echo; printf READY; exec sleep 30", None);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !screen(&stalled.snapshot().unwrap()).contains("READY") {
        assert!(Instant::now() < deadline, "the program never got ready");
        std::thread::sleep(Duration::from_millis(10));
    }
    let chunk = vec![b'x'; 64 * 1024];
    let rejected = (0..400).find_map(|_| {
        match stalled
            .command(RemoteCommand::Write(chunk.clone()))
            .unwrap()
        {
            RemoteReply::InputRejected(rejection) => Some(rejection),
            _ => None,
        }
    });
    assert_eq!(rejected, Some(InputRejection::BacklogFull));
    stalled.close().unwrap();
}

#[cfg(unix)]
fn write(pane: &Pane, input: &str) -> u64 {
    match pane
        .command(RemoteCommand::Write(input.as_bytes().to_vec()))
        .unwrap()
    {
        RemoteReply::Written { anchor } => anchor,
        reply => panic!("input was not written: {reply:?}"),
    }
}

#[cfg(unix)]
fn command_done(pane: &Pane) -> Option<i32> {
    let info = until(pane, "the input's command never finished", |info| {
        matches!(
            info.shell.and_then(|shell| shell.input_command),
            Some(InputCommand::Finished(_))
        )
    });
    let Some(InputCommand::Finished(code)) = info.shell.unwrap().input_command else {
        unreachable!()
    };
    code
}

/// Audit I14: the pane follows the OSC 133 command lifecycle and the
/// command that input started.
#[cfg(unix)]
#[test]
fn shell_state_follows_commands_and_input() {
    let pane = launched_pane(
        "printf '\\033]133;A\\007'; read line; \
             printf '\\033]133;C\\007'; sleep 0.2; \
             printf '\\033]133;D;3\\007\\033]133;A\\007'; sleep 30",
        None,
    );
    let info = until(&pane, "no prompt", |info| info.shell.is_some());
    assert_eq!(info.shell, Some(ShellState::default()));
    write(&pane, "go\r");
    until(&pane, "the command never ran", |info| {
        info.shell.is_some_and(|shell| shell.running)
    });
    assert_eq!(command_done(&pane), Some(3));
    assert_eq!(
        pane.info().shell,
        Some(ShellState {
            running: false,
            commands_started: 1,
            commands_finished: 1,
            exit_code: Some(3),
            input_command: Some(InputCommand::Finished(Some(3))),
        })
    );
    // Later input waits for a later command.
    write(&pane, "more");
    assert_eq!(
        pane.info().shell.unwrap().input_command,
        Some(InputCommand::Pending)
    );
    pane.close().unwrap();
}

/// Audit I14: fish reports a finish at its first prompt. Input typed
/// before that prompt finishes with the command it starts, not then.
#[cfg(unix)]
#[test]
fn a_first_prompt_finish_is_not_the_inputs_command() {
    let pane = launched_pane(
        "sleep 0.5; printf '\\033]133;D;0\\007\\033]133;A\\007\\033]133;B\\007'; \
             read line; printf '\\033]133;C\\007'; sleep 0.2; \
             printf '\\033]133;D;5\\007\\033]133;A\\007'; sleep 30",
        None,
    );
    write(&pane, "go\r");
    assert!(pane.info().shell.is_none(), "the prompt came too early");
    assert_eq!(command_done(&pane), Some(5));
    pane.close().unwrap();
}

/// Audit I14: input typed while a command still runs (its prompt hook
/// reports the finish late) finishes with the next command.
#[cfg(unix)]
#[test]
fn a_late_finish_belongs_to_the_earlier_command() {
    let pane = launched_pane(
        "printf '\\033]133;A\\007'; read first; printf '\\033]133;C\\007'; \
             read second; printf '\\033]133;D;1\\007\\033]133;A\\007'; \
             printf '\\033]133;C\\007'; sleep 0.2; \
             printf '\\033]133;D;7\\007\\033]133;A\\007'; sleep 30",
        None,
    );
    until(&pane, "no prompt", |info| info.shell.is_some());
    write(&pane, "first\r");
    until(&pane, "the first command never ran", |info| {
        info.shell.is_some_and(|shell| shell.running)
    });
    write(&pane, "second\r");
    assert_eq!(command_done(&pane), Some(7));
    pane.close().unwrap();
}

/// Audit I14: a shell that reports no command starts (PowerShell)
/// finishes the input's command with the next finish after a prompt.
#[cfg(unix)]
#[test]
fn shells_without_command_starts_finish_after_a_prompt() {
    let pane = launched_pane(
        "printf '\\033]133;A\\007\\033]133;B\\007'; read line; sleep 0.2; \
             printf '\\033]133;D;4\\007\\033]133;A\\007'; sleep 30",
        None,
    );
    until(&pane, "no prompt", |info| info.shell.is_some());
    write(&pane, "go\r");
    assert_eq!(command_done(&pane), Some(4));
    pane.close().unwrap();
}

/// Audit I14: a write's anchor is the line the cursor was on, counted
/// from the first line that scrolled into history.
#[cfg(unix)]
#[test]
fn writes_report_the_cursor_line_as_their_anchor() {
    let pane = launched_pane("stty -echo; seq 1 100; printf READY; exec sleep 30", None);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !screen(&pane.snapshot().unwrap()).contains("READY") {
        assert!(Instant::now() < deadline, "the program never got ready");
        std::thread::sleep(Duration::from_millis(10));
    }
    let state = pane.snapshot().unwrap();
    let anchor = write(&pane, "x");
    assert_eq!(
        anchor,
        state.lines_scrolled + state.cursor_position.1 as u64
    );
    let RemoteReply::LineTexts(texts) = pane
        .command(RemoteCommand::LineTexts {
            first: 0,
            last: 0,
            anchor: Some(anchor),
        })
        .unwrap()
    else {
        panic!("expected lines");
    };
    assert_eq!(texts.lines[0].text, "READY");
    assert_eq!(texts.rows, 24);
    assert_eq!(texts.scrolled, state.lines_scrolled);
    pane.close().unwrap();
}

/// Audit B26/B27: a pane lists the directory it started in, not a
/// missing one it was asked for, and keeps its launch settings without
/// the variables of its launch or those the host adds.
#[cfg(unix)]
#[test]
fn panes_keep_their_launch_settings_and_actual_directory() {
    let missing = std::env::temp_dir().join("termy-no-such-directory");
    let pane = launched_pane("sleep 30", Some(&missing.to_string_lossy()));
    let started = std::env::current_dir().unwrap();
    assert_eq!(
        pane.info().working_directory.as_deref(),
        Some(&*started.to_string_lossy())
    );
    let settings = pane.launch_config();
    assert_eq!(settings.config.scrollback_history, 1234);
    assert!(settings.config.environment.is_empty());
    assert!(
        settings
            .shell_integration
            .is_some_and(|shell| shell.enabled)
    );
    pane.close().unwrap();
}
