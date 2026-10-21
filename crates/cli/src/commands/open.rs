use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn run(path: Option<PathBuf>, new_window: bool, new_tab: bool) {
    match launch_termy(path, new_window, new_tab) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

fn launch_termy(path: Option<PathBuf>, new_window: bool, new_tab: bool) -> Result<(), String> {
    let working_dir = path.as_deref().map(resolve_working_dir).transpose()?;
    let app_binary = find_termy_app_binary()?;

    let mut command = Command::new(&app_binary);
    if new_window {
        command.arg("--new-window");
    } else if new_tab {
        command.arg("--new-tab");
    }
    if let Some(working_dir) = working_dir {
        command.arg("--working-directory").arg(working_dir);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Failed to launch {}: {error}", app_binary.display()))?;

    Ok(())
}

fn resolve_working_dir(path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("Failed to resolve current directory: {error}"))?
            .join(path)
    };

    // dunce keeps Windows paths in their plain `C:\...` form; the app rejects
    // the `\\?\` verbatim paths std's canonicalize returns.
    let path = dunce::canonicalize(&path)
        .map_err(|error| format!("Failed to resolve {}: {error}", path.display()))?;

    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }

    Ok(path)
}

fn find_termy_app_binary() -> Result<PathBuf, String> {
    let reported_exe_path =
        std::env::current_exe().map_err(|error| format!("Failed to resolve CLI path: {error}"))?;
    find_app_for_cli(reported_exe_path, sibling_app_binary_names())
}

fn find_app_for_cli(reported_exe_path: PathBuf, sibling_names: &[&str]) -> Result<PathBuf, String> {
    find_app_for_cli_with_fallbacks(reported_exe_path, sibling_names, installed_app_candidates)
}

fn find_app_for_cli_with_fallbacks(
    reported_exe_path: PathBuf,
    sibling_names: &[&str],
    fallbacks: impl FnOnce(&Path) -> Vec<PathBuf>,
) -> Result<PathBuf, String> {
    let exe_path = resolve_executable_path(reported_exe_path);
    let exe_dir = exe_path
        .parent()
        .ok_or_else(|| format!("CLI path {} has no parent directory", exe_path.display()))?;

    // Debian/RPM put the CLI in lib/termy and the launcher in bin. Prefer
    // that launcher so its X11/Wayland environment setup is preserved.
    if exe_dir.ends_with("lib/termy") {
        let launcher = resolve_executable_path(exe_dir.join("../../bin/termy"));
        if is_executable_file(&launcher) && !is_same_file(&launcher, &exe_path) {
            return Ok(launcher);
        }
    }

    for sibling_name in sibling_names {
        let sibling = exe_dir.join(sibling_name);
        if is_executable_file(&sibling) && !is_same_file(&sibling, &exe_path) {
            return Ok(sibling);
        }
    }

    // A copied CLI (Windows Install CLI puts `termy.exe` in
    // %LOCALAPPDATA%\Termy\bin) has no app beside it; the sibling check above
    // finds the CLI itself there, so ask where the app was installed.
    for candidate in fallbacks(exe_dir) {
        if is_executable_file(&candidate) && !is_same_file(&candidate, &exe_path) {
            return Ok(resolve_executable_path(candidate));
        }
    }

    // Never look relative to the current directory: that would let an
    // untrusted checkout's `target/*/termy` run when the CLI is used inside it.
    Err(APP_NOT_FOUND_MESSAGE.to_string())
}

#[cfg(target_os = "windows")]
const APP_NOT_FOUND_MESSAGE: &str = "Termy app binary not found. Reinstall Termy or run Install CLI from the app again (from source: cargo build -p termy)";

#[cfg(not(target_os = "windows"))]
const APP_NOT_FOUND_MESSAGE: &str =
    "Termy app binary not found. Build it with: cargo build -p termy";

#[cfg(target_os = "windows")]
fn installed_app_candidates(cli_dir: &Path) -> Vec<PathBuf> {
    termy_core::cli_install_core::windows_app_binary_candidates(cli_dir)
}

#[cfg(not(target_os = "windows"))]
fn installed_app_candidates(_cli_dir: &Path) -> Vec<PathBuf> {
    Vec::new()
}

fn resolve_executable_path(path: PathBuf) -> PathBuf {
    dunce::canonicalize(&path).unwrap_or(path)
}

/// Compares resolved paths so the CLI never launches itself, whatever
/// spelling (case, `..`, symlinks) a candidate uses.
fn is_same_file(a: &Path, b: &Path) -> bool {
    let a = resolve_executable_path(a.to_path_buf());
    let b = resolve_executable_path(b.to_path_buf());

    #[cfg(target_os = "windows")]
    {
        a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
    }

    #[cfg(not(target_os = "windows"))]
    {
        a == b
    }
}

