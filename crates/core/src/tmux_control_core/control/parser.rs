#![cfg_attr(not(unix), allow(dead_code))]

use super::super::payload::{
    is_refresh_notification, parse_exit_reason, parse_output_notification,
    parse_subscription_changed, strip_control_line_wrappers,
};
use super::super::types::{TmuxControlError, TmuxNotification};

#[derive(Debug, PartialEq, Eq)]
pub enum ControlStateEvent {
    None,
    Notification(TmuxNotification),
    CommandBegin,
    CommandComplete { is_error: bool, output: String },
    Exit(Option<String>),
}

#[derive(Debug)]
struct ControlCommandBlock {
    command_tag: String,
    output: String,
}

#[derive(Debug, Default)]
pub struct ControlStateMachine {
    current_block: Option<ControlCommandBlock>,
}

impl ControlStateMachine {
    pub fn on_line(
        &mut self,
        line: &[u8],
    ) -> std::result::Result<ControlStateEvent, TmuxControlError> {
        let line = strip_control_line_wrappers(line);
        if line.is_empty() && self.current_block.is_none() {
            return Ok(ControlStateEvent::None);
        }

        // tmux never emits notifications inside a %begin/%end output block: a
        // command's output is written synchronously and anything async is
        // queued behind its terminator (verified against tmux 3.4, including a
        // pane flooding output while commands run). Every line inside an open
        // block is therefore command output, even when it looks like a protocol
        // line: `capture-pane` of a pane that printed `%exit` or `%output %3 ...`
        // returns those strings verbatim and must not end the session or inject
        // bytes into another pane. Only the terminator carrying this block's tag
        // closes it.
        if let Some(block) = self.current_block.as_mut() {
            if let Some(is_error) = block_terminator(line, &block.command_tag) {
                let output = std::mem::take(&mut block.output);
                self.current_block = None;
                return Ok(ControlStateEvent::CommandComplete { is_error, output });
            }

            // Preserve empty lines inside command output blocks so pane captures
            // round-trip faithfully through control-mode framing.
            //
            // Command output is not guaranteed to be UTF-8. tmux octal-escapes
            // formats (`#{pane_current_path}`, window and session names), but
            // some commands print stored bytes raw: `show-buffer` of a buffer
            // holding `\xff` returns that byte verbatim inside the block
            // (verified against tmux 3.4). One such line must not end the whole
            // control session, so decode it lossily; the command still gets an
            // answer and later commands keep working.
            if !block.output.is_empty() {
                block.output.push('\n');
            }
            block.output.push_str(&String::from_utf8_lossy(line));
            return Ok(ControlStateEvent::None);
        }

        if line.starts_with(b"%begin") {
            let command_tag = parse_control_block_tag(line, b"%begin")?;
            self.current_block = Some(ControlCommandBlock {
                command_tag,
                output: String::new(),
            });
            return Ok(ControlStateEvent::CommandBegin);
        }

        if line.starts_with(b"%end") || line.starts_with(b"%error") {
            return Err(TmuxControlError::protocol(
                "received command terminator without open %begin block",
            ));
        }

        if let Some((pane_id, bytes)) = parse_output_notification(line) {
            return Ok(ControlStateEvent::Notification(TmuxNotification::Output {
                pane_id,
                bytes,
            }));
        }
        if let Some((name, session, window, pane, value)) = parse_subscription_changed(line) {
            return Ok(ControlStateEvent::Notification(
                TmuxNotification::SubscriptionChanged {
                    name,
                    session,
                    window,
                    pane,
                    value,
                },
            ));
        }
        if is_refresh_notification(line) {
            return Ok(ControlStateEvent::Notification(
                TmuxNotification::NeedsRefresh,
            ));
        }
        if line.starts_with(b"%exit") {
            return Ok(ControlStateEvent::Exit(parse_exit_reason(line)));
        }

        Ok(ControlStateEvent::None)
    }
}

/// Returns `Some(is_error)` when `line` is the `%end`/`%error` terminator for
/// the block opened with `command_tag`.
fn block_terminator(line: &[u8], command_tag: &str) -> Option<bool> {
    [(b"%end".as_slice(), false), (b"%error".as_slice(), true)]
        .into_iter()
        .find(|(marker, _)| {
            line.starts_with(marker)
                && parse_control_block_tag(line, marker).is_ok_and(|tag| tag == command_tag)
        })
        .map(|(_, is_error)| is_error)
}

