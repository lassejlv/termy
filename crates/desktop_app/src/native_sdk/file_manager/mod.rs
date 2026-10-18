pub const OPEN_TAB_HERE_LABEL: &str = "Open new Termy tab here";

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::path::{Path, PathBuf};

/// Register a file-manager action that opens a Termy tab in the selected folder.
///
/// Windows writes per-user Explorer verbs, unless the installer already
/// registered machine-wide ones or this is a Cargo build. Linux installs
/// Nautilus/Nemo/Caja scripts and KDE service menus. macOS installs a Finder
/// service that resolves the installed app by bundle identifier for both
/// running and cold launches.
pub fn register_open_tab_here(
    executable: &Path,
    on_open_directory: impl Fn(PathBuf) + Send + Sync + 'static,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let _ = on_open_directory;
        windows::register(executable)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = on_open_directory;
        linux::register(executable)
    }
    #[cfg(target_os = "macos")]
    {
        macos::register(executable, on_open_directory)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        let _ = (executable, on_open_directory);
        Ok(())
    }
}

/// Remove the file-manager action written by [`register_open_tab_here`].
///
/// Windows deletes the per-user Explorer verbs; the uninstaller runs this so
/// no menu entry outlives the app. Other platforms have nothing to remove.
pub fn unregister_open_tab_here() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        windows::unregister()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(())
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn quoted_windows_path(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('"', "\\\""))
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn posix_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// `%V` is the folder, or `C:\` for a drive root. A bare `"%V"` would end a
/// root's argument in `\"`, which Windows parses as an escaped quote, so the
/// command appends `\.`; Termy strips it again when parsing the argument.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn explorer_open_tab_command(executable: &Path) -> String {
    format!(
        r#"{} --working-directory "%V\.""#,
        quoted_windows_path(executable)
    )
}

/// The executable an Explorer verb command launches: its first argument,
/// quoted or not.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn explorer_command_executable(command: &str) -> Option<PathBuf> {
    let command = command.trim_start();
    let executable = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => command.split_whitespace().next()?,
    };
    (!executable.is_empty()).then(|| PathBuf::from(executable))
}

/// Cargo output (`target\debug`, `target\<triple>\release`, ...) is not an
/// installed copy, so it must not claim the Explorer menu.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn is_cargo_build_output(executable: &Path) -> bool {
    let lowercase = executable.to_string_lossy().to_ascii_lowercase();
    let components: Vec<&str> = lowercase.split(['\\', '/']).collect();
    // The last component is the executable itself.
    let directories = &components[..components.len().saturating_sub(1)];
    directories.iter().enumerate().any(|(index, component)| {
        *component == "target"
            && directories[index + 1..]
                .iter()
                .any(|profile| matches!(*profile, "debug" | "release" | "profiling"))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        explorer_command_executable, explorer_open_tab_command, is_cargo_build_output,
        posix_single_quote, quoted_windows_path,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn windows_command_passes_the_selected_folder() {
        let command = explorer_open_tab_command(Path::new(r"C:\Program Files\Termy\termy.exe"));
        assert!(command.contains(r#"--working-directory "%V\.""#));
        assert!(command.contains("termy.exe"));
    }

    #[test]
    fn explorer_command_executable_reads_the_first_argument() {
        let command = explorer_open_tab_command(Path::new(r"C:\Program Files\Termy\termy.exe"));
        assert_eq!(
            explorer_command_executable(&command),
            Some(PathBuf::from(r"C:\Program Files\Termy\termy.exe"))
        );
        assert_eq!(
            explorer_command_executable(r#"C:\Termy\termy.exe --working-directory "%V""#),
            Some(PathBuf::from(r"C:\Termy\termy.exe"))
        );
        assert_eq!(explorer_command_executable("  "), None);
        assert_eq!(explorer_command_executable(r#""" --flag"#), None);
    }

    #[test]
    fn cargo_build_output_is_not_an_installed_copy() {
        for path in [
            r"C:\src\termy\target\debug\termy.exe",
            r"C:\src\termy\target\release\termy.exe",
            r"C:\src\termy\Target\x86_64-pc-windows-msvc\Release\termy.exe",
            "/home/me/termy/target/debug/termy",
        ] {
            assert!(is_cargo_build_output(Path::new(path)), "{path}");
        }
        for path in [
            r"C:\Program Files\Termy\termy.exe",
            r"C:\Users\me\AppData\Local\Programs\Termy\termy.exe",
            r"C:\target\termy.exe",
            r"C:\tools\release\termy.exe",
            r"C:\target\release.exe",
        ] {
            assert!(!is_cargo_build_output(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn posix_quotes_paths_with_spaces_and_quotes() {
        assert_eq!(posix_single_quote("/tmp/demo"), "'/tmp/demo'");
        assert_eq!(posix_single_quote("/tmp/it's here"), "'/tmp/it'\\''s here'");
        assert_eq!(
            quoted_windows_path(Path::new(r"C:\Program Files\Termy\termy.exe")),
            r#""C:\Program Files\Termy\termy.exe""#
        );
    }
}
