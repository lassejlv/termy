use super::*;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn dropping_a_hup_ignoring_child_has_bounded_shutdown() {
    const MARKER: &[u8] = b"TMON_HUP_IGNORED_READY";

    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (exit_tx, exit_rx) = mpsc::sync_channel(1);
    let mut output = Vec::new();
    let mut ready_reported = false;
    let terminal = Pty::spawn(
        SpawnConfig {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "trap '' HUP; printf TMON_HUP_IGNORED_READY; exec sleep 30".to_string(),
            ],
            working_directory: None,
            environment: Vec::new(),
        },
        test_size(),
        move |bytes| {
            output.extend_from_slice(bytes);
            if !ready_reported && output.windows(MARKER.len()).any(|part| part == MARKER) {
                ready_reported = true;
                let _ = ready_tx.send(());
            }
            Vec::new()
        },
        move || {
            let _ = exit_tx.send(());
        },
    )
    .expect("HUP-ignoring PTY child should start");
    let child_pid = terminal.child_pid;
    let cleanup = KillProcessOnDrop(child_pid);
    ready_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("child should install its HUP handler before terminal drop");

    drop(terminal);
    let exited = exit_rx.recv_timeout(Duration::from_millis(750)).is_ok();
    if !exited {
        force_child_shutdown(child_pid);
        let _ = exit_rx.recv_timeout(Duration::from_secs(2));
    }
    // The child has either exited through the implementation under test or
    // was killed explicitly above, so the cleanup guard must not target a
    // subsequently reused PID during normal scope teardown.
    std::mem::forget(cleanup);

    assert!(exited, "dropping a PTY left its HUP-ignoring child alive");
}

