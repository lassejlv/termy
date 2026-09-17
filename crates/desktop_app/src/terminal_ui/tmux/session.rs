#![cfg_attr(not(unix), allow(unused_variables))]

use anyhow::{Context, Result, anyhow};
#[cfg(unix)]
use std::env;
use std::process::Command;
#[cfg(unix)]
use termy_core::{
    Utf8LocaleOverridePlan, normalized_path_env, preferred_utf8_locale, utf8_locale_override_plan,
};

use super::snapshot::{parse_session_summaries, session_snapshot_format};
#[cfg(test)]
use termy_core::tmux_control_core::command::quote_tmux_arg;
use termy_core::tmux_control_core::command::tmux_command_line;
use termy_core::tmux_control_core::types::{TmuxSessionSummary, TmuxSocketTarget};

pub(crate) fn append_socket_args(command: &mut Command, socket_target: &TmuxSocketTarget) {
    if let Some(socket_name) = socket_target.socket_name() {
        command.arg("-L").arg(socket_name);
    }
}

pub(crate) fn normalize_tmux_command_env(command: &mut Command) {
    #[cfg(unix)]
    {
        // Finder/DMG launches on macOS use a minimal process environment.
        // Normalize PATH + locale for tmux subprocesses so startup behavior
        // matches terminal-launched runs.
        if let Some(path) = normalized_path_env(env::var_os("PATH").as_deref()) {
            command.env("PATH", path);
        }

        let lc_all = env::var("LC_ALL").ok();
        let lc_ctype = env::var("LC_CTYPE").ok();
        let lang = env::var("LANG").ok();
        let target_utf8_locale =
            preferred_utf8_locale(lc_all.as_deref(), lc_ctype.as_deref(), lang.as_deref());
        match utf8_locale_override_plan(lc_all.as_deref(), lc_ctype.as_deref(), lang.as_deref()) {
            Utf8LocaleOverridePlan::None => {}
            Utf8LocaleOverridePlan::LcCtypeOnly => {
                command.env("LC_CTYPE", &target_utf8_locale);
            }
            Utf8LocaleOverridePlan::LcAllAndLcCtype => {
                command.env("LC_ALL", &target_utf8_locale);
                command.env("LC_CTYPE", &target_utf8_locale);
            }
        }
    }
}

/// Build the base tmux `Command`, routing through the configured command
/// prefix when present (`["wsl.exe", "-e"]` yields `wsl.exe -e <binary> ...`).
pub(crate) fn tmux_base_command(command_prefix: &[String], binary: &str) -> Command {
    let mut command = match command_prefix.split_first() {
        Some((program, rest)) => {
            let mut command = Command::new(program);
            command.args(rest);
            command.arg(binary);
            command
        }
        None => Command::new(binary),
    };
    normalize_tmux_command_env(&mut command);
    command
}

pub(crate) fn run_tmux_command_with_socket(
    command_prefix: &[String],
    binary: &str,
    socket_target: &TmuxSocketTarget,
    args: &[&str],
) -> Result<std::process::Output> {
    let mut command = tmux_base_command(command_prefix, binary);
    append_socket_args(&mut command, socket_target);
    let output = command.args(args).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() {
            return Err(anyhow!(
                "tmux command exited with status {}",
                output
                    .status
                    .code()
                    .map_or_else(|| "signal".to_string(), |code| code.to_string())
            ));
        }
        return Err(anyhow!("{stderr}"));
    }

    Ok(output)
}

