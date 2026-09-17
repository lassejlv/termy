//! Shell integration support for OSC 133 command lifecycle tracking
//! and OSC 9;4 progress indicators.

use std::time::Instant;

/// OSC 133 command lifecycle state machine.
///
/// Tracks the current phase of shell command execution based on
/// semantic shell integration escape sequences:
/// - OSC 133;A = Prompt start (shell is showing prompt)
/// - OSC 133;B = Command start (user is typing)
/// - OSC 133;C = Command executing (command has been submitted)
/// - OSC 133;D;code = Command finished with exit code
#[derive(Debug, Clone, Default)]
pub struct CommandLifecycle {
    /// Current phase of command execution
    pub phase: CommandPhase,
    /// Timestamp when command execution started (OSC 133;C)
    pub command_start: Option<Instant>,
    /// Last command exit code (from OSC 133;D)
    pub last_exit_code: Option<i32>,
}

impl CommandLifecycle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Transition to prompt shown state (OSC 133;A)
    pub fn prompt_start(&mut self) {
        self.phase = CommandPhase::PromptShown;
    }

    /// Transition to command input state (OSC 133;B)
    pub fn command_start(&mut self) {
        self.phase = CommandPhase::CommandInput;
    }

    /// Transition to executing state (OSC 133;C)
    pub fn command_executing(&mut self) {
        self.phase = CommandPhase::Executing;
        self.command_start = Some(Instant::now());
    }

    /// Transition to idle state with exit code (OSC 133;D)
    pub fn command_finished(&mut self, exit_code: Option<i32>) {
        self.last_exit_code = exit_code;
        self.phase = CommandPhase::Idle;
        self.command_start = None;
    }

    /// Returns the duration since command started executing, if available
    pub fn elapsed(&self) -> Option<std::time::Duration> {
        self.command_start.map(|start| start.elapsed())
    }

    /// Returns true if a command is currently executing
    pub fn is_executing(&self) -> bool {
        self.phase == CommandPhase::Executing
    }
}

/// Phase of command execution in the shell integration state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommandPhase {
    /// At prompt, no command running
    #[default]
    Idle,
    /// OSC 133;A received - prompt displayed
    PromptShown,
    /// OSC 133;B received - user typing command
    CommandInput,
    /// OSC 133;C received - command running
    Executing,
}

/// OSC 9;4 progress indicator state.
///
/// Supports the ConEmu/Windows Terminal progress protocol:
/// - ESC ] 9 ; 4 ; state ; progress ST
///
/// States:
/// - 0 = Clear/remove progress
/// - 1 = Normal progress (green)
/// - 2 = Error state (red)
/// - 3 = Indeterminate/busy (spinning)
/// - 4 = Warning state (yellow)
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgressState {
    /// No progress indicator (state 0)
    #[default]
    Clear,
    /// Normal progress with percentage 0-100 (state 1)
    InProgress(u8),
    /// Error state with percentage (state 2)
    Error(u8),
    /// Indeterminate/busy spinner (state 3)
    Indeterminate,
    /// Warning state with percentage (state 4)
    Warning(u8),
}

impl ProgressState {
    /// Parse OSC 9;4 state and progress values
    pub fn from_osc(state: u8, progress: u8) -> Self {
        let progress = progress.min(100);
        match state {
            0 => Self::Clear,
            1 => Self::InProgress(progress),
            2 => Self::Error(progress),
            3 => Self::Indeterminate,
            4 => Self::Warning(progress),
            _ => Self::Clear,
        }
    }

    /// Returns true if progress is active (not cleared)
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Clear)
    }

    /// Returns the progress percentage if applicable
    pub fn percentage(&self) -> Option<u8> {
        match self {
            Self::InProgress(p) | Self::Error(p) | Self::Warning(p) => Some(*p),
            Self::Clear | Self::Indeterminate => None,
        }
    }

    /// Returns true if this is an error state
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }

    /// Returns true if this is a warning state
    pub fn is_warning(&self) -> bool {
        matches!(self, Self::Warning(_))
    }

    /// Returns true if this is an indeterminate/busy state
    pub fn is_indeterminate(&self) -> bool {
        matches!(self, Self::Indeterminate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lifecycle_transitions() {
        let mut lifecycle = CommandLifecycle::new();
        assert_eq!(lifecycle.phase, CommandPhase::Idle);
        assert!(!lifecycle.is_executing());

        lifecycle.prompt_start();
        assert_eq!(lifecycle.phase, CommandPhase::PromptShown);

        lifecycle.command_start();
        assert_eq!(lifecycle.phase, CommandPhase::CommandInput);

        lifecycle.command_executing();
        assert_eq!(lifecycle.phase, CommandPhase::Executing);
        assert!(lifecycle.is_executing());
        assert!(lifecycle.command_start.is_some());

        lifecycle.command_finished(Some(0));
        assert_eq!(lifecycle.phase, CommandPhase::Idle);
        assert_eq!(lifecycle.last_exit_code, Some(0));
    }

    #[test]
    fn progress_state_from_osc() {
        assert_eq!(ProgressState::from_osc(0, 50), ProgressState::Clear);
        assert_eq!(
            ProgressState::from_osc(1, 50),
            ProgressState::InProgress(50)
        );
        assert_eq!(ProgressState::from_osc(2, 75), ProgressState::Error(75));
        assert_eq!(ProgressState::from_osc(3, 0), ProgressState::Indeterminate);
        assert_eq!(ProgressState::from_osc(4, 25), ProgressState::Warning(25));
        // Invalid state defaults to Clear
        assert_eq!(ProgressState::from_osc(99, 50), ProgressState::Clear);
    }

    #[test]
    fn progress_state_clamps_percentage() {
        assert_eq!(
            ProgressState::from_osc(1, 150),
            ProgressState::InProgress(100)
        );
        assert_eq!(
            ProgressState::from_osc(1, 255),
            ProgressState::InProgress(100)
        );
    }

    #[test]
    fn progress_state_queries() {
        assert!(ProgressState::InProgress(50).is_active());
        assert!(!ProgressState::Clear.is_active());

        assert_eq!(ProgressState::InProgress(50).percentage(), Some(50));
        assert_eq!(ProgressState::Indeterminate.percentage(), None);

        assert!(ProgressState::Error(50).is_error());
        assert!(ProgressState::Warning(50).is_warning());
        assert!(ProgressState::Indeterminate.is_indeterminate());
    }
}
