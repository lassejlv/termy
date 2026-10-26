use super::*;

fn size(cols: u16, rows: u16) -> TerminalSize {
    TerminalSize {
        cols,
        rows,
        ..TerminalSize::default()
    }
}

fn frame(cols: u16, rows: u16) -> RemoteState {
    RemoteState::capture(&Terminal::new_display(size(cols, rows), None))
}

#[test]
fn size_claim_reports_an_override_only_after_a_frame_showed_this_size() {
    let mut claim = SizeClaim::attached(&frame(80, 24));
    assert!(!claim.overridden(&frame(80, 24)));
    // Created or reattached at this size: another client's resize counts.
    assert!(claim.overridden(&frame(40, 10)));

    claim.requested(size(120, 40), &frame(80, 24));
    // The resize is still queued: the host reports its old size.
    claim.published(&frame(80, 24));
    assert!(!claim.overridden(&frame(80, 24)));
    // A frame showed it (whether or not a view drew that frame), then
    // another client resized the pane.
    claim.published(&frame(120, 40));
    claim.published(&frame(40, 10));
    assert!(claim.overridden(&frame(40, 10)));

    // Asking for the size the host already has is confirmed at once.
    claim.requested(size(40, 10), &frame(40, 10));
    assert!(!claim.overridden(&frame(40, 10)));
    assert!(claim.overridden(&frame(41, 10)));
}

#[test]
fn first_streamed_images_invalidate_the_pending_graphics_snapshot() {
    let terminal = Terminal::new_display(crate::TerminalSize::default(), None);
    terminal.feed_output(b"\x1b_Ga=T,i=1,f=32,s=1,v=1,C=1,q=2;AQID/w==\x1b\\");
    let ready = RemoteState::capture(&terminal);
    assert_eq!(ready.graphics.as_ref().unwrap().len(), 1);
    let pending = awaiting_graphics(ready.clone());
    assert!(pending.graphics.as_ref().unwrap().is_empty());
    // A renderer may cache the initial empty snapshot before the reader
    // publishes these same host placements. Its revision must then change
    // even when there is no further PTY output or window resize.
    assert_ne!(pending.graphics_revision, ready.graphics_revision);
}

/// One frame from `frames` and `images`, as the host sends it: images
/// first.
fn send(
    frames: &mut crate::remote::frame::FrameEncoder,
    images: &mut graphics::GraphicsEncoder,
    reader: &mut FrameReader,
    terminal: &Terminal,
) -> (crate::remote::frame::FrameUpdate, graphics::GraphicsUpdate) {
    let state = RemoteState::capture(terminal);
    let (uploads, placements) = images.encode(state.graphics.as_deref().unwrap_or_default());
    for upload in uploads {
        reader.graphics.insert(upload);
    }
    (frames.encode(&state), placements)
}

/// Audit I1: a frame that does not fit the client's frame makes it ask
/// for a full one and ignore deltas until then, instead of failing.
#[test]
fn a_frame_that_does_not_fit_waits_for_a_full_one() {
    let terminal = Terminal::new_display(size(20, 4), None);
    terminal.feed_output(b"\x1b_Ga=T,i=1,f=32,s=1,v=1,C=1,q=2;AQID/w==\x1b\\");
    let mut frames = crate::remote::frame::FrameEncoder::default();
    let mut images = graphics::GraphicsEncoder::default();
    let mut reader = FrameReader::default();
    let initial = frame(20, 4);
    let (update, placements) = send(&mut frames, &mut images, &mut reader, &terminal);
    let (shown, _) = reader.frame(&initial, update, placements).unwrap().unwrap();
    assert_eq!(shown.graphics.as_ref().unwrap().len(), 1);

    // The client's frame is not the one the delta was made against.
    terminal.feed_output(b"a");
    let (update, placements) = send(&mut frames, &mut images, &mut reader, &terminal);
    let corrupted = frame(20, 4);
    assert!(reader.frame(&corrupted, update, placements).is_err());
    // Deltas sent before the host saw the resync are ignored.
    terminal.feed_output(b"b");
    let (update, placements) = send(&mut frames, &mut images, &mut reader, &terminal);
    assert!(reader.frame(&shown, update, placements).unwrap().is_none());

    // The host starts afresh: a full frame with its images again.
    frames.reset();
    images = graphics::GraphicsEncoder::default();
    terminal.feed_output(b"c");
    let (update, placements) = send(&mut frames, &mut images, &mut reader, &terminal);
    let (state, change) = reader.frame(&shown, update, placements).unwrap().unwrap();
    assert_eq!(change, crate::remote::frame::FrameChange::Full);
    assert!(state.same_frame(&RemoteState::capture(&terminal)));
    assert_eq!(state.graphics.as_ref().unwrap().len(), 1);
    // Deltas apply again.
    terminal.feed_output(b"d");
    let (update, placements) = send(&mut frames, &mut images, &mut reader, &terminal);
    assert!(reader.frame(&state, update, placements).unwrap().is_some());
}

