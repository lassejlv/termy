#![cfg(unix)]

use std::{
    io::Write,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::{Duration, Instant, SystemTime},
};

use engine::{MousePointerShape, Terminal, pty::PtyCommand};
use mux::{Client, PROTOCOL_VERSION, TerminalSize};

#[test]
fn detached_tabs_keep_their_processes_and_terminal_state() {
    let socket_path = unique_socket_path("detach");
    let server_path = socket_path.clone();
    let server = thread::spawn(move || mux::serve(&server_path));
    wait_for_listener(&socket_path);

    let command = PtyCommand::new("/bin/sh").with_arguments([
        "-c",
        "printf '\\033]0;kept-title\\007\
         \\033]7;file://localhost/tmp\\007\
         \\033]11;#1e1e1e\\007\
         \\033]22;pointer\\007\
         PID=%s\\n' \"$$\"; \
         while IFS= read -r line; do \
           if [ \"$line\" = size ]; then stty size; else printf 'seen:%s\\n' \"$line\"; fi; \
         done",
    ]);
    let size = TerminalSize::new(6, 40, 400, 120);
    let mut first = Client::connect_or_spawn(&socket_path, Path::new("/unused-test-executable"))
        .expect("first client should connect");
    let first_restore = first
        .attach(&command, size, 100, 50)
        .expect("first client should attach");
    assert_eq!(first_restore.tabs.len(), 1);
    let first_tab_id = first_restore.active_tab_id;

    first
        .write(first_tab_id, b"before-detach\n")
        .expect("input should reach the first shell");
    let initial_output = drain_until(&mut first, |output| {
        output.contains("PID=") && output.contains("seen:before-detach")
    });
    let pid_start = initial_output
        .find("PID=")
        .expect("shell should report its pid");
    let pid_line = initial_output[pid_start..]
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '=')
        .collect::<String>();
    drop(first);

    thread::sleep(Duration::from_millis(100));
    let mut second = Client::connect_or_spawn(&socket_path, Path::new("/unused-test-executable"))
        .expect("second client should connect");
    let reattach_size = TerminalSize::new(7, 45, 450, 140);
    let second_restore = second
        .attach(&command, reattach_size, 100, 50)
        .expect("second client should reattach");
    assert_eq!(second_restore.active_tab_id, first_tab_id);
    assert_eq!(second_restore.tabs.len(), 1);
    assert_eq!(second_restore.tabs[0].title, "kept-title");
    assert_eq!(
        second_restore.tabs[0].current_directory.as_deref(),
        Some(Path::new("/tmp"))
    );
    assert_eq!(second_restore.tabs[0].dynamic_colors[1], Some([30, 30, 30]));
    assert_eq!(
        second_restore.tabs[0].pointer_shape,
        MousePointerShape::Pointer
    );
    let restored_text = snapshot_text(&second_restore.tabs[0].terminal_snapshot);
    assert!(restored_text.contains(&pid_line));
    assert!(restored_text.contains("seen:before-detach"));
    assert_eq!(
        snapshot_dimensions(&second_restore.tabs[0].terminal_snapshot),
        (45, 7),
        "reattach must resize the snapshot before returning it"
    );

    second
        .write(first_tab_id, b"size\n")
        .expect("reattached shell should accept input");
    drain_until(&mut second, |output| output.contains("7 45"));

    second
        .write(first_tab_id, b"after-reattach\n")
        .expect("reattached input should reach the same shell");
    drain_until(&mut second, |output| output.contains("seen:after-reattach"));
    second
        .resize_all(TerminalSize::new(8, 50, 500, 160))
        .expect("resize should reach the detached PTY");
    second
        .write(first_tab_id, b"size\n")
        .expect("resized shell should accept input");
    drain_until(&mut second, |output| output.contains("8 50"));

    let second_command =
        PtyCommand::new("/bin/sh").with_arguments(["-c", "printf 'SECOND-TAB\\n'; sleep 60"]);
    let second_tab = second
        .new_tab(&second_command, size, 100)
        .expect("second tab should spawn");
    assert_eq!(second_tab.index, 1);
    second
        .activate_tab(second_tab.id)
        .expect("second tab should activate");
    drain_until(&mut second, |output| output.contains("SECOND-TAB"));
    drop(second);

    let mut third = Client::connect_or_spawn(&socket_path, Path::new("/unused-test-executable"))
        .expect("third client should connect");
    let third_restore = third
        .attach(&command, size, 100, 50)
        .expect("third client should restore both tabs");
    assert_eq!(third_restore.tabs.len(), 2);
    assert_eq!(third_restore.tabs[0].id, first_tab_id);
    assert_eq!(third_restore.tabs[1].id, second_tab.id);
    assert_eq!(third_restore.active_tab_id, second_tab.id);

    third
        .close_tab(second_tab.id)
        .expect("closing a non-final tab should terminate it");
    assert!(
        third.close_tab(first_tab_id).is_err(),
        "closing the final tab must detach rather than kill it"
    );
    third
        .shutdown_daemon()
        .expect("test daemon should shut down");
    drop(third);
    server
        .join()
        .expect("server thread should join")
        .expect("server should exit cleanly");
}

