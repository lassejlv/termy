use anyhow::{Result, anyhow};

use termy_core::tmux_control_core::types::TmuxShutdownMode;

pub(crate) fn is_tmux_missing_client_error(error: &anyhow::Error) -> bool {
    error_chain_contains(error, "can't find client")
}

pub(crate) fn is_tmux_missing_session_error(error: &anyhow::Error) -> bool {
    error_chain_contains(error, "can't find session")
}

pub(crate) fn is_tmux_no_server_error(error: &anyhow::Error) -> bool {
    error_chain_contains(error, "no server running on")
}

fn error_chain_contains(error: &anyhow::Error, needle: &str) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains(needle))
}

pub(crate) fn normalize_shutdown_teardown_result(
    session_name: &str,
    result: Result<()>,
) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if is_tmux_missing_session_error(&error) || is_tmux_no_server_error(&error) => {
            // Teardown mode is idempotent. If session/server is already gone,
            // cleanup is complete and should not block hard cutovers.
            Ok(())
        }
        Err(error) => Err(error).map_err(|error| {
            anyhow!("failed to teardown tmux managed session '{session_name}': {error:#}")
        }),
    }
}

pub(crate) fn run_shutdown_actions<DetachFn, TeardownFn>(
    mode: TmuxShutdownMode,
    session_name: &str,
    detach: DetachFn,
    teardown: TeardownFn,
) -> Result<()>
where
    DetachFn: FnOnce() -> Result<()>,
    TeardownFn: FnOnce() -> Result<()>,
{
    let detach_result = detach();
    let teardown_result = if matches!(mode, TmuxShutdownMode::DetachAndTeardownSession) {
        Some(teardown())
    } else {
        None
    };

    match (detach_result, teardown_result) {
        (Ok(()), None) => Ok(()),
        (Err(detach_error), None) => Err(detach_error),
        (Ok(()), Some(Ok(()))) => Ok(()),
        (Err(detach_error), Some(Ok(()))) => Err(detach_error),
        (Ok(()), Some(Err(teardown_error))) => Err(teardown_error),
        (Err(detach_error), Some(Err(teardown_error))) => Err(anyhow!(
            "failed shutdown for tmux session '{session_name}': detach failed: {detach_error:#}; teardown failed: {teardown_error:#}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::TmuxShutdownMode;
    use super::{normalize_shutdown_teardown_result, run_shutdown_actions};
    use anyhow::anyhow;
    use std::cell::Cell;

    #[test]
    fn teardown_mode_attempts_teardown_even_when_detach_fails() {
        let teardown_called = Cell::new(false);
        let result = run_shutdown_actions(
            TmuxShutdownMode::DetachAndTeardownSession,
            "test-session",
            || Err(anyhow!("forced detach failure")),
            || {
                teardown_called.set(true);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(teardown_called.get());
    }

    #[test]
    fn teardown_result_is_idempotent_when_session_is_already_missing() {
        let result = normalize_shutdown_teardown_result(
            "test-session",
            Err(anyhow!(
                "tmux session kill failed: can't find session: test-session"
            )),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn teardown_result_is_idempotent_when_server_is_already_gone() {
        let result = normalize_shutdown_teardown_result(
            "test-session",
            Err(anyhow!(
                "tmux session kill failed: no server running on /tmp/tmux-501/termy"
            )),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn teardown_result_checks_wrapped_error_chain() {
        let result = normalize_shutdown_teardown_result(
            "test-session",
            Err(anyhow!("can't find session: test-session").context("tmux session kill failed")),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn run_shutdown_actions_includes_both_failures_when_detach_and_teardown_fail() {
        let result = run_shutdown_actions(
            TmuxShutdownMode::DetachAndTeardownSession,
            "test-session",
            || Err(anyhow!("forced detach failure")),
            || Err(anyhow!("forced teardown failure")),
        );
        let error = result.expect_err("both failures should produce a combined error");
        let message = error.to_string();
        assert!(message.contains("forced detach failure"));
        assert!(message.contains("forced teardown failure"));
    }
}