/// Audit I2: a hidden view keeps only the images on its screen, and
/// drops the rest of its cache when the host starts afresh on resume.
#[test]
fn a_paused_view_releases_images_it_does_not_show() {
    let terminal = Terminal::new_display(size(20, 4), None);
    let mut frames = crate::remote::frame::FrameEncoder::default();
    let mut images = graphics::GraphicsEncoder::default();
    let mut reader = FrameReader::default();
    let mut shown = frame(20, 4);
    for id in 1..=3 {
        terminal.feed_output(b"\x1b_Ga=d,d=A,q=2\x1b\\");
        terminal.feed_output(
            format!("\x1b_Ga=T,i={id},f=32,s=1,v=1,C=1,q=2;AQID/w==\x1b\\").as_bytes(),
        );
        let (update, placements) = send(&mut frames, &mut images, &mut reader, &terminal);
        shown = reader.frame(&shown, update, placements).unwrap().unwrap().0;
    }
    assert_eq!(reader.graphics.cached(), 3);
    reader.paused(true, &shown);
    assert_eq!(reader.graphics.cached(), 1);
    reader.paused(false, &shown);
    assert_eq!(reader.graphics.cached(), 0);
}

/// Audit B22: hovering queues one motion report behind the last one not
/// yet sent, and never moves a report past other input.
/// Audit B22: hovering queues one motion report behind the last one not
/// yet sent, and never moves a report past other input.
#[test]
fn motion_reports_coalesce_only_at_the_end_of_the_queue() {
    let queue = InputQueue::default();
    let push = |command| {
        queue.push(Input {
            command,
            reply: None,
        })
    };
    let motion = |bytes: &[u8]| RemoteCommand::WriteMotion(bytes.to_vec());
    for bytes in [b"1", b"2", b"3"] {
        assert_eq!(push(motion(bytes)), Queued::Yes);
    }
    // A key behind it: the next report queues after the key.
    assert_eq!(push(RemoteCommand::Write(b"k".to_vec())), Queued::Yes);
    assert_eq!(push(motion(b"4")), Queued::Yes);
    let sent = |queue: &InputQueue| match queue.pop().unwrap().command {
        RemoteCommand::WriteMotion(bytes) | RemoteCommand::Write(bytes) => bytes,
        _ => unreachable!(),
    };
    assert_eq!(sent(&queue), b"3");
    assert_eq!(sent(&queue), b"k");
    assert_eq!(sent(&queue), b"4");
    // Once the input thread took a report, the next one queues anew.
    assert_eq!(push(motion(b"5")), Queued::Yes);
    assert_eq!(sent(&queue), b"5");
    // A full queue drops motion quietly, and typing with a report.
    for _ in 0..INPUT_CAPACITY {
        push(RemoteCommand::Write(b"x".to_vec()));
    }
    assert_eq!(push(motion(b"6")), Queued::DroppedMotion);
    assert_eq!(push(RemoteCommand::Write(b"y".to_vec())), Queued::Dropped);
}

