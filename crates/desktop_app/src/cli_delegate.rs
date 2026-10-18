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
    )
}

pub(crate) fn delegate_to_cli_or_exit(args: Vec<String>) -> ! {
    // The Windows build uses the GUI subsystem and starts without a console,
    // so reuse the launching shell's console before anything is printed.
    #[cfg(target_os = "windows")]
    let attached_console = windows_console::attach_parent_console();

    let cli_binary = find_termy_cli_binary().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });

    let mut command = Command::new(&cli_binary);
    // A console child of a console-less parent gets a new console window.
    // When every stream is already redirected there is nothing to show in it.
    #[cfg(target_os = "windows")]
    if !attached_console && windows_console::all_std_handles_usable() {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let status = command
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
    termy_cli_binary_next_to(&exe_path)?.ok_or_else(|| {
        "termy-cli binary not found. Build it with: cargo build -p termy_cli".to_string()
    })
}

/// Only the directory of the running executable is searched. Paths relative to
/// the current directory would run whatever binary a checkout happens to ship.
fn termy_cli_binary_next_to(exe_path: &Path) -> Result<Option<PathBuf>, String> {
    let exe_dir = exe_path
        .parent()
        .ok_or_else(|| format!("App path {} has no parent directory", exe_path.display()))?;

    let cli_name = format!("termy-cli{}", std::env::consts::EXE_SUFFIX);
    let sibling = exe_dir.join(&cli_name);
    Ok((is_executable_file(&sibling) && sibling != exe_path).then_some(sibling))
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

#[cfg(target_os = "windows")]
mod windows_console {
    use std::fs::OpenOptions;
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::{GetHandleInformation, HANDLE};
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };

    const STD_HANDLES: [STD_HANDLE; 3] = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];

    /// Attaches to the console of the shell that launched Termy, so
    /// `termy --version` prints where it was typed. Streams the parent already
    /// redirected (pipes, files) are kept; only missing ones are pointed at the
    /// console. Returns false, changing nothing, without a parent console.
    pub(super) fn attach_parent_console() -> bool {
        // SAFETY: AttachConsole takes no pointers; failure only means the
        // parent has no console (Explorer, a shortcut) or we already have one.
        if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_err() {
            return false;
        }
        for kind in STD_HANDLES {
            if has_usable_handle(kind) {
                continue;
            }
            let device = if kind == STD_INPUT_HANDLE {
                "CONIN$"
            } else {
                "CONOUT$"
            };
            // Console APIs used by the CLI (modes, screen buffer info) need
            // both read and write access on these handles.
            let Ok(file) = OpenOptions::new().read(true).write(true).open(device) else {
                continue;
            };
            // The handle now backs this process's standard stream for the
            // rest of its life, so it is intentionally never closed.
            let handle = HANDLE(file.into_raw_handle());
            // SAFETY: `handle` is a valid console handle owned by the process.
            let _ = unsafe { SetStdHandle(kind, handle) };
        }
        true
    }

    pub(super) fn all_std_handles_usable() -> bool {
        STD_HANDLES.into_iter().all(has_usable_handle)
    }

    fn has_usable_handle(kind: STD_HANDLE) -> bool {
        // SAFETY: GetStdHandle only reads the process parameters.
        let Ok(handle) = (unsafe { GetStdHandle(kind) }) else {
            return false;
        };
        if handle.is_invalid() {
            return false;
        }
        // GUI processes can inherit handle values that are not open here.
        let mut flags = 0u32;
        // SAFETY: GetHandleInformation validates `handle` and writes `flags`.
        unsafe { GetHandleInformation(handle, &mut flags) }.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{should_delegate_to_cli, termy_cli_binary_next_to};

    #[test]
    fn cli_binary_is_only_resolved_next_to_the_executable() {
        let app_dir = tempfile::tempdir().expect("app dir");
        let exe_path = app_dir.path().join("termy");
        let cli_name = format!("termy-cli{}", std::env::consts::EXE_SUFFIX);

        assert_eq!(termy_cli_binary_next_to(&exe_path), Ok(None));

        let sibling = app_dir.path().join(&cli_name);
        std::fs::write(&sibling, "").expect("sibling cli");
        assert_eq!(termy_cli_binary_next_to(&exe_path), Ok(Some(sibling)));
    }

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
