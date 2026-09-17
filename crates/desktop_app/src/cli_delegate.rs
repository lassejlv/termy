use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Returns true when argv should be handled by `termy-cli` instead of launching GPUI.
pub(crate) fn should_delegate_to_cli(args: &[String]) -> bool {
    let mut index = 0;

    while index < args.len() {
        let arg = args[index].as_str();
        if arg.starts_with("termy://") {
            index += 1;
            continue;
        }
        if arg == "--working-directory" {
            return false;
        }
        if arg.starts_with("--working-directory=") {
            return false;
        }
        if arg == "--" {
            return false;
        }
        if is_cli_entrypoint(arg) {
            return true;
        }
        if arg.starts_with('-') {
            return false;
        }
        // Positional path opens the GUI in the requested working directory.
        return false;
    }

    false
}

fn is_cli_entrypoint(arg: &str) -> bool {
    matches!(
        arg,
        "-h" | "--help"
            | "-V"
            | "--version"
            | "-help"
            | "-version"
            | "plugin"
            | "mux"
            | "-list-fonts"
            | "-list-keybinds"
            | "-list-themes"
            | "-list-colors"
            | "-list-actions"
            | "-edit-config"
            | "-show-config"
            | "-validate-config"
            | "-prettify-config"
            | "-tui"
            | "-update"
            | "-export-theme"
            | "-validate-theme-repo"
    )
}

pub(crate) fn delegate_to_cli_or_exit(args: Vec<String>) -> ! {
    let cli_binary = find_termy_cli_binary().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });

    let status = Command::new(&cli_binary)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .unwrap_or_else(|error| {
            eprintln!("Failed to run {}: {error}", cli_binary.display());
            std::process::exit(1);
        });

    std::process::exit(status.code().unwrap_or(1));
}

fn find_termy_cli_binary() -> Result<PathBuf, String> {
    let exe_path =
        std::env::current_exe().map_err(|error| format!("Failed to resolve app path: {error}"))?;
    let exe_dir = exe_path
        .parent()
        .ok_or_else(|| format!("App path {} has no parent directory", exe_path.display()))?;

    let cli_name = format!("termy-cli{}", std::env::consts::EXE_SUFFIX);
    let sibling = exe_dir.join(&cli_name);
    if is_executable_file(&sibling) && sibling != exe_path {
        return Ok(sibling);
    }

    for candidate in fallback_termy_cli_binary_paths(&cli_name) {
        if is_executable_file(&candidate) && candidate != exe_path {
            return Ok(candidate);
        }
    }

    Err("termy-cli binary not found. Build it with: cargo build -p termy_cli".to_string())
}

fn fallback_termy_cli_binary_paths(cli_name: &str) -> [PathBuf; 2] {
    [
        PathBuf::from("target/debug").join(cli_name),
        PathBuf::from("target/release").join(cli_name),
    ]
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

#[cfg(test)]
mod tests {
    use super::should_delegate_to_cli;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn delegates_version_flag() {
        assert!(should_delegate_to_cli(&args(&["-version"])));
        assert!(should_delegate_to_cli(&args(&["--version"])));
        assert!(should_delegate_to_cli(&args(&["-V"])));
    }

    #[test]
    fn delegates_help_and_config_commands() {
        assert!(should_delegate_to_cli(&args(&["-h"])));
        assert!(should_delegate_to_cli(&args(&["-help"])));
        assert!(should_delegate_to_cli(&args(&["--help"])));
        assert!(should_delegate_to_cli(&args(&["-show-config"])));
        assert!(should_delegate_to_cli(&args(&["-list-keybinds"])));
    }

    #[test]
    fn delegates_all_cli_commands() {
        for command in [
            "plugin",
            "mux",
            "-list-fonts",
            "-list-keybinds",
            "-list-themes",
            "-list-colors",
            "-list-actions",
            "-edit-config",
            "-validate-config",
            "-prettify-config",
            "-tui",
            "-update",
            "-export-theme",
            "-validate-theme-repo",
        ] {
            assert!(
                should_delegate_to_cli(&args(&[command])),
                "expected {command} to delegate"
            );
        }
    }

    #[test]
    fn does_not_delegate_unknown_desktop_flags() {
        assert!(!should_delegate_to_cli(&args(&["-psn_0_12345"])));
        assert!(!should_delegate_to_cli(&args(&["--gpui-runtime-flag"])));
    }

    #[test]
    fn does_not_delegate_working_directory_flags() {
        assert!(!should_delegate_to_cli(&args(&[
            "--working-directory",
            "/tmp"
        ])));
        assert!(!should_delegate_to_cli(&args(&[
            "--working-directory=/tmp"
        ])));
    }

    #[test]
    fn does_not_delegate_positional_working_directory() {
        assert!(!should_delegate_to_cli(&args(&["/tmp/project"])));
        assert!(!should_delegate_to_cli(&args(&["."])));
    }

    #[test]
    fn does_not_delegate_deeplink_only_startup() {
        assert!(!should_delegate_to_cli(&args(&["termy://settings"])));
    }

    #[test]
    fn delegates_when_cli_flag_follows_deeplink() {
        assert!(should_delegate_to_cli(&args(&[
            "termy://settings",
            "-version"
        ])));
    }

    #[test]
    fn does_not_delegate_double_dash_path() {
        assert!(!should_delegate_to_cli(&args(&["--", "/tmp/project"])));
    }

    #[test]
    fn does_not_delegate_empty_args() {
        assert!(!should_delegate_to_cli(&[]));
    }
}