#[test]
fn program_status_snapshots_do_not_accumulate_for_slow_clients() {
    let mut pending = Pending::default();
    pending.extend_events(vec![TerminalEvent::Bell]).unwrap();
    for _ in 0..2048 {
        pending
            .extend_events(vec![TerminalEvent::ProgramStatus(vec![])])
            .unwrap();
    }
    assert_eq!(pending.events.len(), 2);
    assert!(matches!(pending.events[0], TerminalEvent::Bell));
    assert!(matches!(pending.events[1], TerminalEvent::ProgramStatus(_)));
}

#[test]
fn late_reply_after_timeout_is_not_returned_for_the_next_request() {
    use std::net::TcpListener;
    let temp = crate::multiplexer::discovery::private_tempdir();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::fs::write(
        temp.path().join("endpoint.json"),
        serde_json::json!({ "version": VERSION, "port": port, "token": "token" }).to_string(),
    )
    .unwrap();
    let host = std::thread::spawn(move || {
        let accept = || {
            let (mut stream, _) = listener.accept().unwrap();
            let Request::Authenticate { client_nonce, .. } = read_message(&mut stream).unwrap()
            else {
                panic!("expected authentication");
            };
            let server_nonce = [2; 32];
            let server_proof = host_proof("token", &client_nonce, &server_nonce);
            let challenge = Response::Challenge {
                server_nonce,
                server_proof,
            };
            write_message(&mut stream, &challenge).unwrap();
            let _: Request = read_message(&mut stream).unwrap();
            write_message(&mut stream, &Response::Ok).unwrap();
            let _: Request = read_message(&mut stream).unwrap();
            stream
        };
        // The first request is answered only after the client gave up.
        let mut slow = accept();
        std::thread::sleep(Duration::from_millis(300));
        let layout = |text: &str| Response::Layout {
            revision: 1,
            layout: Some(text.into()),
        };
        let _ = write_message(&mut slow, &layout("first"));
        let mut next = accept();
        write_message(&mut next, &layout("second")).unwrap();
    });

    let connection = Connection::connect(temp.path()).unwrap();
    let short_timeout = Some(Duration::from_millis(100));
    let stream = connection.stream.lock().unwrap();
    stream
        .as_ref()
        .unwrap()
        .set_read_timeout(short_timeout)
        .unwrap();
    drop(stream);
    assert!(connection.request(Request::GetLayout).is_err());
    let Response::Layout { layout, .. } = connection.request(Request::GetLayout).unwrap() else {
        panic!("expected a layout reply");
    };
    assert_eq!(layout.as_deref(), Some("second"));
    host.join().unwrap();
}

#[test]
fn command_finish_and_prompt_reach_the_view_in_order() {
    let mut pending = Pending::default();
    pending
        .extend_events(vec![
            TerminalEvent::ShellCommandFinished(Some(1)),
            TerminalEvent::ShellPromptStart,
        ])
        .unwrap();
    assert!(matches!(
        pending.events.as_slice(),
        [
            TerminalEvent::ShellCommandFinished(Some(1)),
            TerminalEvent::ShellPromptStart
        ]
    ));
}

#[test]
fn suspended_ui_queue_survives_a_flood_of_prompts() {
    let mut pending = Pending::default();
    for code in 0..10_000 {
        pending
            .extend_events(vec![
                TerminalEvent::ShellCommandExecuting,
                TerminalEvent::ShellCommandFinished(Some(code)),
                TerminalEvent::ShellPromptStart,
            ])
            .unwrap();
    }
    assert!(pending.events.len() <= 64, "{}", pending.events.len());
    assert!(matches!(
        pending.events.last(),
        Some(TerminalEvent::ShellPromptStart)
    ));
}

