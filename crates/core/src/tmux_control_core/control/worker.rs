use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

use flume::{Receiver, RecvTimeoutError, SendTimeoutError, Sender};

use super::super::command::{control_request_lines, validate_tmux_control_arg};
use super::super::types::{TmuxControlError, TmuxNotification};

use super::{
    channel::{ControlCommandTracker, ControlRequest, PendingCommand, complete_pending_command},
    coalescer::{NotificationCoalescer, flush_notification_coalescer, signal_fatal_exit},
    parser::{ControlStateEvent, ControlStateMachine},
};

/// How long the writer waits for tmux to finish one request before failing it
/// and moving on to the next. Longer than any caller's own wait (pane captures
/// allow 10s), so it never cuts off a caller that is still waiting; it exists
/// so a completion that never arrives can't wedge every later command.
pub const COMMAND_COMPLETION_TIMEOUT: Duration = Duration::from_secs(15);

#[allow(clippy::too_many_arguments)]
pub fn spawn_control_threads<W, R>(
    child: Option<std::process::Child>,
    child_stdin: W,
    child_stdout: R,
    request_rx: Receiver<ControlRequest>,
    pending_tx: Sender<PendingCommand>,
    pending_rx: Receiver<PendingCommand>,
    notifications_tx: Sender<TmuxNotification>,
    fatal_exit_tx: Sender<Option<String>>,
    event_wakeup_tx: Option<Sender<()>>,
) where
    W: Write + Send + 'static,
    R: Read + Send + 'static,
{
    spawn_control_threads_with_timeout(
        child,
        child_stdin,
        child_stdout,
        request_rx,
        pending_tx,
        pending_rx,
        notifications_tx,
        fatal_exit_tx,
        event_wakeup_tx,
        COMMAND_COMPLETION_TIMEOUT,
    );
}

