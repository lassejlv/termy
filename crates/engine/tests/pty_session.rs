use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use engine::pty::{PTY_OUTPUT_BUFFER_LIMIT, PtyCommand, PtyEvent, PtySession, pty_size};

#[test]
fn native_pty_runs_a_real_child_and_streams_output() {
    let (sender, receiver) = mpsc::channel();
    let session = PtySession::spawn(
        &PtyCommand::new("/bin/sh").with_arguments([
            "-c",
            "printf '%s|' \"$TERM\"; /usr/bin/tput clear >/dev/null && printf tmon-pty-ok",
        ]),
        pty_size(80, 24, 9.0, 18.0),
        move |event| {
            let _ = sender.send(event);
        },
    )
    .expect("PTY session starts");
    assert!(session.child_pid().is_some());

    let mut output = Vec::new();
    for _ in 0..10 {
        match receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("PTY event arrives")
        {
            PtyEvent::Wake => {
                output.extend(session.take_output().expect("PTY output drains"));
                if output
                    .windows(b"xterm-256color|tmon-pty-ok".len())
                    .any(|window| window == b"xterm-256color|tmon-pty-ok")
                {
                    return;
                }
            }
            PtyEvent::Exit { .. } => {}
            PtyEvent::ReadError(error) => panic!("PTY read failed: {error}"),
        }
    }
    panic!(
        "expected child output, got {}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn burst_output_is_buffered_behind_a_single_pending_wakeup() {
    const OUTPUT_BYTES: usize = PTY_OUTPUT_BUFFER_LIMIT / 2;
    let (sender, receiver) = mpsc::channel();
    let session = PtySession::spawn(
        &PtyCommand::new("/bin/sh").with_arguments([
            "-c",
            "/usr/bin/yes x | /usr/bin/tr -d '\n' | /usr/bin/head -c 262144",
        ]),
        pty_size(80, 24, 9.0, 18.0),
        move |event| {
            let _ = sender.send(event);
        },
    )
    .expect("PTY session starts");

    let mut wakes = 0;
    loop {
        match receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("PTY burst event arrives")
        {
            PtyEvent::Wake => wakes += 1,
            PtyEvent::Exit { .. } => break,
            PtyEvent::ReadError(error) => panic!("PTY read failed: {error}"),
        }
    }
    std::thread::sleep(Duration::from_millis(50));
    for event in receiver.try_iter() {
        match event {
            PtyEvent::Wake => wakes += 1,
            PtyEvent::Exit { .. } => {}
            PtyEvent::ReadError(error) => panic!("PTY read failed: {error}"),
        }
    }

    assert_eq!(wakes, 1);
    assert_eq!(
        session.take_output().expect("PTY output drains").len(),
        OUTPUT_BYTES
    );
}

#[test]
fn oversized_output_is_lossless_ordered_bounded_and_rearms_wakes() {
    const TOKEN: &[u8] = b"0123456789abcdef";
    const OUTPUT_BYTES: usize = PTY_OUTPUT_BUFFER_LIMIT * 3 + 123;
    let command = format!(
        "/usr/bin/yes {} | /usr/bin/tr -d '\n' | /usr/bin/head -c {OUTPUT_BYTES}",
        String::from_utf8_lossy(TOKEN)
    );
    let (sender, receiver) = mpsc::channel();
    let session = PtySession::spawn(
        &PtyCommand::new("/bin/sh").with_arguments(["-c", command.as_str()]),
        pty_size(80, 24, 9.0, 18.0),
        move |event| {
            let _ = sender.send(event);
        },
    )
    .expect("PTY session starts");

    match receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("initial PTY wake arrives")
    {
        PtyEvent::Wake => {}
        PtyEvent::Exit { .. } => panic!("oversized child exited before its output wake"),
        PtyEvent::ReadError(error) => panic!("PTY read failed: {error}"),
    }
    // Let the child outrun the consumer. The worker must stop at the explicit userspace budget.
    std::thread::sleep(Duration::from_millis(100));
    let blocked = session.buffer_metrics().expect("buffer metrics read");
    assert!(blocked.pending_bytes <= PTY_OUTPUT_BUFFER_LIMIT);
    assert!(blocked.high_water_bytes <= PTY_OUTPUT_BUFFER_LIMIT);
    assert!(blocked.high_water_bytes >= PTY_OUTPUT_BUFFER_LIMIT / 2);
    assert!(
        blocked.producer_waits > 0,
        "producer should observe backpressure"
    );

    let mut output = Vec::with_capacity(OUTPUT_BYTES);
    let mut scratch = Vec::new();
    session
        .drain_output_into(&mut scratch)
        .expect("initial PTY output drains");
    output.extend_from_slice(&scratch);
    let mut wakes = 1_u64;
    loop {
        match receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("bounded PTY makes progress without a lost wake")
        {
            PtyEvent::Wake => {
                wakes += 1;
                session
                    .drain_output_into(&mut scratch)
                    .expect("PTY output drains into reusable scratch");
                output.extend_from_slice(&scratch);
            }
            PtyEvent::Exit { .. } => {
                session
                    .drain_output_into(&mut scratch)
                    .expect("final PTY output drains");
                output.extend_from_slice(&scratch);
                break;
            }
            PtyEvent::ReadError(error) => panic!("PTY read failed: {error}"),
        }
    }

    let expected = (0..OUTPUT_BYTES)
        .map(|index| TOKEN[index % TOKEN.len()])
        .collect::<Vec<_>>();
    assert_eq!(
        output, expected,
        "bounded backpressure must not reorder or drop bytes"
    );
    let metrics = session.buffer_metrics().expect("final buffer metrics read");
    assert_eq!(metrics.pending_bytes, 0);
    assert_eq!(metrics.bytes_buffered, OUTPUT_BYTES as u64);
    assert_eq!(metrics.bytes_drained, OUTPUT_BYTES as u64);
    assert_eq!(metrics.wake_events, wakes);
    assert!(wakes > 1, "each bounded drain must rearm the wake edge");
}

#[test]
fn sustained_output_clears_the_debug_throughput_floor() {
    const OUTPUT_BYTES: u32 = 8 * 1024 * 1024;
    const MINIMUM_BYTES_PER_SECOND: f64 = 8.0 * 1024.0 * 1024.0;
    let command =
        format!("/usr/bin/yes x | /usr/bin/tr -d '\\n' | /usr/bin/head -c {OUTPUT_BYTES}");
    let (sender, receiver) = mpsc::channel();
    let started = Instant::now();
    let session = PtySession::spawn(
        &PtyCommand::new("/bin/sh").with_arguments(["-c", command.as_str()]),
        pty_size(80, 24, 9.0, 18.0),
        move |event| {
            let _ = sender.send(event);
        },
    )
    .expect("sustained-output PTY starts");

    let mut streamed_bytes = 0_u32;
    let mut scratch = Vec::with_capacity(PTY_OUTPUT_BUFFER_LIMIT);
    loop {
        match receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("sustained PTY output keeps making progress")
        {
            PtyEvent::Wake => {
                session
                    .drain_output_into(&mut scratch)
                    .expect("sustained PTY output drains");
                assert!(scratch.iter().all(|byte| *byte == b'x'));
                streamed_bytes += u32::try_from(scratch.len()).expect("PTY chunk length fits u32");
            }
            PtyEvent::Exit { code, signal } => {
                session
                    .drain_output_into(&mut scratch)
                    .expect("final sustained PTY output drains");
                assert!(scratch.iter().all(|byte| *byte == b'x'));
                streamed_bytes += u32::try_from(scratch.len()).expect("PTY chunk length fits u32");
                assert_eq!(code, 0, "unexpected child signal: {signal:?}");
                break;
            }
            PtyEvent::ReadError(error) => panic!("PTY read failed: {error}"),
        }
    }

    let elapsed = started.elapsed();
    let bytes_per_second = f64::from(streamed_bytes) / elapsed.as_secs_f64();
    eprintln!(
        "streamed {streamed_bytes} bytes in {elapsed:?} ({:.1} MiB/s)",
        bytes_per_second / (1024.0 * 1024.0)
    );
    assert_eq!(streamed_bytes, OUTPUT_BYTES);
    assert!(
        bytes_per_second >= MINIMUM_BYTES_PER_SECOND,
        "PTY throughput {:.1} MiB/s fell below the 8 MiB/s debug-build floor",
        bytes_per_second / (1024.0 * 1024.0)
    );
    let metrics = session.buffer_metrics().expect("PTY metrics read");
    assert_eq!(metrics.pending_bytes, 0);
    assert_eq!(metrics.bytes_buffered, u64::from(OUTPUT_BYTES));
    assert_eq!(metrics.bytes_drained, u64::from(OUTPUT_BYTES));
}

#[test]
fn native_pty_resize_is_visible_to_the_child_process() {
    let (sender, receiver) = mpsc::channel();
    let session = PtySession::spawn(
        &PtyCommand::new("/bin/sh")
            .with_arguments(["-c", "/bin/stty size; read line; /bin/stty size"]),
        pty_size(80, 24, 9.0, 18.0),
        move |event| {
            let _ = sender.send(event);
        },
    )
    .expect("PTY session starts");

    let mut output = Vec::new();
    loop {
        match receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("initial PTY output arrives")
        {
            PtyEvent::Wake => {
                output.extend(session.take_output().expect("PTY output drains"));
                if output.windows(5).any(|window| window == b"24 80") {
                    break;
                }
            }
            PtyEvent::Exit { .. } => panic!("child exited before resize"),
            PtyEvent::ReadError(error) => panic!("PTY read failed: {error}"),
        }
    }

    session
        .resize(pty_size(80, 24, 9.0, 18.0))
        .expect("duplicate initial PTY resize is accepted");
    session
        .resize(pty_size(120, 40, 9.0, 18.0))
        .expect("PTY resizes");
    session
        .resize(pty_size(120, 40, 9.0, 18.0))
        .expect("duplicate resized PTY size is accepted");
    assert_eq!(session.io_metrics().resize_requests, 3);
    assert_eq!(session.io_metrics().resize_ioctls, 1);
    assert_eq!(session.io_metrics().resize_suppressed, 2);
    session.write(b"\n").expect("PTY input writes");
    for _ in 0..10 {
        match receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("resized PTY output arrives")
        {
            PtyEvent::Wake => {
                output.extend(session.take_output().expect("PTY output drains"));
                if output.windows(6).any(|window| window == b"40 120") {
                    return;
                }
            }
            PtyEvent::Exit { .. } => {
                output.extend(session.take_output().expect("PTY output drains"));
            }
            PtyEvent::ReadError(error) => panic!("PTY read failed: {error}"),
        }
    }
    panic!(
        "resized dimensions missing from {:?}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn dropping_a_live_session_stops_and_joins_its_worker() {
    struct NotifyOnDrop(Option<mpsc::Sender<()>>);

    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    let (sender, receiver) = mpsc::channel();
    let callback_lifetime = NotifyOnDrop(Some(sender));
    let session = PtySession::spawn(
        &PtyCommand::new("/bin/sleep").with_arguments(["30"]),
        pty_size(80, 24, 9.0, 18.0),
        move |_| {
            let _ = &callback_lifetime;
        },
    )
    .expect("long-lived PTY session starts");

    let started = Instant::now();
    drop(session);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "PTY teardown should not wait for the child timeout"
    );
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("worker-owned callback is dropped before session teardown returns");
}

#[test]
fn dropping_a_backpressured_session_wakes_and_joins_its_worker() {
    let (sender, receiver) = mpsc::channel();
    let session = PtySession::spawn(
        &PtyCommand::new("/usr/bin/yes").with_arguments(["x"]),
        pty_size(80, 24, 9.0, 18.0),
        move |event| {
            let _ = sender.send(event);
        },
    )
    .expect("continuous-output PTY session starts");

    match receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("continuous output wakes the consumer")
    {
        PtyEvent::Wake => {}
        PtyEvent::Exit { .. } => panic!("continuous child exited unexpectedly"),
        PtyEvent::ReadError(error) => panic!("PTY read failed: {error}"),
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        session
            .buffer_metrics()
            .expect("buffer metrics read")
            .producer_waits
            > 0,
        "reader should be waiting on the full bounded buffer"
    );

    let started = Instant::now();
    drop(session);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "closing the output buffer must wake a backpressured worker"
    );
}