fn sibling_app_binary_names() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    {
        &["Termy", "termy"]
    }

    #[cfg(target_os = "windows")]
    {
        &["termy.exe"]
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        LINUX_APP_BINARY_NAMES
    }
}

#[cfg(any(not(any(target_os = "macos", target_os = "windows")), all(test, unix)))]
const LINUX_APP_BINARY_NAMES: &[&str] = &["termy", "termy-bin"];

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

#[cfg(test)]
mod tests {
    use super::resolve_working_dir;

    #[cfg(unix)]
    #[test]
    fn packaged_linux_cli_finds_gui_after_resolving_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("usr/lib/termy");
        let bin = temp.path().join("usr/bin");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bundle.join("termy-cli"), b"cli").unwrap();
        std::fs::write(bundle.join("termy-bin"), b"gui").unwrap();
        std::os::unix::fs::symlink("../lib/termy/termy-cli", bin.join("termy-cli")).unwrap();
        assert_eq!(
            super::find_app_for_cli(bin.join("termy-cli"), super::LINUX_APP_BINARY_NAMES).unwrap(),
            bundle.join("termy-bin").canonicalize().unwrap()
        );
        std::fs::write(bin.join("termy"), b"system launcher").unwrap();
        assert_eq!(
            super::find_app_for_cli(bin.join("termy-cli"), super::LINUX_APP_BINARY_NAMES).unwrap(),
            bin.join("termy").canonicalize().unwrap()
        );
        std::fs::remove_file(bin.join("termy")).unwrap();
        // Tarball installations should keep using the launcher when available.
        std::fs::write(bundle.join("termy"), b"launcher").unwrap();
        assert_eq!(
            super::find_app_for_cli(bin.join("termy-cli"), super::LINUX_APP_BINARY_NAMES).unwrap(),
            bundle.join("termy").canonicalize().unwrap()
        );
    }

    #[test]
    fn open_resolves_existing_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let resolved = resolve_working_dir(temp.path()).expect("directory should resolve");
        assert_eq!(resolved, dunce::canonicalize(temp.path()).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn open_resolves_relative_directory_without_verbatim_prefix() {
        let resolved = resolve_working_dir(std::path::Path::new(".")).expect("cwd should resolve");
        assert!(
            !resolved.as_os_str().to_string_lossy().starts_with(r"\\?\"),
            "{}",
            resolved.display()
        );
    }

    #[test]
    fn copied_cli_skips_itself_and_uses_installed_app() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        let app_dir = temp.path().join("Termy");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&app_dir).unwrap();
        // The installed CLI copy has the app's file name.
        let cli = bin.join("termy.exe");
        let app = app_dir.join("termy.exe");
        std::fs::write(&cli, b"cli").unwrap();
        std::fs::write(&app, b"gui").unwrap();

        let fallback_cli = cli.clone();
        let fallback_app = app.clone();
        let found = super::find_app_for_cli_with_fallbacks(cli, &["termy.exe"], move |_| {
            vec![fallback_cli, bin.join("missing.exe"), fallback_app]
        })
        .unwrap();
        assert_eq!(found, dunce::canonicalize(&app).unwrap());
    }

    #[test]
    fn copied_cli_without_installed_app_reports_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let cli = temp.path().join("termy.exe");
        std::fs::write(&cli, b"cli").unwrap();
        let fallback_cli = cli.clone();
        let error = super::find_app_for_cli_with_fallbacks(cli, &["termy.exe"], move |_| {
            vec![fallback_cli]
        })
        .unwrap_err();
        assert!(error.contains("Termy app binary not found"));
    }

    #[test]
    fn open_rejects_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = temp.path().join("file.txt");
        std::fs::write(&file, "content").expect("write file");
        let error = resolve_working_dir(&file).expect_err("file should be rejected");
        assert!(error.contains("is not a directory"));
    }

    #[test]
    fn bundled_native_app_binary_is_a_sibling_candidate() {
        #[cfg(target_os = "macos")]
        assert_eq!(super::sibling_app_binary_names().first(), Some(&"Termy"));
    }

    #[cfg(unix)]
    #[test]
    fn installed_cli_symlink_resolves_back_to_bundle() {
        use super::resolve_executable_path;
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let bundled = temp.path().join("Termy.app/Contents/MacOS/termy-cli");
        std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        std::fs::write(&bundled, b"cli").unwrap();
        let installed = temp.path().join("home/.local/bin/termy");
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        symlink(&bundled, &installed).unwrap();

        assert_eq!(
            resolve_executable_path(installed),
            bundled.canonicalize().unwrap()
        );
    }
}