#[allow(clippy::too_many_arguments)]
fn spawn_control_threads_with_timeout<W, R>(
    child: Option<std::process::Child>,
    child_stdin: W,
    child_stdout: R,
    request_rx: Receiver<ControlRequest>,
    pending_tx: Sender<PendingCommand>,
    pending_rx: Receiver<PendingCommand>,
    notifications_tx: Sender<TmuxNotification>,
    fatal_exit_tx: Sender<Option<String>>,
    event_wakeup_tx: Option<Sender<()>>,
    completion_timeout: Duration,
) where
    W: Write + Send + 'static,
    R: Read + Send + 'static,
{
    if let Some(mut child) = child {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }

    std::thread::spawn(move || {
        let mut stdin = child_stdin;
        while let Ok(request) = request_rx.recv() {
            let ControlRequest {
                command,
                completion_token,
                response_tx,
            } = request;

            // A request must be exactly its command line plus its token line:
            // an embedded line break would add lines (an empty one detaches
            // the client) and NUL truncates the line inside tmux.
            if let Err(error) = validate_tmux_control_arg(command.as_str()) {
                if let Some(response_tx) = response_tx {
                    let _ = response_tx.try_send(Err(TmuxControlError::protocol(format!(
                        "refusing tmux control command: {error}"
                    ))));
                }
                continue;
            }

            let lines = control_request_lines(command.as_str(), completion_token.as_str());
            let (completion_tx, completion_rx) = flume::bounded(1);
            let response_tx_for_writer = response_tx.clone();
            let pending = PendingCommand {
                command: command.clone(),
                completion_token,
                response_tx,
                completion_tx,
            };

            // The slot is free unless an earlier request timed out before tmux
            // even began it. Wait for tmux to catch up rather than write a
            // command the reader could not attribute in order.
            match pending_tx.send_timeout(pending, completion_timeout) {
                Ok(()) => {}
                Err(SendTimeoutError::Timeout(pending)) => {
                    complete_pending_command(
                        pending,
                        Err(TmuxControlError::channel(
                            "tmux control is not responding; command was not sent",
                        )),
                    );
                    continue;
                }
                Err(SendTimeoutError::Disconnected(pending)) => {
                    complete_pending_command(
                        pending,
                        Err(TmuxControlError::channel(
                            "tmux control reader is unavailable",
                        )),
                    );
                    break;
                }
            }

            if stdin
                .write_all(lines.as_bytes())
                .and_then(|()| stdin.flush())
                .is_err()
            {
                if let Some(response_tx) = response_tx_for_writer {
                    let _ = response_tx.try_send(Err(TmuxControlError::channel(
                        "failed to write command to tmux control stdin",
                    )));
                }
                break;
            }

            match completion_rx.recv_timeout(completion_timeout) {
                Ok(()) => {}
                Err(RecvTimeoutError::Timeout) => {
                    // Release the caller, but leave the request tracked by the
                    // reader: tmux still runs it and its token line in order,
                    // so its late blocks are consumed as its own and never
                    // attributed to the next request.
                    if let Some(response_tx) = response_tx_for_writer {
                        let _ = response_tx.try_send(Err(TmuxControlError::channel(format!(
                            "timed out after {completion_timeout:?} waiting for tmux to complete command '{command}'"
                        ))));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    std::thread::spawn(move || {
        let mut reader = BufReader::new(child_stdout);
        let mut line = Vec::<u8>::new();
        let mut commands = ControlCommandTracker::default();
        let mut control_state = ControlStateMachine::default();
        let mut notifications = NotificationCoalescer::default();

        let shut_down = |commands: &mut ControlCommandTracker,
                         notifications: &mut NotificationCoalescer,
                         exit_reason: Option<String>,
                         error: TmuxControlError| {
            commands.fail_all(&pending_rx, &error);
            signal_fatal_exit(&fatal_exit_tx, exit_reason.clone());
            notifications.push(TmuxNotification::Exit(exit_reason));
            let _ = flush_notification_coalescer(
                notifications,
                &notifications_tx,
                event_wakeup_tx.as_ref(),
            );
        };
        let shut_down_on_error = |commands: &mut ControlCommandTracker,
                                  notifications: &mut NotificationCoalescer,
                                  error: TmuxControlError| {
            shut_down(commands, notifications, Some(error.message.clone()), error);
        };

        loop {
            line.clear();
            let Ok(read) = reader.read_until(b'\n', &mut line) else {
                shut_down(
                    &mut commands,
                    &mut notifications,
                    Some("tmux control mode read failure".to_string()),
                    TmuxControlError::channel("tmux control mode read failure"),
                );
                break;
            };

            if read == 0 {
                shut_down(
                    &mut commands,
                    &mut notifications,
                    None,
                    TmuxControlError::channel("tmux control mode closed"),
                );
                break;
            }

            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }

            let event = match control_state.on_line(&line) {
                Ok(event) => event,
                Err(error) => {
                    shut_down_on_error(&mut commands, &mut notifications, error);
                    break;
                }
            };

            let tracked = match event {
                ControlStateEvent::None => Ok(()),
                ControlStateEvent::Notification(notification) => {
                    notifications.push(notification);
                    Ok(())
                }
                ControlStateEvent::CommandBegin => commands.on_block_begin(&pending_rx),
                ControlStateEvent::CommandComplete { is_error, output } => {
                    commands.on_block_complete(is_error, output)
                }
                ControlStateEvent::Exit(reason) => {
                    let error = TmuxControlError::channel(
                        reason
                            .clone()
                            .unwrap_or_else(|| "tmux control mode exited".to_string()),
                    );
                    shut_down(&mut commands, &mut notifications, reason, error);
                    break;
                }
            };
            if let Err(error) = tracked {
                shut_down_on_error(&mut commands, &mut notifications, error);
                break;
            }

            if let Err(error) = flush_notification_coalescer(
                &mut notifications,
                &notifications_tx,
                event_wakeup_tx.as_ref(),
            ) {
                shut_down_on_error(&mut commands, &mut notifications, error);
                break;
            }
        }
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::channel::{ControlCommandResult, ControlRequest};
    use super::spawn_control_threads_with_timeout;
    use crate::tmux_control_core::types::{TmuxControlError, TmuxControlErrorKind};
    use std::io::{BufRead, BufReader, ErrorKind, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    type Response = Result<ControlCommandResult, TmuxControlError>;

    const LONG_TIMEOUT: Duration = Duration::from_secs(30);

    /// Drives the worker threads against a scripted fake tmux.
    struct FakeTmux {
        request_tx: flume::Sender<ControlRequest>,
        commands: BufReader<UnixStream>,
        output: UnixStream,
    }

    impl FakeTmux {
        fn spawn() -> Self {
            Self::spawn_with_timeout(LONG_TIMEOUT)
        }

        fn spawn_with_timeout(completion_timeout: Duration) -> Self {
            let (worker_stdin, fake_stdin) = UnixStream::pair().expect("stdin pair");
            let (worker_stdout, fake_stdout) = UnixStream::pair().expect("stdout pair");
            fake_stdin
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("read timeout");
            let (request_tx, request_rx) = flume::bounded(16);
            let (pending_tx, pending_rx) = flume::bounded(1);
            let (notifications_tx, _notifications_rx) = flume::bounded(64);
            let (fatal_exit_tx, _fatal_exit_rx) = flume::bounded(1);
            spawn_control_threads_with_timeout(
                None,
                worker_stdin,
                worker_stdout,
                request_rx,
                pending_tx,
                pending_rx,
                notifications_tx,
                fatal_exit_tx,
                None,
                completion_timeout,
            );
            Self {
                request_tx,
                commands: BufReader::new(fake_stdin),
                output: fake_stdout,
            }
        }

        fn enqueue(&self, command: &str, token: &str) -> flume::Receiver<Response> {
            let (response_tx, response_rx) = flume::bounded(1);
            self.request_tx
                .send(ControlRequest {
                    command: command.to_string(),
                    completion_token: token.to_string(),
                    response_tx: Some(response_tx),
                })
                .expect("enqueue");
            response_rx
        }

        fn read_line(&mut self) -> String {
            let mut line = String::new();
            self.commands
                .read_line(&mut line)
                .expect("worker should write to tmux");
            line
        }

        /// Asserts the worker wrote `command` and then, on its own line, the
        /// command printing `token`.
        fn expect_written(&mut self, command: &str, token: &str) {
            if !command.trim().is_empty() {
                assert_eq!(self.read_line(), format!("{command}\n"));
            }
            assert_eq!(self.read_line(), format!("display-message -p {token}\n"));
        }

        fn expect_nothing_written(&mut self, wait: Duration) {
            let stream = self.commands.get_ref();
            stream.set_read_timeout(Some(wait)).expect("read timeout");
            let mut line = String::new();
            let error = self
                .commands
                .read_line(&mut line)
                .expect_err("worker must not write");
            assert!(
                matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
                "{error:?} after {line:?}"
            );
            self.commands
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("read timeout");
        }

        /// Enqueues `command` and checks what the worker wrote to tmux.
        fn send(&mut self, command: &str, token: &str) -> flume::Receiver<Response> {
            let response_rx = self.enqueue(command, token);
            self.expect_written(command, token);
            response_rx
        }

        fn emit(&mut self, lines: impl AsRef<[u8]>) {
            self.output.write_all(lines.as_ref()).expect("emit");
        }
    }

    fn recv(response_rx: &flume::Receiver<Response>) -> Response {
        response_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("command should complete")
    }

    fn recv_output(response_rx: &flume::Receiver<Response>) -> String {
        recv(response_rx).expect("command succeeds").output
    }

    #[test]
    fn command_ending_in_tmux_comment_still_completes() {
        let mut tmux = FakeTmux::spawn();

        // tmux 3.4 treats everything from an unquoted `#` word to the end of
        // the line as a comment; the token line still runs on its own.
        let commented = tmux.send("display-message -p A # trailing note", "__termy_cmd_done_1");
        tmux.emit(concat!(
            "%begin 1 10 1\nA\n%end 1 10 1\n",
            "%begin 1 11 1\n__termy_cmd_done_1\n%end 1 11 1\n",
        ));
        assert_eq!(recv_output(&commented), "A");

        // A line that is only a comment produces no block at all.
        let comment_only = tmux.send("# just a comment", "__termy_cmd_done_2");
        tmux.emit("%begin 1 12 1\n__termy_cmd_done_2\n%end 1 12 1\n");
        assert_eq!(recv_output(&comment_only), "");

        // tmux closes an unterminated quote at the end of the line.
        let unterminated = tmux.send("display-message -p 'open ; x", "__termy_cmd_done_3");
        tmux.emit(concat!(
            "%begin 1 13 1\nopen ; x\n%end 1 13 1\n",
            "%begin 1 14 1\n__termy_cmd_done_3\n%end 1 14 1\n",
        ));
        assert_eq!(recv_output(&unterminated), "open ; x");
    }

    #[test]
    fn error_block_fails_command_and_keeps_channel_usable() {
        let mut tmux = FakeTmux::spawn();

        // tmux 3.4: the failing command's `;` list stops at the error, but the
        // token is on its own line and still produces its block.
        let failed = tmux.send(
            "select-pane -t %999 ; display-message -p skipped",
            "__termy_cmd_done_101",
        );
        tmux.emit(concat!(
            "%begin 1 10 1\ncan't find pane: %999\n%error 1 10 1\n",
            "%begin 1 11 1\n__termy_cmd_done_101\n%end 1 11 1\n",
        ));
        let error = recv(&failed).expect_err("failing command must fail");
        assert_eq!(error.kind, TmuxControlErrorKind::Runtime);
        assert_eq!(error.message, "can't find pane: %999");

        let ok = tmux.send("display-message -p ok", "__termy_cmd_done_102");
        tmux.emit(
            "%begin 1 12 1\nok\n%end 1 12 1\n%begin 1 13 1\n__termy_cmd_done_102\n%end 1 13 1\n",
        );
        assert_eq!(recv_output(&ok), "ok");
    }

    #[test]
    fn parse_error_fails_command_once_its_token_arrives() {
        let mut tmux = FakeTmux::spawn();

        let failed = tmux.send("display-message -p {", "__termy_cmd_done_111");
        tmux.emit("%begin 1 10 1\nparse error: syntax error\n%error 1 10 1\n");
        assert!(
            failed.recv_timeout(Duration::from_millis(100)).is_err(),
            "request completes on its token block, not the error block"
        );
        tmux.emit("%begin 1 11 1\n__termy_cmd_done_111\n%end 1 11 1\n");
        assert_eq!(
            recv(&failed).expect_err("parse error").message,
            "parse error: syntax error"
        );
    }

    #[test]
    fn nested_error_and_stray_token_are_not_next_command_output() {
        let mut tmux = FakeTmux::spawn();

        // A failing command nested in `if-shell` doesn't stop the outer list.
        let failed = tmux.send(
            "if-shell true 'select-pane -t %999'",
            "__termy_cmd_done_201",
        );
        tmux.emit(concat!(
            "%begin 1 20 1\n%end 1 20 1\n",
            "%begin 1 21 1\ncan't find pane: %999\n%error 1 21 1\n",
            "%begin 1 22 1\n__termy_cmd_done_201\n%end 1 22 1\n",
        ));
        assert_eq!(
            recv(&failed).expect_err("nested error").message,
            "can't find pane: %999"
        );

        // A token block from a request that is no longer tracked is dropped.
        let next = tmux.send("display-message -p next", "__termy_cmd_done_202");
        tmux.emit(concat!(
            "%begin 1 23 1\n__termy_cmd_done_150\n%end 1 23 1\n",
            "%begin 1 24 1\nnext\n%end 1 24 1\n",
            "%begin 1 25 1\n__termy_cmd_done_202\n%end 1 25 1\n",
        ));
        assert_eq!(recv_output(&next), "next");
    }

    #[test]
    fn non_utf8_command_output_keeps_the_session_alive() {
        let mut tmux = FakeTmux::spawn();

        // tmux 3.4 prints a stored buffer's bytes raw to a UTF-8 client.
        let raw = tmux.send("show-buffer -b raw", "__termy_cmd_done_311");
        tmux.emit(b"%begin 1 31 1\nb\xffb\n%end 1 31 1\n");
        tmux.emit("%begin 1 32 1\n__termy_cmd_done_311\n%end 1 32 1\n");
        assert_eq!(recv_output(&raw), "b\u{fffd}b");

        let ok = tmux.send("display-message -p ok", "__termy_cmd_done_312");
        tmux.emit(
            "%begin 1 33 1\nok\n%end 1 33 1\n%begin 1 34 1\n__termy_cmd_done_312\n%end 1 34 1\n",
        );
        assert_eq!(recv_output(&ok), "ok");
    }

    #[test]
    fn blank_command_sends_only_the_token_line() {
        let mut tmux = FakeTmux::spawn();

        let blank = tmux.send("", "__termy_cmd_done_301");
        tmux.emit("%begin 1 30 1\n__termy_cmd_done_301\n%end 1 30 1\n");
        assert_eq!(recv_output(&blank), "");
    }

    #[test]
    fn command_with_line_break_is_refused_without_writing() {
        let mut tmux = FakeTmux::spawn();

        let refused = tmux.enqueue(
            "display-message -p a\n\nkill-server",
            "__termy_cmd_done_401",
        );
        let error = recv(&refused).expect_err("line break must be refused");
        assert_eq!(error.kind, TmuxControlErrorKind::Protocol);

        let ok = tmux.send("display-message -p ok", "__termy_cmd_done_402");
        tmux.emit(
            "%begin 1 40 1\nok\n%end 1 40 1\n%begin 1 41 1\n__termy_cmd_done_402\n%end 1 41 1\n",
        );
        assert_eq!(recv_output(&ok), "ok");
    }

    #[test]
    fn lost_completion_times_out_and_late_blocks_stay_with_their_request() {
        let mut tmux = FakeTmux::spawn_with_timeout(Duration::from_millis(500));

        // tmux is busy (e.g. a foreground `run-shell`) and doesn't answer in
        // time: the request fails instead of wedging the writer.
        let slow = tmux.send("run-shell 'sleep 60'", "__termy_cmd_done_501");
        let error = recv(&slow).expect_err("slow command must time out");
        assert_eq!(error.kind, TmuxControlErrorKind::Channel);
        assert!(error.message.contains("timed out"), "{error:?}");

        // tmux never began the slow command, so the next one waits for it
        // rather than being written where the reader can't attribute it.
        let next = tmux.enqueue("display-message -p next", "__termy_cmd_done_502");
        tmux.expect_nothing_written(Duration::from_millis(100));

        // tmux catches up: the slow command's late blocks are its own, and
        // the next command is written once tmux has begun the slow one.
        tmux.emit("%begin 1 50 1\nslow output\n%end 1 50 1\n");
        tmux.expect_written("display-message -p next", "__termy_cmd_done_502");
        tmux.emit(concat!(
            "%begin 1 51 1\n__termy_cmd_done_501\n%end 1 51 1\n",
            "%begin 1 52 1\nnext\n%end 1 52 1\n",
            "%begin 1 53 1\n__termy_cmd_done_502\n%end 1 53 1\n",
        ));
        assert_eq!(recv_output(&next), "next");
    }

    #[test]
    fn unresponsive_tmux_fails_queued_commands_without_writing_them() {
        let mut tmux = FakeTmux::spawn_with_timeout(Duration::from_millis(100));

        let first = tmux.send("display-message -p first", "__termy_cmd_done_601");
        assert!(recv(&first).is_err());

        let second = tmux.enqueue("display-message -p second", "__termy_cmd_done_602");
        let error = recv(&second).expect_err("tmux never answered");
        assert_eq!(error.kind, TmuxControlErrorKind::Channel);
        assert!(error.message.contains("not responding"), "{error:?}");
        tmux.expect_nothing_written(Duration::from_millis(200));
    }
}
