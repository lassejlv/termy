use std::io::{Read, Write};

use tmon_pty::{Command, PtySize, SpawnedPty, spawn};

const TEST_SIZE: PtySize = PtySize {
    rows: 24,
    cols: 80,
    pixel_width: 720,
    pixel_height: 432,
};

#[test]
fn child_has_a_controlling_terminal_session_environment_and_working_directory() {
    let command = Command::new("/bin/sh")
        .args([
            "-c",
            concat!(
                "test -t 0 && test -t 1 && test -t 2 || exit 20; ",
                "pgid=\"$(ps -o pgid= -p $$ | tr -d ' ')\"; test \"$pgid\" = \"$$\" || exit 21; ",
                "printf '%s|%s|native-pty-ok' \"$TMON_PTY_TEST\" \"$PWD\"",
            ),
        ])
        .current_dir("/tmp")
        .env("TMON_PTY_TEST", "custom");

    let SpawnedPty { master, mut child } = spawn(&command, TEST_SIZE).expect("native PTY starts");
    let mut reader = master.try_clone().expect("PTY reader clones");
    let mut output = Vec::new();
    reader.read_to_end(&mut output).expect("PTY output reads");
    let status = child.wait().expect("PTY child is reaped");

    assert_eq!(status.exit_code(), 0, "output: {output:?}");
    assert_eq!(status.signal(), None);
    assert!(
        output
            .windows(b"custom|/private/tmp|native-pty-ok".len())
            .any(|window| window == b"custom|/private/tmp|native-pty-ok"),
        "unexpected PTY output: {:?}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn path_lookup_and_bidirectional_io_are_direct_and_simple() {
    let command = Command::new("sh").args(["-c", "read line; printf 'received:%s' \"$line\""]);
    let SpawnedPty { master, mut child } = spawn(&command, TEST_SIZE).expect("PATH command starts");
    let mut reader = master.try_clone().expect("PTY reader clones");
    let mut writer = master.try_clone().expect("PTY writer clones");

    writer.write_all(b"hello\n").expect("PTY input writes");
    writer.flush().expect("PTY input flushes");
    let mut output = Vec::new();
    reader.read_to_end(&mut output).expect("PTY output reads");
    let status = child.wait().expect("PTY child is reaped");

    assert_eq!(status.exit_code(), 0);
    assert!(
        output
            .windows(b"received:hello".len())
            .any(|window| window == b"received:hello"),
        "unexpected PTY output: {:?}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn exec_errors_and_signal_exits_are_reported() {
    let error = spawn(&Command::new("/definitely/not/a/tmon-command"), TEST_SIZE)
        .expect_err("missing executable is rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

    let SpawnedPty { master, mut child } = spawn(
        &Command::new("/bin/sh").args(["-c", "kill -TERM $$"]),
        TEST_SIZE,
    )
    .expect("signal test starts");
    let mut reader = master.try_clone().expect("PTY reader clones");
    std::io::copy(&mut reader, &mut std::io::sink()).expect("PTY reaches EOF");
    let status = child.wait().expect("signalled child is reaped");

    assert_eq!(status.exit_code(), 1);
    assert_eq!(status.signal(), Some("Terminated: 15"));
}
