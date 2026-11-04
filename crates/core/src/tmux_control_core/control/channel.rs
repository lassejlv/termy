#![cfg_attr(not(unix), allow(dead_code))]

use std::collections::VecDeque;

use super::super::command::is_control_completion_token;
use super::super::types::TmuxControlError;
#[cfg(test)]
use super::super::types::TmuxControlErrorKind;
use flume::{Receiver, Sender, TryRecvError, TrySendError};

pub const REQUEST_QUEUE_BOUND: usize = 1024;
pub const PENDING_QUEUE_BOUND: usize = 1;
pub const NOTIFICATION_QUEUE_BOUND: usize = 2048;
pub const FATAL_EXIT_QUEUE_BOUND: usize = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlCommandResult {
    pub output: String,
}

#[derive(Debug)]
pub struct ControlRequest {
    pub command: String,
    pub completion_token: String,
    pub response_tx: Option<Sender<std::result::Result<ControlCommandResult, TmuxControlError>>>,
}

#[derive(Debug)]
pub struct PendingCommand {
    pub command: String,
    pub completion_token: String,
    pub response_tx: Option<Sender<std::result::Result<ControlCommandResult, TmuxControlError>>>,
    pub completion_tx: Sender<()>,
}

#[derive(Debug)]
pub struct TrackedPendingCommand {
    pub pending: PendingCommand,
    pub output: String,
    /// Output of the `%error` blocks seen so far. `Some` fails the request
    /// once its completion token arrives.
    pub error_output: Option<String>,
}

impl TrackedPendingCommand {
    pub fn new(pending: PendingCommand) -> Self {
        Self {
            pending,
            output: String::new(),
            error_output: None,
        }
    }

    fn complete(self) {
        let response = match self.error_output {
            Some(error_output) => {
                map_command_completion_response(&self.pending.command, true, error_output)
            }
            None => map_command_completion_response(&self.pending.command, false, self.output),
        };
        complete_pending_command(self.pending, response);
    }
}

/// Matches tmux `%begin`/`%end` blocks to the requests that produced them.
///
/// The worker writes each request as two control-mode lines: the caller's
/// command, then a line printing the request's token (see
/// [`control_request_lines`]). tmux runs control-mode lines in the order it
/// reads them and emits one block per command, so a request's blocks (none or
/// more for the command, then exactly one holding only its token) arrive
/// contiguously and in request order. Every block therefore belongs to the
/// oldest outstanding request, which completes when its own token block
/// arrives; `%error` blocks before that fail it with their output.
///
/// [`control_request_lines`]: super::super::command::control_request_lines
#[derive(Debug, Default)]
pub struct ControlCommandTracker {
    outstanding: VecDeque<TrackedPendingCommand>,
    /// `Some(tracked)` while a block is open: whether it belongs to the front
    /// of `outstanding` (`false` for unsolicited blocks).
    open_block: Option<bool>,
}

impl ControlCommandTracker {
    pub fn on_block_begin(
        &mut self,
        pending_rx: &Receiver<PendingCommand>,
    ) -> std::result::Result<(), TmuxControlError> {
        while let Some(pending) = claim_pending_for_command_begin(pending_rx)? {
            self.outstanding
                .push_back(TrackedPendingCommand::new(pending));
        }
        // tmux may emit unsolicited startup/control blocks before the first
        // app-issued command is tracked; consume those without binding a
        // request.
        self.open_block = Some(!self.outstanding.is_empty());
        Ok(())
    }

    pub fn on_block_complete(
        &mut self,
        is_error: bool,
        output: String,
    ) -> std::result::Result<(), TmuxControlError> {
        let Some(tracked) = self.open_block.take() else {
            return Err(TmuxControlError::protocol(
                "received command completion without a pending request",
            ));
        };
        if !tracked {
            return Ok(());
        }

        if !is_error
            && let Some(index) = self
                .outstanding
                .iter()
                .position(|command| command.pending.completion_token == output)
        {
            // Requests ahead of this one can only be here if their token
            // block never arrived. Fail them so they can't swallow the output
            // of every later request.
            for lost in self.outstanding.drain(..index) {
                complete_pending_command(
                    lost.pending,
                    Err(TmuxControlError::protocol(
                        "tmux control completion token was lost",
                    )),
                );
            }
            if let Some(done) = self.outstanding.pop_front() {
                done.complete();
            }
            return Ok(());
        }

        let Some(front) = self.outstanding.front_mut() else {
            return Ok(());
        };
        if is_error {
            append_command_output_chunk(
                front.error_output.get_or_insert_with(String::new),
                output.as_str(),
            );
        } else if !is_control_completion_token(output.as_str()) {
            // A token that matches no outstanding request belongs to one that
            // is no longer tracked; it is never output of this request.
            append_command_output_chunk(&mut front.output, output.as_str());
        }
        Ok(())
    }

