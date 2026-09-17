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
        target.as_deref().is_some_and(|target| {
            managed_target_binary_exists(target)
                && managed_target_dir_in_path(target, path_env.as_deref())
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
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = configured_shell;
        Ok(InstallCliResult {
            install_path,
            shell_setup: None,
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

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create directory: {}", error))?;
    }

    std::fs::copy(&cli_source, &target)
        .map_err(|error| format!("Failed to copy CLI binary: {}", error))?;

    Ok(target)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn install_cli_binary() -> Result<PathBuf, String> {
    Err("CLI installation is not supported on this platform".to_string())
}

fn find_cli_binary() -> Result<PathBuf, String> {
    if let Ok(exe_path) = std::env::current_exe() {
        let exe_dir = exe_path
            .parent()
            .ok_or("Failed to get executable directory")?;

        #[cfg(target_os = "windows")]
        let cli_name = "termy-cli.exe";
        #[cfg(not(target_os = "windows"))]
        let cli_name = "termy-cli";

        let cli_path = exe_dir.join(cli_name);
        if cli_path.exists() {
            return Ok(cli_path);
        }

        #[cfg(target_os = "macos")]
        {
            if exe_dir.ends_with("Contents/MacOS") {
                let bundle_cli = exe_dir.join("termy-cli");
                if bundle_cli.exists() {
                    return Ok(bundle_cli);
                }
            }
        }
    }

    let possible_paths = fallback_cli_binary_paths();

    for path in &possible_paths {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    Err("CLI binary not found. Make sure to build it with: cargo build -p termy_cli".to_string())
}

fn fallback_cli_binary_paths() -> [PathBuf; 2] {
    let exe_suffix = std::env::consts::EXE_SUFFIX;
    [
        PathBuf::from(format!("./target/release/termy-cli{exe_suffix}")),
        PathBuf::from(format!("./target/debug/termy-cli{exe_suffix}")),
    ]
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
    fn fallback_cli_paths_include_platform_exe_suffix() {
        let paths = fallback_cli_binary_paths();
        let exe_suffix = std::env::consts::EXE_SUFFIX;
        assert_eq!(
            paths[0],
            PathBuf::from(format!("./target/release/termy-cli{exe_suffix}"))
        );
        assert_eq!(
            paths[1],
            PathBuf::from(format!("./target/debug/termy-cli{exe_suffix}"))
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
