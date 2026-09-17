use super::*;
use std::{
    fs,
    sync::atomic::AtomicUsize,
    time::{Duration, SystemTime},
};

static WINDOWS_TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let sequence = WINDOWS_TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tmon-conpty-{label}-{}-{sequence}-{timestamp}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("test directory should be created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn test_size() -> Size {
    Size {
        cols: 80,
        rows: 24,
        cell_width: 8.0,
        cell_height: 16.0,
    }
}

fn windows_command_processor() -> String {
    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
}

fn collect_session_output(config: SpawnConfig, interact: impl FnOnce(&Pty)) -> Vec<u8> {
    assert!(available(), "windows-latest must provide ConPTY");
    let output = Arc::new(Mutex::new(Vec::new()));
    let callback_output = output.clone();
    let exit_output = output;
    let (exit_sender, exit_receiver) = mpsc::channel();
    let terminal = Pty::spawn(
        config,
        test_size(),
        move |bytes| {
            callback_output
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend_from_slice(bytes);
            Vec::new()
        },
        move || {
            let snapshot = exit_output
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let _ = exit_sender.send(snapshot);
        },
    )
    .expect("ConPTY child should start");
    interact(&terminal);
    let output = exit_receiver
        .recv_timeout(Duration::from_secs(20))
        .expect("ConPTY child should exit before the test timeout");
    drop(terminal);
    output
}

fn output_contains(output: &[u8], marker: &str) -> bool {
    output
        .windows(marker.len())
        .any(|candidate| candidate == marker.as_bytes())
}

fn unique_marker(label: &str) -> String {
    let sequence = WINDOWS_TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("TMON_{label}_{}_{sequence}", std::process::id())
}

fn decode_nul_terminated(value: &[u16]) -> String {
    String::from_utf16(&value[..value.len() - 1]).expect("test data is valid UTF-16")
}

#[test]
fn win32_abi_layouts_match_the_sdk() {
    assert_eq!(mem::size_of::<Coord>(), 4);
    assert_eq!(mem::align_of::<Coord>(), 2);
    assert_eq!(mem::size_of::<AttributeStorageBlock>(), 16);
    assert_eq!(mem::align_of::<AttributeStorageBlock>(), 16);
    if cfg!(target_pointer_width = "64") {
        assert_eq!(mem::size_of::<StartupInfoW>(), 104);
        assert_eq!(mem::size_of::<StartupInfoExW>(), 112);
        assert_eq!(mem::size_of::<ProcessInformation>(), 24);
    } else {
        assert_eq!(mem::size_of::<StartupInfoW>(), 68);
        assert_eq!(mem::size_of::<StartupInfoExW>(), 72);
        assert_eq!(mem::size_of::<ProcessInformation>(), 16);
    }
    assert_eq!(
        mem::size_of::<CreatePseudoConsoleFn>(),
        mem::size_of::<usize>()
    );
    assert_eq!(
        mem::size_of::<ResizePseudoConsoleFn>(),
        mem::size_of::<usize>()
    );
    assert_eq!(
        mem::size_of::<ClosePseudoConsoleFn>(),
        mem::size_of::<usize>()
    );
}

#[test]
fn command_line_quotes_windows_backslashes_and_quotes() {
    let command = command_line(
        r#"C:\Program Files\shell.exe"#,
        &[
            "plain".to_string(),
            "".to_string(),
            r#"two words\"#.to_string(),
            r#"say "hi""#.to_string(),
        ],
    )
    .expect("command should encode");
    assert_eq!(
        decode_nul_terminated(&command),
        r#""C:\Program Files\shell.exe" plain "" "two words\\" "say \"hi\"""#
    );
}

#[test]
fn command_line_rejects_the_windows_length_limit() {
    let error = command_line("cmd.exe", &["x".repeat(MAX_COMMAND_LINE_UNITS)])
        .expect_err("oversized command should fail");
    assert_eq!(error.kind(), ErrorKind::InvalidInput);
}

