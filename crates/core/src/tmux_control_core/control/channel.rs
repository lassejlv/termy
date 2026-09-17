#![cfg_attr(not(unix), allow(dead_code))]

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
pub enum ActiveControlCommand {
    Tracked(TrackedPendingCommand),
    Untracked,
}

#[derive(Debug)]
pub struct TrackedPendingCommand {
    pub pending: PendingCommand,
    pub output: String,
    pub is_error: bool,
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
    if let Some(response_tx) = pending.response_tx {
        let _ = response_tx.send(response);
    }
    let _ = pending.completion_tx.send(());
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
