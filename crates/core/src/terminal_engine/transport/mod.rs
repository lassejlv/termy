//! Native PTY transport, independent of terminal parsing and screen state.
//!
//! A dedicated writer owns partial writes and bounded input/reply budgets.
//! Resize requests coalesce and remain serviceable while input is backpressured.
//! The output callback runs on the reader thread; its return value queues a
//! protocol reply, and the exit callback follows final readable child output.

use std::path::PathBuf;

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
))]
mod limits;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PtySize {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) cell_width: f32,
    pub(crate) cell_height: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct SpawnConfig {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) environment: Vec<(String, String)>,
}

/// How a PTY child ended: an exit code, or the signal that terminated it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChildExit {
    pub(crate) code: Option<i32>,
    pub(crate) signal: Option<i32>,
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
mod unix;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
pub(crate) use unix::Transport;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::Transport;

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
mod unsupported;
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
pub(crate) use unsupported::Transport;

pub(crate) fn available() -> bool {
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    {
        true
    }
    #[cfg(windows)]
    {
        windows::available()
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        windows
    )))]
    {
        false
    }
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "android", target_os = "macos")
))]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[test]
    fn native_interface_delivers_borrowed_owned_and_protocol_input() {
        assert!(available());
        let (output_tx, output_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::channel();
        let terminal = Transport::spawn(
            SpawnConfig {
                program: "/bin/sh".to_owned(),
                args: vec![
                    "-c".to_owned(),
                    concat!(
                        "stty -echo; printf '<ready>'; ",
                        "IFS= read -r first; printf '<first:%s>' \"$first\"; ",
                        "IFS= read -r second; printf '<second:%s>' \"$second\"; ",
                        "IFS= read -r third; printf '<third:%s>' \"$third\"",
                    )
                    .to_owned(),
                ],
                working_directory: None,
                environment: Vec::new(),
            },
            PtySize {
                cols: 80,
                rows: 24,
                cell_width: 8.0,
                cell_height: 16.0,
            },
            move |bytes| {
                let _ = output_tx.send(bytes.to_vec());
                Vec::new()
            },
            move || {
                let _ = exit_tx.send(());
            },
        )
        .expect("native shell should start");
        assert_ne!(terminal.child_pid(), 0);
        let mut output = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut wait_for = |marker: &[u8]| {
            while !output.windows(marker.len()).any(|window| window == marker) {
                let remaining = deadline.saturating_duration_since(Instant::now());
                output.extend_from_slice(
                    &output_rx
                        .recv_timeout(remaining)
                        .expect("shell should acknowledge input"),
                );
            }
        };
        wait_for(b"<ready>");
        terminal
            .write(b"borrowed\n")
            .expect("borrowed input should queue");
        wait_for(b"<first:borrowed>");
        terminal
            .write_owned(b"owned\n".to_vec())
            .expect("owned input should queue");
        wait_for(b"<second:owned>");
        terminal
            .write_protocol_reply_owned(b"protocol\n".to_vec())
            .expect("protocol input should queue");
        wait_for(b"<third:protocol>");
        exit_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("shell should finish after final output");
    }
}

/// Variables describing the terminal or multiplexer that launched Termy (for
/// example from an IDE or tmux). A shell inside Termy is in neither, and tools
/// such as tmux and editors misbehave when they believe otherwise. `COLORTERM`
/// and `TERM_PROGRAM_VERSION` are Termy's own to set (or omit when configured
/// off), so an inherited value must not leak through either.
pub(crate) fn belongs_to_parent_terminal(name: &std::ffi::OsStr) -> bool {
    const NAMES: &[&str] = &[
        "TMUX",
        "TMUX_PANE",
        "STY",
        "WINDOW",
        "ZELLIJ",
        "ZELLIJ_SESSION_NAME",
        "ZELLIJ_PANE_ID",
        "KITTY_WINDOW_ID",
        "KITTY_PID",
        "KITTY_LISTEN_ON",
        "KITTY_PUBLIC_KEY",
        "KITTY_INSTALLATION_DIR",
        "ITERM_SESSION_ID",
        "ITERM_PROFILE",
        // iTerm2 sets these (and ssh forwards LC_*), advertising its image protocol.
        "LC_TERMINAL",
        "LC_TERMINAL_VERSION",
        "TERM_SESSION_ID",
        "WT_SESSION",
        "WT_PROFILE_ID",
        "ALACRITTY_WINDOW_ID",
        "ALACRITTY_SOCKET",
        "ALACRITTY_LOG",
        "WEZTERM_PANE",
        "WEZTERM_UNIX_SOCKET",
        "WEZTERM_EXECUTABLE",
        "WEZTERM_EXECUTABLE_DIR",
        "WEZTERM_CONFIG_DIR",
        "WEZTERM_CONFIG_FILE",
        "GHOSTTY_RESOURCES_DIR",
        "GHOSTTY_BIN_DIR",
        "GHOSTTY_SHELL_FEATURES",
        "VTE_VERSION",
        "KONSOLE_VERSION",
        "KONSOLE_DBUS_SESSION",
        "TERMINAL_EMULATOR",
        "COLORTERM",
        "TERM_PROGRAM_VERSION",
        "COLUMNS",
        "LINES",
    ];
    const PREFIX: &str = "VSCODE_";
    let Some(name) = name.to_str() else {
        return false;
    };
    // Windows environment names are case-insensitive.
    if cfg!(windows) {
        NAMES.iter().any(|known| known.eq_ignore_ascii_case(name))
            || name
                .get(..PREFIX.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PREFIX))
    } else {
        NAMES.contains(&name) || name.starts_with(PREFIX)
    }
}