#[test]
fn replayed_events_restore_state_without_reporting_what_happened() {
    let mut pending = Pending::default();
    pending
        .extend_replay(vec![
            TerminalEvent::Title("vim".into()),
            TerminalEvent::WorkingDirectory("/tmp".into()),
            TerminalEvent::ShellCommandExecuting,
            TerminalEvent::ShellCommandFinished(Some(1)),
            TerminalEvent::Bell,
            TerminalEvent::Progress(ProgressState::Indeterminate),
            TerminalEvent::ProgramStatus(vec![]),
            TerminalEvent::Exit,
        ])
        .unwrap();
    assert!(
        matches!(
            pending.events.as_slice(),
            [
                TerminalEvent::Title(_),
                TerminalEvent::WorkingDirectory(_),
                TerminalEvent::Progress(ProgressState::Indeterminate),
                TerminalEvent::ProgramStatus(_),
                TerminalEvent::Exit,
            ]
        ),
        "{:?}",
        pending.events
    );
    // Live events after the replay are delivered as they happen.
    pending
        .extend_events(vec![TerminalEvent::ShellCommandExecuting])
        .unwrap();
    assert!(matches!(
        pending.events.last(),
        Some(TerminalEvent::ShellCommandExecuting)
    ));
}

/// Audit B7: a request the host already denied must not prompt or write
/// later, when a hidden view finally drains; the rest run in order.
#[test]
fn expired_clipboard_requests_are_dropped_and_the_rest_run_in_order() {
    let mut pending = Pending::default();
    let now = Instant::now();
    let load = || RemoteHostRequest::Load(TerminalClipboardTarget::Clipboard);
    let later = now + Duration::from_secs(60);
    for (id, deadline) in [(1, later), (2, now), (3, later)] {
        pending.hosts.push_back((id, load(), deadline));
    }
    let ids: Vec<_> = pending
        .take_host_requests(now)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, [1, 3]);
    assert!(pending.hosts.is_empty());
}

#[test]
fn suspended_ui_queue_coalesces_bells_progress_and_clipboard_writes() {
    let mut pending = Pending::default();
    for round in 0..2048 {
        pending
            .extend_events(vec![
                TerminalEvent::Bell,
                TerminalEvent::Progress(ProgressState::InProgress((round % 100) as u8)),
                TerminalEvent::ClipboardStore(format!("copy {round}")),
            ])
            .unwrap();
    }
    assert_eq!(pending.events.len(), 3);
    assert!(matches!(pending.events[0], TerminalEvent::Bell));
    assert!(matches!(
        pending.events[1],
        TerminalEvent::Progress(ProgressState::InProgress(47))
    ));
    assert!(
        matches!(&pending.events[2], TerminalEvent::ClipboardStore(text) if text == "copy 2047")
    );
}

/// A host that authenticates every connection and hands it to the test,
/// which then plays the host's part on it.
struct FakeHost {
    root: tempfile::TempDir,
    connections: flume::Receiver<TcpStream>,
}

