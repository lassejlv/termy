use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};
pub use termy_core::release_core::ReleaseInfo;
use termy_core::release_core::transport::http_agent;
pub use termy_core::release_core::{UpdateCheck, check_for_updates};

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        asset_name: String,
        url: String,
        checksum_asset_name: Option<String>,
        checksum_url: Option<String>,
        extension: String,
    },
    Downloading {
        version: String,
        downloaded: u64,
        total: u64,
    },
    Downloaded {
        version: String,
        installer_path: PathBuf,
    },
    Installing {
        version: String,
    },
    InstallerLaunched {
        version: String,
    },
    Installed {
        version: String,
    },
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum InstallOutcome {
    Installed,
    InstallerLaunched,
}

/// Prefix of every file the updater writes to its cache directory, so cleanup
/// never touches anything else that lives there.
const CACHE_FILE_PREFIX: &str = "update-";
const PARTIAL_DOWNLOAD_SUFFIX: &str = ".part";

#[cfg(target_os = "windows")]
fn update_cache_dir() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("Termy")
        .join("Cache")
}

#[cfg(not(target_os = "windows"))]
fn update_cache_dir() -> PathBuf {
    std::env::temp_dir().join("termy-updates")
}

pub fn cache_installer_path(version: &str, extension: &str) -> PathBuf {
    let cache_dir = update_cache_dir();
    let _ = std::fs::create_dir_all(&cache_dir);
    cache_dir.join(format!("{CACHE_FILE_PREFIX}{version}.{extension}"))
}

fn is_update_cache_file(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with(CACHE_FILE_PREFIX)
        && [".exe", ".msi", PARTIAL_DOWNLOAD_SUFFIX]
            .iter()
            .any(|extension| name.ends_with(extension))
}

/// Removes installers and partial downloads left behind by earlier updates.
/// A file that is still open (an installer that is running right now) stays
/// until the next cleanup.
pub fn clean_update_cache() {
    let Ok(entries) = std::fs::read_dir(update_cache_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
        if !is_file || !entry.file_name().to_str().is_some_and(is_update_cache_file) {
            continue;
        }
        if let Err(error) = std::fs::remove_file(entry.path()) {
            log::debug!(
                "Could not remove old update file {}: {error}",
                entry.path().display()
            );
        }
    }
}

fn partial_download_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(PARTIAL_DOWNLOAD_SUFFIX);
    PathBuf::from(name)
}

/// Downloads the installer to `<dest>.part` and moves it to `dest` only once
/// its checksum matches, so an interrupted or tampered download never leaves
/// a launchable installer behind.
pub fn download_verified_installer(
    url: &str,
    dest: &Path,
    asset_name: &str,
    checksum_asset_name: Option<&str>,
    checksum_url: Option<&str>,
    progress_tx: flume::Sender<(u64, u64)>,
) -> Result<PathBuf> {
    clean_update_cache();
    let partial = partial_download_path(dest);
    let result = download_installer(url, &partial, progress_tx)
        .and_then(|()| {
            verify_installer_checksum(&partial, asset_name, checksum_asset_name, checksum_url)
        })
        .and_then(|()| {
            std::fs::rename(&partial, dest).context("Failed to move verified installer into place")
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result.map(|()| dest.to_path_buf())
}

fn download_installer(
    url: &str,
    dest: &Path,
    progress_tx: flume::Sender<(u64, u64)>,
) -> Result<()> {
    let response = http_agent()
        .get(url)
        .call()
        .context("Failed to download installer")?;

    let total: u64 = response
        .header("Content-Length")
        .and_then(|h| h.parse().ok())
        .unwrap_or(0);

    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(dest).context("Failed to create installer file")?;
    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 65536]; // 64KiB chunks

    loop {
        let n = reader
            .read(&mut buf)
            .context("Failed to read download stream")?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])?;
        downloaded += n as u64;
        let _ = progress_tx.try_send((downloaded, total));
    }

    // A truncated download fails the checksum check that follows.
    Ok(())
}

pub fn verify_installer_checksum(
    installer_path: &Path,
    asset_name: &str,
    checksum_asset_name: Option<&str>,
    checksum_url: Option<&str>,
) -> Result<()> {
    // Every release publishes checksums.txt (see .github/workflows/release.yml),
    // so a missing checksum means the release is incomplete or tampered with.
    let Some(checksum_url) = checksum_url else {
        anyhow::bail!("Release is missing a checksum asset for {asset_name}");
    };

    let checksum_text = download_checksum_text(checksum_url)?;
    let allow_hash_only = checksum_asset_name.is_some_and(|checksum_asset_name| {
        checksum_asset_name.eq_ignore_ascii_case(&format!("{asset_name}.sha256"))
    });
    let expected = expected_sha256_for_asset(&checksum_text, asset_name, allow_hash_only)
        .with_context(|| format!("Checksum file did not contain an entry for {asset_name}"))?;
    let actual = file_sha256_hex(installer_path)?;

    if !actual.eq_ignore_ascii_case(&expected) {
        anyhow::bail!("Checksum mismatch for {asset_name}: expected {expected}, got {actual}");
    }

    Ok(())
}