#[test]
fn daemon_replaces_a_stale_socket_and_secures_the_live_socket() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::net::UnixListener;

    let socket_path = unique_socket_path("stale");
    let socket_parent = socket_path.parent().expect("socket has a parent");
    std::fs::create_dir_all(socket_parent).expect("test directory should exist");
    std::fs::set_permissions(socket_parent, std::fs::Permissions::from_mode(0o700))
        .expect("test directory should be private");
    let stale = UnixListener::bind(&socket_path).expect("stale socket should bind");
    drop(stale);

    let server_path = socket_path.clone();
    let server = thread::spawn(move || mux::serve(&server_path));
    wait_for_listener(&socket_path);

    let mode = std::fs::metadata(&socket_path)
        .expect("live socket should exist")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let directory_mode = std::fs::metadata(socket_path.parent().expect("socket has a parent"))
        .expect("socket directory should exist")
        .mode()
        & 0o777;
    assert_eq!(directory_mode, 0o700);

    let command = PtyCommand::new("/bin/sh").with_arguments(["-c", "sleep 60"]);
    let mut client = Client::connect_or_spawn(&socket_path, Path::new("/unused-test-executable"))
        .expect("client should connect through replaced socket");
    client
        .attach(&command, TerminalSize::new(3, 20, 200, 60), 10, 5)
        .expect("client should attach");
    client
        .shutdown_daemon()
        .expect("test daemon should shut down");
    drop(client);
    server
        .join()
        .expect("server thread should join")
        .expect("server should exit cleanly");
}

#[test]
fn malformed_client_isolated_from_following_connections() {
    let socket_path = unique_socket_path("malformed");
    let server_path = socket_path.clone();
    let server = thread::spawn(move || mux::serve(&server_path));
    wait_for_listener(&socket_path);

    let mut malformed = UnixStream::connect(&socket_path).expect("connect malformed client");
    malformed
        .write_all(&u64::MAX.to_be_bytes())
        .expect("write malicious frame length");
    drop(malformed);

    let command = PtyCommand::new("/bin/sh").with_arguments(["-c", "sleep 60"]);
    let mut client = Client::connect_or_spawn(&socket_path, Path::new("/unused-test-executable"))
        .expect("healthy client should still connect");
    client
        .attach(&command, TerminalSize::new(3, 20, 200, 60), 10, 5)
        .expect("healthy client should attach after malformed peer");
    client
        .shutdown_daemon()
        .expect("healthy client should stop daemon");
    drop(client);
    server
        .join()
        .expect("server thread should join")
        .expect("malformed client must not crash server");
}

#[test]
fn explicit_termination_stops_detached_sessions_and_removes_only_the_current_socket() {
    let root = unique_test_root("explicit-termination");
    let socket_path = root.join(format!("multiplexer-v{PROTOCOL_VERSION}.sock"));
    let server_path = socket_path.clone();
    let server = thread::spawn(move || mux::serve(&server_path));
    wait_for_listener(&socket_path);

    let command = PtyCommand::new("/bin/sh")
        .with_arguments(["-c", "printf 'SESSION_PID=%s\\n' \"$$\"; exec sleep 60"]);
    let mut client = Client::connect_existing(&socket_path).expect("connect existing daemon");
    client
        .attach(&command, TerminalSize::new(3, 40, 400, 60), 10, 5)
        .expect("attach a session to terminate");
    let output = drain_until(&mut client, |output| output.contains("SESSION_PID="));
    let pid = output
        .split("SESSION_PID=")
        .nth(1)
        .and_then(|suffix| suffix.lines().next())
        .and_then(|value| value.trim().parse::<u32>().ok())
        .expect("parse session pid");
    assert!(process_exists(pid), "session must be alive before detach");
    drop(client);

    let mut administrator =
        Client::connect_existing(&socket_path).expect("connect explicit termination client");
    administrator
        .terminate_all_sessions()
        .expect("explicit termination should be acknowledged");
    drop(administrator);
    server
        .join()
        .expect("server thread should join")
        .expect("server should exit after explicit termination");

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && process_exists(pid) {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !process_exists(pid),
        "explicit termination must stop the PTY process group"
    );
    assert!(!socket_path.exists(), "daemon must remove its own socket");
    assert!(
        root.exists(),
        "daemon must preserve the private runtime directory"
    );
    std::fs::remove_dir_all(root).expect("remove isolated runtime directory");
}

fn process_exists(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn drain_until(client: &mut Client, condition: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut received = Vec::new();
    while Instant::now() < deadline {
        let batch = client.drain().expect("multiplexer drain should work");
        for output in batch.outputs {
            received.extend_from_slice(&output.bytes);
        }
        let text = String::from_utf8_lossy(&received);
        if condition(&text) {
            return text.into_owned();
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "timed out waiting for multiplexer output; received {:?}",
        String::from_utf8_lossy(&received)
    );
}

fn snapshot_text(snapshot: &[u8]) -> String {
    let mut terminal = Terminal::from_snapshot(snapshot).expect("snapshot should restore");
    terminal
        .frame_update(true)
        .row_updates
        .into_iter()
        .flat_map(|row| {
            row.cells
                .into_iter()
                .flat_map(|cell| cell.text.bytes().collect::<Vec<_>>())
                .chain(std::iter::once(b'\n'))
        })
        .map(char::from)
        .collect()
}

fn snapshot_dimensions(snapshot: &[u8]) -> (usize, usize) {
    Terminal::from_snapshot(snapshot)
        .expect("snapshot should restore")
        .dimensions()
}

fn wait_for_listener(socket_path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if UnixStream::connect(socket_path).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "multiplexer listener did not start at {}",
        socket_path.display()
    );
}

fn unique_socket_path(label: &str) -> PathBuf {
    unique_test_root(label).join("mux.sock")
}

fn unique_test_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("clock should follow Unix epoch")
        .subsec_nanos();
    PathBuf::from("/tmp").join(format!("tm-{}-{label}-{nonce}", std::process::id()))
}
