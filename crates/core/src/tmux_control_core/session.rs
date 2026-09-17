//! A minimal, UI-agnostic tmux control-mode (`tmux -CC`) session driver built on
//! the shared protocol core. Spawns tmux over a PTY, runs the control worker, and
//! exposes non-blocking notification polling plus request/response commands — the
//! piece the FFI/native host wraps to bring tmux control mode to the macOS app.

#![cfg(unix)]

use std::fs::File;
use std::io;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use flume::{Receiver, Sender};

use crate::tmux_control_core::command::next_control_completion_token;
use crate::tmux_control_core::control::{
    ControlRequest, FATAL_EXIT_QUEUE_BOUND, NOTIFICATION_QUEUE_BOUND, PENDING_QUEUE_BOUND,
    REQUEST_QUEUE_BOUND, spawn_control_threads, try_enqueue_control_request,
};
use crate::tmux_control_core::types::{TmuxControlError, TmuxNotification};

const COMMAND_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct ControlSession {
    child: Child,
    request_tx: Sender<ControlRequest>,
    notifications_rx: Receiver<TmuxNotification>,
    fatal_exit_rx: Receiver<Option<String>>,
}

impl ControlSession {
    /// Launches `tmux -L <socket> -CC new-session -s <name>` over a PTY (tmux
    /// control mode requires a tty) and starts the control worker threads.
    pub fn launch(tmux_binary: &str, socket_name: &str, session_name: &str) -> io::Result<Self> {
        let pty = rustix_openpty::openpty(None, None)
            .map_err(|error| io::Error::other(format!("failed to allocate tmux pty: {error}")))?;
        let controller = File::from(pty.controller);
        let user = File::from(pty.user);

        let child_stdin = user.try_clone()?;
        let child_stdout = user.try_clone()?;
        let child_stderr = user;

        let child = Command::new(tmux_binary)
            .args(["-L", socket_name, "-CC", "new-session", "-s", session_name])
            // Don't inherit an outer tmux client; nested `TMUX` redirects startup.
            .env_remove("TMUX")
            .env("PROMPT_EOL_MARK", "")
            .stdin(Stdio::from(child_stdin))
            .stdout(Stdio::from(child_stdout))
            .stderr(Stdio::from(child_stderr))
            .spawn()?;

        // The PTY controller (master) is both the read and write side of the
        // control channel.
        let writer = controller.try_clone()?;
        let reader = controller;

        let (request_tx, request_rx) = flume::bounded::<ControlRequest>(REQUEST_QUEUE_BOUND);
        let (pending_tx, pending_rx) = flume::bounded(PENDING_QUEUE_BOUND);
        let (notifications_tx, notifications_rx) =
            flume::bounded::<TmuxNotification>(NOTIFICATION_QUEUE_BOUND);
        let (fatal_exit_tx, fatal_exit_rx) =
            flume::bounded::<Option<String>>(FATAL_EXIT_QUEUE_BOUND);

        // The child is owned here and reaped on drop, so the worker is told not
        // to wait on it (None).
        spawn_control_threads(
            None,
            writer,
            reader,
            request_rx,
            pending_tx,
            pending_rx,
            notifications_tx,
            fatal_exit_tx,
            None,
        );

        Ok(Self {
            child,
            request_tx,
            notifications_rx,
            fatal_exit_rx,
        })
    }

    /// Non-blocking drain of pending control notifications (`%output`,
    /// `%layout-change`, `%window-*`, …).
    pub fn poll(&self) -> Vec<TmuxNotification> {
        let mut notifications = Vec::new();
        while let Ok(notification) = self.notifications_rx.try_recv() {
            notifications.push(notification);
        }
        notifications
    }

    /// Runs a tmux command over the control channel and returns its output,
    /// bounded by a response timeout so a dead server can't hang the caller.
    pub fn send_command(&self, command: &str) -> Result<String, TmuxControlError> {
        let (response_tx, response_rx) = flume::bounded(1);
        let request = ControlRequest {
            command: command.to_string(),
            completion_token: next_control_completion_token(),
            response_tx: Some(response_tx),
        };
        try_enqueue_control_request(&self.request_tx, request)?;
        match response_rx.recv_timeout(COMMAND_RESPONSE_TIMEOUT) {
            Ok(Ok(result)) => Ok(result.output),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(TmuxControlError::channel(
                "tmux control command timed out or the channel closed",
            )),
        }
    }

    /// Returns a fatal-exit reason if the control connection has died.
    pub fn fatal_exit(&self) -> Option<Option<String>> {
        self.fatal_exit_rx.try_recv().ok()
    }
}

impl Drop for ControlSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires tmux >= 3.3"]
    fn launches_and_drives_control_mode() {
        let socket = format!("termy-ctrl-test-{}", std::process::id());
        let session =
            ControlSession::launch("tmux", &socket, "termytest").expect("launch tmux -CC");

        // The control stream should produce startup notifications (window/pane).
        let mut notifications = Vec::new();
        for _ in 0..40 {
            notifications.extend(session.poll());
            if !notifications.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !notifications.is_empty(),
            "expected tmux control-mode startup notifications"
        );

        // A request/response round-trip over the control channel.
        let output = session.send_command("display-message -p termy-ok");
        assert!(output.is_ok(), "send_command failed: {output:?}");

        drop(session);
        let _ = Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .output();
    }
}
