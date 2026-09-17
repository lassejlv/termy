use std::io::{BufRead, BufReader, Read, Write};

use flume::{Receiver, Sender, TrySendError};

use super::super::command::{command_with_completion_token, split_control_completion_token};
use super::super::types::{TmuxControlError, TmuxNotification};

use super::{
    channel::{
        ActiveControlCommand, ControlRequest, PendingCommand, TrackedPendingCommand,
        append_command_output_chunk, claim_pending_for_command_begin, complete_pending_command,
        map_command_completion_response,
    },
    coalescer::{NotificationCoalescer, flush_notification_coalescer, signal_fatal_exit},
    parser::{ControlStateEvent, ControlStateMachine},
};

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
    if let Some(mut child) = child {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }

    std::thread::spawn(move || {
        let mut stdin = child_stdin;
        while let Ok(request) = request_rx.recv() {
            let command = request.command;
            let completion_token = request.completion_token;
            let response_tx = request.response_tx;
            let (completion_tx, completion_rx) = flume::bounded(1);
            let response_tx_for_write_error = response_tx.clone();
            let completion_tx_for_write_error = completion_tx.clone();
            let command_with_token =
                command_with_completion_token(command.as_str(), completion_token.as_str());

            match pending_tx.try_send(PendingCommand {
                command: command.clone(),
                completion_token,
                response_tx,
                completion_tx,
            }) {
                Ok(()) => {}
                Err(TrySendError::Full(pending)) => {
                    complete_pending_command(
                        pending,
                        Err(TmuxControlError::channel(
                            "tmux control pending queue is full",
                        )),
                    );
                    break;
                }
                Err(TrySendError::Disconnected(pending)) => {
                    complete_pending_command(
                        pending,
                        Err(TmuxControlError::channel(
                            "tmux control reader is unavailable",
                        )),
                    );
                    break;
                }
            }

            if stdin.write_all(command_with_token.as_bytes()).is_err() {
                if let Some(response_tx) = response_tx_for_write_error {
                    let _ = response_tx.send(Err(TmuxControlError::channel(
                        "failed to write command to tmux control stdin",
                    )));
                }
                let _ = completion_tx_for_write_error.send(());
                break;
            }
            if stdin.write_all(b"\n").is_err() {
                if let Some(response_tx) = response_tx_for_write_error {
                    let _ = response_tx.send(Err(TmuxControlError::channel(
                        "failed to write command terminator to tmux control stdin",
                    )));
                }
                let _ = completion_tx_for_write_error.send(());
                break;
            }
            if stdin.flush().is_err() {
                if let Some(response_tx) = response_tx_for_write_error {
                    let _ = response_tx.send(Err(TmuxControlError::channel(
                        "failed to flush tmux control stdin",
                    )));
                }
                let _ = completion_tx_for_write_error.send(());
                break;
            }
            if completion_rx.recv().is_err() {
                break;
            }
        }
    });

    std::thread::spawn(move || {
        let mut reader = BufReader::new(child_stdout);
        let mut line = Vec::<u8>::new();
        let mut current_command = None::<ActiveControlCommand>;
        let mut control_state = ControlStateMachine::default();
        let mut notifications = NotificationCoalescer::default();

        let fail_pending = |pending: PendingCommand, error: TmuxControlError| {
            complete_pending_command(pending, Err(error));
        };
        let fail_all_pending = |current_command: &mut Option<ActiveControlCommand>,
                                pending_rx: &Receiver<PendingCommand>,
                                error: TmuxControlError| {
            if let Some(active_command) = current_command.take()
                && let ActiveControlCommand::Tracked(tracked) = active_command
            {
                fail_pending(tracked.pending, error.clone());
            }
            while let Ok(pending) = pending_rx.try_recv() {
                fail_pending(pending, error.clone());
            }
        };
        let fail_with_exit = |current_command: &mut Option<ActiveControlCommand>,
                              pending_rx: &Receiver<PendingCommand>,
                              notifications_tx: &Sender<TmuxNotification>,
                              fatal_exit_tx: &Sender<Option<String>>,
                              notifications: &mut NotificationCoalescer,
                              event_wakeup_tx: Option<&Sender<()>>,
                              error: TmuxControlError| {
            let exit_reason = Some(error.message.clone());
            fail_all_pending(current_command, pending_rx, error);
            signal_fatal_exit(fatal_exit_tx, exit_reason.clone());
            notifications.push(TmuxNotification::Exit(exit_reason));
            let _ = flush_notification_coalescer(notifications, notifications_tx, event_wakeup_tx);
        };

        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line);
            let Ok(read) = read else {
                let exit_reason = Some("tmux control mode read failure".to_string());
                fail_all_pending(
                    &mut current_command,
                    &pending_rx,
                    TmuxControlError::channel("tmux control mode read failure"),
                );
                signal_fatal_exit(&fatal_exit_tx, exit_reason.clone());
                notifications.push(TmuxNotification::Exit(exit_reason));
                let _ = flush_notification_coalescer(
                    &mut notifications,
                    &notifications_tx,
                    event_wakeup_tx.as_ref(),
                );
                break;
            };

            if read == 0 {
                fail_all_pending(
                    &mut current_command,
                    &pending_rx,
                    TmuxControlError::channel("tmux control mode closed"),
                );
                signal_fatal_exit(&fatal_exit_tx, None);
                notifications.push(TmuxNotification::Exit(None));
                let _ = flush_notification_coalescer(
                    &mut notifications,
                    &notifications_tx,
                    event_wakeup_tx.as_ref(),
                );
                break;
            }

            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }

            let event = match control_state.on_line(&line) {
                Ok(event) => event,
                Err(error) => {
                    fail_with_exit(
                        &mut current_command,
                        &pending_rx,
                        &notifications_tx,
                        &fatal_exit_tx,
                        &mut notifications,
                        event_wakeup_tx.as_ref(),
                        error,
                    );
                    break;
                }
            };

            match event {
                ControlStateEvent::None => {}
                ControlStateEvent::Notification(notification) => {
                    notifications.push(notification);
                }
                ControlStateEvent::CommandBegin => {
                    if current_command.is_some() {
                        // Requests can be composed as `cmd1 ; cmd2`, and tmux may emit
                        // a separate begin/end block for each sub-command. Keep the same
                        // pending request active until its completion token is observed.
                        continue;
                    }

                    match claim_pending_for_command_begin(&pending_rx) {
                        Ok(Some(pending)) => {
                            current_command =
                                Some(ActiveControlCommand::Tracked(TrackedPendingCommand {
                                    pending,
                                    output: String::new(),
                                    is_error: false,
                                }));
                        }
                        Ok(None) => {
                            // tmux may emit unsolicited startup/control blocks before the
                            // first app-issued command is tracked; consume these blocks
                            // without binding a pending request.
                            current_command = Some(ActiveControlCommand::Untracked);
                        }
                        Err(error) => {
                            fail_with_exit(
                                &mut current_command,
                                &pending_rx,
                                &notifications_tx,
                                &fatal_exit_tx,
                                &mut notifications,
                                event_wakeup_tx.as_ref(),
                                error,
                            );
                            break;
                        }
                    }
                }
                ControlStateEvent::CommandComplete { is_error, output } => {
                    let Some(active_command) = current_command.take() else {
                        fail_with_exit(
                            &mut current_command,
                            &pending_rx,
                            &notifications_tx,
                            &fatal_exit_tx,
                            &mut notifications,
                            event_wakeup_tx.as_ref(),
                            TmuxControlError::protocol(
                                "received command completion without a pending request",
                            ),
                        );
                        break;
                    };

                    let ActiveControlCommand::Tracked(mut tracked) = active_command else {
                        continue;
                    };

                    let Some(output_chunk) = split_control_completion_token(
                        output.as_str(),
                        tracked.pending.completion_token.as_str(),
                    ) else {
                        append_command_output_chunk(&mut tracked.output, output.as_str());
                        tracked.is_error |= is_error;
                        current_command = Some(ActiveControlCommand::Tracked(tracked));
                        continue;
                    };

                    append_command_output_chunk(&mut tracked.output, output_chunk.as_str());
                    tracked.is_error |= is_error;
                    let response = map_command_completion_response(
                        &tracked.pending.command,
                        tracked.is_error,
                        tracked.output,
                    );
                    complete_pending_command(tracked.pending, response);
                }
                ControlStateEvent::Exit(reason) => {
                    fail_all_pending(
                        &mut current_command,
                        &pending_rx,
                        TmuxControlError::channel(
                            reason
                                .clone()
                                .unwrap_or_else(|| "tmux control mode exited".to_string()),
                        ),
                    );
                    signal_fatal_exit(&fatal_exit_tx, reason.clone());
                    notifications.push(TmuxNotification::Exit(reason));
                    let _ = flush_notification_coalescer(
                        &mut notifications,
                        &notifications_tx,
                        event_wakeup_tx.as_ref(),
                    );
                    break;
                }
            }

            if let Err(error) = flush_notification_coalescer(
                &mut notifications,
                &notifications_tx,
                event_wakeup_tx.as_ref(),
            ) {
                fail_with_exit(
                    &mut current_command,
                    &pending_rx,
                    &notifications_tx,
                    &fatal_exit_tx,
                    &mut notifications,
                    event_wakeup_tx.as_ref(),
                    error,
                );
                break;
            }
        }
    });
}