impl FakeHost {
    fn new() -> Self {
        use std::net::TcpListener;
        let root = crate::multiplexer::discovery::private_tempdir();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write(
            root.path().join("endpoint.json"),
            serde_json::json!({ "version": VERSION, "port": port, "token": "token" }).to_string(),
        )
        .unwrap();
        let (sender, connections) = flume::unbounded();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let Request::Authenticate { client_nonce, .. } = read_message(&mut stream).unwrap()
                else {
                    panic!("expected authentication");
                };
                let server_nonce = [2; 32];
                let server_proof = host_proof("token", &client_nonce, &server_nonce);
                let challenge = Response::Challenge {
                    server_nonce,
                    server_proof,
                };
                write_message(&mut stream, &challenge).unwrap();
                let _: Request = read_message(&mut stream).unwrap();
                write_message(&mut stream, &Response::Ok).unwrap();
                if sender.send(stream).is_err() {
                    return;
                }
            }
        });
        Self { root, connections }
    }

    fn next(&self) -> TcpStream {
        let stream = self
            .connections
            .recv_timeout(Duration::from_secs(5))
            .expect("the client did not connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
    }

    /// Answers a subscription to `pane` and sends it `state`.
    fn subscribed(&self, state: RemoteState) -> TcpStream {
        let mut stream = self.next();
        let Request::Subscribe { pane, .. } = read_message(&mut stream).unwrap() else {
            panic!("expected a subscription");
        };
        assert_eq!(pane, "pane");
        write_message(&mut stream, &Response::Ok).unwrap();
        let frame = crate::remote::frame::FrameEncoder::default().encode(&state);
        let (_, images) = graphics::GraphicsEncoder::default().encode(&[]);
        write_message(&mut stream, &Update::Frame(Box::new(frame), images)).unwrap();
        stream
    }

    /// Attaches a view to "pane". Returns it with the host's ends of its
    /// request, input and subscription connections.
    fn attach(&self) -> (Arc<ClientTerminal>, TcpStream, TcpStream, TcpStream) {
        let client = SessionClient::connect(self.root.path()).unwrap();
        let _client_connection = self.next();
        let attaching = std::thread::spawn(move || {
            client
                .attach_transport("pane", Arc::new(frame(80, 24)), None, true)
                .unwrap()
        });
        let rpc = self.next();
        let input = self.next();
        let subscription = self.subscribed(frame(80, 24));
        (attaching.join().unwrap(), rpc, input, subscription)
    }
}

fn written(stream: &mut TcpStream) -> Vec<u8> {
    match read_message(stream).unwrap() {
        Request::Command {
            command: RemoteCommand::Write(bytes),
            ..
        } => bytes,
        _ => panic!("expected a write"),
    }
}

