use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallShell {
    Zsh,
    Bash,
    Fish,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallCliResult {
    pub install_path: PathBuf,
    pub shell_setup: Option<ShellSetup>,
    /// Windows only: whether the install directory was added to the user `Path`.
    pub user_path_setup: Option<UserPathSetup>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserPathSetup {
    Added,
    AlreadyPresent,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellSetup {
    pub profile_path: PathBuf,
    pub profile_updated: bool,
    pub session_command: String,
}

pub fn is_cli_installed() -> bool {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let path_env = std::env::var_os("PATH");
        let (target, _) = resolve_install_cli_target_for_unix(dirs::home_dir().as_deref());
        managed_target_binary_exists(&target)
            && managed_target_dir_in_path(&target, path_env.as_deref())
    }

    #[cfg(target_os = "windows")]
    {
        let path_env = std::env::var_os("PATH");
        let target = resolve_install_cli_target_for_windows(dirs::data_local_dir().as_deref());
        // The registry check covers the session right after Install CLI,
        // before this process has picked up the new user Path. A stale copy
        // (different from the bundled CLI, or without the app marker) reports
        // as not installed so the install action is offered again.
        target.as_deref().is_some_and(|target| {
            managed_target_binary_exists(target)
                && (managed_target_dir_in_path(target, path_env.as_deref())
                    || windows_user_path_contains_target_dir(target))
                && !windows_installed_cli_needs_refresh(target)
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

pub fn install_cli(configured_shell: Option<&str>) -> Result<InstallCliResult, String> {
    let install_path =
        install_cli_binary().map_err(|error| format!("Failed to install CLI: {error}"))?;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let path_str = install_path.display().to_string();
        let shell_setup = configure_install_cli_shell_path(configured_shell, &install_path)
            .map_err(|error| {
                format!("CLI installed to {path_str} but automated PATH setup failed: {error}")
            })?;

        Ok(InstallCliResult {
            install_path,
            shell_setup: Some(shell_setup),
            user_path_setup: None,
        })
    }

    #[cfg(target_os = "windows")]
    {
        let _ = configured_shell;
        let user_path_setup = match install_path.parent() {
            Some(install_dir) => match ensure_windows_user_path_contains(install_dir) {
                Ok(true) => UserPathSetup::Added,
                Ok(false) => UserPathSetup::AlreadyPresent,
                Err(error) => UserPathSetup::Failed(error),
            },
            None => UserPathSetup::Failed(format!(
                "Installed CLI path {} does not have a parent directory",
                install_path.display()
            )),
        };
        Ok(InstallCliResult {
            install_path,
            shell_setup: None,
            user_path_setup: Some(user_path_setup),
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = configured_shell;
        Ok(InstallCliResult {
            install_path,
            shell_setup: None,
            user_path_setup: None,
        })
    }
}

/// Install an explicitly selected CLI binary into an isolated Unix home.
///
/// This is the same install path used by the in-app action, with the source and
/// home made explicit so release smoke tests cannot touch the developer's real
/// shell profile.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn install_cli_from_source_for_home(
    cli_source: &Path,
    home_dir: &Path,
    configured_shell: Option<&str>,
) -> Result<InstallCliResult, String> {
    let install_path = install_cli_binary_from_source(cli_source, Some(home_dir))
        .map_err(|error| format!("Failed to install CLI: {error}"))?;
    let path_str = install_path.display().to_string();
    let shell_setup =
        configure_install_cli_shell_path_for_home(configured_shell, &install_path, home_dir)
            .map_err(|error| {
                format!("CLI installed to {path_str} but automated PATH setup failed: {error}")
            })?;

    Ok(InstallCliResult {
        install_path,
        shell_setup: Some(shell_setup),
        user_path_setup: None,
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn configure_install_cli_shell_path(
    configured_shell: Option<&str>,
    install_path: &Path,
) -> Result<ShellSetup, String> {
    let home_dir =
        dirs::home_dir().ok_or_else(|| "Could not determine home directory".to_string())?;
    configure_install_cli_shell_path_for_home(configured_shell, install_path, &home_dir)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn configure_install_cli_shell_path_for_home(
    configured_shell: Option<&str>,
    install_path: &Path,
    home_dir: &Path,
) -> Result<ShellSetup, String> {
    let install_dir = install_path.parent().ok_or_else(|| {
        format!(
            "Installed CLI path {} does not have a parent directory",
            install_path.display()
        )
    })?;
    let install_dir = install_dir.to_string_lossy().into_owned();
    let shell = install_cli_shell(configured_shell)?;
    let profile_path = install_cli_profile_path(shell, home_dir);
    let block = install_cli_profile_block(shell, &install_dir);
    let profile_updated = ensure_install_cli_profile_block(&profile_path, &block)?;
    let session_command = install_cli_session_command(shell, &install_dir);

    Ok(ShellSetup {
        profile_path,
        profile_updated,
        session_command,
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn install_cli_shell(configured_shell: Option<&str>) -> Result<InstallShell, String> {
    let env_shell = std::env::var("SHELL").ok();
    resolve_install_cli_shell(configured_shell, env_shell.as_deref())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn resolve_install_cli_shell(
    configured_shell: Option<&str>,
    env_shell: Option<&str>,
) -> Result<InstallShell, String> {
    let candidate = configured_shell
        .map(str::trim)
        .filter(|shell| !shell.is_empty())
        .or_else(|| env_shell.map(str::trim).filter(|shell| !shell.is_empty()))
        .unwrap_or(default_install_cli_shell_path());

    parse_install_cli_shell(candidate).ok_or_else(|| {
        format!(
            "Unsupported shell '{candidate}' for automated PATH setup. Supported shells: zsh, bash, fish."
        )
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn default_install_cli_shell_path() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "/bin/zsh"
    }

    #[cfg(target_os = "linux")]
    {
        "/bin/bash"
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn parse_install_cli_shell(shell: &str) -> Option<InstallShell> {
    let shell = shell.trim().trim_matches('"').trim_matches('\'');
    if shell.is_empty() {
        return None;
    }

    let shell_program = shell.split_whitespace().next()?;
    let shell_name = Path::new(shell_program)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or(shell_program);

    match shell_name {
        "zsh" => Some(InstallShell::Zsh),
        "bash" => Some(InstallShell::Bash),
        "fish" => Some(InstallShell::Fish),
        _ => None,
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn install_cli_profile_path(shell: InstallShell, home_dir: &Path) -> PathBuf {
    match shell {
        InstallShell::Zsh => home_dir.join(".zshrc"),
        // Termy starts bash as a login shell on macOS and as a non-login
        // interactive shell on Linux (see `login_shell_args`).
        InstallShell::Bash if cfg!(target_os = "macos") => bash_login_profile_path(home_dir),
        InstallShell::Bash => home_dir.join(".bashrc"),
        InstallShell::Fish => home_dir.join(".config").join("fish").join("config.fish"),
    }
}

/// The startup file a bash login shell reads in `home_dir`.
///
/// Bash reads only the first of `~/.bash_profile`, `~/.bash_login` and
/// `~/.profile` that exists, so the block goes into that one. Creating
/// `~/.bash_profile` next to an existing `~/.profile` would stop bash from
/// reading `~/.profile` and drop the user's environment from every tab, so
/// it is only created when none of the three exists.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn bash_login_profile_path(home_dir: &Path) -> PathBuf {
    const LOGIN_FILES: [&str; 3] = [".bash_profile", ".bash_login", ".profile"];

    // `exists` follows symlinks: bash skips a dangling link like a missing file.
    LOGIN_FILES
        .iter()
        .map(|name| home_dir.join(name))
        .find(|path| path.exists())
        .unwrap_or_else(|| home_dir.join(LOGIN_FILES[0]))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn install_cli_profile_block(shell: InstallShell, install_dir: &str) -> String {
    const START: &str = "# >>> termy cli path >>>";
    const END: &str = "# <<< termy cli path <<<";

    match shell {
        InstallShell::Zsh | InstallShell::Bash => format!(
            "{START}\n# Added by Termy Install CLI\nTERMY_CLI_PATH={}\ncase \":$PATH:\" in\n  *\":$TERMY_CLI_PATH:\"*) ;;\n  *) export PATH=\"$TERMY_CLI_PATH:$PATH\" ;;\nesac\n{END}",
            single_quote_shell_value(install_dir)
        ),
        InstallShell::Fish => format!(
            "{START}\n# Added by Termy Install CLI\nset -l termy_cli_path {}\nif not contains -- $termy_cli_path $PATH\n    set -gx PATH $termy_cli_path $PATH\nend\n{END}",
            double_quote_fish_value(install_dir)
        ),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn install_cli_session_command(shell: InstallShell, install_dir: &str) -> String {
    match shell {
        InstallShell::Zsh | InstallShell::Bash => format!(
            "TERMY_CLI_PATH={}; case \":$PATH:\" in *\":$TERMY_CLI_PATH:\"*) ;; *) export PATH=\"$TERMY_CLI_PATH:$PATH\" ;; esac",
            single_quote_shell_value(install_dir)
        ),
        InstallShell::Fish => format!(
            "set -l termy_cli_path {}; if not contains -- $termy_cli_path $PATH; set -gx PATH $termy_cli_path $PATH; end",
            double_quote_fish_value(install_dir)
        ),
    }
}

/// Appends `block` to the profile unless its marker is already there.
///
/// The block is only ever appended, so the profile is opened in append mode
/// and extended in place instead of being rewritten through a temp file and a
/// rename. That keeps everything the user set on the file: its mode (a 0600
/// `.zshrc` stays 0600), owner, ACLs, extended attributes and hard links. No
/// sibling path such as `.zshrc.tmp` is created, truncated or followed.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn ensure_install_cli_profile_block(profile_path: &Path, block: &str) -> Result<bool, String> {
    use std::io::{Read, Write};

    const START: &str = "# >>> termy cli path >>>";

    // Update the file a symlinked profile points at (stow, chezmoi,
    // home-manager), creating the parent of a dangling link's target first.
    let profile_path = &resolve_profile_symlinks(profile_path);
    if let Some(parent) = profile_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create shell config directory {}: {}",
                parent.display(),
                error
            )
        })?;
    }

    // A missing profile is created with the default (umask) permissions.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(profile_path)
        .map_err(|error| {
            format!(
                "Failed to open shell config {}: {}",
                profile_path.display(),
                error
            )
        })?;

    let mut existing = Vec::new();
    file.read_to_end(&mut existing).map_err(|error| {
        format!(
            "Failed to read shell config {}: {}",
            profile_path.display(),
            error
        )
    })?;

    let Some(addition) =
        install_cli_profile_block_addition(&String::from_utf8_lossy(&existing), START, block)
    else {
        return Ok(false);
    };

    let written = file
        .write_all(addition.as_bytes())
        .and_then(|()| file.sync_all());
    if let Err(error) = written {
        // Drop a partly written block, so the profile is not left with an
        // unterminated `case` that a later install would also skip.
        let _ = file.set_len(existing.len() as u64);
        return Err(format!(
            "Failed to write shell config {}: {}",
            profile_path.display(),
            error
        ));
    }
    Ok(true)
}

/// Follows `path` through symlinks, including a dangling last link, so the
/// profile can be created where the link expects it.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn resolve_profile_symlinks(path: &Path) -> PathBuf {
    // Bounded like the kernel's ELOOP limit, so a link cycle just stops.
    const MAX_LINKS: usize = 40;

    let mut path = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        let Ok(target) = std::fs::read_link(&path) else {
            break;
        };
        path = match path.parent() {
            Some(parent) => parent.join(target),
            None => target,
        };
    }
    path
}

/// The text to append to a profile holding `existing` so that it ends with
/// `block`, or `None` when the block's `marker` is already there.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn install_cli_profile_block_addition(existing: &str, marker: &str, block: &str) -> Option<String> {
    if existing.contains(marker) {
        return None;
    }

    let mut addition = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        addition.push('\n');
    }
    addition.push_str(block);
    if !addition.ends_with('\n') {
        addition.push('\n');
    }
    Some(addition)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn single_quote_shell_value(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn double_quote_fish_value(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
    )
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn path_exists_or_symlink(path: &Path) -> bool {
    path.exists() || path.symlink_metadata().is_ok()
}

fn managed_target_binary_exists(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn managed_target_dir_in_path(target_path: &Path, path_env: Option<&OsStr>) -> bool {
    let Some(target_dir) = target_path.parent() else {
        return false;
    };
    let Some(path_env) = path_env else {
        return false;
    };

    std::env::split_paths(path_env)
        .any(|path_entry| paths_match_with_canonicalization(&path_entry, target_dir))
}

fn paths_match_with_canonicalization(path_a: &Path, path_b: &Path) -> bool {
    if path_a == path_b {
        return true;
    }

    match (path_a.canonicalize(), path_b.canonicalize()) {
        (Ok(path_a), Ok(path_b)) => path_a == path_b,
        _ => false,
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn resolve_install_cli_target_for_unix(home_dir: Option<&Path>) -> (PathBuf, bool) {
    if let Some(home_dir) = home_dir {
        (home_dir.join(".local").join("bin").join("termy"), false)
    } else {
        (PathBuf::from("/usr/local/bin/termy"), true)
    }
}

#[cfg(target_os = "windows")]
fn resolve_install_cli_target_for_windows(local_app_data: Option<&Path>) -> Option<PathBuf> {
    local_app_data.map(|path| path.join("Termy").join("bin").join("termy.exe"))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn install_cli_binary() -> Result<PathBuf, String> {
    let cli_source = find_cli_binary()?;
    install_cli_binary_from_source(&cli_source, dirs::home_dir().as_deref())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn install_cli_binary_from_source(
    cli_source: &Path,
    home_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    use std::os::unix::fs::symlink;

    let cli_source = absolute_install_cli_source_path(cli_source)?;
    if !cli_source.is_file() {
        return Err(format!("CLI binary not found: {}", cli_source.display()));
    }

    let (target, using_fallback) = resolve_install_cli_target_for_unix(home_dir);

    if let Some(parent) = target.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            if using_fallback && error.kind() == std::io::ErrorKind::PermissionDenied {
                format!(
                    "Failed to create {}: {}. \
                    $HOME is not set, so fell back to system path. \
                    Either: set $HOME and retry (to use ~/.local/bin), \
                    run with elevated privileges (sudo), \
                    or manually create {} with appropriate permissions.",
                    parent.display(),
                    error,
                    parent.display()
                )
            } else {
                format!("Failed to create directory {}: {}", parent.display(), error)
            }
        })?;
    }

    if path_exists_or_symlink(&target) {
        std::fs::remove_file(&target).map_err(|error| {
            format!(
                "Failed to remove existing file at {}: {}",
                target.display(),
                error
            )
        })?;
    }

    symlink(&cli_source, &target).map_err(|error| {
        format!(
            "Failed to create symlink at {}: {}",
            target.display(),
            error
        )
    })?;

    Ok(target)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn absolute_install_cli_source_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let cwd = std::env::current_dir()
        .map_err(|error| format!("Failed to resolve current directory: {error}"))?;
    Ok(cwd.join(path))
}

#[cfg(target_os = "windows")]
fn install_cli_binary() -> Result<PathBuf, String> {
    let cli_source = find_cli_binary()?;

    let target = resolve_install_cli_target_for_windows(dirs::data_local_dir().as_deref())
        .ok_or_else(|| "Could not determine local app data directory".to_string())?;

    install_windows_cli_copy(&cli_source, &target)?;
    write_windows_app_path_marker(&target);

    Ok(target)
}

/// Copies the bundled CLI to `target` unless an identical copy is already
/// there. A running `termy.exe` can't be overwritten but can be renamed, so a
/// failed overwrite moves the old copy to `termy.exe.old` first.
#[cfg(target_os = "windows")]
fn install_windows_cli_copy(source: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create directory {}: {error}", parent.display()))?;
    }

    // Left behind when a previous refresh replaced a running copy.
    let previous = target.with_extension("exe.old");
    let _ = std::fs::remove_file(&previous);

    if windows_cli_copy_is_current(source, target) {
        return Ok(());
    }

    let Err(copy_error) = std::fs::copy(source, target) else {
        return Ok(());
    };
    if !target.exists() {
        return Err(format!(
            "Failed to copy CLI binary to {}: {copy_error}",
            target.display()
        ));
    }

    std::fs::rename(target, &previous).map_err(|error| {
        format!(
            "Failed to replace {} ({copy_error}); close running termy commands and retry: {error}",
            target.display()
        )
    })?;
    if let Err(error) = std::fs::copy(source, target) {
        let _ = std::fs::rename(&previous, target);
        return Err(format!(
            "Failed to copy CLI binary to {}: {error}",
            target.display()
        ));
    }
    let _ = std::fs::remove_file(&previous);
    Ok(())
}

/// `std::fs::copy` keeps the last-write time on Windows, so an up-to-date copy
/// matches the bundled CLI in both size and mtime.
#[cfg(target_os = "windows")]
fn windows_cli_copy_is_current(source: &Path, target: &Path) -> bool {
    let (Ok(source), Ok(target)) = (std::fs::metadata(source), std::fs::metadata(target)) else {
        return false;
    };
    source.len() == target.len()
        && matches!((source.modified(), target.modified()), (Ok(a), Ok(b)) if a == b)
}

#[cfg(target_os = "windows")]
fn windows_installed_cli_needs_refresh(target: &Path) -> bool {
    if !target.with_file_name(WINDOWS_APP_PATH_MARKER).is_file() {
        return true;
    }
    // No bundled CLI next to this executable (e.g. when the installed copy
    // itself asks): nothing to compare against.
    find_cli_binary().is_ok_and(|source| !windows_cli_copy_is_current(&source, target))
}

/// The copied CLI can't find the GUI beside itself, so record which GUI
/// installed it. Best-effort: `termy open` also checks the installer's
/// registry entries and the default install directories.
#[cfg(target_os = "windows")]
fn write_windows_app_path_marker(target: &Path) {
    let Ok(app) = std::env::current_exe() else {
        return;
    };
    let app = dunce::canonicalize(&app).unwrap_or(app);
    // Only the GUI installs the CLI; never point the CLI back at a CLI.
    let is_cli = app
        .file_stem()
        .and_then(OsStr::to_str)
        .is_some_and(|stem| stem.eq_ignore_ascii_case("termy-cli"));
    let Some(app) = app.to_str().filter(|_| !is_cli) else {
        return;
    };
    let marker = target.with_file_name(WINDOWS_APP_PATH_MARKER);
    if let Err(error) = std::fs::write(&marker, app) {
        log::warn!(
            "Failed to record the Termy app path in {}: {error}",
            marker.display()
        );
    }
}

/// Written next to the installed Windows CLI; holds the absolute path of the
/// Termy GUI that installed it.
#[cfg(target_os = "windows")]
pub const WINDOWS_APP_PATH_MARKER: &str = "termy-app-path.txt";

#[cfg(target_os = "windows")]
const WINDOWS_APP_EXE_NAME: &str = "termy.exe";

#[cfg(target_os = "windows")]
const WINDOWS_APP_PATHS_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\App Paths\termy.exe";

/// Inno Setup's uninstall entry; the GUID is `AppId` in scripts/installer/termy.iss.
#[cfg(target_os = "windows")]
const WINDOWS_INNO_UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{7D3DD34B-5F8F-4D7B-BBC9-0F54B4C89142}_is1";

#[cfg(target_os = "windows")]
const WINDOWS_USER_ENVIRONMENT_KEY: &str = "Environment";

/// Where a Windows CLI that isn't next to the GUI (the copy Install CLI puts in
/// `%LOCALAPPDATA%\Termy\bin`) should look for it, most specific first: the
/// marker written at install time, the installer's registry entries, then the
/// default install directories. Entries may not exist; callers must also skip
/// any entry that is the CLI itself.
#[cfg(target_os = "windows")]
pub fn windows_app_binary_candidates(cli_dir: &Path) -> Vec<PathBuf> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let mut candidates = Vec::new();
    if let Ok(recorded) = std::fs::read_to_string(cli_dir.join(WINDOWS_APP_PATH_MARKER)) {
        let recorded = recorded.trim();
        if !recorded.is_empty() {
            candidates.push(PathBuf::from(recorded));
        }
    }

    for root in [
        RegKey::predef(HKEY_LOCAL_MACHINE),
        RegKey::predef(HKEY_CURRENT_USER),
    ] {
        if let Ok(app) = root
            .open_subkey(WINDOWS_APP_PATHS_KEY)
            .and_then(|key| key.get_value::<String, _>(""))
        {
            let app = app.trim().trim_matches('"');
            if !app.is_empty() {
                candidates.push(PathBuf::from(expand_percent_env_vars(app)));
            }
        }
        if let Ok(location) = root
            .open_subkey(WINDOWS_INNO_UNINSTALL_KEY)
            .and_then(|key| key.get_value::<String, _>("InstallLocation"))
        {
            let location = location.trim().trim_matches('"');
            if !location.is_empty() {
                candidates.push(PathBuf::from(location).join(WINDOWS_APP_EXE_NAME));
            }
        }
    }

    for (var, relative) in [
        ("ProgramW6432", "Termy"),
        ("ProgramFiles", "Termy"),
        ("LOCALAPPDATA", r"Programs\Termy"),
    ] {
        if let Some(base) = std::env::var_os(var).filter(|value| !value.is_empty()) {
            let candidate = PathBuf::from(base)
                .join(relative)
                .join(WINDOWS_APP_EXE_NAME);
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }

    candidates
}

#[cfg(target_os = "windows")]
fn read_windows_user_path() -> std::io::Result<Option<String>> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let environment =
        match RegKey::predef(HKEY_CURRENT_USER).open_subkey(WINDOWS_USER_ENVIRONMENT_KEY) {
            Ok(key) => key,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
    match environment.get_value::<String, _>("Path") {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "windows")]
fn windows_user_path_contains_target_dir(target: &Path) -> bool {
    let Some(target_dir) = target.parent().and_then(Path::to_str) else {
        return false;
    };
    read_windows_user_path().ok().flatten().is_some_and(|path| {
        path.split(';')
            .any(|entry| windows_path_entry_matches(entry, target_dir))
    })
}

/// Appends `dir` to the user `Path` (HKCU\Environment) unless an entry already
/// names it, keeping the existing value and writing REG_EXPAND_SZ so `%VAR%`
/// entries keep expanding. Returns whether the value changed.
#[cfg(target_os = "windows")]
fn ensure_windows_user_path_contains(dir: &Path) -> Result<bool, String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ};
    use winreg::{RegKey, RegValue};

    let dir = dir
        .to_str()
        .ok_or_else(|| format!("{} is not valid Unicode", dir.display()))?;
    let (environment, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey_with_flags(WINDOWS_USER_ENVIRONMENT_KEY, KEY_READ | KEY_WRITE)
        .map_err(|error| format!("Failed to open HKCU\\Environment: {error}"))?;
    let existing = match environment.get_value::<String, _>("Path") {
        Ok(value) => Some(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("Failed to read the user Path: {error}")),
    };
    let Some(updated) = windows_path_value_with_dir(existing.as_deref(), dir) else {
        return Ok(false);
    };

    let bytes = updated
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    environment
        .set_raw_value(
            "Path",
            &RegValue {
                bytes,
                vtype: REG_EXPAND_SZ,
            },
        )
        .map_err(|error| format!("Failed to update the user Path: {error}"))?;
    broadcast_windows_environment_change();
    Ok(true)
}

/// Tells Explorer (and anything else listening) to reload the environment, so
/// terminals opened from now on see the new `Path`.
#[cfg(target_os = "windows")]
fn broadcast_windows_environment_change() {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SendMessageTimeoutW(
            hwnd: isize,
            msg: u32,
            wparam: usize,
            lparam: isize,
            flags: u32,
            timeout: u32,
            result: *mut usize,
        ) -> isize;
    }

    const HWND_BROADCAST: isize = 0xffff;
    const WM_SETTINGCHANGE: u32 = 0x001A;
    const SMTO_ABORTIFHUNG: u32 = 0x0002;

    // Broadcasting waits on every top-level window; keep it off the caller's
    // (UI) thread.
    std::thread::spawn(|| {
        let area: Vec<u16> = "Environment"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut result = 0usize;
        // SAFETY: `area` is a NUL-terminated UTF-16 string that outlives the
        // call, and `result` is a valid out pointer.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                area.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                5000,
                &raw mut result,
            );
        }
    });
}

/// The user `Path` value with `dir` appended, or `None` when an entry already
/// names it (case-insensitively, ignoring quotes, trailing separators and
/// `%VAR%` spelling).
#[cfg(any(target_os = "windows", test))]
fn windows_path_value_with_dir(existing: Option<&str>, dir: &str) -> Option<String> {
    let existing = existing.unwrap_or_default();
    if existing
        .split(';')
        .any(|entry| windows_path_entry_matches(entry, dir))
    {
        return None;
    }
    let existing = existing.trim_end_matches(';');
    Some(if existing.trim().is_empty() {
        dir.to_string()
    } else {
        format!("{existing};{dir}")
    })
}

#[cfg(any(target_os = "windows", test))]
fn windows_path_entry_matches(entry: &str, dir: &str) -> bool {
    fn normalize(value: &str) -> String {
        value
            .trim()
            .trim_matches('"')
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    }

    let wanted = normalize(dir);
    !wanted.is_empty()
        && (normalize(entry) == wanted || normalize(&expand_percent_env_vars(entry)) == wanted)
}

/// Expands `%NAME%` references; unknown names are left as written.
#[cfg(any(target_os = "windows", test))]
fn expand_percent_env_vars(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        output.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let name = &after[..end];
        let expanded = if name.is_empty() {
            None
        } else {
            std::env::var(name).ok()
        };
        if let Some(expanded) = expanded {
            output.push_str(&expanded);
        } else {
            output.push('%');
            output.push_str(name);
            output.push('%');
        }
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    output
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn install_cli_binary() -> Result<PathBuf, String> {
    Err("CLI installation is not supported on this platform".to_string())
}

fn find_cli_binary() -> Result<PathBuf, String> {
    let exe_path = std::env::current_exe()
        .map_err(|error| format!("Failed to resolve the Termy executable: {error}"))?;
    let exe_dir = exe_path
        .parent()
        .ok_or("Failed to get executable directory")?;

    cli_binary_candidates(exe_dir)
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| {
            "CLI binary not found. Make sure to build it with: cargo build -p termy_cli".to_string()
        })
}

/// Where the CLI may be, relative to the running executable only: next to it
/// (app bundles, packages, `cargo run`), or in the other profile of the same
/// cargo target dir. Never relative to the current directory, so starting
/// Termy inside a checkout can't install that checkout's `target/` binary.
fn cli_binary_candidates(exe_dir: &Path) -> Vec<PathBuf> {
    let cli_name = format!("termy-cli{}", std::env::consts::EXE_SUFFIX);
    let mut candidates = vec![exe_dir.join(&cli_name)];
    if let Some(target_dir) = exe_dir
        .parent()
        .filter(|dir| dir.file_name() == Some(OsStr::new("target")))
    {
        for profile in ["release", "debug"] {
            let path = target_dir.join(profile).join(&cli_name);
            if !candidates.contains(&path) {
                candidates.push(path);
            }
        }
    }
    candidates
}

#[cfg(test)]
mod tests;