#[test]
fn batch_command_line_uses_cmd_outer_quotes_without_wrapping_executables() {
    let command = batch_command_line(
        Path::new(r"C:\Windows\System32\cmd.exe"),
        Path::new(r"C:\tools\do & thing.cmd"),
        &[
            "two words".to_string(),
            "x&y".to_string(),
            String::new(),
            "bang!".to_string(),
        ],
    )
    .expect("safe batch arguments should encode");
    assert_eq!(
        decode_nul_terminated(&command),
        r#"C:\Windows\System32\cmd.exe /D /V:OFF /S /C ""C:\tools\do & thing.cmd" "two words" "x&y" "" "bang!"""#
    );
    assert!(is_batch_program(Path::new("task.BAT")));
    assert!(is_batch_program(Path::new("task.cmd")));
    assert!(is_batch_program(Path::new("task.CMD. ")));
    assert!(!is_batch_program(Path::new("task.exe")));
    assert!(!is_batch_program(Path::new("task.com")));
}

#[test]
fn batch_command_line_rejects_values_cmd_cannot_preserve() {
    for argument in ["%PATH%", "a\"b", "first\nsecond"] {
        let error = batch_command_line(
            Path::new(r"C:\Windows\System32\cmd.exe"),
            Path::new(r"C:\tools\task.cmd"),
            &[argument.to_string()],
        )
        .expect_err("unsafe batch argument should fail");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
    let error = batch_command_line(
        Path::new(r"C:\Windows\System32\cmd.exe"),
        Path::new(r"C:\100%\task.cmd"),
        &[],
    )
    .expect_err("unsafe batch path should fail");
    assert_eq!(error.kind(), ErrorKind::InvalidInput);
}

#[test]
fn working_directory_is_lexically_absolute_and_rejects_namespaces() {
    assert_eq!(
        normalize_absolute_path(Path::new(r"C:\base\one\..\.\two"))
            .expect("ordinary absolute path should normalize"),
        PathBuf::from(r"C:\base\two")
    );
    for path in [r"\\?\C:\work", r"\\.\C:\work", r"D:work"] {
        let error = normalize_absolute_path(Path::new(path))
            .expect_err("unsafe or drive-relative path should fail");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
}

#[test]
fn environment_is_case_insensitive_sorted_and_double_terminated() {
    let block = build_environment_block(
        vec![
            (OsString::from("z"), OsString::from("old")),
            (OsString::from("Path"), OsString::from("old-path")),
        ],
        &[
            ("PATH".to_string(), "new-path".to_string()),
            ("a".to_string(), "first".to_string()),
        ],
        Some(Path::new(r"C:\work")),
    )
    .expect("environment should encode");
    let decoded = String::from_utf16(&block).expect("test data is valid UTF-16");
    assert_eq!(
        decoded,
        "=C:=C:\\work\0a=first\0PATH=new-path\0PWD=C:\\work\0z=old\0\0"
    );
    assert_eq!(&block[block.len() - 2..], &[0, 0]);
}

#[test]
fn environment_tracks_the_normalized_drive_current_directory() {
    let entry = drive_current_directory_entry(Path::new(r"D:\work\project"))
        .expect("drive path should produce a hidden environment entry");
    assert_eq!(entry.0, OsString::from("=D:"));
    assert_eq!(entry.1, OsString::from(r"D:\work\project"));
    assert!(drive_current_directory_entry(Path::new(r"\\server\share\work")).is_none());
}

#[test]
fn environment_preserves_unpaired_utf16_code_units() {
    let name = OsString::from_wide(&[b'Z' as u16, 0xD800]);
    let value = OsString::from_wide(&[0xDC00, b'V' as u16]);
    let block = build_environment_block(vec![(name, value)], &[], None)
        .expect("Windows environment code units should be preserved");
    assert_eq!(
        block,
        [b'Z' as u16, 0xD800, b'=' as u16, 0xDC00, b'V' as u16, 0, 0]
    );
}

#[test]
fn environment_rejects_invalid_override_names() {
    for name in ["", "A=B"] {
        let error =
            build_environment_block(Vec::new(), &[(name.to_string(), "value".to_string())], None)
                .expect_err("invalid environment name should fail");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
}

#[test]
fn conpty_size_conversion_rejects_zero_and_overflow() {
    let valid = Coord::try_from(Size {
        cols: 80,
        rows: 24,
        cell_width: 8.0,
        cell_height: 16.0,
    })
    .expect("normal terminal size should fit");
    assert_eq!(valid, Coord { x: 80, y: 24 });

    for (cols, rows) in [(0, 24), (80, 0), (i16::MAX as u16 + 1, 24)] {
        let error = Coord::try_from(Size {
            cols,
            rows,
            cell_width: 8.0,
            cell_height: 16.0,
        })
        .expect_err("invalid size should fail");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
}

#[test]
fn control_state_coalesces_resize_and_close_is_sticky() {
    let state = ControlState::default();
    assert!(state.request_resize(Coord { x: 80, y: 24 }));
    assert!(state.request_resize(Coord { x: 120, y: 40 }));
    assert_eq!(state.take_resize(), Some(Coord { x: 120, y: 40 }));
    assert_eq!(state.take_resize(), None);

    assert!(state.request_resize(Coord { x: 90, y: 30 }));
    state.request_close();
    assert!(state.is_closed());
    assert_eq!(state.take_resize(), None);
    assert!(!state.request_resize(Coord { x: 100, y: 50 }));
}

fn writer_harness() -> (WriterHandle, mpsc::Receiver<WriterCommand>) {
    writer_harness_with_limits(
        MAX_PENDING_WRITE_BYTES,
        MAX_PENDING_WRITE_ENTRIES,
        MAX_PENDING_PROTOCOL_REPLY_BYTES,
        MAX_PENDING_PROTOCOL_REPLY_ENTRIES,
    )
}

fn writer_harness_with_limits(
    write_bytes: usize,
    write_entries: usize,
    protocol_reply_bytes: usize,
    protocol_reply_entries: usize,
) -> (WriterHandle, mpsc::Receiver<WriterCommand>) {
    let state = Arc::new(ControlState::default());
    let (sender, _receiver) = mpsc::sync_channel(1);
    WriterHandle::channel_with_limits(
        ControlHandle { sender, state },
        write_bytes,
        write_entries,
        protocol_reply_bytes,
        protocol_reply_entries,
    )
}

#[test]
fn write_budget_enforces_both_limits_and_raii_releases_them() {
    let budget = Arc::new(WriteBudget::default());
    assert!(budget.reserve(0).is_none());
    let byte_limit = budget
        .reserve(MAX_PENDING_WRITE_BYTES)
        .expect("exact byte limit should reserve");
    assert_eq!(budget.usage(), (MAX_PENDING_WRITE_BYTES, 1));
    assert!(budget.reserve(1).is_none());
    assert_eq!(budget.usage(), (MAX_PENDING_WRITE_BYTES, 1));
    drop(byte_limit);
    assert_eq!(budget.usage(), (0, 0));

    let reservations = (0..MAX_PENDING_WRITE_ENTRIES)
        .map(|_| budget.reserve(1).expect("entry should reserve"))
        .collect::<Vec<_>>();
    assert_eq!(
        budget.usage(),
        (MAX_PENDING_WRITE_ENTRIES, MAX_PENDING_WRITE_ENTRIES)
    );
    assert!(budget.reserve(1).is_none());
    drop(reservations);
    assert_eq!(budget.usage(), (0, 0));
}

#[test]
fn writer_queue_is_bounded_fifo_and_releases_failed_reservations() {
    let (writer, receiver) = writer_harness();
    for value in 0..MAX_PENDING_WRITE_ENTRIES {
        writer
            .write_owned(vec![(value % 251) as u8])
            .expect("write within entry limit should queue");
    }
    let error = writer
        .write_owned(vec![255])
        .expect_err("write beyond entry limit should not queue");
    assert_eq!(error.kind(), ErrorKind::WouldBlock);
    assert_eq!(
        writer.budget.usage(),
        (MAX_PENDING_WRITE_ENTRIES, MAX_PENDING_WRITE_ENTRIES)
    );

    for expected in 0..MAX_PENDING_WRITE_ENTRIES {
        let WriterCommand::Write(write) = receiver
            .recv()
            .expect("every accepted write should remain queued")
        else {
            panic!("write FIFO unexpectedly contained a wake");
        };
        assert_eq!(write.bytes, [(expected % 251) as u8]);
    }
    assert_eq!(writer.budget.usage(), (0, 0));

    let (disconnected, disconnected_receiver) = writer_harness();
    drop(disconnected_receiver);
    let error = disconnected
        .write_owned(vec![1, 2, 3])
        .expect_err("disconnected writer should fail");
    assert_eq!(error.kind(), ErrorKind::BrokenPipe);
    assert_eq!(disconnected.budget.usage(), (0, 0));
}

#[test]
fn owned_writer_payload_accounts_for_spare_vec_capacity() {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(b"tiny");
    let retained_capacity = bytes.capacity();
    assert!(retained_capacity > bytes.len());

    let (rejected, rejected_receiver) = writer_harness_with_limits(retained_capacity - 1, 1, 1, 1);
    let error = rejected
        .write_owned(bytes)
        .expect_err("spare owned capacity must count against the heap cap");
    assert_eq!(error.kind(), ErrorKind::WouldBlock);
    assert_eq!(rejected.budget.usage(), (0, 0));
    assert!(rejected_receiver.try_recv().is_err());

    let mut admitted_bytes = Vec::with_capacity(retained_capacity);
    admitted_bytes.extend_from_slice(b"tiny");
    assert_eq!(admitted_bytes.capacity(), retained_capacity);
    let (admitted, admitted_receiver) = writer_harness_with_limits(retained_capacity, 1, 1, 1);
    admitted
        .write_owned(admitted_bytes)
        .expect("the exact retained allocation should be admitted");
    assert_eq!(admitted.budget.usage(), (retained_capacity, 1));
    let active = admitted_receiver
        .recv()
        .expect("accepted owned write should remain queued");
    assert_eq!(admitted.budget.usage(), (retained_capacity, 1));
    drop(active);
    assert_eq!(admitted.budget.usage(), (0, 0));
}

#[test]
fn protocol_reply_lane_survives_normal_saturation_and_is_bounded() {
    let (writer, receiver) = writer_harness_with_limits(1, 1, 5, 1);
    writer
        .write_owned(vec![b'n'])
        .expect("normal lane should fill");
    writer
        .write_protocol_reply_owned(b"reply".to_vec())
        .expect("normal saturation must not reject a protocol reply");
    assert_eq!(writer.budget.usage(), (1, 1));
    assert_eq!(writer.protocol_replies.budget.usage(), (5, 1));

    let error = writer
        .write_protocol_reply_owned(b"x".to_vec())
        .expect_err("the independent reply lane must remain bounded");
    assert_eq!(error.kind(), ErrorKind::WouldBlock);
    assert_eq!(writer.protocol_replies.budget.usage(), (5, 1));

    let active_reply = writer
        .protocol_replies
        .pop()
        .expect("accepted reply should remain reserved while active");
    writer.close();
    assert_eq!(writer.protocol_replies.budget.usage(), (5, 1));
    drop(active_reply);
    assert_eq!(writer.protocol_replies.budget.usage(), (0, 0));
    drop(receiver);
    drop(writer);

    let (queued, _queued_receiver) = writer_harness_with_limits(1, 1, 8, 1);
    queued
        .write_protocol_reply_owned(b"reply".to_vec())
        .expect("reply should queue before close");
    assert_eq!(queued.protocol_replies.budget.usage(), (5, 1));
    queued.close();
    assert_eq!(queued.protocol_replies.budget.usage(), (0, 0));

    let (disconnected, disconnected_receiver) = writer_harness_with_limits(1, 1, 8, 1);
    let reply_budget = disconnected.protocol_replies.budget.clone();
    drop(disconnected_receiver);
    let error = disconnected
        .write_protocol_reply_owned(b"reply".to_vec())
        .expect_err("a disconnected writer must reject a queued reply");
    assert_eq!(error.kind(), ErrorKind::BrokenPipe);
    assert!(disconnected.control.state.is_closed());
    drop(disconnected);
    assert_eq!(reply_budget.usage(), (0, 0));
}

#[test]
fn unqueueable_reader_reply_closes_instead_of_being_dropped() {
    let (writer, _receiver) = writer_harness_with_limits(1, 1, 0, 0);
    assert!(!queue_reader_protocol_reply(&writer, b"reply".to_vec()));
    assert!(writer.control.state.is_closed());
    assert_eq!(writer.protocol_replies.budget.usage(), (0, 0));
}

#[test]
fn protocol_reply_queue_is_fifo_ahead_of_normal_commands() {
    let (writer, receiver) = writer_harness_with_limits(16, 2, 16, 2);
    writer
        .write_owned(b"normal".to_vec())
        .expect("normal input should queue");
    writer
        .write_protocol_reply_owned(b"one".to_vec())
        .expect("first reply should queue");
    writer
        .write_protocol_reply_owned(b"two".to_vec())
        .expect("second reply should queue");

    let first = writer
        .protocol_replies
        .pop()
        .expect("first priority reply should be available");
    let second = writer
        .protocol_replies
        .pop()
        .expect("second priority reply should be available");
    assert_eq!(first.bytes, b"one");
    assert_eq!(second.bytes, b"two");
    let WriterCommand::Write(normal) = receiver
        .recv()
        .expect("normal command should remain in its FIFO")
    else {
        panic!("normal FIFO unexpectedly contained a wake");
    };
    assert_eq!(normal.bytes, b"normal");
}

#[test]
fn empty_writes_succeed_even_after_close_and_queued_drop_releases_budget() {
    let (writer, receiver) = writer_harness();
    writer
        .write_owned(vec![1, 2, 3])
        .expect("write should queue before close");
    assert_eq!(writer.budget.usage(), (3, 1));
    drop(receiver);
    assert_eq!(writer.budget.usage(), (0, 0));

    let oversized = vec![0; MAX_PENDING_WRITE_BYTES + 1];
    let error = writer
        .write(&oversized)
        .expect_err("oversized borrowed write should fail before enqueue");
    assert_eq!(error.kind(), ErrorKind::WouldBlock);
    assert_eq!(writer.budget.usage(), (0, 0));

    writer.control.state.request_close();
    writer
        .write_owned(Vec::new())
        .expect("empty writes are no-ops even after close");
    writer
        .write_owned(Vec::with_capacity(64))
        .expect("empty spare capacity is dropped without a queue node");
    let error = writer
        .write_owned(vec![1])
        .expect_err("non-empty writes after close should fail");
    assert_eq!(error.kind(), ErrorKind::BrokenPipe);
}

#[test]
fn writer_and_control_wakes_coalesce_while_latest_resize_wins() {
    let (writer, writer_receiver) = writer_harness();
    for _ in 0..128 {
        writer.wake().expect("connected wake should succeed");
    }
    assert!(matches!(
        writer_receiver.try_recv(),
        Ok(WriterCommand::Wake(_))
    ));
    assert!(matches!(
        writer_receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    writer.wake().expect("fresh connected wake should succeed");
    assert!(matches!(
        writer_receiver.try_recv(),
        Ok(WriterCommand::Wake(_))
    ));

    let state = Arc::new(ControlState::default());
    let (sender, receiver) = mpsc::sync_channel(1);
    let control = ControlHandle {
        sender,
        state: state.clone(),
    };
    for size in [
        Coord { x: 80, y: 24 },
        Coord { x: 100, y: 30 },
        Coord { x: 132, y: 44 },
    ] {
        control
            .request_resize(size)
            .expect("coalesced resize should succeed");
    }
    assert!(matches!(receiver.try_recv(), Ok(ControlCommand::Wake)));
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert_eq!(state.take_resize(), Some(Coord { x: 132, y: 44 }));

    for _ in 0..128 {
        control.close();
    }
    assert!(state.is_closed());
    assert!(matches!(receiver.try_recv(), Ok(ControlCommand::Wake)));
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn conpty_delivers_final_child_output_before_exit_callback() {
    let marker = unique_marker("FINAL_OUTPUT");
    let command = format!("(for /L %i in (1,1,512) do @echo tmon-drain-%i) & echo {marker}");
    let output = collect_session_output(
        SpawnConfig {
            program: windows_command_processor(),
            args: vec![
                "/D".to_string(),
                "/S".to_string(),
                "/C".to_string(),
                command,
            ],
            working_directory: None,
            environment: Vec::new(),
        },
        |_| {},
    );
    assert!(
        output_contains(&output, &marker),
        "the exit callback snapshot must contain the final output marker"
    );
}

#[test]
fn conpty_immediate_resize_and_stdin_round_trip_exit_cleanly() {
    let payload = unique_marker("INPUT");
    let expected = format!("TMON_ROUND_TRIP_{payload}");
    let input =
        format!("set /p TMON_VALUE=& echo TMON_ROUND_TRIP_!TMON_VALUE!& exit\r\n{payload}\r\n");
    let output = collect_session_output(
        SpawnConfig {
            program: windows_command_processor(),
            args: vec!["/D".to_string(), "/Q".to_string(), "/V:ON".to_string()],
            working_directory: None,
            environment: Vec::new(),
        },
        |terminal| {
            terminal
                .resize(Size {
                    cols: 113,
                    rows: 37,
                    cell_width: 9.0,
                    cell_height: 18.0,
                })
                .expect("immediate ConPTY resize should queue");
            terminal
                .write(input.as_bytes())
                .expect("ConPTY input should queue");
        },
    );
    assert!(
        output_contains(&output, &expected),
        "cmd.exe should consume stdin and emit the transformed marker"
    );
}

#[test]
fn conpty_batch_wrapper_uses_the_normalized_working_directory() {
    let directory = TestDirectory::new("batch cwd with spaces");
    let work = directory.path().join("working directory");
    fs::create_dir(&work).expect("working directory should be created");
    let script = directory.path().join("script with spaces.cmd");
    let marker = unique_marker("BATCH_CWD");
    fs::write(
        &script,
        format!("@echo off\r\n>cwd-proof.txt echo {marker}\r\necho {marker}\r\necho ARG=%1\r\n"),
    )
    .expect("batch script should be written");
    let lexical_working_directory = work.join("not-created").join("..").join(".");
    let output = collect_session_output(
        SpawnConfig {
            program: script.to_string_lossy().into_owned(),
            args: vec!["hello & goodbye".to_string()],
            working_directory: Some(lexical_working_directory),
            environment: Vec::new(),
        },
        |_| {},
    );

    assert!(output_contains(&output, &marker));
    assert!(output_contains(&output, "ARG=\"hello & goodbye\""));
    let proof = fs::read_to_string(work.join("cwd-proof.txt"))
        .expect("batch file should write relative to the normalized cwd");
    assert_eq!(proof.trim(), marker);
}