fn download_checksum_text(url: &str) -> Result<String> {
    http_agent()
        .get(url)
        .call()
        .context("Failed to download checksum file")?
        .into_string()
        .context("Failed to read checksum file")
}

fn expected_sha256_for_asset(
    checksum_text: &str,
    asset_name: &str,
    allow_hash_only: bool,
) -> Option<String> {
    let mut only_hash = None;
    let mut hash_count = 0usize;

    for line in checksum_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let hashes = line
            .split_whitespace()
            .filter(|part| is_sha256_hex(part.trim()))
            .collect::<Vec<_>>();
        for hash in hashes {
            hash_count += 1;
            only_hash = Some(hash.to_ascii_lowercase());
            if checksum_line_matches_asset(line, asset_name) {
                return Some(hash.to_ascii_lowercase());
            }
        }
    }

    (allow_hash_only && hash_count == 1).then_some(only_hash?)
}

fn checksum_line_matches_asset(line: &str, asset_name: &str) -> bool {
    let normalized_asset = normalize_checksum_name(asset_name);
    line.split_whitespace()
        .skip(1)
        .map(normalize_checksum_name)
        .any(|part| part == normalized_asset)
}

fn normalize_checksum_name(name: &str) -> String {
    name.trim_matches(|ch| ch == '*' || ch == '"' || ch == '\'')
        .trim_start_matches("./")
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn file_sha256_hex(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).context("Failed to open downloaded installer")?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];

    loop {
        let n = file
            .read(&mut buf)
            .context("Failed to read downloaded installer for checksum")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(target_os = "windows")]
pub fn do_install(installer_path: &Path) -> Result<InstallOutcome> {
    let extension = installer_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // Plain "open", never "runas": the installer elevates itself only when the
    // install needs it, so the Termy it relaunches runs as the user, not as
    // administrator.
    match extension.as_str() {
        "msi" => {
            shell_execute_open(
                "msiexec.exe",
                &windows_msi_installer_parameters(installer_path),
            )
            .context("Failed to launch MSI installer")?;
        }
        "exe" => {
            shell_execute_open(
                &installer_path.to_string_lossy(),
                &windows_exe_installer_parameters(current_install_scope()),
            )
            .context("Failed to launch EXE installer")?;
        }
        _ => {
            anyhow::bail!("Unsupported installer format: {extension}");
        }
    }

    Ok(InstallOutcome::InstallerLaunched)
}

/// How the running copy of Termy was installed, so an update installs over it
/// instead of next to it.
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallScope {
    /// Per-user install under `%LOCALAPPDATA%\Programs` (Setup's default).
    CurrentUser,
    /// All-users install under Program Files.
    AllUsers,
}

#[cfg(target_os = "windows")]
impl InstallScope {
    fn setup_switch(self) -> &'static str {
        match self {
            Self::CurrentUser => "/CURRENTUSER",
            Self::AllUsers => "/ALLUSERS",
        }
    }
}

/// `None` for a copy outside both install roots (a development build or a
/// custom directory); Setup then falls back to its previous-install detection.
#[cfg(target_os = "windows")]
fn current_install_scope() -> Option<InstallScope> {
    let canonical = |path: PathBuf| dunce::canonicalize(&path).unwrap_or(path);
    let env_dir = |name: &str| std::env::var_os(name).map(PathBuf::from).map(canonical);

    let exe = canonical(std::env::current_exe().ok()?);
    let program_files = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(env_dir)
        .collect::<Vec<_>>();
    install_scope_for(&exe, env_dir("LOCALAPPDATA").as_deref(), &program_files)
}

#[cfg(target_os = "windows")]
fn install_scope_for(
    exe: &Path,
    local_app_data: Option<&Path>,
    program_files: &[PathBuf],
) -> Option<InstallScope> {
    if local_app_data.is_some_and(|dir| path_is_within(exe, dir)) {
        Some(InstallScope::CurrentUser)
    } else if program_files.iter().any(|dir| path_is_within(exe, dir)) {
        Some(InstallScope::AllUsers)
    } else {
        None
    }
}

/// Case-insensitive, separator-agnostic prefix check, as Windows paths are.
#[cfg(target_os = "windows")]
fn path_is_within(path: &Path, dir: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    let path = normalize(path);
    let dir = normalize(dir);
    !dir.is_empty()
        && path
            .strip_prefix(&dir)
            .is_some_and(|rest| rest.starts_with('\\'))
}

#[cfg(target_os = "windows")]
fn shell_execute_open(file: &str, parameters: &str) -> Result<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    let operation = wide_null("open");
    let file = wide_null(file);
    let parameters = wide_null(parameters);

    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::from_raw(operation.as_ptr()),
            PCWSTR::from_raw(file.as_ptr()),
            PCWSTR::from_raw(parameters.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as isize;
    if code <= 32 {
        anyhow::bail!("ShellExecuteW failed with code {code}");
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "windows")]
fn windows_msi_installer_parameters(installer_path: &Path) -> String {
    format!(
        "/i {} /passive /norestart",
        quote_windows_arg(&installer_path.to_string_lossy())
    )
}

