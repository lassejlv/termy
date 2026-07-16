#![cfg_attr(not(unix), allow(dead_code))]

#[cfg(unix)]
use anyhow::anyhow;
use anyhow::{Context, Result};
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::fd::{FromRawFd, IntoRawFd};
#[cfg(unix)]
use std::process::Command;
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use super::session::normalize_tmux_command_env;
use super::session::{append_socket_args, tmux_base_command};
use tmux_control_core::types::{
    TmuxLaunchTarget, TmuxRuntimeConfig, TmuxShutdownMode, TmuxSocketTarget,
};

pub(crate) const PERSISTENT_SESSION_NAME: &str = "termy";

const MANAGED_SESSION_WINDOW_OPTION_BASE_OVERRIDES: [(&str, &str); 2] =
    [("pane-border-status", "off"), ("pane-border-format", "")];
const MANAGED_SESSION_WINDOW_OPTION_ACTIVE_BORDER_OFF_OVERRIDES: [(&str, &str); 3] = [
    ("pane-border-indicators", "off"),
    ("pane-border-style", "fg=default,bg=default"),
    ("pane-active-border-style", "fg=default,bg=default"),
];

#[derive(Debug, Clone)]
pub(crate) struct SessionLaunchPlan {
    pub(crate) session_name: String,
    pub(crate) socket_target: TmuxSocketTarget,
    pub(crate) attach_existing: bool,
    pub(crate) shutdown_mode_on_drop: TmuxShutdownMode,
}

pub(crate) fn launch_plan(config: &TmuxRuntimeConfig) -> SessionLaunchPlan {
    match &config.launch {
        TmuxLaunchTarget::Managed { persistence } => {
            if *persistence {
                SessionLaunchPlan {
                    session_name: PERSISTENT_SESSION_NAME.to_string(),
                    socket_target: TmuxSocketTarget::DedicatedTermy,
                    attach_existing: true,
                    shutdown_mode_on_drop: TmuxShutdownMode::DetachOnly,
                }
            } else {
                SessionLaunchPlan {
                    session_name: managed_session_name(),
                    socket_target: TmuxSocketTarget::DedicatedTermy,
                    attach_existing: false,
                    shutdown_mode_on_drop: TmuxShutdownMode::DetachAndTeardownSession,
                }
            }
        }
        TmuxLaunchTarget::Session { name, socket } => SessionLaunchPlan {
            session_name: name.trim().to_string(),
            socket_target: socket.clone(),
            attach_existing: true,
            shutdown_mode_on_drop: TmuxShutdownMode::DetachOnly,
        },
    }
}

#[cfg(unix)]
pub(crate) fn spawn_tmux_control_mode(
    config: &TmuxRuntimeConfig,
    socket_target: &TmuxSocketTarget,
    session_name: &str,
    attach_existing: bool,
    working_dir: Option<&str>,
) -> Result<(std::process::Child, File, File)> {
    let pty = rustix_openpty::openpty(None, None)
        .map_err(|error| anyhow!("failed to allocate tmux control pty: {error}"))?;

    let controller = unsafe { File::from_raw_fd(pty.controller.into_raw_fd()) };
    let user = unsafe { File::from_raw_fd(pty.user.into_raw_fd()) };

    let child_stdin = user
        .try_clone()
        .context("failed to clone tmux pty slave for stdin")?;
    let child_stdout = user
        .try_clone()
        .context("failed to clone tmux pty slave for stdout")?;
    let child_stderr = user;

    let mut command = Command::new(config.binary.as_str());
    normalize_tmux_command_env(&mut command);
    append_socket_args(&mut command, socket_target);
    // New tmux sessions must not inherit the app process cwd (for example `/` from Finder).
    command.args(new_session_args(session_name, attach_existing, working_dir));
    let child = command
        // tmux windows/panes are authoritative in tmux runtime mode; disable
        // direct shell OSC integration hooks to avoid prompt-width drift artifacts.
        .env("TERMY_SHELL_INTEGRATION", "0")
        .env_remove("TERMY_TAB_TITLE_PREFIX")
        // Avoid inheriting an outer tmux client context; nested `TMUX` can
        // redirect control-mode startup away from the requested session/socket.
        .env_remove("TMUX")
        // zsh can emit inverse PROMPT_EOL_MARK (%) when line-state and repaint diverge.
        // Disable it for tmux-managed shells to avoid persistent visual artifacts.
        .env("PROMPT_EOL_MARK", "")
        .stdin(Stdio::from(child_stdin))
        .stdout(Stdio::from(child_stdout))
        .stderr(Stdio::from(child_stderr))
        .spawn()
        .with_context(|| {
            format!(
                "failed to spawn tmux control mode using '{}'",
                config.binary
            )
        })?;

    let writer = controller
        .try_clone()
        .context("failed to clone tmux pty controller for writer")?;

    Ok((child, writer, controller))
}

