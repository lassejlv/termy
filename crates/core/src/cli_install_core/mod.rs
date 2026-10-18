use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::io::Write;

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
    let profile_path = home_dir.join(install_cli_profile_relative_path(shell));
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
fn install_cli_profile_relative_path(shell: InstallShell) -> &'static str {
    match shell {
        InstallShell::Zsh => ".zshrc",
        InstallShell::Bash => {
            if cfg!(target_os = "macos") {
                ".bash_profile"
            } else {
                ".bashrc"
            }
        }
        InstallShell::Fish => ".config/fish/config.fish",
    }
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn ensure_install_cli_profile_block(profile_path: &Path, block: &str) -> Result<bool, String> {
    const START: &str = "# >>> termy cli path >>>";

    // Update the file a symlinked profile points at (stow, chezmoi,
    // home-manager): renaming over the link would replace it.
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

    let existing = match std::fs::read_to_string(profile_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(format!(
                "Failed to read shell config {}: {}",
                profile_path.display(),
                error
            ));
        }
    };

    let Some(updated) = append_install_cli_profile_block_if_missing(&existing, START, block) else {
        return Ok(false);
    };

    let temp_path = profile_path.with_extension("tmp");
    {
        let mut temp_file = std::fs::File::create(&temp_path).map_err(|error| {
            format!(
                "Failed to write shell config {}: {}",
                profile_path.display(),
                error
            )
        })?;
        temp_file.write_all(updated.as_bytes()).map_err(|error| {
            format!(
                "Failed to write shell config {}: {}",
                profile_path.display(),
                error
            )
        })?;
        temp_file.sync_all().map_err(|error| {
            format!(
                "Failed to write shell config {}: {}",
                profile_path.display(),
                error
            )
        })?;
    }
    std::fs::rename(&temp_path, profile_path).map_err(|error| {
        format!(
            "Failed to write shell config {}: {}",
            profile_path.display(),
            error
        )
    })?;
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn append_install_cli_profile_block_if_missing(
    existing: &str,
    marker: &str,
    block: &str,
) -> Option<String> {
    if existing.contains(marker) {
        return None;
    }

    let mut updated = existing.to_string();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(block);
    if !updated.ends_with('\n') {
        updated.push('\n');
    }
    Some(updated)
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
        match expanded {
            Some(expanded) => output.push_str(&expanded),
            None => {
                output.push('%');
                output.push_str(name);
                output.push('%');
            }
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
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn install_cli_parse_shell_detects_supported_shells() {
        assert_eq!(parse_install_cli_shell("/bin/zsh"), Some(InstallShell::Zsh));
        assert_eq!(
            parse_install_cli_shell("\"/bin/bash\""),
            Some(InstallShell::Bash)
        );
        assert_eq!(
            parse_install_cli_shell("/opt/homebrew/bin/fish"),
            Some(InstallShell::Fish)
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn install_cli_resolve_shell_uses_source_order() {
        assert_eq!(
            resolve_install_cli_shell(Some("/bin/fish"), Some("/bin/zsh")).unwrap(),
            InstallShell::Fish
        );
        assert_eq!(
            resolve_install_cli_shell(Some("   "), Some("/bin/zsh")).unwrap(),
            InstallShell::Zsh
        );
        assert_eq!(
            resolve_install_cli_shell(None, Some("/bin/bash")).unwrap(),
            InstallShell::Bash
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn install_cli_resolve_shell_errors_for_unsupported_configured_shell() {
        let error = resolve_install_cli_shell(Some("/bin/tcsh"), Some("/bin/zsh")).unwrap_err();
        assert!(error.contains("Unsupported shell '/bin/tcsh'"));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn install_cli_resolve_shell_defaults_when_sources_missing() {
        let shell = resolve_install_cli_shell(None, None).unwrap();
        #[cfg(target_os = "macos")]
        assert_eq!(shell, InstallShell::Zsh);
        #[cfg(target_os = "linux")]
        assert_eq!(shell, InstallShell::Bash);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn install_cli_profile_relative_paths_match_shell() {
        assert_eq!(
            install_cli_profile_relative_path(InstallShell::Zsh),
            ".zshrc"
        );
        assert_eq!(
            install_cli_profile_relative_path(InstallShell::Fish),
            ".config/fish/config.fish"
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            install_cli_profile_relative_path(InstallShell::Bash),
            ".bash_profile"
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            install_cli_profile_relative_path(InstallShell::Bash),
            ".bashrc"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn install_cli_profile_append_is_idempotent() {
        let marker = "# >>> termy cli path >>>";
        let block = install_cli_profile_block(InstallShell::Zsh, "/tmp/bin");
        let once = append_install_cli_profile_block_if_missing("", marker, &block).unwrap();
        assert!(once.contains(marker));
        assert!(append_install_cli_profile_block_if_missing(&once, marker, &block).is_none());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn install_cli_profile_blocks_include_guarded_path_logic() {
        let sh_block = install_cli_profile_block(InstallShell::Bash, "/tmp/bin");
        assert!(sh_block.contains("case \":$PATH:\" in"));
        assert!(sh_block.contains("export PATH=\"$TERMY_CLI_PATH:$PATH\""));

        let fish_block = install_cli_profile_block(InstallShell::Fish, "/tmp/bin");
        assert!(fish_block.contains("if not contains -- $termy_cli_path $PATH"));
        assert!(fish_block.contains("set -gx PATH $termy_cli_path $PATH"));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn install_cli_session_commands_match_shell_syntax() {
        let sh_command = install_cli_session_command(InstallShell::Zsh, "/tmp/my bin");
        assert!(sh_command.contains("TERMY_CLI_PATH='/tmp/my bin'"));
        assert!(sh_command.contains("case \":$PATH:\" in"));

        let fish_command = install_cli_session_command(InstallShell::Fish, "/tmp/my bin");
        assert!(fish_command.contains("set -l termy_cli_path \"/tmp/my bin\""));
        assert!(fish_command.contains("if not contains -- $termy_cli_path $PATH"));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn install_cli_shell_value_escaping_handles_quotes() {
        assert_eq!(single_quote_shell_value("a'b"), "'a'\\''b'");
        assert_eq!(double_quote_fish_value("a\"b$c"), "\"a\\\"b\\$c\"");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn install_cli_source_path_is_absolutized_for_relative_paths() {
        let rel = Path::new("target/debug/termy-cli");
        let abs = absolute_install_cli_source_path(rel).unwrap();
        assert!(abs.is_absolute());
        assert!(abs.ends_with(rel));
    }

    #[test]
    fn cli_binary_candidates_are_relative_to_the_executable_only() {
        let exe_suffix = std::env::consts::EXE_SUFFIX;
        let root = std::env::temp_dir().join("termy-cli-candidates");

        let installed = root.join("Termy.app").join("Contents").join("MacOS");
        assert_eq!(
            cli_binary_candidates(&installed),
            vec![installed.join(format!("termy-cli{exe_suffix}"))]
        );

        let debug = root.join("target").join("debug");
        let candidates = cli_binary_candidates(&debug);
        assert_eq!(
            candidates,
            vec![
                debug.join(format!("termy-cli{exe_suffix}")),
                root.join("target")
                    .join("release")
                    .join(format!("termy-cli{exe_suffix}")),
            ]
        );
        assert!(candidates.iter().all(|path| path.starts_with(&root)));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn profile_update_writes_through_a_symlinked_profile() {
        let home = TempDir::new().expect("home");
        let dotfiles = home.path().join("dotfiles");
        std::fs::create_dir_all(&dotfiles).expect("dotfiles");
        let target = dotfiles.join("zshrc");
        std::fs::write(&target, "export EDITOR=vi\n").expect("write target");
        let profile = home.path().join(".zshrc");
        std::os::unix::fs::symlink(Path::new("dotfiles").join("zshrc"), &profile).expect("symlink");

        let block = install_cli_profile_block(InstallShell::Zsh, "/tmp/bin");
        assert!(ensure_install_cli_profile_block(&profile, &block).expect("update profile"));

        assert!(
            std::fs::symlink_metadata(&profile)
                .expect("profile metadata")
                .file_type()
                .is_symlink()
        );
        let contents = std::fs::read_to_string(&target).expect("read target");
        assert!(contents.starts_with("export EDITOR=vi\n"));
        assert!(contents.contains("# >>> termy cli path >>>"));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn profile_update_creates_the_target_of_a_dangling_symlink() {
        let home = TempDir::new().expect("home");
        let target = home.path().join("dotfiles").join("zshrc");
        let profile = home.path().join(".zshrc");
        std::os::unix::fs::symlink(&target, &profile).expect("symlink");

        let block = install_cli_profile_block(InstallShell::Zsh, "/tmp/bin");
        assert!(ensure_install_cli_profile_block(&profile, &block).expect("update profile"));

        assert!(
            std::fs::read_to_string(&target)
                .expect("read target")
                .contains("# >>> termy cli path >>>")
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn unix_install_target_prefers_home_local_bin() {
        let home = Path::new("/tmp/termy-home");
        let (target, using_fallback) = resolve_install_cli_target_for_unix(Some(home));
        assert_eq!(target, home.join(".local").join("bin").join("termy"));
        assert!(!using_fallback);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn unix_install_target_uses_system_fallback_without_home() {
        let (target, using_fallback) = resolve_install_cli_target_for_unix(None);
        assert_eq!(target, PathBuf::from("/usr/local/bin/termy"));
        assert!(using_fallback);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn explicit_source_install_stays_inside_supplied_home() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("bundle").join("termy-cli");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"cli").unwrap();
        let home = temp.path().join("home");

        let result = install_cli_from_source_for_home(&source, &home, Some("/bin/zsh"))
            .expect("isolated install should succeed");

        assert_eq!(result.install_path, home.join(".local/bin/termy"));
        assert_eq!(std::fs::read_link(&result.install_path).unwrap(), source);
        assert!(home.join(".zshrc").is_file());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_install_target_uses_local_app_data() {
        let base = Path::new("C:/Users/Test/AppData/Local");
        let target = resolve_install_cli_target_for_windows(Some(base)).unwrap();
        assert_eq!(target, base.join("Termy").join("bin").join("termy.exe"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_install_target_missing_app_data_returns_none() {
        assert!(resolve_install_cli_target_for_windows(None).is_none());
    }

    #[test]
    fn windows_user_path_append_preserves_existing_value() {
        assert_eq!(
            windows_path_value_with_dir(Some(r"%USERPROFILE%\bin;C:\tools;"), r"C:\Termy\bin"),
            Some(r"%USERPROFILE%\bin;C:\tools;C:\Termy\bin".to_string())
        );
        assert_eq!(
            windows_path_value_with_dir(None, r"C:\Termy\bin"),
            Some(r"C:\Termy\bin".to_string())
        );
        assert_eq!(
            windows_path_value_with_dir(Some(""), r"C:\Termy\bin"),
            Some(r"C:\Termy\bin".to_string())
        );
    }

    #[test]
    fn windows_user_path_append_dedupes_case_insensitively() {
        for existing in [
            r"C:\tools;c:\termy\BIN",
            r"C:\Termy\bin\;C:\tools",
            r#""C:\Termy\bin";C:\tools"#,
            "C:/Termy/bin",
        ] {
            assert_eq!(
                windows_path_value_with_dir(Some(existing), r"C:\Termy\bin"),
                None,
                "{existing}"
            );
        }
    }

    #[test]
    fn windows_user_path_entries_match_after_env_expansion() {
        let variable = std::env::vars().find(|(name, value)| {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && value.chars().next().is_some_and(char::is_alphanumeric)
                && !value.contains('%')
        });
        if let Some((name, value)) = variable {
            assert_eq!(
                expand_percent_env_vars(&format!("%{name}%\\bin")),
                format!("{value}\\bin")
            );
            assert!(windows_path_entry_matches(&format!("%{name}%"), &value));
        }
        assert_eq!(
            expand_percent_env_vars("%TERMY_SURELY_UNSET_VAR%\\bin"),
            "%TERMY_SURELY_UNSET_VAR%\\bin"
        );
        assert_eq!(expand_percent_env_vars("100%"), "100%");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_cli_copy_refreshes_only_when_bundled_cli_changes() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("termy-cli.exe");
        let target = temp.path().join("bin").join("termy.exe");
        std::fs::write(&source, b"cli v1").unwrap();

        install_windows_cli_copy(&source, &target).unwrap();
        assert!(windows_cli_copy_is_current(&source, &target));
        assert_eq!(std::fs::read(&target).unwrap(), b"cli v1");

        std::fs::write(&source, b"cli v2 is longer").unwrap();
        assert!(!windows_cli_copy_is_current(&source, &target));
        install_windows_cli_copy(&source, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"cli v2 is longer");
        assert!(!target.with_extension("exe.old").exists());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_app_candidates_start_with_the_install_marker() {
        let temp = TempDir::new().unwrap();
        let app = temp.path().join("Termy").join("termy.exe");
        std::fs::write(
            temp.path().join(WINDOWS_APP_PATH_MARKER),
            format!("{}\r\n", app.display()),
        )
        .unwrap();

        let candidates = windows_app_binary_candidates(temp.path());
        assert_eq!(candidates.first(), Some(&app));
    }

    #[test]
    fn managed_target_binary_exists_detects_existing_file() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("termy");
        std::fs::write(&file, b"test").unwrap();
        assert!(managed_target_binary_exists(&file));
    }

    #[test]
    fn managed_target_binary_exists_reports_missing_path() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("missing");
        assert!(!managed_target_binary_exists(&missing));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn managed_target_binary_exists_treats_broken_symlink_as_missing() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let broken_link = temp.path().join("termy");
        symlink(temp.path().join("does-not-exist"), &broken_link).unwrap();
        assert!(!managed_target_binary_exists(&broken_link));
    }

    #[test]
    fn managed_target_dir_in_path_requires_parent_directory() {
        let temp = TempDir::new().unwrap();
        let bin_dir = temp.path().join("bin");
        let other_dir = temp.path().join("other");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::create_dir_all(&other_dir).unwrap();

        let target = bin_dir.join(format!("termy{}", std::env::consts::EXE_SUFFIX));
        let path_env = std::env::join_paths([bin_dir.as_path()]).unwrap();
        assert!(managed_target_dir_in_path(
            &target,
            Some(path_env.as_os_str())
        ));

        let other_path_env = std::env::join_paths([other_dir.as_path()]).unwrap();
        assert!(!managed_target_dir_in_path(
            &target,
            Some(other_path_env.as_os_str())
        ));
        assert!(!managed_target_dir_in_path(&target, None));
    }

    #[test]
    fn managed_target_dir_in_path_matches_after_canonicalization() {
        let temp = TempDir::new().unwrap();
        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();

        let target = bin_dir.join(format!("termy{}", std::env::consts::EXE_SUFFIX));
        let normalized_via_parent = bin_dir.join("..").join("bin");
        let path_env = std::env::join_paths([normalized_via_parent]).unwrap();

        assert!(managed_target_dir_in_path(
            &target,
            Some(path_env.as_os_str())
        ));
    }

    #[test]
    fn path_exists_or_symlink_detects_existing_file() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("termy");
        std::fs::write(&file, b"test").unwrap();
        assert!(path_exists_or_symlink(&file));
    }

    #[test]
    fn path_exists_or_symlink_reports_missing_path() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("missing");
        assert!(!path_exists_or_symlink(&missing));
    }
}