/// Events drained until `done` returns true for one of them.
#[track_caller]
fn events_until(
    terminal: &ClientTerminal,
    mut done: impl FnMut(&TerminalEvent) -> bool,
) -> Vec<TerminalEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = Vec::new();
    loop {
        for event in terminal.take_events() {
            if matches!(event, TerminalEvent::Wakeup) {
                continue;
            }
            let finished = done(&event);
            seen.push(event);
            if finished {
                return seen;
            }
        }
        assert!(Instant::now() < deadline, "events so far: {seen:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[track_caller]
fn wait_for_cols(terminal: &ClientTerminal, cols: u16) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while terminal.state().size.cols != cols {
        assert!(Instant::now() < deadline, "the new state never arrived");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn is_disconnected_title(event: &TerminalEvent) -> bool {
    matches!(event, TerminalEvent::Title(title) if title.starts_with("Session disconnected"))
}

/// Audit B9: a request the host answers with an error fails alone. The
/// pane stays connected, and later input still arrives.
#[test]
fn a_failed_request_leaves_the_pane_connected() {
    let host = FakeHost::new();
    let (terminal, mut rpc, mut input, _subscription) = host.attach();

    terminal.send(RemoteCommand::Write(b"one".to_vec()));
    assert_eq!(written(&mut input), b"one");
    write_message(&mut input, &Response::Error("busy".into())).unwrap();
    terminal.send(RemoteCommand::Write(b"two".to_vec()));
    assert_eq!(written(&mut input), b"two");
    write_message(&mut input, &Response::Reply(RemoteReply::Ok)).unwrap();

    let lines = || RemoteCommand::Lines { first: 0, last: 1 };
    std::thread::scope(|scope| {
        let request = scope.spawn(|| terminal.request(lines()));
        let _: Request = read_message(&mut rpc).unwrap();
        write_message(&mut rpc, &Response::Error("too slow".into())).unwrap();
        let error = request.join().unwrap().unwrap_err();
        assert!(!is_connection_error(&error), "{error:#}");

        let request = scope.spawn(|| terminal.request(lines()));
        let _: Request = read_message(&mut rpc).unwrap();
        write_message(&mut rpc, &Response::Reply(RemoteReply::Ok)).unwrap();
        assert!(matches!(request.join().unwrap(), Ok(RemoteReply::Ok)));
    });
    assert!(!terminal.shared.disconnected.load(Ordering::Acquire));
    assert!(!terminal.take_events().iter().any(is_disconnected_title));
}

/// Audit B9: a lost stream is subscribed again with a fresh state. The
/// tab says it is disconnected only while that fails, and a pane the host
/// no longer has ends with `Exit`.
#[test]
fn a_lost_stream_is_reattached_until_the_pane_is_gone() {
    let host = FakeHost::new();
    let (terminal, _rpc, _input, subscription) = host.attach();

    drop(subscription);
    let subscription = host.subscribed(frame(40, 10));
    wait_for_cols(&terminal, 40);
    assert!(!terminal.take_events().iter().any(is_disconnected_title));

    // The first attempt fails, here because the host's refresh of the
    // pane timed out: that may pass (review finding 1). The tab shows it
    // until the next attempt works.
    drop(subscription);
    let mut slow = host.next();
    let _: Request = read_message(&mut slow).unwrap();
    let timeout = Response::Error("refresh terminal session: timed out".into());
    write_message(&mut slow, &timeout).unwrap();
    events_until(&terminal, is_disconnected_title);
    assert!(terminal.request(RemoteCommand::Snapshot).is_err());
    let subscription = host.subscribed(frame(50, 10));
    wait_for_cols(&terminal, 50);
    events_until(&terminal, |event| {
        matches!(event, TerminalEvent::ResetTitle)
    });

    // The host closed the pane while the stream was down.
    drop(subscription);
    let mut refused = host.next();
    let _: Request = read_message(&mut refused).unwrap();
    let gone = Response::PaneNotFound("terminal session no longer exists: pane".into());
    write_message(&mut refused, &gone).unwrap();
    events_until(&terminal, |event| matches!(event, TerminalEvent::Exit));
    assert!(!terminal.host_lost());
    // Requests fail at once instead of reconnecting (audit I3), and
    // dropped input does not blame the program.
    assert!(terminal.request(RemoteCommand::Snapshot).is_err());
    terminal.send(RemoteCommand::Write(b"lost".to_vec()));
    std::thread::sleep(REATTACH_MIN_DELAY * 2);
    assert!(host.connections.is_empty());
    assert!(
        !terminal
            .take_events()
            .iter()
            .any(|event| matches!(event, TerminalEvent::InputDropped))
    );
}

fn is_ended_title(event: &TerminalEvent) -> bool {
    matches!(event, TerminalEvent::Title(title) if title.starts_with("Session ended"))
}

/// Review finding 2: a host that shuts down ends its panes without
/// `Exit`, so views keep their tabs, marked as ended.
#[test]
fn a_host_shutdown_ends_the_pane_without_an_exit() {
    let host = FakeHost::new();
    let (terminal, _rpc, _input, mut subscription) = host.attach();
    write_message(&mut subscription, &Update::HostShutdown).unwrap();
    drop(subscription);
    let events = events_until(&terminal, is_ended_title);
    assert!(!events.iter().any(is_exit), "{events:?}");
    assert!(terminal.host_lost());
    std::thread::sleep(REATTACH_MIN_DELAY * 2);
    assert!(host.connections.is_empty(), "the view subscribed again");
    assert!(terminal.request(RemoteCommand::Snapshot).is_err());
}

/// Review finding 2: a host that another one replaced never had this
/// pane; the view is not asked to subscribe on the new host.
#[test]
fn a_replaced_host_ends_the_pane_without_an_exit() {
    let host = FakeHost::new();
    let (terminal, _rpc, _input, subscription) = host.attach();
    let endpoint = host.root.path().join("endpoint.json");
    let replaced = std::fs::read_to_string(&endpoint)
        .unwrap()
        .replace("\"token\"", "\"other\"");
    std::fs::write(&endpoint, replaced).unwrap();
    drop(subscription);
    let events = events_until(&terminal, is_ended_title);
    assert!(!events.iter().any(is_exit), "{events:?}");
    assert!(terminal.host_lost());
}

/// Review finding 4: a host whose updates cannot be read is shown as
/// disconnected at once, and not subscribed to again in a tight loop.
#[test]
fn an_unreadable_stream_is_shown_and_retried_slowly() {
    let host = FakeHost::new();
    let (terminal, _rpc, _input, mut subscription) = host.attach();
    // A host never sends its internal state update to a client.
    let state = Update::State(Arc::new(frame(80, 24)));
    write_message(&mut subscription, &state).unwrap();
    events_until(&terminal, is_disconnected_title);
    std::thread::sleep(Duration::from_millis(500));
    assert!(host.connections.is_empty(), "subscribed again at once");
    assert!(terminal.request(RemoteCommand::Snapshot).is_err());
}

/// Review finding 4: a host that accepts each new subscription and then
/// drops it before a frame is shown as disconnected too.
#[test]
fn streams_that_never_show_a_frame_are_shown_as_disconnected() {
    let host = FakeHost::new();
    let (terminal, _rpc, _input, subscription) = host.attach();
    drop(subscription);
    for _ in 0..UNHEALTHY_STREAMS {
        let mut stream = host.next();
        let _: Request = read_message(&mut stream).unwrap();
        write_message(&mut stream, &Response::Ok).unwrap();
    }
    events_until(&terminal, is_disconnected_title);
}

/// Review finding 6: the size and options a view set while its stream
/// was lost reach the host once it is back.
#[test]
fn settings_dropped_while_disconnected_are_sent_after_reattaching() {
    let host = FakeHost::new();
    let (terminal, _rpc, mut input, subscription) = host.attach();
    let resized = |input: &mut TcpStream| match read_message(input).unwrap() {
        Request::Command {
            command: RemoteCommand::Resize(size),
            ..
        } => (size.cols, size.rows),
        _ => panic!("expected a resize"),
    };
    terminal.send(RemoteCommand::Resize(size(100, 30)));
    assert_eq!(resized(&mut input), (100, 30));
    write_message(&mut input, &Response::Reply(RemoteReply::Ok)).unwrap();

    drop(subscription);
    drop(host.next());
    events_until(&terminal, is_disconnected_title);
    terminal.send(RemoteCommand::Resize(size(120, 40)));
    terminal.send(RemoteCommand::NudgeResize);
    let _subscription = host.subscribed(frame(100, 30));
    events_until(&terminal, |event| {
        matches!(event, TerminalEvent::ResetTitle)
    });
    assert_eq!(resized(&mut input), (120, 40));
    write_message(&mut input, &Response::Reply(RemoteReply::Ok)).unwrap();
    // The new size is the claim now, until a frame shows it.
    assert!(!terminal.host_size_overridden());
}

/// Review finding 9: a pane whose `Create` reply came after the client
/// gave up is closed.
#[test]
fn a_pane_created_after_the_client_gave_up_is_closed() {
    let host = FakeHost::new();
    let client = SessionClient::connect(host.root.path()).unwrap();
    let mut connection = host.next();
    let stream = client.connection.stream.lock().unwrap();
    stream
        .as_ref()
        .unwrap()
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    drop(stream);
    let launch = PaneLaunch {
        size: TerminalSize::default(),
        working_directory: None,
        shell_integration: None,
        config: TerminalRuntimeConfig::default(),
        launch: None,
        environment: None,
    };
    assert!(client.create_detached(launch).is_err());
    let Request::Create(_) = read_message(&mut connection).unwrap() else {
        panic!("expected a create");
    };
    let info = PaneInfo {
        id: "late".into(),
        child_pid: None,
        title: None,
        working_directory: None,
        exited: false,
        shell: None,
    };
    let late = Response::Attached(info, Arc::new(frame(80, 24)));
    write_message(&mut connection, &late).unwrap();
    let mut close = host.next();
    match read_message(&mut close).unwrap() {
        Request::Close(id) => assert_eq!(id, "late"),
        _ => panic!("expected the late pane to be closed"),
    }
    write_message(&mut close, &Response::Ok).unwrap();
}

/// Input keeps its order; a newer size takes the place of the queued
/// one, behind the input queued before it.
#[test]
fn coalesced_commands_keep_their_order_with_typing() {
    let queue = InputQueue::default();
    for command in [
        RemoteCommand::Write(b"a".to_vec()),
        RemoteCommand::Resize(size(80, 24)),
        RemoteCommand::Write(b"b".to_vec()),
        RemoteCommand::Resize(size(100, 30)),
        RemoteCommand::Write(b"c".to_vec()),
    ] {
        assert_eq!(
            queue.push(Input {
                command,
                reply: None,
            }),
            Queued::Yes
        );
    }
    queue.close();
    let order: Vec<String> = std::iter::from_fn(|| queue.pop())
        .map(|input| match input.command {
            RemoteCommand::Write(bytes) => String::from_utf8(bytes).unwrap(),
            RemoteCommand::Resize(size) => size.cols.to_string(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(order, ["a", "b", "100", "c"]);
}

/// Audit B10: a stream that ends after `Exit` is how a closed pane ends,
/// not a lost connection.
#[test]
fn the_stream_of_a_pane_that_ended_closes_quietly() {
    let host = FakeHost::new();
    let (terminal, _rpc, _input, mut subscription) = host.attach();
    let exit = Update::Events(vec![TerminalEvent::Exit]);
    write_message(&mut subscription, &exit).unwrap();
    drop(subscription);
    let events = events_until(&terminal, |event| matches!(event, TerminalEvent::Exit));
    std::thread::sleep(REATTACH_MIN_DELAY * 2);
    assert!(host.connections.is_empty(), "the view subscribed again");
    assert!(!events.iter().any(is_disconnected_title));
    assert!(!terminal.take_events().iter().any(is_disconnected_title));
}

/// Audit B9: queuing input never blocks the UI. Sizes and options keep
/// only their latest value; typed input beyond the queue is dropped.
#[test]
fn a_full_input_queue_keeps_the_latest_size_and_drops_typing() {
    let queue = InputQueue::default();
    let input = |command| Input {
        command,
        reply: None,
    };
    assert_eq!(
        queue.push(input(RemoteCommand::Resize(size(80, 24)))),
        Queued::Yes
    );
    for _ in 0..INPUT_CAPACITY - 1 {
        assert_eq!(
            queue.push(input(RemoteCommand::Write(b"x".to_vec()))),
            Queued::Yes
        );
    }
    assert_eq!(
        queue.push(input(RemoteCommand::Write(b"y".to_vec()))),
        Queued::Dropped
    );
    assert_eq!(
        queue.push(input(RemoteCommand::Resize(size(100, 30)))),
        Queued::Yes
    );
    assert_eq!(
        queue.push(input(RemoteCommand::Resize(size(120, 40)))),
        Queued::Yes
    );
    let waited = Instant::now();
    let (reply, _) = flume::bounded(1);
    let paste = Input {
        command: RemoteCommand::Write(b"z".to_vec()),
        reply: Some(reply),
    };
    assert!(
        queue
            .push_until(paste, Instant::now() + Duration::from_millis(50))
            .is_err()
    );
    assert!(waited.elapsed() >= Duration::from_millis(50));
    queue.close();
    let mut resizes = Vec::new();
    let mut writes = 0;
    while let Some(Input { command, .. }) = queue.pop() {
        match command {
            RemoteCommand::Resize(size) => resizes.push(size.cols),
            RemoteCommand::Write(_) => writes += 1,
            _ => unreachable!(),
        }
    }
    assert_eq!(resizes, [120]);
    assert_eq!(writes, INPUT_CAPACITY - 1);
}
