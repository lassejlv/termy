use crate::command_core::CommandId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandCapabilities {
    pub tmux_runtime_active: bool,
    pub install_cli_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandUnavailableReason {
    RequiresTmuxRuntime,
    InstallCliAlreadyInstalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandAvailability {
    pub enabled: bool,
    pub reason: Option<CommandUnavailableReason>,
}

impl CommandId {
    pub const fn availability(self, caps: CommandCapabilities) -> CommandAvailability {
        if self.is_tmux_only() && !caps.tmux_runtime_active {
            return CommandAvailability {
                enabled: false,
                reason: Some(CommandUnavailableReason::RequiresTmuxRuntime),
            };
        }

        if matches!(self, Self::InstallCli) && !caps.install_cli_available {
            return CommandAvailability {
                enabled: false,
                reason: Some(CommandUnavailableReason::InstallCliAlreadyInstalled),
            };
        }

        CommandAvailability {
            enabled: true,
            reason: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandCapabilities, CommandUnavailableReason};
    use crate::command_core::CommandId;

    #[test]
    fn resize_commands_are_available_without_tmux_runtime() {
        let caps = CommandCapabilities {
            tmux_runtime_active: false,
            install_cli_available: true,
        };
        let availability = CommandId::ResizePaneLeft.availability(caps);
        assert!(availability.enabled);
        assert_eq!(availability.reason, None);
    }

    #[test]
    fn command_availability_reports_install_cli_when_already_installed() {
        let caps = CommandCapabilities {
            tmux_runtime_active: true,
            install_cli_available: false,
        };
        let availability = CommandId::InstallCli.availability(caps);
        assert!(!availability.enabled);
        assert_eq!(
            availability.reason,
            Some(CommandUnavailableReason::InstallCliAlreadyInstalled)
        );
    }
}