    /// Fails every tracked and queued request with `error`.
    pub fn fail_all(&mut self, pending_rx: &Receiver<PendingCommand>, error: &TmuxControlError) {
        self.open_block = None;
        for tracked in self.outstanding.drain(..) {
            complete_pending_command(tracked.pending, Err(error.clone()));
        }
        while let Ok(pending) = pending_rx.try_recv() {
            complete_pending_command(pending, Err(error.clone()));
        }
    }
}

pub fn try_enqueue_control_request(
    request_tx: &Sender<ControlRequest>,
    request: ControlRequest,
) -> std::result::Result<(), TmuxControlError> {
    match request_tx.try_send(request) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(TmuxControlError::channel(format!(
            "tmux control request queue is full (capacity {REQUEST_QUEUE_BOUND})"
        ))),
        Err(TrySendError::Disconnected(_)) => {
            Err(TmuxControlError::channel("tmux control channel is closed"))
        }
    }
}

pub fn claim_pending_for_command_begin(
    pending_rx: &Receiver<PendingCommand>,
) -> std::result::Result<Option<PendingCommand>, TmuxControlError> {
    // Hard cutover: `%begin` must bind only to a request that is already pending.
    // Waiting here can mis-associate unsolicited tmux blocks with unrelated requests.
    match pending_rx.try_recv() {
        Ok(pending) => Ok(Some(pending)),
        Err(TryRecvError::Empty) => Ok(None),
        Err(TryRecvError::Disconnected) => Err(TmuxControlError::channel(
            "tmux control pending-command channel closed",
        )),
    }
}

pub fn complete_pending_command(
    pending: PendingCommand,
    response: std::result::Result<ControlCommandResult, TmuxControlError>,
) {
    // Never block the reader: a request the writer already failed on timeout
    // has its single response slot filled, and the late answer is dropped.
    if let Some(response_tx) = pending.response_tx {
        let _ = response_tx.try_send(response);
    }
    let _ = pending.completion_tx.try_send(());
}

pub fn map_command_completion_response(
    command: &str,
    is_error: bool,
    output: String,
) -> std::result::Result<ControlCommandResult, TmuxControlError> {
    if !is_error {
        return Ok(ControlCommandResult { output });
    }

    let trimmed = output.trim();
    Err(TmuxControlError::runtime(if trimmed.is_empty() {
        format!("command '{command}' failed")
    } else {
        trimmed.to_string()
    }))
}

pub fn append_command_output_chunk(accumulator: &mut String, chunk: &str) {
    if !accumulator.is_empty() {
        accumulator.push('\n');
    }
    accumulator.push_str(chunk);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_enqueue_reports_backpressure_when_queue_is_full() {
        let (request_tx, _request_rx) = flume::bounded::<ControlRequest>(1);
        request_tx
            .try_send(ControlRequest {
                command: "first".to_string(),
                completion_token: "tok-first".to_string(),
                response_tx: None,
            })
            .expect("seed queue");

        let err = try_enqueue_control_request(
            &request_tx,
            ControlRequest {
                command: "second".to_string(),
                completion_token: "tok-second".to_string(),
                response_tx: None,
            },
        )
        .expect_err("full queue should fail");
        assert_eq!(err.kind, TmuxControlErrorKind::Channel);
        assert!(err.message.contains("request queue is full"));
    }

    #[test]
    fn claim_pending_for_command_begin_is_non_blocking() {
        let (pending_tx, pending_rx) = flume::bounded::<PendingCommand>(1);
        let (response_tx, _response_rx) = flume::bounded(1);
        let (completion_tx, _completion_rx) = flume::bounded(1);

        let claimed = claim_pending_for_command_begin(&pending_rx).expect("claim should succeed");
        assert!(claimed.is_none(), "empty queue must stay untracked");

        pending_tx
            .send(PendingCommand {
                command: "list-windows".to_string(),
                completion_token: "__done".to_string(),
                response_tx: Some(response_tx),
                completion_tx,
            })
            .expect("queue pending request");
        let claimed = claim_pending_for_command_begin(&pending_rx).expect("claim should succeed");
        assert!(
            claimed.is_some(),
            "queued request must be claimable immediately"
        );
    }

    #[test]
    fn command_completion_response_maps_error_output_to_runtime_error() {
        let success = map_command_completion_response("list-windows", false, "row".to_string())
            .expect("successful completion should preserve output");
        assert_eq!(success.output, "row");

        let error =
            map_command_completion_response("kill-pane", true, "pane not found\n".to_string())
                .expect_err("error completion should fail");
        assert_eq!(error.kind, TmuxControlErrorKind::Runtime);
        assert_eq!(error.message, "pane not found");

        let fallback_error = map_command_completion_response("kill-pane", true, " \n".to_string())
            .expect_err("empty error completion should fail");
        assert_eq!(fallback_error.kind, TmuxControlErrorKind::Runtime);
        assert_eq!(fallback_error.message, "command 'kill-pane' failed");
    }
}
