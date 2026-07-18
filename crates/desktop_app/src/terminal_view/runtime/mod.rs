use std::time::Instant;

use flume::Sender;
use termy_terminal_ui::{TmuxClient, TmuxLaunchTarget, TmuxRuntimeConfig, TmuxSnapshot};

use super::*;

mod tmux;
mod tmux_sync;

pub(super) use tmux_sync::{TmuxResizeScheduler, TmuxResizeWakeup};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RuntimeKind {
    Native,
    Tmux,
}

fn tmux_startup_fallback_message(reason: &str, error: &anyhow::Error) -> String {
    format!("tmux is unavailable ({reason}: {error:#}); starting in native mode")
}

impl RuntimeKind {
    pub(super) const fn uses_tmux(self) -> bool {
        matches!(self, Self::Tmux)
    }

    pub(super) fn from_app_config(config: &AppConfig) -> Self {
        Self::from_runtime_options(
            config.tmux_enabled,
            cfg!(target_os = "windows"),
            config.tmux_command_prefix_argv().is_empty(),
        )
    }

    fn from_runtime_options(
        tmux_enabled: bool,
        is_windows: bool,
        command_prefix_is_empty: bool,
    ) -> Self {
        if !tmux_enabled {
            return Self::Native;
        }
        // Windows has no local pty spawn path for tmux; the runtime is only
        // reachable through a configured command prefix (e.g. `wsl.exe -e`).
        if is_windows && command_prefix_is_empty {
            return Self::Native;
        }
        Self::Tmux
    }
}

#[allow(clippy::large_enum_variant)]
pub(super) enum RuntimeState {
    Native,
    Tmux(TmuxRuntime),
}

impl RuntimeState {
    pub(super) const fn kind(&self) -> RuntimeKind {
        match self {
            Self::Native => RuntimeKind::Native,
            Self::Tmux(_) => RuntimeKind::Tmux,
        }
    }

    pub(super) fn as_tmux(&self) -> Option<&TmuxRuntime> {
        match self {
            Self::Native => None,
            Self::Tmux(runtime) => Some(runtime),
        }
    }

    pub(super) fn as_tmux_mut(&mut self) -> Option<&mut TmuxRuntime> {
        match self {
            Self::Native => None,
            Self::Tmux(runtime) => Some(runtime),
        }
    }
}

pub(super) struct TmuxRuntime {
    pub(super) config: TmuxRuntimeConfig,
    pub(super) client: TmuxClient,
    pub(super) preferred_cwd: Option<String>,
    pub(super) client_cols: u16,
    pub(super) client_rows: u16,
    pub(super) resize_scheduler: TmuxResizeScheduler,
    pub(super) resize_wakeup_scheduled: bool,
    pub(super) title_refresh_deadline: Option<Instant>,
    pub(super) title_refresh_wakeup_scheduled: bool,
}

impl TmuxRuntime {
    pub(super) fn new(
        config: TmuxRuntimeConfig,
        client: TmuxClient,
        preferred_cwd: Option<String>,
        cols: u16,
        rows: u16,
    ) -> Self {
        Self {
            config,
            client,
            preferred_cwd,
            client_cols: cols,
            client_rows: rows,
            resize_scheduler: TmuxResizeScheduler::default(),
            resize_wakeup_scheduled: false,
            title_refresh_deadline: None,
            title_refresh_wakeup_scheduled: false,
        }
    }
}

impl TerminalView {
    #[cfg(any(not(target_os = "windows"), test))]
    pub(super) fn runtime_kind_from_app_config(config: &AppConfig) -> RuntimeKind {
        RuntimeKind::from_app_config(config)
    }

    pub(super) fn tmux_runtime_from_app_config(config: &AppConfig) -> TmuxRuntimeConfig {
        TmuxRuntimeConfig {
            binary: config.tmux_binary.trim().to_string(),
            command_prefix: config.tmux_command_prefix_argv(),
            launch: TmuxLaunchTarget::Managed {
                persistence: config.tmux_persistence,
            },
            show_active_pane_border: config.tmux_show_active_pane_border,
        }
    }

