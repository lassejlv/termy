#![cfg(unix)]

use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::Path,
    time::{Duration, Instant},
};
use termy_core::multiplexer::{PaneLaunch, SessionClient, connect_or_start};
use termy_core::*;

mod clipboard;
mod host;
mod layout;
mod views;

struct Host {
    client: SessionClient,
    root: std::path::PathBuf,
    _temp: tempfile::TempDir,
}

impl Host {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("host");
        let executable = std::env::var_os("TERMY_MUX_TEST_HOST_BINARY").map_or_else(
            || Path::new(env!("CARGO_BIN_EXE_termy-session-host")).to_owned(),
            std::path::PathBuf::from,
        );
        let client = connect_or_start(&root, &executable).unwrap();
        Self {
            client,
            root,
            _temp: temp,
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.client.shutdown();
    }
}

#[track_caller]
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "condition did not become true");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Drains events until the terminal stays quiet for a full window, so late
/// frames from earlier output or scrolls on a slow host are not mistaken for
/// wakeups caused by whatever the test does next.
#[track_caller]
fn wait_until_quiet(terminal: &Terminal) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        terminal.drain_events(&mut |_| None);
        std::thread::sleep(Duration::from_millis(100));
        if !terminal.has_pending_events() {
            return;
        }
        assert!(Instant::now() < deadline, "terminal did not go quiet");
    }
}

fn shell(script: &str) -> PaneLaunch {
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
    }
}

fn shell_pane(host: &Host, script: &str) -> (String, Terminal) {
    host.client
        .create(
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
            None,
        )
        .unwrap()
}

/// Drains `terminal` until a drained batch has an event for which `done`
/// returns true, and returns every event drained, without wakeups.
#[track_caller]
fn events_until(
    terminal: &Terminal,
    mut done: impl FnMut(&TerminalEvent) -> bool,
) -> Vec<TerminalEvent> {
    let mut seen = Vec::new();
    wait_until(|| {
        let mut finished = false;
        for event in terminal.drain_events(&mut |_| None).0 {
            if !matches!(event, TerminalEvent::Wakeup) {
                finished |= done(&event);
                seen.push(event);
            }
        }
        finished
    });
    seen
}

/// A pane running `script` with `/bin/sh -c`; `$1` is the host directory.
fn script_pane(host: &Host, script: &str, args: &[&str]) -> (String, Terminal) {
    let mut argv = vec![
        "-c".into(),
        script.into(),
        "ipc-test".into(),
        host.root.to_string_lossy().into_owned(),
    ];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    host.client
        .create(
            PaneLaunch {
                size: TerminalSize::default(),
                working_directory: None,
                shell_integration: None,
                config: TerminalRuntimeConfig::default(),
                launch: Some(TerminalLaunch::Program {
                    program: "/bin/sh".into(),
                    args: argv,
                }),
                environment: None,
            },
            None,
        )
        .unwrap()
}

#[track_caller]
fn wait_for_text(terminal: &Terminal, text: &str) {
    wait_until(|| {
        let render = terminal.render_read(true);
        let screen: String = render.cells.iter().map(|cell| cell.text.as_str()).collect();
        screen.contains(text)
    });
}
