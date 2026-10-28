//! Windows shell selection, startup-command wrapping and prompt integration.

use super::{ChildEnvironment, ResolvedTerminalLaunch, TabTitleShellIntegration, WindowsShell};
use crate::path_env::normalized_path_env;
use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
};

/// Inline prompt wrapper passed to `-Command`. Inline commands are not
/// subject to the script execution policy, unlike a dot-sourced file.
const POWERSHELL_INTEGRATION_SCRIPT: &str = include_str!("../../../../assets/shell/termy.ps1");

/// cmd.exe has no prompt hook, so integration is a default `PROMPT`: OSC 133
/// prompt start, the working directory as OSC 9;9, the usual `$P$G`, then
/// OSC 133 command-input start.
const CMD_INTEGRATION_PROMPT: &str = "$e]133;A$e\\$e]9;9;$P$e\\$P$G$e]133;B$e\\";

/// How a shell takes a command string, judged by its executable name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WindowsShellFamily {
    Cmd,
    PowerShell,
    Posix,
    Other,
}

pub(super) fn windows_shell_family(program: &str) -> WindowsShellFamily {
    let name = Path::new(program.trim())
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Win32 ignores trailing spaces and periods in file names.
    let name = name.trim_end_matches([' ', '.']).to_ascii_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    match stem {
        "cmd" => WindowsShellFamily::Cmd,
        "pwsh" | "pwsh-preview" | "powershell" => WindowsShellFamily::PowerShell,
        "bash" | "sh" | "zsh" | "fish" | "dash" | "ksh" | "mksh" => WindowsShellFamily::Posix,
        _ => WindowsShellFamily::Other,
    }
}

#[cfg(test)]
pub(super) fn windows_cmd_path() -> String {
    windows_cmd_path_in(ChildEnvironment::Process)
}

/// The command processor named by `COMSPEC` in the child's environment.
/// That and PATH come from the child's environment; the system folders are
/// the same for every process on the machine.
fn windows_cmd_path_in(source: ChildEnvironment) -> String {
    if let Some(comspec) = source
        .var_os("COMSPEC")
        .and_then(|comspec| comspec.into_string().ok())
        && !comspec.trim().is_empty()
    {
        return comspec;
    }
    "C:\\Windows\\System32\\cmd.exe".to_string()
}

fn system_root() -> PathBuf {
    env::var_os("SystemRoot")
        .or_else(|| env::var_os("WINDIR"))
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
}

/// Whether `path` is a regular file, or an app execution alias (the
/// `WindowsApps` reparse points behind Store installs such as `pwsh.exe`),
/// which `metadata` cannot follow but `CreateProcessW` starts.
fn is_launchable_file(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(metadata) => metadata.is_file(),
        Err(_) => std::fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.is_dir()),
    }
}

/// Every `file_name` on the PATH a child would see (the process PATH plus
/// the registry's, which picks up installs made after Termy started).
fn find_all_on_path(source: ChildEnvironment, file_name: &str) -> Vec<PathBuf> {
    let Some(path) = normalized_path_env(
        source
            .var_os("PATH")
            .or_else(|| source.var_os("Path"))
            .as_deref(),
    ) else {
        return Vec::new();
    };
    env::split_paths(&path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(file_name))
        .filter(|candidate| is_launchable_file(candidate))
        .collect()
}

fn find_on_path(source: ChildEnvironment, file_name: &str) -> Option<PathBuf> {
    find_all_on_path(source, file_name).into_iter().next()
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn windows_pwsh_path(source: ChildEnvironment) -> Option<PathBuf> {
    find_on_path(source, "pwsh.exe").or_else(|| {
        ["ProgramFiles", "ProgramW6432"]
            .into_iter()
            .filter_map(env::var_os)
            .filter(|base| !base.is_empty())
            .map(|base| PathBuf::from(base).join(r"PowerShell\7\pwsh.exe"))
            .find(|candidate| candidate.is_file())
    })
}

fn windows_powershell_path(source: ChildEnvironment) -> Option<PathBuf> {
    find_on_path(source, "powershell.exe").or_else(|| {
        Some(system_root().join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
            .filter(|candidate| candidate.is_file())
    })
}

/// The shell [`WindowsShell::Auto`] stands for on this machine.
fn windows_auto_shell(source: ChildEnvironment) -> (WindowsShellFamily, String) {
    if let Some(pwsh) = windows_pwsh_path(source) {
        return (WindowsShellFamily::PowerShell, path_text(&pwsh));
    }
    if let Some(powershell) = windows_powershell_path(source) {
        return (WindowsShellFamily::PowerShell, path_text(&powershell));
    }
    (WindowsShellFamily::Cmd, windows_cmd_path_in(source))
}

/// Paths under `%SystemRoot%` or the Store alias directory, where `bash.exe`
/// is WSL's launcher rather than Git Bash.
fn is_system_owned_path(path: &Path) -> bool {
    let normalize = |path: &Path| {
        let mut text = path.to_string_lossy().replace('/', "\\").to_lowercase();
        if !text.ends_with('\\') {
            text.push('\\');
        }
        text
    };
    let candidate = normalize(path);
    let mut owned_roots = vec![normalize(&system_root())];
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        owned_roots.push(normalize(
            &PathBuf::from(local_app_data).join(r"Microsoft\WindowsApps"),
        ));
    }
    owned_roots.iter().any(|root| candidate.starts_with(root))
}

fn git_for_windows_install_paths() -> Vec<PathBuf> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\GitForWindows"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\GitForWindows"),
        (HKEY_CURRENT_USER, r"SOFTWARE\GitForWindows"),
    ]
    .into_iter()
    .filter_map(|(root, subkey)| {
        RegKey::predef(root)
            .open_subkey(subkey)
            .ok()?
            .get_value::<String, _>("InstallPath")
            .ok()
    })
    .map(|install_path| install_path.trim().to_string())
    .filter(|install_path| !install_path.is_empty())
    .map(PathBuf::from)
    .collect()
}