fn parse_control_block_tag(
    line: &[u8],
    marker: &[u8],
) -> std::result::Result<String, TmuxControlError> {
    let Some(rest) = line.strip_prefix(marker) else {
        return Err(TmuxControlError::protocol(
            "failed to parse tmux control command marker",
        ));
    };
    let rest = rest.strip_prefix(b" ").unwrap_or(rest);
    let tag = std::str::from_utf8(rest)
        .map_err(|_| TmuxControlError::parse("received non-utf8 command marker metadata"))?;
    let tag = tag.trim();
    if tag.is_empty() {
        return Err(TmuxControlError::protocol(format!(
            "received '{}' without command metadata",
            String::from_utf8_lossy(marker)
        )));
    }
    Ok(tag.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::super::command::{control_request_lines, next_control_completion_token};
    use super::super::super::types::TmuxControlError;
    use super::super::super::types::{TmuxControlErrorKind, TmuxNotification};
    use super::super::channel::{ControlCommandResult, ControlCommandTracker, PendingCommand};
    use super::{ControlStateEvent, ControlStateMachine};
    use std::time::Duration;

    type Response = Result<ControlCommandResult, TmuxControlError>;

    fn expect_command_complete(event: ControlStateEvent) -> (bool, String) {
        match event {
            ControlStateEvent::CommandComplete { is_error, output } => (is_error, output),
            other => panic!("expected command completion event, got {other:?}"),
        }
    }

    /// Feeds tmux output lines through the parser and the request tracker,
    /// the way the worker's reader thread does.
    fn feed(
        sm: &mut ControlStateMachine,
        tracker: &mut ControlCommandTracker,
        pending_rx: &flume::Receiver<PendingCommand>,
        lines: &[&str],
    ) {
        for line in lines {
            match sm.on_line(line.as_bytes()).expect("parse line") {
                ControlStateEvent::CommandBegin => {
                    tracker.on_block_begin(pending_rx).expect("block begin");
                }
                ControlStateEvent::CommandComplete { is_error, output } => {
                    tracker
                        .on_block_complete(is_error, output)
                        .expect("block complete");
                }
                _ => {}
            }
        }
    }

    fn queue_pending(
        pending_tx: &flume::Sender<PendingCommand>,
        command: &str,
        token: &str,
    ) -> (flume::Receiver<Response>, flume::Receiver<()>) {
        let (response_tx, response_rx) = flume::bounded(1);
        let (completion_tx, completion_rx) = flume::bounded(1);
        pending_tx
            .send(PendingCommand {
                command: command.to_string(),
                completion_token: token.to_string(),
                response_tx: Some(response_tx),
                completion_tx,
            })
            .expect("queue pending request");
        (response_rx, completion_rx)
    }

    #[test]
    fn unsolicited_command_block_does_not_consume_next_tracked_pending_request() {
        let mut sm = ControlStateMachine::default();
        let mut tracker = ControlCommandTracker::default();
        let (pending_tx, pending_rx) = flume::bounded::<PendingCommand>(1);

        // A startup block before any request is consumed untracked.
        feed(
            &mut sm,
            &mut tracker,
            &pending_rx,
            &["%begin 1 1 0", "startup noise", "%end 1 1 0"],
        );

        let (response_rx, completion_rx) =
            queue_pending(&pending_tx, "list-windows", "__termy_cmd_done_1");
        feed(
            &mut sm,
            &mut tracker,
            &pending_rx,
            &[
                "%begin 2 2 0",
                "ok",
                "%end 2 2 0",
                "%begin 2 3 0",
                "__termy_cmd_done_1",
                "%end 2 3 0",
            ],
        );

        assert!(
            completion_rx
                .recv_timeout(Duration::from_millis(50))
                .is_ok()
        );
        let completion = response_rx
            .recv_timeout(Duration::from_millis(50))
            .expect("response sent")
            .expect("tracked command should succeed");
        assert_eq!(completion.output, "ok");
    }

    #[test]
    fn tracked_command_spanning_multiple_blocks_completes_only_after_token_block() {
        let mut sm = ControlStateMachine::default();
        let mut tracker = ControlCommandTracker::default();
        let (pending_tx, pending_rx) = flume::bounded::<PendingCommand>(1);

        let token = next_control_completion_token();
        let (response_rx, completion_rx) = queue_pending(&pending_tx, "cmd1 ; cmd2", &token);
        feed(
            &mut sm,
            &mut tracker,
            &pending_rx,
            &[
                "%begin 20 1 0",
                "first block payload",
                "%end 20 1 0",
                "%begin 20 2 0",
                "second block payload",
                "%end 20 2 0",
            ],
        );
        assert!(
            completion_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "command should stay active until its token block is observed",
        );

        feed(
            &mut sm,
            &mut tracker,
            &pending_rx,
            &["%begin 20 3 0", token.as_str(), "%end 20 3 0"],
        );
        assert!(
            completion_rx
                .recv_timeout(Duration::from_millis(50))
                .is_ok()
        );
        let completion = response_rx
            .recv_timeout(Duration::from_millis(50))
            .expect("response sent")
            .expect("tracked command should succeed");
        assert_eq!(
            completion.output,
            "first block payload\nsecond block payload"
        );
    }

    #[test]
    fn command_ending_in_comment_completes_on_its_own_token_line() {
        // What tmux 3.4 emits for the lines written for a command whose tail
        // is a comment: the comment swallows `; display-message -p skipped`,
        // but the token sits on its own line and still gets its own block.
        let mut sm = ControlStateMachine::default();
        let mut tracker = ControlCommandTracker::default();
        let (pending_tx, pending_rx) = flume::bounded::<PendingCommand>(1);

        let token = next_control_completion_token();
        let command = "display-message -p A # note ; display-message -p skipped";
        assert_eq!(
            control_request_lines(command, &token).lines().count(),
            2,
            "token must not share the command's line"
        );
        let (response_rx, _completion_rx) = queue_pending(&pending_tx, command, &token);
        feed(
            &mut sm,
            &mut tracker,
            &pending_rx,
            &[
                "%begin 3 1 1",
                "A",
                "%end 3 1 1",
                "%begin 3 2 1",
                token.as_str(),
                "%end 3 2 1",
            ],
        );
        let completion = response_rx
            .recv_timeout(Duration::from_millis(50))
            .expect("response sent")
            .expect("commented command should succeed");
        assert_eq!(completion.output, "A");
    }

    #[test]
    fn error_block_fails_request_with_error_output_only() {
        let mut sm = ControlStateMachine::default();
        let mut tracker = ControlCommandTracker::default();
        let (pending_tx, pending_rx) = flume::bounded::<PendingCommand>(1);

        let token = next_control_completion_token();
        let (response_rx, completion_rx) = queue_pending(
            &pending_tx,
            "display-message -p before ; select-pane -t %999",
            &token,
        );
        feed(
            &mut sm,
            &mut tracker,
            &pending_rx,
            &[
                "%begin 4 1 1",
                "before",
                "%end 4 1 1",
                "%begin 4 2 1",
                "can't find pane: %999",
                "%error 4 2 1",
            ],
        );
        assert!(
            completion_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "an error block alone must not complete the request"
        );
        feed(
            &mut sm,
            &mut tracker,
            &pending_rx,
            &["%begin 4 3 1", token.as_str(), "%end 4 3 1"],
        );
        let error = response_rx
            .recv_timeout(Duration::from_millis(50))
            .expect("response sent")
            .expect_err("error block should fail the request");
        assert_eq!(error.kind, TmuxControlErrorKind::Runtime);
        assert_eq!(error.message, "can't find pane: %999");
    }

    #[test]
    fn token_of_later_request_fails_requests_whose_token_was_lost() {
        let mut sm = ControlStateMachine::default();
        let mut tracker = ControlCommandTracker::default();
        let (pending_tx, pending_rx) = flume::bounded::<PendingCommand>(2);

        let lost_token = next_control_completion_token();
        let next_token = next_control_completion_token();
        let (lost_rx, _) = queue_pending(&pending_tx, "lost", &lost_token);
        let (next_rx, _) = queue_pending(&pending_tx, "next", &next_token);
        feed(
            &mut sm,
            &mut tracker,
            &pending_rx,
            &[
                "%begin 5 1 1",
                "orphaned",
                "%end 5 1 1",
                "%begin 5 2 1",
                next_token.as_str(),
                "%end 5 2 1",
            ],
        );
        let lost = lost_rx
            .recv_timeout(Duration::from_millis(50))
            .expect("lost request answered")
            .expect_err("lost request fails");
        assert_eq!(lost.kind, TmuxControlErrorKind::Protocol);
        assert!(
            next_rx
                .recv_timeout(Duration::from_millis(50))
                .expect("next request answered")
                .is_ok()
        );
    }

    #[test]
    fn control_state_machine_preserves_blank_lines_inside_command_output() {
        let mut sm = ControlStateMachine::default();
        assert_eq!(
            sm.on_line(b"%begin 13 1 0").expect("begin"),
            ControlStateEvent::CommandBegin
        );
        assert_eq!(
            sm.on_line(b"row-1").expect("row 1"),
            ControlStateEvent::None
        );
        assert_eq!(sm.on_line(b"").expect("blank row"), ControlStateEvent::None);
        assert_eq!(
            sm.on_line(b"row-3").expect("row 3"),
            ControlStateEvent::None
        );
        let (is_error, output) = expect_command_complete(sm.on_line(b"%end 13 1 0").expect("end"));
        assert!(!is_error);
        assert_eq!(output, "row-1\n\nrow-3");
    }

    #[test]
    fn control_state_machine_decodes_non_utf8_command_output_lossily() {
        // `show-buffer` of a buffer holding a 0xff byte prints it raw inside
        // the block (tmux 3.4). The block still completes and the session
        // keeps parsing later lines.
        let mut sm = ControlStateMachine::default();
        assert_eq!(
            sm.on_line(b"%begin 14 1 1").expect("begin"),
            ControlStateEvent::CommandBegin
        );
        assert_eq!(
            sm.on_line(b"b\xffb").expect("non-utf8 output line"),
            ControlStateEvent::None
        );
        assert_eq!(
            sm.on_line(b"/tmp/a\xc3").expect("truncated utf8 sequence"),
            ControlStateEvent::None
        );
        let (is_error, output) = expect_command_complete(sm.on_line(b"%end 14 1 1").expect("end"));
        assert!(!is_error);
        assert_eq!(output, "b\u{fffd}b\n/tmp/a\u{fffd}");

        let (is_error, output) = {
            sm.on_line(b"%begin 14 2 1").expect("next begin");
            sm.on_line(b"ok").expect("next output");
            expect_command_complete(sm.on_line(b"%end 14 2 1").expect("next end"))
        };
        assert!(!is_error);
        assert_eq!(output, "ok");
    }

    #[test]
    fn control_state_machine_handles_non_utf8_notifications() {
        let mut sm = ControlStateMachine::default();
        // tmux writes subscription values raw (tmux 3.4).
        assert_eq!(
            sm.on_line(b"%subscription-changed p_path $0 @0 0 %0 : /tmp/a\xffb")
                .expect("non-utf8 subscription value"),
            ControlStateEvent::Notification(TmuxNotification::SubscriptionChanged {
                name: "p_path".to_string(),
                session: "$0".to_string(),
                window: "@0".to_string(),
                pane: "%0".to_string(),
                value: "/tmp/a\u{fffd}b".to_string(),
            })
        );
        // Names in refresh notifications only trigger a refresh.
        for line in [
            b"%window-renamed @0 w\xffz".as_slice(),
            b"%session-changed $0 s\xffs",
            b"%unlinked-window-renamed @3 \xfe",
        ] {
            assert_eq!(
                sm.on_line(line).expect("non-utf8 refresh notification"),
                ControlStateEvent::Notification(TmuxNotification::NeedsRefresh)
            );
        }
        // Unhandled notifications are ignored, not fatal.
        assert_eq!(
            sm.on_line(b"%session-renamed $0 s\xffs")
                .expect("ignored notification"),
            ControlStateEvent::None
        );
        // `%output` stays byte-exact: bytes >= 0x80 arrive unescaped.
        assert_eq!(
            sm.on_line(b"%output %0 x\xffy").expect("non-utf8 output"),
            ControlStateEvent::Notification(TmuxNotification::Output {
                pane_id: "%0".to_string(),
                bytes: b"x\xffy".to_vec(),
            })
        );
        assert_eq!(
            sm.on_line(b"%exit detached from s\xffs").expect("exit"),
            ControlStateEvent::Exit(Some("detached from s\u{fffd}s".to_string()))
        );
    }

    #[test]
    fn control_state_machine_reports_error_block_output() {
        let mut sm = ControlStateMachine::default();
        assert_eq!(
            sm.on_line(b"%begin 11 8 0").expect("begin"),
            ControlStateEvent::CommandBegin
        );
        assert_eq!(
            sm.on_line(b"pane not found").expect("output line"),
            ControlStateEvent::None
        );

        let (is_error, output) =
            expect_command_complete(sm.on_line(b"%error 11 8 0").expect("error"));
        assert!(is_error);
        assert_eq!(output.trim(), "pane not found");
    }

    #[test]
    fn control_state_machine_treats_protocol_lines_inside_block_as_output() {
        // `capture-pane -p` of a pane that printed protocol-looking text returns
        // it verbatim inside the block (verified against tmux 3.4).
        let mut sm = ControlStateMachine::default();
        assert_eq!(
            sm.on_line(b"%begin 10 3 1").expect("begin"),
            ControlStateEvent::CommandBegin
        );
        let pane_text: [&[u8]; 8] = [
            b"%exit",
            b"%output %3 \\033]52;c;aGk=\\007",
            b"%layout-change @1 tiled",
            b"%subscription-changed p_all $0 @0 0 %0 : /tmp",
            b"%begin 1 1 1",
            b"%end 10 3 0",
            b"%error 99 3 1",
            b"%end",
        ];
        for line in pane_text {
            assert_eq!(
                sm.on_line(line).expect("pane text line"),
                ControlStateEvent::None,
                "{:?} must be command output",
                String::from_utf8_lossy(line)
            );
        }
        let (is_error, output) = expect_command_complete(sm.on_line(b"%end 10 3 1").expect("end"));
        assert!(!is_error);
        assert_eq!(
            output,
            pane_text
                .iter()
                .map(|line| std::str::from_utf8(line).expect("utf8"))
                .collect::<Vec<_>>()
                .join("\n")
        );

        // Outside a block the same lines are real notifications again.
        assert_eq!(
            sm.on_line(b"%output %1 hi\\015\\012")
                .expect("notification"),
            ControlStateEvent::Notification(TmuxNotification::Output {
                pane_id: "%1".to_string(),
                bytes: b"hi\r\n".to_vec(),
            })
        );
        assert_eq!(
            sm.on_line(b"%layout-change @1 tiled").expect("refresh"),
            ControlStateEvent::Notification(TmuxNotification::NeedsRefresh)
        );
        assert_eq!(
            sm.on_line(b"%exit").expect("exit"),
            ControlStateEvent::Exit(None)
        );
    }

    #[test]
    fn control_state_machine_error_block_keeps_protocol_lines_as_output() {
        let mut sm = ControlStateMachine::default();
        assert_eq!(
            sm.on_line(b"%begin 91 1 1").expect("begin"),
            ControlStateEvent::CommandBegin
        );
        assert_eq!(
            sm.on_line(b"%exit").expect("output line"),
            ControlStateEvent::None
        );
        let (is_error, output) =
            expect_command_complete(sm.on_line(b"%error 91 1 1").expect("error"));
        assert!(is_error);
        assert_eq!(output, "%exit");
    }

    #[test]
    fn control_state_machine_rejects_terminator_without_open_block() {
        let mut sm = ControlStateMachine::default();
        let err = sm
            .on_line(b"%end 45 9 0")
            .expect_err("stray terminator should fail");
        assert_eq!(err.kind, TmuxControlErrorKind::Protocol);
        assert!(err.message.contains("without open %begin"));
    }

    #[test]
    fn control_state_machine_accepts_dcs_wrapped_control_markers() {
        let mut sm = ControlStateMachine::default();
        assert_eq!(
            sm.on_line(b"\x1bP1000p%begin 9 4 0")
                .expect("wrapped begin"),
            ControlStateEvent::CommandBegin
        );
        assert_eq!(
            sm.on_line(b"ok").expect("output line"),
            ControlStateEvent::None
        );
        let (is_error, output) =
            expect_command_complete(sm.on_line(b"%end 9 4 0\x1b\\").expect("wrapped end"));
        assert!(!is_error);
        assert_eq!(output, "ok");
    }

    #[test]
    fn control_state_machine_ignores_standalone_dcs_terminator_line() {
        let mut sm = ControlStateMachine::default();
        assert_eq!(
            sm.on_line(b"\x1b\\").expect("dcs terminator"),
            ControlStateEvent::None
        );
    }
}