pub(crate) fn verify_tmux_version(
    command_prefix: &[String],
    binary: &str,
    minimum_major: u8,
    minimum_minor: u8,
) -> Result<()> {
    let mut command = tmux_base_command(command_prefix, binary);
    let output = command
        .arg("-V")
        .output()
        .with_context(|| format!("failed to execute '{binary}' -V"))?;
    if !output.status.success() {
        return Err(anyhow!("'{binary} -V' failed"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = stdout
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("unable to parse tmux version output: '{}'", stdout.trim()))?;

    let (major, minor) = parse_version_prefix(version)
        .ok_or_else(|| anyhow!("unsupported tmux version format: '{version}'"))?;
    if (major, minor) < (minimum_major, minimum_minor) {
        return Err(anyhow!(
            "tmux {minimum_major}.{minimum_minor}+ required, found {version}"
        ));
    }

    Ok(())
}

pub(crate) fn list_sessions(
    command_prefix: &[String],
    binary: &str,
    socket_target: TmuxSocketTarget,
) -> Result<Vec<TmuxSessionSummary>> {
    let format = session_snapshot_format();
    let output = run_tmux_command_with_socket(
        command_prefix,
        binary,
        &socket_target,
        &["list-sessions", "-F", format],
    )
    .with_context(|| {
        format!(
            "tmux session listing failed: {}",
            tmux_command_line(&["list-sessions", "-F", format])
        )
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_session_summaries(stdout.as_ref())
}

pub(crate) fn rename_session(
    command_prefix: &[String],
    binary: &str,
    socket_target: TmuxSocketTarget,
    current_session_name: &str,
    next_session_name: &str,
) -> Result<()> {
    let current_session_name = current_session_name.trim();
    if current_session_name.is_empty() {
        return Err(anyhow!("tmux current session name cannot be empty"));
    }

    let next_session_name = next_session_name.trim();
    if next_session_name.is_empty() {
        return Err(anyhow!("tmux new session name cannot be empty"));
    }

    run_tmux_command_with_socket(
        command_prefix,
        binary,
        &socket_target,
        &[
            "rename-session",
            "-t",
            current_session_name,
            next_session_name,
        ],
    )
    .with_context(|| {
        format!(
            "tmux session rename failed: {}",
            tmux_command_line(&[
                "rename-session",
                "-t",
                current_session_name,
                next_session_name
            ])
        )
    })?;
    Ok(())
}

pub(crate) fn kill_session(
    command_prefix: &[String],
    binary: &str,
    socket_target: TmuxSocketTarget,
    session_name: &str,
) -> Result<()> {
    let session_name = session_name.trim();
    if session_name.is_empty() {
        return Err(anyhow!("tmux session name cannot be empty"));
    }

    run_tmux_command_with_socket(
        command_prefix,
        binary,
        &socket_target,
        &["kill-session", "-t", session_name],
    )
    .with_context(|| {
        format!(
            "tmux session kill failed: {}",
            tmux_command_line(&["kill-session", "-t", session_name])
        )
    })?;
    Ok(())
}

pub(crate) fn parse_version_prefix(version: &str) -> Option<(u8, u8)> {
    let mut digits = String::new();
    for ch in version.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            digits.push(ch);
        } else {
            break;
        }
    }

    let mut parts = digits.split('.');
    let major = parts.next()?.parse::<u8>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u8>().ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::path::PathBuf;

    #[test]
    fn rename_session_rejects_empty_session_names() {
        let empty_current = rename_session(&[], "tmux", TmuxSocketTarget::Default, " ", "next")
            .expect_err("expected empty current session name failure");
        assert!(
            empty_current
                .to_string()
                .contains("tmux current session name cannot be empty")
        );

        let empty_next = rename_session(&[], "tmux", TmuxSocketTarget::Default, "current", " ")
            .expect_err("expected empty next session name failure");
        assert!(
            empty_next
                .to_string()
                .contains("tmux new session name cannot be empty")
        );
    }

    #[test]
    fn kill_session_rejects_empty_session_name() {
        let error = kill_session(&[], "tmux", TmuxSocketTarget::Default, " ")
            .expect_err("expected empty session name failure");
        assert!(
            error
                .to_string()
                .contains("tmux session name cannot be empty")
        );
    }

    #[test]
    fn tmux_base_command_without_prefix_runs_binary_directly() {
        let command = tmux_base_command(&[], "/usr/bin/tmux");
        assert_eq!(command.get_program().to_string_lossy(), "/usr/bin/tmux");
        assert_eq!(command.get_args().count(), 0);
    }

    #[test]
    fn tmux_base_command_with_prefix_wraps_binary() {
        let prefix = vec!["wsl.exe".to_string(), "-e".to_string()];
        let command = tmux_base_command(&prefix, "tmux");
        assert_eq!(command.get_program().to_string_lossy(), "wsl.exe");
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(args, vec!["-e", "tmux"]);
    }

    #[test]
    fn tmux_version_parser_accepts_patch_suffixes() {
        assert_eq!(parse_version_prefix("3.6a"), Some((3, 6)));
        assert_eq!(parse_version_prefix("3.3"), Some((3, 3)));
        assert_eq!(parse_version_prefix("2"), Some((2, 0)));
    }

    #[test]
    fn quote_tmux_arg_preserves_safe_values_without_quotes() {
        assert_eq!(quote_tmux_arg("-L"), "-L");
        assert_eq!(
            quote_tmux_arg("name_with-safe.chars"),
            "name_with-safe.chars"
        );
    }

    #[test]
    fn append_socket_args_omits_default_socket() {
        let mut command = Command::new("tmux");
        append_socket_args(&mut command, &TmuxSocketTarget::Default);
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(args.is_empty());
    }

    #[test]
    fn append_socket_args_adds_named_and_dedicated_sockets() {
        let mut dedicated = Command::new("tmux");
        append_socket_args(&mut dedicated, &TmuxSocketTarget::DedicatedTermy);
        let dedicated_args = dedicated
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(dedicated_args, vec!["-L", "termy"]);

        let mut named = Command::new("tmux");
        append_socket_args(&mut named, &TmuxSocketTarget::Named("work".to_string()));
        let named_args = named
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(named_args, vec!["-L", "work"]);
    }

    #[cfg(unix)]
    #[test]
    fn normalized_tmux_path_starts_from_default_system_path_when_missing() {
        let path = normalized_path_env(None).expect("normalized path");
        let parsed = std::env::split_paths(&OsString::from(path)).collect::<Vec<_>>();
        assert!(parsed.contains(&PathBuf::from("/usr/bin")));
        assert!(parsed.contains(&PathBuf::from("/bin")));
        assert!(parsed.contains(&PathBuf::from("/usr/sbin")));
        assert!(parsed.contains(&PathBuf::from("/sbin")));
        assert!(parsed.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(parsed.contains(&PathBuf::from("/opt/homebrew/sbin")));
        assert!(parsed.contains(&PathBuf::from("/usr/local/bin")));
        assert!(parsed.contains(&PathBuf::from("/usr/local/sbin")));
    }

    #[cfg(unix)]
    #[test]
    fn normalized_tmux_path_treats_empty_path_as_missing() {
        let raw = OsString::from("");
        let path = normalized_path_env(Some(raw.as_os_str())).expect("normalized path");
        let parsed = std::env::split_paths(&OsString::from(path)).collect::<Vec<_>>();
        assert!(parsed.contains(&PathBuf::from("/usr/bin")));
        assert!(parsed.contains(&PathBuf::from("/bin")));
        assert!(parsed.contains(&PathBuf::from("/usr/sbin")));
        assert!(parsed.contains(&PathBuf::from("/sbin")));
    }

    #[cfg(unix)]
    #[test]
    fn normalized_tmux_path_appends_missing_entries_without_duplication() {
        let raw = OsString::from("/opt/homebrew/bin:/usr/bin:/bin");
        let path = normalized_path_env(Some(raw.as_os_str())).expect("normalized path");
        let parsed = std::env::split_paths(&OsString::from(path)).collect::<Vec<_>>();
        let homebrew_bin = PathBuf::from("/opt/homebrew/bin");
        assert_eq!(
            parsed
                .iter()
                .filter(|entry| **entry == homebrew_bin)
                .count(),
            1
        );
        assert!(parsed.contains(&PathBuf::from("/opt/homebrew/sbin")));
        assert!(parsed.contains(&PathBuf::from("/usr/local/bin")));
        assert!(parsed.contains(&PathBuf::from("/usr/local/sbin")));
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_respects_lc_all_precedence() {
        assert_eq!(
            utf8_locale_override_plan(Some("C"), Some("en_US.UTF-8"), Some("en_US.UTF-8")),
            Utf8LocaleOverridePlan::LcAllAndLcCtype
        );
        assert_eq!(
            utf8_locale_override_plan(Some("en_US.UTF-8"), Some("C"), Some("C")),
            Utf8LocaleOverridePlan::None
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_uses_lc_ctype_or_lang_when_lc_all_missing() {
        assert_eq!(
            utf8_locale_override_plan(None, Some("C"), Some("en_US.UTF-8")),
            Utf8LocaleOverridePlan::LcCtypeOnly
        );
        assert_eq!(
            utf8_locale_override_plan(None, Some("en_US.UTF-8"), None),
            Utf8LocaleOverridePlan::None
        );
        assert_eq!(
            utf8_locale_override_plan(None, None, Some("en_US.UTF-8")),
            Utf8LocaleOverridePlan::None
        );
    }

    #[cfg(unix)]
    #[test]
    fn preferred_utf8_locale_preserves_modifier_and_converts_encoding() {
        assert_eq!(
            preferred_utf8_locale(None, Some("en_US.ISO8859-1@euro"), None),
            "en_US.UTF-8@euro"
        );
    }
}