fn git_bash_candidates(source: ChildEnvironment) -> Vec<PathBuf> {
    let mut candidates = git_for_windows_install_paths()
        .into_iter()
        .map(|install| install.join(r"bin\bash.exe"))
        .collect::<Vec<_>>();

    // `git.exe` on PATH lives in `<Git>\cmd`, `<Git>\bin` or
    // `<Git>\mingw64\bin`; Git Bash is `<Git>\bin\bash.exe` in every layout.
    for git in find_all_on_path(source, "git.exe") {
        let Some(directory) = git.parent() else {
            continue;
        };
        candidates.push(directory.join("bash.exe"));
        candidates.extend(
            directory
                .ancestors()
                .take(3)
                .map(|ancestor| ancestor.join(r"bin\bash.exe")),
        );
    }

    for (variable, relative) in [
        ("ProgramFiles", r"Git\bin\bash.exe"),
        ("ProgramW6432", r"Git\bin\bash.exe"),
        ("ProgramFiles(x86)", r"Git\bin\bash.exe"),
        ("LOCALAPPDATA", r"Programs\Git\bin\bash.exe"),
        ("SCOOP", r"apps\git\current\bin\bash.exe"),
        ("USERPROFILE", r"scoop\apps\git\current\bin\bash.exe"),
    ] {
        if let Some(base) = env::var_os(variable).filter(|value| !value.is_empty()) {
            candidates.push(PathBuf::from(base).join(relative));
        }
    }

    candidates.extend(find_all_on_path(source, "bash.exe"));
    candidates
}

#[cfg(test)]
pub(super) fn windows_git_bash_path() -> String {
    windows_git_bash_path_in(ChildEnvironment::Process)
}

fn windows_git_bash_path_in(source: ChildEnvironment) -> String {
    git_bash_candidates(source)
        .into_iter()
        .find(|candidate| candidate.is_file() && !is_system_owned_path(candidate))
        .map_or_else(
            // Name the expected install rather than a bare `bash.exe`, which
            // PATH lookup would resolve to WSL; a missing Git Bash then fails
            // to launch with a clear not-found error.
            || {
                let program_files = env::var_os("ProgramFiles")
                    .filter(|value| !value.is_empty())
                    .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from);
                path_text(&program_files.join(r"Git\bin\bash.exe"))
            },
            |path| path_text(&path),
        )
}

/// The shell family and executable a preset launches.
fn windows_shell_program(
    windows_shell: WindowsShell,
    source: ChildEnvironment,
) -> (WindowsShellFamily, String) {
    match windows_shell {
        WindowsShell::Cmd => (WindowsShellFamily::Cmd, windows_cmd_path_in(source)),
        WindowsShell::PowerShell => (WindowsShellFamily::PowerShell, "powershell.exe".to_string()),
        WindowsShell::PowerShellCore => (WindowsShellFamily::PowerShell, "pwsh.exe".to_string()),
        WindowsShell::GitBash => (WindowsShellFamily::Posix, windows_git_bash_path_in(source)),
        WindowsShell::Auto => windows_auto_shell(source),
    }
}

pub(super) fn windows_shell_launch(
    windows_shell: WindowsShell,
    source: ChildEnvironment,
) -> ResolvedTerminalLaunch {
    let (family, program) = windows_shell_program(windows_shell, source);
    let args = match family {
        WindowsShellFamily::PowerShell => vec!["-NoLogo".to_string()],
        WindowsShellFamily::Posix => vec!["--login".to_string(), "-i".to_string()],
        WindowsShellFamily::Cmd | WindowsShellFamily::Other => Vec::new(),
    };
    ResolvedTerminalLaunch { program, args }
}