#[cfg(target_os = "windows")]
fn windows_exe_installer_parameters(scope: Option<InstallScope>) -> String {
    [
        "/SILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/CLOSEAPPLICATIONS",
    ]
    .into_iter()
    .chain(scope.map(InstallScope::setup_switch))
    .collect::<Vec<_>>()
    .join(" ")
}

#[cfg(target_os = "windows")]
fn quote_windows_arg(arg: &str) -> String {
    if !arg.is_empty()
        && !arg
            .bytes()
            .any(|byte| byte == b' ' || byte == b'\t' || byte == b'"')
    {
        return arg.to_string();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0usize;

    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', (backslashes * 2) + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
    }

    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(not(target_os = "windows"))]
pub fn do_install(_installer_path: &Path) -> Result<InstallOutcome> {
    anyhow::bail!("Auto-install is only supported on Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_parser_finds_matching_manifest_entry() {
        let checksums = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  Termy-v1.2.3-macos-arm64.dmg
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb *Termy-v1.2.3-windows-x64-Setup.exe
";

        assert_eq!(
            expected_sha256_for_asset(checksums, "Termy-v1.2.3-windows-x64-Setup.exe", false),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string())
        );
    }

    #[test]
    fn checksum_parser_accepts_single_hash_asset_file_when_allowed() {
        assert_eq!(
            expected_sha256_for_asset(
                "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC",
                "Termy-v1.2.3-windows-x64-Setup.exe",
                true,
            ),
            Some("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string())
        );
    }

    #[test]
    fn checksum_parser_rejects_hash_only_manifest_without_asset_name() {
        assert_eq!(
            expected_sha256_for_asset(
                "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC",
                "Termy-v1.2.3-windows-x64-Setup.exe",
                false,
            ),
            None
        );
    }

    #[test]
    fn checksum_parser_rejects_ambiguous_hash_only_asset_file() {
        let checksums = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
";

        assert_eq!(
            expected_sha256_for_asset(checksums, "Termy-v1.2.3-windows-x64-Setup.exe", true),
            None
        );
    }

    #[test]
    fn cache_cleanup_matches_only_updater_files() {
        for name in [
            "update-1.2.3.exe",
            "update-1.2.3.msi",
            "update-1.2.3.exe.part",
            "Update-1.2.3.EXE",
        ] {
            assert!(is_update_cache_file(name), "{name}");
        }
        for name in [
            "termy.exe",
            "update-notes.txt",
            "settings.json",
            "update-1.2.3",
        ] {
            assert!(!is_update_cache_file(name), "{name}");
        }
    }

    #[test]
    fn partial_download_sits_next_to_the_installer() {
        assert_eq!(
            partial_download_path(Path::new("cache/update-1.2.3.exe")),
            PathBuf::from("cache/update-1.2.3.exe.part")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_exe_installer_uses_silent_handoff_flags() {
        assert_eq!(
            windows_exe_installer_parameters(None),
            "/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS"
        );
        assert_eq!(
            windows_exe_installer_parameters(Some(InstallScope::CurrentUser)),
            "/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /CURRENTUSER"
        );
        assert_eq!(
            windows_exe_installer_parameters(Some(InstallScope::AllUsers)),
            "/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /ALLUSERS"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn install_scope_follows_the_running_copy() {
        let local_app_data = Path::new(r"C:\Users\me\AppData\Local");
        let program_files = [
            PathBuf::from(r"C:\Program Files"),
            PathBuf::from(r"C:\Program Files (x86)"),
        ];
        let scope =
            |exe: &str| install_scope_for(Path::new(exe), Some(local_app_data), &program_files);

        assert_eq!(
            scope(r"C:\Users\me\AppData\Local\Programs\Termy\termy.exe"),
            Some(InstallScope::CurrentUser)
        );
        assert_eq!(
            scope("c:/users/ME/appdata/local/Programs/Termy/termy.exe"),
            Some(InstallScope::CurrentUser)
        );
        assert_eq!(
            scope(r"C:\Program Files\Termy\termy.exe"),
            Some(InstallScope::AllUsers)
        );
        // A sibling directory that only shares a name prefix is not inside it.
        assert_eq!(scope(r"C:\Program Files Extra\Termy\termy.exe"), None);
        assert_eq!(scope(r"D:\dev\termy\target\release\termy.exe"), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_msi_installer_quotes_installer_path() {
        let params = windows_msi_installer_parameters(Path::new(
            "C:\\Users\\me\\Downloads\\Termy Setup.msi",
        ));
        assert_eq!(
            params,
            "/i \"C:\\Users\\me\\Downloads\\Termy Setup.msi\" /passive /norestart"
        );
    }
}