/// Spawn the tmux control-mode client through the configured command prefix
/// (`wsl.exe -e tmux ...`, `ssh myhost tmux ...`) with plain piped stdio.
///
/// Control-mode clients do not need a pty, so this path works on every
/// platform — including Windows, where the local pty spawn path is
/// unavailable. The working directory is intentionally not forwarded: the
/// filesystem behind the prefix (WSL distro, remote host) does not share
/// paths with the host process.
pub(crate) fn spawn_tmux_control_mode_via_prefix(
    config: &TmuxRuntimeConfig,
    socket_target: &TmuxSocketTarget,
    session_name: &str,
    attach_existing: bool,
) -> Result<(
    std::process::Child,
    std::process::ChildStdin,
    std::process::ChildStdout,
)> {
    let mut command = tmux_base_command(&config.command_prefix, config.binary.as_str());
    append_socket_args(&mut command, socket_target);
    command.args(new_session_args(session_name, attach_existing, None));
    let mut child = command
        .env("TERMY_SHELL_INTEGRATION", "0")
        .env_remove("TERMY_TAB_TITLE_PREFIX")
        // Avoid inheriting an outer tmux client context; nested `TMUX` can
        // redirect control-mode startup away from the requested session/socket.
        .env_remove("TMUX")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Keep stderr attached to the app so prefix failures (ssh auth, WSL
        // startup) stay visible in logs; the control protocol itself only
        // uses stdout.
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| {
            format!(
                "failed to spawn tmux control mode via '{}'",
                config.command_prefix.join(" ")
            )
        })?;

    let child_stdin = child
        .stdin
        .take()
        .context("tmux control mode child is missing a piped stdin")?;
    let child_stdout = child
        .stdout
        .take()
        .context("tmux control mode child is missing a piped stdout")?;
    Ok((child, child_stdin, child_stdout))
}

pub(crate) fn append_working_dir_args<'a>(args: &mut Vec<&'a str>, working_dir: Option<&'a str>) {
    let working_dir = working_dir
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| is_safe_tmux_working_dir_arg(value));
    if let Some(working_dir) = working_dir {
        args.push("-c");
        args.push(working_dir);
    }
}

fn is_safe_tmux_working_dir_arg(value: &str) -> bool {
    !value
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte == b'#')
}

fn new_session_args<'a>(
    session_name: &'a str,
    attach_existing: bool,
    working_dir: Option<&'a str>,
) -> Vec<&'a str> {
    let mut args = vec!["-CC", "new-session"];
    if attach_existing {
        args.push("-A");
    }
    append_working_dir_args(&mut args, working_dir);
    args.push("-s");
    args.push(session_name);
    args
}

pub(crate) fn managed_session_window_option_overrides(
    show_active_pane_border: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut overrides = MANAGED_SESSION_WINDOW_OPTION_BASE_OVERRIDES.to_vec();
    if !show_active_pane_border {
        overrides.extend_from_slice(&MANAGED_SESSION_WINDOW_OPTION_ACTIVE_BORDER_OFF_OVERRIDES);
    }
    overrides
}