pub(super) fn windows_startup_command_shell(
    windows_shell: WindowsShell,
    command: &str,
    source: ChildEnvironment,
) -> ResolvedTerminalLaunch {
    let (family, program) = windows_shell_program(windows_shell, source);
    windows_startup_command_launch(family, program, command, source)
}

/// Run `command` through `program`, a shell of `family`. Shells that take no
/// known command flag fall back to the `COMSPEC` command processor.
pub(super) fn windows_startup_command_launch(
    family: WindowsShellFamily,
    program: String,
    command: &str,
    source: ChildEnvironment,
) -> ResolvedTerminalLaunch {
    match family {
        // cmd.exe parses the text after `/C` itself. `/S` strips exactly the
        // outer quote pair the launcher adds and keeps the command verbatim.
        WindowsShellFamily::Cmd => cmd_startup_command(program, command),
        WindowsShellFamily::PowerShell => ResolvedTerminalLaunch {
            program,
            args: vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                command.to_string(),
            ],
        },
        WindowsShellFamily::Posix => ResolvedTerminalLaunch {
            program,
            args: vec!["-lc".to_string(), command.to_string()],
        },
        WindowsShellFamily::Other => cmd_startup_command(windows_cmd_path_in(source), command),
    }
}

fn cmd_startup_command(program: String, command: &str) -> ResolvedTerminalLaunch {
    ResolvedTerminalLaunch {
        program,
        args: vec![
            "/D".to_string(),
            "/S".to_string(),
            "/C".to_string(),
            command.to_string(),
        ],
    }
}

/// Add Termy's prompt markers to an interactive PowerShell or cmd.exe
/// session, which have no sourced integration script like bash and zsh.
/// PowerShell gets an inline wrapper around its `prompt` function; cmd.exe a
/// default `PROMPT`, unless the user already set one, in `environment` or
/// in the `inherited_prompt` the shell would otherwise start with.
pub(super) fn apply_windows_shell_integration(
    launch: &mut ResolvedTerminalLaunch,
    environment: &mut HashMap<String, String>,
    shell_integration: Option<&TabTitleShellIntegration>,
    inherited_prompt: Option<&std::ffi::OsStr>,
) {
    if !shell_integration.is_some_and(|integration| integration.enabled) {
        return;
    }
    match windows_shell_family(&launch.program) {
        // Only a plain interactive session: extra arguments may already run
        // a command or select another mode.
        WindowsShellFamily::PowerShell
            if launch
                .args
                .iter()
                .all(|argument| argument.eq_ignore_ascii_case("-NoLogo")) =>
        {
            launch.args = vec![
                "-NoLogo".to_string(),
                "-NoExit".to_string(),
                "-Command".to_string(),
                POWERSHELL_INTEGRATION_SCRIPT.to_string(),
            ];
        }
        WindowsShellFamily::Cmd if launch.args.is_empty() => {
            let user_prompt = inherited_prompt_is_custom(inherited_prompt)
                || environment
                    .keys()
                    .any(|name| name.eq_ignore_ascii_case("PROMPT"));
            if !user_prompt {
                environment.insert("PROMPT".to_string(), CMD_INTEGRATION_PROMPT.to_string());
            }
        }
        _ => {}
    }
}

/// cmd.exe exports its default `$P$G` as `PROMPT`, so Termy started from a
/// cmd window inherits it; only another value is the user's own prompt.
pub(super) fn inherited_prompt_is_custom(prompt: Option<&std::ffi::OsStr>) -> bool {
    prompt.is_some_and(|prompt| {
        let prompt = prompt.to_string_lossy();
        let prompt = prompt.trim();
        !prompt.is_empty() && !prompt.eq_ignore_ascii_case("$P$G")
    })
}

/// cmd.exe cannot use a UNC working directory: it warns and starts in the
/// Windows directory instead.
pub(super) fn warn_cmd_unc_working_directory(
    launch: &ResolvedTerminalLaunch,
    working_directory: Option<&Path>,
) {
    let Some(directory) = working_directory else {
        return;
    };
    let text = directory.to_string_lossy();
    let unc = (text.starts_with(r"\\") && !text.starts_with(r"\\?\") && !text.starts_with(r"\\.\"))
        || text
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"));
    if unc && windows_shell_family(&launch.program) == WindowsShellFamily::Cmd {
        log::warn!(
            "cmd.exe does not support the UNC working directory {text}; it starts in the \
             Windows directory instead. Map the share to a drive letter or use PowerShell."
        );
    }
}