    pub(super) fn runtime_startup_from_app_config(
        config: &AppConfig,
        event_wakeup_tx: &Sender<()>,
        native_terminal_wakeup_router: &NativeTerminalWakeupRouter,
        configured_working_dir: Option<&str>,
        tab_shell_integration: &TabTitleShellIntegration,
        terminal_runtime: &TerminalRuntimeConfig,
        startup_command: Option<&str>,
        initial_cols: u16,
        initial_rows: u16,
    ) -> (RuntimeState, Option<TmuxSnapshot>, Option<Terminal>) {
        let start_native = || {
            let native_terminal = match Terminal::new_native(
                TerminalSize {
                    cols: initial_cols,
                    rows: initial_rows,
                    ..TerminalSize::default()
                },
                configured_working_dir,
                Some(native_terminal_wakeup_router),
                Some(tab_shell_integration),
                Some(terminal_runtime),
                startup_command,
            ) {
                Ok(terminal) => terminal,
                Err(error) => {
                    eprintln!("Termy startup blocked: failed to start native runtime: {error}");
                    std::process::exit(1);
                }
            };
            (RuntimeState::Native, None, Some(native_terminal))
        };

        match RuntimeKind::from_app_config(config) {
            RuntimeKind::Tmux => {
                let tmux_runtime = Self::tmux_runtime_from_app_config(config);
                let initial_working_dir = termy_terminal_ui::resolve_launch_working_directory(
                    configured_working_dir,
                    terminal_runtime.working_dir_fallback,
                )
                .map(|path| path.to_string_lossy().into_owned());
                let tmux_client = match TmuxClient::new(
                    tmux_runtime.clone(),
                    initial_cols,
                    initial_rows,
                    initial_working_dir.as_deref(),
                    Some(event_wakeup_tx.clone()),
                ) {
                    Ok(client) => client,
                    Err(error) => {
                        let message = tmux_startup_fallback_message(
                            "failed to start tmux control runtime",
                            &error,
                        );
                        log::warn!("{message}");
                        termy_toast::warning(message);
                        return start_native();
                    }
                };
                let initial_snapshot = match tmux_client.refresh_snapshot() {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        if let Err(cleanup_error) = tmux_client.shutdown_default() {
                            eprintln!(
                                "Termy startup warning: failed to cleanup tmux client after \
                                 snapshot startup failure: {cleanup_error}"
                            );
                        }
                        let message = tmux_startup_fallback_message(
                            "failed to fetch initial tmux snapshot",
                            &error,
                        );
                        log::warn!("{message}");
                        termy_toast::warning(message);
                        return start_native();
                    }
                };
                (
                    RuntimeState::Tmux(TmuxRuntime::new(
                        tmux_runtime,
                        tmux_client,
                        initial_working_dir,
                        initial_cols,
                        initial_rows,
                    )),
                    Some(initial_snapshot),
                    None,
                )
            }
            RuntimeKind::Native => start_native(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_runtime_gate_requires_command_prefix_on_windows() {
        assert_eq!(
            RuntimeKind::from_runtime_options(false, false, true),
            RuntimeKind::Native
        );
        assert_eq!(
            RuntimeKind::from_runtime_options(true, false, true),
            RuntimeKind::Tmux
        );
        assert_eq!(
            RuntimeKind::from_runtime_options(true, true, true),
            RuntimeKind::Native
        );
        assert_eq!(
            RuntimeKind::from_runtime_options(true, true, false),
            RuntimeKind::Tmux
        );
    }

    #[test]
    fn tmux_startup_fallback_message_preserves_error_and_recovery() {
        let error = anyhow::anyhow!("tmux executable was not found");
        let message = tmux_startup_fallback_message("failed to start tmux", &error);
        assert!(message.contains("tmux executable was not found"));
        assert!(message.contains("starting in native mode"));
    }
}