pub(crate) fn managed_session_window_option_override_commands<'a>(
    all_windows_target: &'a str,
    show_active_pane_border: bool,
) -> impl Iterator<Item = [&'a str; 6]> + 'a {
    managed_session_window_option_overrides(show_active_pane_border)
        .into_iter()
        .map(move |(option, value)| {
            [
                "set-window-option",
                "-q",
                "-t",
                all_windows_target,
                option,
                value,
            ]
        })
}

pub(crate) fn managed_session_name() -> String {
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("termy-{}-{}", std::process::id(), now_ns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_launch_plan_reuses_fixed_session_without_teardown() {
        let plan = launch_plan(&TmuxRuntimeConfig {
            binary: "tmux".to_string(),
            command_prefix: Vec::new(),
            launch: TmuxLaunchTarget::Managed { persistence: true },
            show_active_pane_border: false,
        });
        assert_eq!(plan.session_name, PERSISTENT_SESSION_NAME);
        assert_eq!(plan.socket_target, TmuxSocketTarget::DedicatedTermy);
        assert!(plan.attach_existing);
        assert_eq!(plan.shutdown_mode_on_drop, TmuxShutdownMode::DetachOnly);
    }

    #[test]
    fn isolated_launch_plan_uses_fresh_session_and_teardown() {
        let plan = launch_plan(&TmuxRuntimeConfig {
            binary: "tmux".to_string(),
            command_prefix: Vec::new(),
            launch: TmuxLaunchTarget::Managed { persistence: false },
            show_active_pane_border: false,
        });
        assert!(plan.session_name.starts_with("termy-"));
        assert_eq!(plan.socket_target, TmuxSocketTarget::DedicatedTermy);
        assert!(!plan.attach_existing);
        assert_eq!(
            plan.shutdown_mode_on_drop,
            TmuxShutdownMode::DetachAndTeardownSession
        );
    }

    #[test]
    fn explicit_session_launch_plan_uses_requested_target_without_teardown() {
        let plan = launch_plan(&TmuxRuntimeConfig {
            binary: "tmux".to_string(),
            command_prefix: Vec::new(),
            launch: TmuxLaunchTarget::Session {
                name: "work".to_string(),
                socket: TmuxSocketTarget::Named("work".to_string()),
            },
            show_active_pane_border: false,
        });
        assert_eq!(plan.session_name, "work");
        assert_eq!(
            plan.socket_target,
            TmuxSocketTarget::Named("work".to_string())
        );
        assert!(plan.attach_existing);
        assert_eq!(plan.shutdown_mode_on_drop, TmuxShutdownMode::DetachOnly);
    }

    #[test]
    fn managed_session_name_prefix_is_stable() {
        let name = managed_session_name();
        assert!(name.starts_with("termy-"));
    }

    #[test]
    fn managed_session_window_option_overrides_include_active_border_neutralization() {
        let overrides = managed_session_window_option_overrides(false);
        assert!(overrides.contains(&("pane-border-status", "off")));
        assert!(overrides.contains(&("pane-border-format", "")));
        assert!(overrides.contains(&("pane-border-indicators", "off")));
        assert!(overrides.contains(&("pane-border-style", "fg=default,bg=default")));
        assert!(overrides.contains(&("pane-active-border-style", "fg=default,bg=default")));
    }

    #[test]
    fn managed_session_window_option_override_commands_include_expected_target_and_flags() {
        let target = "termy:*";
        let commands = managed_session_window_option_override_commands(target, false)
            .collect::<Vec<[&str; 6]>>();
        assert!(commands.contains(&[
            "set-window-option",
            "-q",
            "-t",
            target,
            "pane-border-status",
            "off",
        ]));
        assert!(commands.contains(&[
            "set-window-option",
            "-q",
            "-t",
            target,
            "pane-border-format",
            "",
        ]));
        assert!(commands.contains(&[
            "set-window-option",
            "-q",
            "-t",
            target,
            "pane-border-indicators",
            "off",
        ]));
        assert!(commands.contains(&[
            "set-window-option",
            "-q",
            "-t",
            target,
            "pane-border-style",
            "fg=default,bg=default",
        ]));
        assert!(commands.contains(&[
            "set-window-option",
            "-q",
            "-t",
            target,
            "pane-active-border-style",
            "fg=default,bg=default",
        ]));
    }

    #[test]
    fn managed_session_window_option_overrides_skip_active_border_neutralization_when_enabled() {
        let overrides = managed_session_window_option_overrides(true);
        assert!(overrides.contains(&("pane-border-status", "off")));
        assert!(overrides.contains(&("pane-border-format", "")));
        assert!(!overrides.contains(&("pane-border-indicators", "off")));
        assert!(!overrides.contains(&("pane-border-style", "fg=default,bg=default")));
        assert!(!overrides.contains(&("pane-active-border-style", "fg=default,bg=default")));
    }

    #[test]
    fn managed_session_window_option_override_commands_skip_active_border_neutralization_when_enabled()
     {
        let target = "termy:*";
        let commands = managed_session_window_option_override_commands(target, true)
            .collect::<Vec<[&str; 6]>>();
        assert!(commands.contains(&[
            "set-window-option",
            "-q",
            "-t",
            target,
            "pane-border-status",
            "off",
        ]));
        assert!(commands.contains(&[
            "set-window-option",
            "-q",
            "-t",
            target,
            "pane-border-format",
            "",
        ]));
        assert!(!commands.contains(&[
            "set-window-option",
            "-q",
            "-t",
            target,
            "pane-border-indicators",
            "off",
        ]));
        assert!(!commands.contains(&[
            "set-window-option",
            "-q",
            "-t",
            target,
            "pane-border-style",
            "fg=default,bg=default",
        ]));
        assert!(!commands.contains(&[
            "set-window-option",
            "-q",
            "-t",
            target,
            "pane-active-border-style",
            "fg=default,bg=default",
        ]));
    }

    #[test]
    fn new_session_args_include_working_directory_when_provided() {
        assert_eq!(
            new_session_args("work", true, Some("/tmp/project")),
            vec![
                "-CC",
                "new-session",
                "-A",
                "-c",
                "/tmp/project",
                "-s",
                "work"
            ]
        );
    }

    #[test]
    fn new_session_args_omit_working_directory_when_missing() {
        assert_eq!(
            new_session_args("work", false, Some("  ")),
            vec!["-CC", "new-session", "-s", "work"]
        );
    }

    #[test]
    fn new_session_args_omit_unsafe_working_directories() {
        assert_eq!(
            new_session_args("work", false, Some("/tmp/project\nrun-shell")),
            vec!["-CC", "new-session", "-s", "work"]
        );
        assert_eq!(
            new_session_args("work", false, Some("/tmp/#(run-shell)")),
            vec!["-CC", "new-session", "-s", "work"]
        );
    }
}
