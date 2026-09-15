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
/// Windows writes per-user Explorer verbs. Linux installs Nautilus/Nemo/Caja
/// scripts and KDE service menus. macOS registers a Finder service provider
/// for the running app and installs a user Services wrapper for cold launches.
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

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn quoted_windows_path(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('"', "\\\""))
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) fn posix_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn explorer_open_tab_command(executable: &Path) -> String {
    format!(
        "{} --working-directory \"%V\"",
        quoted_windows_path(executable)
    )
}

#[cfg(test)]
mod tests {
    use super::{explorer_open_tab_command, posix_single_quote, quoted_windows_path};
    use std::path::Path;

    #[test]
    fn windows_command_passes_the_selected_folder() {
        let command = explorer_open_tab_command(Path::new(r"C:\Program Files\Termy\termy.exe"));
        assert!(command.contains("--working-directory \"%V\""));
        assert!(command.contains("termy.exe"));
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