#[cfg(unix)]
#[test]
fn dropping_interrupts_a_hup_ignoring_session_with_an_open_slave_descendant() {
    let (sender, receiver) = mpsc::channel();
    let session = PtySession::spawn(
        &PtyCommand::new("/bin/sh").with_arguments([
            "-c",
            "trap '' HUP; (trap '' HUP; sleep 30) & printf teardown-ready; wait",
        ]),
        pty_size(80, 24, 9.0, 18.0),
        move |event| {
            let _ = sender.send(event);
        },
    )
    .expect("HUP-ignoring PTY session starts");

    let mut output = Vec::new();
    loop {
        match receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("HUP-ignoring child reaches its wait state")
        {
            PtyEvent::Wake => {
                output.extend(session.take_output().expect("PTY output drains"));
                if output
                    .windows(b"teardown-ready".len())
                    .any(|window| window == b"teardown-ready")
                {
                    break;
                }
            }
            PtyEvent::Exit { .. } => panic!("HUP-ignoring child exited before teardown"),
            PtyEvent::ReadError(error) => panic!("PTY read failed: {error}"),
        }
    }

    let started = Instant::now();
    drop(session);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "shutdown cancellation must interrupt the reader while a descendant retains the slave"
    );
}

#[cfg(unix)]
#[test]
fn dropping_reaps_a_hup_ignoring_child_that_closed_its_stdio() {
    let (sender, receiver) = mpsc::channel();
    let session = PtySession::spawn(
        &PtyCommand::new("/bin/sh").with_arguments([
            "-c",
            "trap '' HUP; printf close-ready; exec 0<&-; exec 1>&-; exec 2>&-; sleep 30",
        ]),
        pty_size(80, 24, 9.0, 18.0),
        move |event| {
            let _ = sender.send(event);
        },
    )
    .expect("closed-stdio PTY session starts");

    loop {
        match receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("child reports readiness before closing stdio")
        {
            PtyEvent::Wake => {
                if session
                    .take_output()
                    .expect("PTY output drains")
                    .windows(b"close-ready".len())
                    .any(|window| window == b"close-ready")
                {
                    break;
                }
            }
            PtyEvent::Exit { .. } => panic!("HUP-ignoring child exited before teardown"),
            PtyEvent::ReadError(error) => panic!("PTY read failed: {error}"),
        }
    }
    std::thread::sleep(Duration::from_millis(50));

    let started = Instant::now();
    drop(session);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "hard termination must precede join when the reader has already reached EOF"
    );
}
