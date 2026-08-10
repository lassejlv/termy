use super::super::*;
use super::TmuxResizeWakeup;
use termy_terminal_ui::{CommandLifecycle, ProgressState, TmuxPaneState, TmuxWindowState};

mod actions;
mod events;
mod snapshot;
mod transition;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TmuxSnapshotRefreshMode {
    None,
    Debounced,
    Immediate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TmuxCutoverCleanupDecision {
    Proceed,
    AbortOldCleanupFailure,
    AbortOldAndNewCleanupFailure,
}

fn tmux_cutover_cleanup_decision(
    old_cleanup_succeeded: bool,
    new_cleanup_succeeded: bool,
) -> TmuxCutoverCleanupDecision {
    if old_cleanup_succeeded {
        TmuxCutoverCleanupDecision::Proceed
    } else if new_cleanup_succeeded {
        TmuxCutoverCleanupDecision::AbortOldCleanupFailure
    } else {
        TmuxCutoverCleanupDecision::AbortOldAndNewCleanupFailure
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TmuxDetachTransitionDecision {
    AbortNativeRuntimeStart,
    AbortTmuxShutdown,
    CommitNativeTransition,
}

fn tmux_detach_transition_decision(
    native_runtime_started: bool,
    shutdown_succeeded: bool,
) -> TmuxDetachTransitionDecision {
    if !native_runtime_started {
        TmuxDetachTransitionDecision::AbortNativeRuntimeStart
    } else if !shutdown_succeeded {
        TmuxDetachTransitionDecision::AbortTmuxShutdown
    } else {
        TmuxDetachTransitionDecision::CommitNativeTransition
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TmuxExitRecoveryDecision {
    TransitionToNative,
    RestartControlMode,
}

fn tmux_exit_recovery_decision(exclusive: bool) -> TmuxExitRecoveryDecision {
    if exclusive {
        TmuxExitRecoveryDecision::RestartControlMode
    } else {
        TmuxExitRecoveryDecision::TransitionToNative
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TmuxPostActionRefresh {
    ImmediateSnapshot,
    EventDriven,
}

fn tmux_hydration_warning_message(failures: &[String]) -> Option<String> {
    if failures.is_empty() {
        return None;
    }

    let preview = failures
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if failures.len() > 3 { ", ..." } else { "" };
    Some(format!(
        "tmux pane restore degraded for {} pane(s): {preview}{suffix}",
        failures.len()
    ))
}

impl TerminalPane {
    fn from_tmux_state(state: &TmuxPaneState, terminal: Terminal, degraded: bool) -> Self {
        let cached_element_ids = PaneCachedElementIds::new(&state.id);
        Self {
            id: state.id.clone(),
            left: state.left,
            top: state.top,
            width: state.width,
            height: state.height,
            pane_zoom_steps: 0,
            degraded,
            tmux_mouse_mode: Some(state.mouse_mode),
            progress_state: ProgressState::default(),
            terminal,
            render_cache: std::cell::RefCell::new(TerminalPaneRenderCache::default()),
            last_alternate_screen: std::cell::Cell::new(false),
            cached_element_ids,
        }
    }
}

impl TerminalTab {
    fn from_tmux_window(id: TabId, window: &TmuxWindowState, panes: Vec<TerminalPane>) -> Self {
        let title = DEFAULT_TAB_TITLE.to_string();
        // Width starts as lazy/unknown and is measured when tab-strip layout runs.
        // Initializing to zero keeps creation deterministic before first paint.
        let title_text_width = 0.0;
        let sticky_title_width = TerminalView::tab_display_width_for_text_px_without_close_with_max(
            title_text_width,
            TAB_MAX_WIDTH,
        );
        let display_width =
            TerminalView::tab_display_width_for_text_px_with_max(title_text_width, TAB_MAX_WIDTH);

        Self {
            id,
            window_id: window.id.clone(),
            window_index: window.index,
            active_pane_id: window
                .active_pane_id
                .clone()
                .or_else(|| panes.first().map(|pane| pane.id.clone()))
                .unwrap_or_default(),
            panes,
            pinned: false,
            manual_title: None,
            explicit_title: None,
            explicit_title_is_prediction: false,
            shell_title: None,
            current_command: None,
            pending_command_title: None,
            pending_command_token: 0,
            last_prompt_cwd: None,
            title,
            title_text_width,
            sticky_title_width,
            display_width,
            running_process: false,
            command_lifecycle: CommandLifecycle::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_cutover_cleanup_decision_distinguishes_old_only_vs_combined_failure() {
        assert_eq!(
            tmux_cutover_cleanup_decision(true, true),
            TmuxCutoverCleanupDecision::Proceed
        );
        assert_eq!(
            tmux_cutover_cleanup_decision(false, true),
            TmuxCutoverCleanupDecision::AbortOldCleanupFailure
        );
        assert_eq!(
            tmux_cutover_cleanup_decision(false, false),
            TmuxCutoverCleanupDecision::AbortOldAndNewCleanupFailure
        );
    }

    #[test]
    fn tmux_detach_transition_decision_requires_native_and_shutdown_success() {
        assert_eq!(
            tmux_detach_transition_decision(false, false),
            TmuxDetachTransitionDecision::AbortNativeRuntimeStart
        );
        assert_eq!(
            tmux_detach_transition_decision(true, false),
            TmuxDetachTransitionDecision::AbortTmuxShutdown
        );
        assert_eq!(
            tmux_detach_transition_decision(true, true),
            TmuxDetachTransitionDecision::CommitNativeTransition
        );
    }

    #[test]
    fn tmux_exit_recovery_decision_respects_exclusive_mode() {
        assert_eq!(
            tmux_exit_recovery_decision(false),
            TmuxExitRecoveryDecision::TransitionToNative
        );
        assert_eq!(
            tmux_exit_recovery_decision(true),
            TmuxExitRecoveryDecision::RestartControlMode
        );
    }

    #[test]
    fn tmux_hydration_warning_message_is_none_for_empty_failures() {
        assert!(tmux_hydration_warning_message(&[]).is_none());
    }

    #[test]
    fn tmux_hydration_warning_message_truncates_preview_after_three_entries() {
        let failures = vec![
            "%1 (capture timeout)".to_string(),
            "%2 (capture timeout)".to_string(),
            "%3 (capture timeout)".to_string(),
            "%4 (capture timeout)".to_string(),
        ];
        let message = tmux_hydration_warning_message(&failures).expect("warning expected");
        assert!(message.contains("4 pane(s)"));
        assert!(
            message
                .contains("%1 (capture timeout), %2 (capture timeout), %3 (capture timeout), ...")
        );
    }
}
