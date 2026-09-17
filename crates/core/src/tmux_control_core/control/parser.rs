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

        if line.starts_with(b"%begin") {
            if self.current_block.is_some() {
                return Err(TmuxControlError::protocol(
                    "received nested %begin while previous command block was open",
                ));
            }
            let command_tag = parse_control_block_tag(line, b"%begin")?;
            self.current_block = Some(ControlCommandBlock {
                command_tag,
                output: String::new(),
            });
            return Ok(ControlStateEvent::CommandBegin);
        }

        if line.starts_with(b"%end") || line.starts_with(b"%error") {
            let is_error = line.starts_with(b"%error");
            let command_tag =
                parse_control_block_tag(line, if is_error { b"%error" } else { b"%end" })?;
            let block = self.current_block.take().ok_or_else(|| {
                TmuxControlError::protocol("received command terminator without open %begin block")
            })?;
            if command_tag != block.command_tag {
                return Err(TmuxControlError::protocol(format!(
                    "received mismatched command terminator: expected '{}', got '{}'",
                    block.command_tag, command_tag
                )));
            }
            return Ok(ControlStateEvent::CommandComplete {
                is_error,
                output: block.output,
            });
        }

        // Notifications can interleave with both async and sync command blocks.
        // Always route pane output/layout events to the UI instead of treating
        // them as command stdout, otherwise redraw control bytes can be dropped.
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

        if let Some(block) = self.current_block.as_mut() {
            // Preserve empty lines inside command output blocks so pane captures
            // round-trip faithfully through control-mode framing.
            let line_text = std::str::from_utf8(line).map_err(|_| {
                TmuxControlError::parse("received non-utf8 bytes in control command output")
            })?;
            if !block.output.is_empty() {
                block.output.push('\n');
            }
            block.output.push_str(line_text);
        }

        Ok(ControlStateEvent::None)
    }
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
    use super::super::super::command::split_control_completion_token;
    use super::super::super::types::{TmuxControlErrorKind, TmuxNotification};
    use super::super::channel::{
        ActiveControlCommand, PendingCommand, TrackedPendingCommand, append_command_output_chunk,
        claim_pending_for_command_begin, complete_pending_command, map_command_completion_response,
    };
    use super::{ControlStateEvent, ControlStateMachine};
    use std::time::Duration;

    fn expect_command_complete(event: ControlStateEvent) -> (bool, String) {
        match event {
            ControlStateEvent::CommandComplete { is_error, output } => (is_error, output),
            other => panic!("expected command completion event, got {other:?}"),
        }
    }

    #[test]
    fn unsolicited_command_block_does_not_consume_next_tracked_pending_request() {
        let mut sm = ControlStateMachine::default();
        let (pending_tx, pending_rx) = flume::bounded::<PendingCommand>(1);

        assert_eq!(
            sm.on_line(b"%begin 1 1 0").expect("startup begin"),
            ControlStateEvent::CommandBegin
        );
        let mut active_command = match claim_pending_for_command_begin(&pending_rx).expect("claim")
        {
            Some(pending) => Some(ActiveControlCommand::Tracked(TrackedPendingCommand {
                pending,
                output: String::new(),
                is_error: false,
            })),
            None => Some(ActiveControlCommand::Untracked),
        };
        assert!(matches!(
            active_command,
            Some(ActiveControlCommand::Untracked)
        ));
        assert_eq!(
            sm.on_line(b"startup noise").expect("startup payload"),
            ControlStateEvent::None
        );
        expect_command_complete(sm.on_line(b"%end 1 1 0").expect("startup end"));
        match active_command
            .take()
            .expect("startup block should be active")
        {
            ActiveControlCommand::Tracked(_) => {
                panic!("unsolicited block must stay untracked")
            }
            ActiveControlCommand::Untracked => {}
        }

        let (response_tx, response_rx) = flume::bounded(1);
        let (completion_tx, completion_rx) = flume::bounded(1);
        pending_tx
            .send(PendingCommand {
                command: "list-windows".to_string(),
                completion_token: "tok-list-windows".to_string(),
                response_tx: Some(response_tx),
                completion_tx,
            })
            .expect("queue pending request");

        assert_eq!(
            sm.on_line(b"%begin 2 2 0").expect("command begin"),
            ControlStateEvent::CommandBegin
        );
        active_command = match claim_pending_for_command_begin(&pending_rx).expect("claim") {
            Some(pending) => Some(ActiveControlCommand::Tracked(TrackedPendingCommand {
                pending,
                output: String::new(),
                is_error: false,
            })),
            None => Some(ActiveControlCommand::Untracked),
        };
        assert!(matches!(
            active_command,
            Some(ActiveControlCommand::Tracked(_))
        ));
        assert_eq!(
            sm.on_line(b"ok").expect("command output"),
            ControlStateEvent::None
        );
        let (is_error, output) = expect_command_complete(sm.on_line(b"%end 2 2 0").expect("end"));
        let mut tracked = match active_command.take().expect("tracked command") {
            ActiveControlCommand::Tracked(tracked) => tracked,
            ActiveControlCommand::Untracked => panic!("tracked request became untracked"),
        };
        append_command_output_chunk(&mut tracked.output, output.as_str());
        let response =
            map_command_completion_response(&tracked.pending.command, is_error, tracked.output);
        complete_pending_command(tracked.pending, response);

        assert!(
            completion_rx
                .recv_timeout(Duration::from_millis(50))
                .is_ok()
        );
        let result = response_rx
            .recv_timeout(Duration::from_millis(50))
            .expect("response sent");
        let completion = result.expect("tracked command should succeed");
        assert_eq!(completion.output, "ok");
    }

    #[test]
    fn tracked_command_spanning_multiple_blocks_completes_only_after_token_block() {
        let mut sm = ControlStateMachine::default();
        let (pending_tx, pending_rx) = flume::bounded::<PendingCommand>(1);

        let completion_token = "__termy_cmd_done_multi".to_string();
        let (response_tx, response_rx) = flume::bounded(1);
        let (completion_tx, completion_rx) = flume::bounded(1);
        pending_tx
            .send(PendingCommand {
                command: "cmd1 ; cmd2".to_string(),
                completion_token: completion_token.clone(),
                response_tx: Some(response_tx),
                completion_tx,
            })
            .expect("queue pending request");

        assert_eq!(
            sm.on_line(b"%begin 20 1 0").expect("begin 1"),
            ControlStateEvent::CommandBegin
        );
        let mut active_command = match claim_pending_for_command_begin(&pending_rx).expect("claim")
        {
            Some(pending) => Some(ActiveControlCommand::Tracked(TrackedPendingCommand {
                pending,
                output: String::new(),
                is_error: false,
            })),
            None => Some(ActiveControlCommand::Untracked),
        };
        assert!(matches!(
            active_command,
            Some(ActiveControlCommand::Tracked(_))
        ));

        assert_eq!(
            sm.on_line(b"first block payload").expect("payload 1"),
            ControlStateEvent::None
        );
        let (is_error_1, output_1) =
            expect_command_complete(sm.on_line(b"%end 20 1 0").expect("end 1"));
        assert!(!is_error_1);
        let tracked = match active_command
            .as_mut()
            .expect("tracked command should remain active")
        {
            ActiveControlCommand::Tracked(tracked) => tracked,
            ActiveControlCommand::Untracked => panic!("tracked request became untracked"),
        };
        assert!(
            split_control_completion_token(&output_1, completion_token.as_str()).is_none(),
            "command should stay active until completion token is observed",
        );
        append_command_output_chunk(&mut tracked.output, output_1.as_str());
        assert!(
            completion_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err()
        );

        assert_eq!(
            sm.on_line(b"%begin 20 2 0").expect("begin 2"),
            ControlStateEvent::CommandBegin
        );
        assert_eq!(
            sm.on_line(b"second block payload").expect("payload 2"),
            ControlStateEvent::None
        );
        assert_eq!(
            sm.on_line(completion_token.as_bytes())
                .expect("token payload"),
            ControlStateEvent::None
        );
        let (is_error_2, output_2) =
            expect_command_complete(sm.on_line(b"%end 20 2 0").expect("end 2"));
        assert!(!is_error_2);

        let mut tracked = match active_command.take().expect("active command") {
            ActiveControlCommand::Tracked(tracked) => tracked,
            ActiveControlCommand::Untracked => panic!("tracked request became untracked"),
        };
        let output_chunk = split_control_completion_token(&output_2, completion_token.as_str())
            .expect("token should be present in final block output");
        append_command_output_chunk(&mut tracked.output, output_chunk.as_str());
        let response =
            map_command_completion_response(&tracked.pending.command, false, tracked.output);
        complete_pending_command(tracked.pending, response);

        assert!(
            completion_rx
                .recv_timeout(Duration::from_millis(50))
                .is_ok()
        );
        let result = response_rx
            .recv_timeout(Duration::from_millis(50))
            .expect("response sent");
        let completion = result.expect("tracked command should succeed");
        assert_eq!(
            completion.output,
            "first block payload\nsecond block payload"
        );
    }

    #[test]
    fn control_state_machine_keeps_notifications_out_of_command_output() {
        let mut sm = ControlStateMachine::default();
        assert_eq!(
            sm.on_line(b"%begin 10 3 0").expect("begin"),
            ControlStateEvent::CommandBegin
        );

        assert_eq!(
            sm.on_line(b"%output %1 hi\\012").expect("notification"),
            ControlStateEvent::Notification(TmuxNotification::Output {
                pane_id: "%1".to_string(),
                bytes: b"hi\r\n".to_vec(),
            })
        );

        assert_eq!(
            sm.on_line(b"ok").expect("output line"),
            ControlStateEvent::None
        );
        let (is_error, output) = expect_command_complete(sm.on_line(b"%end 10 3 0").expect("end"));
        assert!(!is_error);
        assert_eq!(output.trim(), "ok");
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
    fn control_state_machine_rejects_mismatched_end_marker() {
        let mut sm = ControlStateMachine::default();
        assert_eq!(
            sm.on_line(b"%begin 44 9 0").expect("begin"),
            ControlStateEvent::CommandBegin
        );
        let err = sm
            .on_line(b"%end 45 9 0")
            .expect_err("mismatch should fail");
        assert_eq!(err.kind, TmuxControlErrorKind::Protocol);
        assert!(err.message.contains("mismatched command terminator"));
    }

    #[test]
    fn control_state_machine_routes_refresh_notifications_while_collecting_output() {
        let mut sm = ControlStateMachine::default();
        assert_eq!(
            sm.on_line(b"%begin 72 1 0").expect("begin"),
            ControlStateEvent::CommandBegin
        );
        assert_eq!(
            sm.on_line(b"%layout-change @1 tiled").expect("refresh"),
            ControlStateEvent::Notification(TmuxNotification::NeedsRefresh)
        );
        assert_eq!(
            sm.on_line(b"captured").expect("output line"),
            ControlStateEvent::None
        );

        let (is_error, output) = expect_command_complete(sm.on_line(b"%end 72 1 0").expect("end"));
        assert!(!is_error);
        assert_eq!(output, "captured");
    }

    #[test]
    fn control_state_machine_routes_subscription_changed_notifications_inside_command_block() {
        let mut sm = ControlStateMachine::default();
        assert_eq!(
            sm.on_line(b"%begin 81 1 0").expect("begin"),
            ControlStateEvent::CommandBegin
        );
        assert_eq!(
            sm.on_line(b"%subscription-changed p_all $0 @0 0 %0 : /tmp")
                .expect("subscription"),
            ControlStateEvent::Notification(TmuxNotification::SubscriptionChanged {
                name: "p_all".to_string(),
                session: "$0".to_string(),
                window: "@0".to_string(),
                pane: "%0".to_string(),
                value: "/tmp".to_string(),
            })
        );
        assert_eq!(
            sm.on_line(b"captured").expect("output line"),
            ControlStateEvent::None
        );

        let (is_error, output) = expect_command_complete(sm.on_line(b"%end 81 1 0").expect("end"));
        assert!(!is_error);
        assert_eq!(output, "captured");
    }

    #[test]
    fn control_state_machine_handles_error_block_with_interleaved_refresh() {
        let mut sm = ControlStateMachine::default();
        assert_eq!(
            sm.on_line(b"%begin 91 1 0").expect("begin"),
            ControlStateEvent::CommandBegin
        );
        assert_eq!(
            sm.on_line(b"%layout-change @1 tiled").expect("refresh"),
            ControlStateEvent::Notification(TmuxNotification::NeedsRefresh)
        );
        assert_eq!(
            sm.on_line(b"failed").expect("output line"),
            ControlStateEvent::None
        );

        let (is_error, output) =
            expect_command_complete(sm.on_line(b"%error 91 1 0").expect("error"));
        assert!(is_error);
        assert_eq!(output, "failed");
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