#[test]
fn child_environment_rejects_invalid_override_names() {
    for name in ["", "BAD=NAME"] {
        let error = child_environment(&[(name.to_string(), "value".to_string())], None)
            .expect_err("invalid environment name should fail");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
}

#[test]
fn child_environment_reports_the_actual_working_directory() {
    let directory = Path::new("/tmp/tmon working directory");
    let environment = child_environment(
        &[("PWD".to_string(), "/stale/value".to_string())],
        Some(directory),
    )
    .expect("working directory should be representable in the child environment");
    assert!(
        environment
            .iter()
            .any(|entry| { entry.as_bytes() == b"PWD=/tmp/tmon working directory" })
    );
}

#[test]
fn missing_absolute_executable_fails_synchronously() {
    let directory = TestDirectory::new("missing-executable");
    let error = assert_spawn_error(
        SpawnConfig {
            program: directory
                .path()
                .join("does-not-exist")
                .to_string_lossy()
                .into_owned(),
            args: Vec::new(),
            working_directory: None,
            environment: Vec::new(),
        },
        ErrorKind::NotFound,
    );
    assert!(error.to_string().contains("execute terminal program"));
}

#[test]
fn non_executable_file_fails_synchronously() {
    let directory = TestDirectory::new("non-executable");
    let program = directory.path().join("program");
    write_test_file(&program, b"#!/bin/sh\nexit 0\n", 0o644);

    let error = assert_spawn_error(
        SpawnConfig {
            program: program.to_string_lossy().into_owned(),
            args: Vec::new(),
            working_directory: None,
            environment: Vec::new(),
        },
        ErrorKind::PermissionDenied,
    );
    assert!(error.to_string().contains("execute terminal program"));
}

#[test]
fn missing_working_directory_fails_synchronously() {
    let directory = TestDirectory::new("missing-working-directory");
    let error = assert_spawn_error(
        SpawnConfig {
            program: "/bin/sh".to_string(),
            args: Vec::new(),
            working_directory: Some(directory.path().join("does-not-exist")),
            environment: Vec::new(),
        },
        ErrorKind::NotFound,
    );
    assert!(
        error
            .to_string()
            .contains("change terminal working directory")
    );
}

#[test]
fn relative_path_entry_uses_child_working_directory() {
    const SENTINEL: &[u8] = b"TMON_CHILD_CWD_PATH";

    let directory = TestDirectory::new("relative-path");
    let child_directory = directory.path().join("child");
    let bin_directory = child_directory.join("bin");
    std::fs::create_dir_all(&bin_directory).expect("child bin directory should be created");
    let program = bin_directory.join("tmon-relative-path-probe");
    write_test_file(&program, b"#!/bin/sh\nprintf TMON_CHILD_CWD_PATH\n", 0o755);
    let config = SpawnConfig {
        program: "tmon-relative-path-probe".to_string(),
        args: Vec::new(),
        working_directory: Some(child_directory),
        environment: vec![("PATH".to_string(), "bin".to_string())],
    };

    let resolved = resolve_program(&config).expect("relative PATH entry should resolve");
    assert!(resolved.is_absolute());
    assert_eq!(
        resolved,
        program
            .canonicalize()
            .expect("test executable should canonicalize")
    );

    let (sentinel_tx, sentinel_rx) = mpsc::sync_channel(1);
    let (exit_tx, exit_rx) = mpsc::sync_channel(1);
    let mut output = Vec::new();
    let mut sentinel_reported = false;
    let terminal = Pty::spawn(
        config,
        test_size(),
        move |bytes| {
            output.extend_from_slice(bytes);
            if !sentinel_reported && output.windows(SENTINEL.len()).any(|part| part == SENTINEL) {
                sentinel_reported = true;
                let _ = sentinel_tx.send(());
            }
            Vec::new()
        },
        move || {
            let _ = exit_tx.send(());
        },
    )
    .expect("child-cwd relative PATH executable should start");

    sentinel_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("resolved child executable should produce its sentinel");
    exit_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("resolved child executable should exit");
    drop(terminal);
}

#[test]
fn zero_timeout_exit_drain_reads_buffered_data_without_waiting_for_eof() {
    const SENTINEL: &[u8] = b"TMON_TAIL_SENTINEL";

    let (reader_stream, mut writer_stream) = UnixStream::pair().expect("socket pair should open");
    writer_stream
        .write_all(SENTINEL)
        .expect("sentinel should be buffered");
    // SAFETY: ownership of the socket fd is transferred into `reader`.
    let mut reader = unsafe { File::from_raw_fd(reader_stream.into_raw_fd()) };
    let mut buffer = [0u8; 32 * 1024];
    let mut output = Vec::new();
    drain_ready_pty_output(&mut reader, &mut buffer, &mut |bytes| {
        output.extend_from_slice(bytes);
        Vec::new()
    });

    assert_eq!(output, SENTINEL);
    // The peer deliberately remains open through the drain, proving the
    // zero-timeout loop did not wait for EOF or descendant-style closure.
    writer_stream
        .write_all(b"still-open")
        .expect("drain should not close the peer");
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn exit_drain_preserves_buffered_tail_beyond_legacy_eight_mib_cap() {
    const BUFFER_BYTES: usize = 32 * 1024;
    const LEGACY_EXIT_DRAIN_READS: usize = 256;
    const TAIL_BYTES: usize = (LEGACY_EXIT_DRAIN_READS + 1) * BUFFER_BYTES;

    let directory = TestDirectory::new("large-exit-tail");
    let tail_path = directory.path().join("tail");
    let mut tail_writer = File::create(&tail_path).expect("tail file should be created");
    let chunk = [b'T'; BUFFER_BYTES];
    for _ in 0..=LEGACY_EXIT_DRAIN_READS {
        tail_writer
            .write_all(&chunk)
            .expect("complete tail should be queued");
    }
    drop(tail_writer);

    let mut reader = File::open(&tail_path).expect("queued tail should be readable");
    let mut buffer = [0u8; BUFFER_BYTES];
    let mut delivered = 0;
    drain_ready_pty_output(&mut reader, &mut buffer, &mut |bytes| {
        delivered += bytes.len();
        Vec::new()
    });

    assert_eq!(
        delivered, TAIL_BYTES,
        "all output already readable at direct-child exit must precede the exit callback"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn direct_child_large_tail_is_complete_before_exit_callback() {
    const HELPER_ENV: &str = "TMON_PTY_LARGE_EXIT_TAIL_HELPER";
    const BEGIN: &[u8] = b"<TMON_LARGE_EXIT_TAIL_BEGIN>";
    const END: &[u8] = b"<TMON_LARGE_EXIT_TAIL_END>";
    const CHUNK_BYTES: usize = 32 * 1024;
    const LEGACY_EXIT_DRAIN_READS: usize = 256;
    const TAIL_BYTES: usize = (LEGACY_EXIT_DRAIN_READS + 1) * CHUNK_BYTES;

    if std::env::var_os(HELPER_ENV).as_deref() == Some(OsStr::new("1")) {
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(BEGIN)
            .expect("helper should write the leading marker");
        let chunk = [b'T'; CHUNK_BYTES];
        for _ in 0..=LEGACY_EXIT_DRAIN_READS {
            stdout
                .write_all(&chunk)
                .expect("helper should write its complete exit tail");
        }
        stdout
            .write_all(END)
            .expect("helper should write the trailing marker");
        stdout.flush().expect("helper should flush its exit tail");
        // Exit immediately after the tail so the watcher and reader race in
        // the same way as a real short-lived terminal command.
        unsafe { _exit(0) }
    }

    let executable = std::env::current_exe().expect("test executable should exist");
    let output = Arc::new(Mutex::new(Vec::with_capacity(
        BEGIN.len() + TAIL_BYTES + END.len(),
    )));
    let callback_output = output.clone();
    let (exit_tx, exit_rx) = mpsc::sync_channel(1);
    let terminal = Pty::spawn(
        SpawnConfig {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "tmon::pty::tests::process::direct_child_large_tail_is_complete_before_exit_callback"
                    .to_string(),
                "--exact".to_string(),
                "--nocapture".to_string(),
            ],
            working_directory: None,
            environment: vec![(HELPER_ENV.to_string(), "1".to_string())],
        },
        test_size(),
        move |bytes| {
            callback_output
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend_from_slice(bytes);
            Vec::new()
        },
        move || {
            let _ = exit_tx.send(());
        },
    )
    .expect("large-tail PTY helper should start");

    exit_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("large-tail helper should exit after all output is delivered");
    let output = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let begin = output
        .windows(BEGIN.len())
        .position(|window| window == BEGIN)
        .expect("leading marker should be delivered")
        + BEGIN.len();
    let end = output[begin..]
        .windows(END.len())
        .position(|window| window == END)
        .map(|position| begin + position)
        .expect("trailing marker should precede the exit callback");
    assert_eq!(
        end - begin,
        TAIL_BYTES,
        "the complete tail beyond the legacy eight MiB cap should be delivered"
    );
    assert!(
        output[begin..end].iter().all(|byte| *byte == b'T'),
        "the large tail should not be reordered or corrupted"
    );
    drop(output);
    drop(terminal);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn exit_drain_is_time_bounded_for_a_live_descendant_style_source() {
    let (reader_stream, mut writer_stream) = UnixStream::pair().expect("socket pair should open");
    writer_stream
        .set_nonblocking(true)
        .expect("producer should be nonblocking");
    let keep_writing = Arc::new(AtomicBool::new(true));
    let producer_control = keep_writing.clone();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let producer = thread::spawn(move || {
        let chunk = [b'D'; 32 * 1024];
        let mut ready_reported = false;
        while producer_control.load(Ordering::Acquire) {
            match writer_stream.write(&chunk) {
                Ok(0) => break,
                Ok(_) => {
                    if !ready_reported {
                        ready_reported = true;
                        let _ = ready_tx.send(());
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::yield_now();
                }
                Err(_) => break,
            }
        }
    });
    ready_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("producer should queue descendant-style output");

    // SAFETY: ownership of the socket fd is transferred into `reader`.
    let mut reader = unsafe { File::from_raw_fd(reader_stream.into_raw_fd()) };
    let mut buffer = [0u8; 32 * 1024];
    let mut delivered = 0;
    let started = Instant::now();

    drain_ready_pty_output(&mut reader, &mut buffer, &mut |bytes| {
        delivered += bytes.len();
        Vec::new()
    });

    keep_writing.store(false, Ordering::Release);
    drop(reader);
    producer.join().expect("producer should stop cleanly");
    assert!(delivered > 0, "the ready source should be drained");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "continuous descendant output must not indefinitely delay direct-child exit"
    );
}

#[test]
fn direct_child_exit_delivers_final_sentinel_before_exit_callback() {
    const SENTINEL: &[u8] = b"TMON_FINAL_SENTINEL";

    let sentinel_seen = Arc::new(AtomicBool::new(false));
    let output_sentinel_seen = sentinel_seen.clone();
    let (exit_tx, exit_rx) = mpsc::sync_channel(1);
    let mut output = Vec::new();
    let terminal = Pty::spawn(
        SpawnConfig {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'TMON_FINAL_SENTINEL\\n'; exit 0".to_string(),
            ],
            working_directory: None,
            environment: Vec::new(),
        },
        test_size(),
        move |bytes| {
            output.extend_from_slice(bytes);
            if output.windows(SENTINEL.len()).any(|part| part == SENTINEL) {
                output_sentinel_seen.store(true, Ordering::Release);
            }
            Vec::new()
        },
        move || {
            let _ = exit_tx.send(sentinel_seen.load(Ordering::Acquire));
        },
    )
    .expect("PTY should start");

    assert!(
        exit_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("exit callback should run after delivering the final sentinel"),
        "exit callback ran before the final PTY sentinel was delivered"
    );
    drop(terminal);
}

#[test]
fn direct_child_exit_is_reported_while_descendant_holds_slave_open() {
    const MARKER: &[u8] = b"TMON_DESCENDANT_READY:";

    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let mut output = Vec::new();
    let mut acknowledged = false;
    let exit_count = Arc::new(AtomicUsize::new(0));
    let callback_count = exit_count.clone();
    let (exit_tx, exit_rx) = mpsc::channel();
    let exit_sender_guard = exit_tx.clone();

    let terminal = Pty::spawn(
        SpawnConfig {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                concat!(
                    "trap '' HUP\n",
                    "sleep 30 &\n",
                    "printf 'TMON_DESCENDANT_READY:%s\\n' \"$!\"\n",
                    "read confirmation\n",
                    "exit 0\n",
                )
                .to_string(),
            ],
            working_directory: None,
            environment: Vec::new(),
        },
        test_size(),
        move |bytes| {
            output.extend_from_slice(bytes);
            if acknowledged {
                return Vec::new();
            }

            let Some(marker) = output.windows(MARKER.len()).position(|part| part == MARKER) else {
                return Vec::new();
            };
            let pid_start = marker + MARKER.len();
            let Some(line_length) = output[pid_start..].iter().position(|byte| *byte == b'\n')
            else {
                return Vec::new();
            };
            let pid = std::str::from_utf8(&output[pid_start..pid_start + line_length])
                .map_err(|error| error.to_string())
                .and_then(|text| {
                    text.trim()
                        .parse::<c_int>()
                        .map_err(|error| error.to_string())
                });
            let _ = ready_tx.send(pid);
            acknowledged = true;
            b"\n".to_vec()
        },
        move || {
            callback_count.fetch_add(1, Ordering::AcqRel);
            let _ = exit_tx.send(());
        },
    )
    .expect("PTY should start");

    let descendant_pid = ready_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("background descendant should report its PID")
        .expect("background descendant PID should be valid");
    let descendant = KillProcessOnDrop(descendant_pid);
    // Signal 0 only checks that the background descendant is still alive.
    // SAFETY: `descendant_pid` was just reported by the direct child.
    assert_eq!(unsafe { kill(descendant_pid, 0) }, 0);

    exit_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("direct-child exit should not wait for the descendant's PTY descriptor");
    assert!(matches!(
        exit_rx.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(exit_count.load(Ordering::Acquire), 1);

    drop(exit_sender_guard);
    drop(terminal);
    drop(descendant);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn resize_preempts_saturated_write_and_preserves_input_order() {
    const HELPER_ENV: &str = "TMON_PTY_BACKPRESSURE_TEST_HELPER";
    const READY: &[u8] = b"TMON_BACKPRESSURE_READY";
    const WINCH: &[u8] = b"TMON_BACKPRESSURE_WINCH";
    const ORDERED: &[u8] = b"TMON_BACKPRESSURE_ORDERED";
    const WRITE_COUNT: usize = 300;
    const CHUNK_BYTES: usize = 16 * 1024;

    fn identifiable_write(index: usize) -> Vec<u8> {
        let mut bytes = vec![(index % 251) as u8; CHUNK_BYTES];
        bytes[..size_of::<u32>()].copy_from_slice(&(index as u32).to_le_bytes());
        bytes
    }

    if std::env::var_os(HELPER_ENV).as_deref() == Some(OsStr::new("1")) {
        let mut input = io::stdin().lock();
        let mut chunk = vec![0; CHUNK_BYTES];
        for index in 0..WRITE_COUNT {
            input
                .read_exact(&mut chunk)
                .unwrap_or_else(|error| panic!("helper did not receive write {index}: {error}"));
            assert!(
                chunk == identifiable_write(index),
                "write {index} was reordered or corrupted"
            );
        }
        io::stdout()
            .write_all(ORDERED)
            .expect("helper should report ordered input");
        io::stdout()
            .flush()
            .expect("helper should flush its ordered-input marker");
        return;
    }

    let executable = std::env::current_exe().expect("test executable should exist");
    let script = concat!(
        "stty raw -echo\n",
        "resized=\n",
        "trap 'resized=1; printf TMON_BACKPRESSURE_WINCH' WINCH\n",
        "printf TMON_BACKPRESSURE_READY\n",
        "while [ -z \"$resized\" ]; do :; done\n",
        "exec \"$1\" \"$2\" --exact --nocapture\n",
    );
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (winch_tx, winch_rx) = mpsc::sync_channel(1);
    let (ordered_tx, ordered_rx) = mpsc::sync_channel(1);
    let (exit_tx, exit_rx) = mpsc::sync_channel(1);
    let mut output = Vec::new();
    let mut ready_reported = false;
    let mut winch_reported = false;
    let mut ordered_reported = false;
    let terminal = Pty::spawn(
        SpawnConfig {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                script.to_string(),
                "tmon-backpressure-probe".to_string(),
                executable.to_string_lossy().into_owned(),
                "tmon::pty::tests::process::resize_preempts_saturated_write_and_preserves_input_order"
                    .to_string(),
            ],
            working_directory: None,
            environment: vec![(HELPER_ENV.to_string(), "1".to_string())],
        },
        test_size(),
        move |bytes| {
            output.extend_from_slice(bytes);
            if !ready_reported && output.windows(READY.len()).any(|part| part == READY) {
                ready_reported = true;
                let _ = ready_tx.send(());
            }
            if !winch_reported && output.windows(WINCH.len()).any(|part| part == WINCH) {
                winch_reported = true;
                let _ = winch_tx.send(());
            }
            if !ordered_reported && output.windows(ORDERED.len()).any(|part| part == ORDERED) {
                ordered_reported = true;
                let _ = ordered_tx.send(());
            }
            Vec::new()
        },
        move || {
            let _ = exit_tx.send(());
        },
    )
    .expect("backpressure PTY should start");

    ready_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("child should stop reading after configuring its PTY");
    for index in 0..WRITE_COUNT {
        terminal
            .write_owned(identifiable_write(index))
            .unwrap_or_else(|error| panic!("write {index} should queue: {error}"));
    }
    terminal
        .resize(Size {
            cols: 100,
            ..test_size()
        })
        .expect("resize should be published outside the write backlog");

    winch_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("resize should preempt the saturated write");
    ordered_rx
        .recv_timeout(Duration::from_secs(8))
        .expect("queued input should retain byte order after backpressure clears");
    exit_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("ordering helper should exit after reading all input");
    drop(terminal);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn canonical_unicode_erase_uses_iutf8() {
    const READY: &[u8] = b"TMON_IUTF8_READY";
    const RESULT: &[u8] = b"TMON_IUTF8_RESULT:";

    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let (exit_tx, exit_rx) = mpsc::channel();
    let mut output = Vec::new();
    let mut input_sent = false;
    let mut result_sent = false;
    let terminal = Pty::spawn(
        SpawnConfig {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                concat!(
                    "stty -echo erase '^?'\n",
                    "printf TMON_IUTF8_READY\n",
                    "IFS= read -r line\n",
                    "printf 'TMON_IUTF8_RESULT:%s\\n' \"$line\"\n",
                    "read confirmation\n",
                )
                .to_string(),
            ],
            working_directory: None,
            environment: Vec::new(),
        },
        test_size(),
        move |bytes| {
            output.extend_from_slice(bytes);
            let mut reply = Vec::new();
            if !input_sent && output.windows(READY.len()).any(|part| part == READY) {
                input_sent = true;
                // Canonical erase should remove the complete two-byte `é`,
                // leaving only `X` for the shell's read.
                reply.extend_from_slice(&[0xc3, 0xa9, 0x7f, b'X', b'\n']);
            }

            if !result_sent
                && let Some(marker) = output.windows(RESULT.len()).position(|part| part == RESULT)
            {
                let value_start = marker + RESULT.len();
                if let Some(line_length) =
                    output[value_start..].iter().position(|byte| *byte == b'\n')
                {
                    let mut value = output[value_start..value_start + line_length].to_vec();
                    if value.last() == Some(&b'\r') {
                        value.pop();
                    }
                    let _ = result_tx.send(value);
                    result_sent = true;
                    reply.push(b'\n');
                }
            }
            reply
        },
        move || {
            let _ = exit_tx.send(());
        },
    )
    .expect("PTY should start");

    assert_eq!(
        result_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("child should report the canonical input"),
        b"X"
    );
    exit_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("child should exit after the result acknowledgment");
    drop(terminal);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SignalObservation {
    Ready,
    Survived,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn child_resets_inherited_ignored_signals() {
    const HELPER_ENV: &str = "TMON_SIGNAL_RESET_TEST_HELPER";
    if std::env::var_os(HELPER_ENV).as_deref() != Some(OsStr::new("1")) {
        let status = Command::new(std::env::current_exe().expect("test executable should exist"))
            .arg("tmon::pty::tests::process::child_resets_inherited_ignored_signals")
            .arg("--exact")
            .arg("--nocapture")
            .env(HELPER_ENV, "1")
            .status()
            .expect("isolated signal test should start");
        assert!(status.success(), "isolated signal test failed: {status}");
        return;
    }

    const READY: &[u8] = b"TMON_SIGNAL_READY";
    const SURVIVED: &[u8] = b"TMON_SIGNAL_SURVIVED";
    let (observation_tx, observation_rx) = mpsc::channel();
    let (exit_tx, exit_rx) = mpsc::channel();
    let mut output = Vec::new();
    let mut ready_acknowledged = false;
    let mut survived_acknowledged = false;
    let config = SpawnConfig {
        program: "/bin/sh".to_string(),
        args: vec![
            "-c".to_string(),
            concat!(
                "printf TMON_SIGNAL_READY\n",
                "read ready\n",
                "kill -TERM $$\n",
                "printf TMON_SIGNAL_SURVIVED\n",
                "read survived\n",
            )
            .to_string(),
        ],
        working_directory: None,
        environment: Vec::new(),
    };

    // This process is an isolated test helper, so temporarily changing its
    // process-wide disposition cannot interfere with the main test runner.
    // SAFETY: SIG_IGN is the POSIX sentinel value, and the exact prior
    // disposition is restored immediately after forkpty returns.
    let previous = unsafe { signal(SIGTERM, SIGNAL_IGNORE) };
    assert_ne!(previous, SIGNAL_ERROR, "ignoring SIGTERM should succeed");
    let terminal = Pty::spawn(
        config,
        test_size(),
        move |bytes| {
            output.extend_from_slice(bytes);
            if !ready_acknowledged && output.windows(READY.len()).any(|part| part == READY) {
                ready_acknowledged = true;
                let _ = observation_tx.send(SignalObservation::Ready);
                return b"\n".to_vec();
            }
            if !survived_acknowledged && output.windows(SURVIVED.len()).any(|part| part == SURVIVED)
            {
                survived_acknowledged = true;
                let _ = observation_tx.send(SignalObservation::Survived);
                return b"\n".to_vec();
            }
            Vec::new()
        },
        move || {
            let _ = exit_tx.send(());
        },
    );
    // SAFETY: `previous` is the handler returned by the successful signal call above.
    let restore_result = unsafe { signal(SIGTERM, previous) };
    assert_ne!(
        restore_result, SIGNAL_ERROR,
        "restoring SIGTERM should succeed"
    );
    let terminal = terminal.expect("PTY should start");

    assert_eq!(
        observation_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("child should reach the pre-signal handshake"),
        SignalObservation::Ready
    );
    exit_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("SIGTERM should terminate the child promptly");
    assert_ne!(
        observation_rx.try_recv(),
        Ok(SignalObservation::Survived),
        "the child inherited SIG_IGN instead of resetting SIGTERM"
    );
    drop(terminal);
}
