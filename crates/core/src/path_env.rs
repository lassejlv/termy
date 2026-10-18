use std::{env, ffi::OsStr, path::PathBuf};

#[cfg(target_os = "windows")]
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
};

#[cfg(not(target_os = "windows"))]
const DEFAULT_SYSTEM_PATH_ENTRIES: [&str; 4] = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];
#[cfg(not(target_os = "windows"))]
const EXTRA_PATH_ENTRIES: [&str; 4] = [
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
];

#[cfg(target_os = "windows")]
const WINDOWS_ENV_PATH_REGISTRY_KEYS: [(&str, &str); 2] = [
    (
        "HKLM",
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
    ),
    ("HKCU", "Environment"),
];

fn push_unique_path(entries: &mut Vec<PathBuf>, entry: PathBuf) {
    if entry.as_os_str().is_empty() {
        return;
    }

    #[cfg(target_os = "windows")]
    let duplicate = {
        let candidate = windows_path_identity(&entry);
        entries
            .iter()
            .any(|existing| windows_path_identity(existing) == candidate)
    };

    #[cfg(not(target_os = "windows"))]
    let duplicate = entries.iter().any(|existing| existing == &entry);

    if !duplicate {
        entries.push(entry);
    }
}

/// How Windows compares directories: case-insensitively, with either slash,
/// and with or without a trailing separator (`C:\Tools\` is `c:/tools`).
#[cfg(target_os = "windows")]
fn windows_path_identity(entry: &std::path::Path) -> String {
    let text = entry.to_string_lossy().replace('/', "\\");
    let trimmed = text.trim_end_matches('\\');
    // Keep a drive root's separator: `C:` alone means that drive's current
    // directory, not its root.
    let trimmed = if trimmed.len() == 2 && trimmed.ends_with(':') {
        &text[..3.min(text.len())]
    } else {
        trimmed
    };
    trimmed.to_lowercase()
}

fn push_split_paths(entries: &mut Vec<PathBuf>, path: Option<&OsStr>) {
    if let Some(path) = path {
        for entry in env::split_paths(path) {
            push_unique_path(entries, entry);
        }
    }
}

/// Expand `%NAME%` references in a `REG_EXPAND_SZ` value. Works on UTF-16
/// code units so that values which are not valid Unicode survive unchanged.
#[cfg(target_os = "windows")]
fn expand_windows_env_vars(value: &OsStr) -> OsString {
    const PERCENT: u16 = b'%' as u16;
    let input = value.encode_wide().collect::<Vec<_>>();
    let mut output = Vec::with_capacity(input.len());
    let mut rest = input.as_slice();

    while let Some(start) = rest.iter().position(|unit| *unit == PERCENT) {
        output.extend_from_slice(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.iter().position(|unit| *unit == PERCENT) else {
            output.push(PERCENT);
            output.extend_from_slice(after);
            rest = &[];
            break;
        };

        let name = &after[..end];
        // `=` and NUL cannot appear in a variable name; looking them up
        // would be an invalid environment query.
        let lookup = (!name.is_empty() && !name.iter().any(|unit| matches!(*unit, 0 | 0x3D)))
            .then(|| env::var_os(OsString::from_wide(name)))
            .flatten();
        if name.is_empty() {
            output.extend([PERCENT, PERCENT]);
        } else if let Some(value) = lookup {
            output.extend(value.encode_wide());
        } else {
            output.push(PERCENT);
            output.extend_from_slice(name);
            output.push(PERCENT);
        }
        rest = &after[end + 1..];
    }

    output.extend_from_slice(rest);
    OsString::from_wide(&output)
}

#[cfg(target_os = "windows")]
fn registry_path_value(root: &str, subkey: &str) -> Option<OsString> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let root = match root {
        "HKLM" => RegKey::predef(HKEY_LOCAL_MACHINE),
        "HKCU" => RegKey::predef(HKEY_CURRENT_USER),
        _ => return None,
    };
    let key = root.open_subkey(subkey).ok()?;
    key.get_value::<OsString, _>("Path")
        .or_else(|_| key.get_value::<OsString, _>("PATH"))
        .ok()
}

#[cfg(target_os = "windows")]
fn registry_path_values() -> Vec<OsString> {
    WINDOWS_ENV_PATH_REGISTRY_KEYS
        .into_iter()
        .filter_map(|(root, subkey)| registry_path_value(root, subkey))
        .collect()
}

#[cfg(target_os = "windows")]
fn windows_system_path_entries() -> Vec<PathBuf> {
    let system_root = env::var_os("SystemRoot")
        .or_else(|| env::var_os("WINDIR"))
        .unwrap_or_else(|| OsString::from(r"C:\Windows"));
    let system_root = PathBuf::from(system_root);

    [
        system_root.join("System32"),
        system_root.clone(),
        system_root.join(r"System32\Wbem"),
        system_root.join(r"System32\WindowsPowerShell\v1.0"),
        system_root.join(r"System32\OpenSSH"),
    ]
    .into()
}

#[cfg(target_os = "windows")]
fn push_if_env_dir(entries: &mut Vec<PathBuf>, var: &str, relative: &str) {
    if let Some(base) = env::var_os(var).filter(|value| !value.is_empty()) {
        push_unique_path(entries, PathBuf::from(base).join(relative));
    }
}

#[cfg(target_os = "windows")]
fn push_if_env_path(entries: &mut Vec<PathBuf>, var: &str) {
    if let Some(path) = env::var_os(var).filter(|value| !value.is_empty()) {
        push_unique_path(entries, PathBuf::from(path));
    }
}

#[cfg(target_os = "windows")]
fn windows_tool_path_entries() -> Vec<PathBuf> {
    let mut entries = Vec::new();

    push_if_env_dir(&mut entries, "SCOOP", "shims");
    push_if_env_dir(&mut entries, "SCOOP_GLOBAL", "shims");
    push_if_env_dir(&mut entries, "USERPROFILE", r"scoop\shims");
    push_if_env_dir(&mut entries, "CARGO_HOME", "bin");
    push_if_env_dir(&mut entries, "USERPROFILE", r".cargo\bin");
    push_if_env_dir(&mut entries, "BUN_INSTALL", "bin");
    push_if_env_dir(&mut entries, "USERPROFILE", r".bun\bin");
    push_if_env_dir(&mut entries, "APPDATA", "npm");
    push_if_env_path(&mut entries, "PNPM_HOME");
    push_if_env_dir(&mut entries, "LOCALAPPDATA", "pnpm");
    push_if_env_dir(&mut entries, "LOCALAPPDATA", r"Microsoft\WindowsApps");

    entries
}

#[cfg(target_os = "windows")]
fn normalized_windows_path_env(
    path: Option<&OsStr>,
    registry_paths: impl IntoIterator<Item = OsString>,
    tool_paths: impl IntoIterator<Item = PathBuf>,
) -> Option<String> {
    let mut path_entries = Vec::new();
    push_split_paths(&mut path_entries, path);

    for entry in windows_system_path_entries() {
        push_unique_path(&mut path_entries, entry);
    }

    for value in registry_paths {
        let expanded = expand_windows_env_vars(&value);
        push_split_paths(&mut path_entries, Some(expanded.as_os_str()));
    }

    for entry in tool_paths {
        push_unique_path(&mut path_entries, entry);
    }

    env::join_paths(path_entries.iter())
        .ok()
        .map(|joined| joined.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "windows"))]
pub fn normalized_path_env(path: Option<&OsStr>) -> Option<String> {
    let mut path_entries = Vec::new();
    push_split_paths(&mut path_entries, path);

    if path_entries.is_empty() {
        for entry in DEFAULT_SYSTEM_PATH_ENTRIES {
            push_unique_path(&mut path_entries, PathBuf::from(entry));
        }
    }

    for entry in EXTRA_PATH_ENTRIES {
        push_unique_path(&mut path_entries, PathBuf::from(entry));
    }

    env::join_paths(path_entries.iter())
        .ok()
        .map(|joined| joined.to_string_lossy().into_owned())
}

#[cfg(target_os = "windows")]
pub fn normalized_path_env(path: Option<&OsStr>) -> Option<String> {
    normalized_windows_path_env(path, registry_path_values(), windows_tool_path_entries())
}

#[cfg(all(test, unix))]
mod tests {
    use super::normalized_path_env;
    use std::{ffi::OsString, path::PathBuf};

    #[test]
    fn normalized_path_env_starts_from_default_system_path_when_missing() {
        let path = normalized_path_env(None).expect("normalized path");
        let parsed = std::env::split_paths(&OsString::from(path)).collect::<Vec<_>>();
        assert!(parsed.contains(&PathBuf::from("/usr/bin")));
        assert!(parsed.contains(&PathBuf::from("/bin")));
        assert!(parsed.contains(&PathBuf::from("/usr/sbin")));
        assert!(parsed.contains(&PathBuf::from("/sbin")));
        assert!(parsed.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(parsed.contains(&PathBuf::from("/opt/homebrew/sbin")));
        assert!(parsed.contains(&PathBuf::from("/usr/local/bin")));
        assert!(parsed.contains(&PathBuf::from("/usr/local/sbin")));
    }

    #[test]
    fn normalized_path_env_treats_empty_path_as_missing() {
        let raw = OsString::from("");
        let path = normalized_path_env(Some(raw.as_os_str())).expect("normalized path");
        let parsed = std::env::split_paths(&OsString::from(path)).collect::<Vec<_>>();
        assert!(parsed.contains(&PathBuf::from("/usr/bin")));
        assert!(parsed.contains(&PathBuf::from("/bin")));
        assert!(parsed.contains(&PathBuf::from("/usr/sbin")));
        assert!(parsed.contains(&PathBuf::from("/sbin")));
    }

    #[test]
    fn normalized_path_env_appends_missing_entries_without_duplication() {
        let raw = OsString::from("/opt/homebrew/bin:/usr/bin:/bin");
        let path = normalized_path_env(Some(raw.as_os_str())).expect("normalized path");
        let parsed = std::env::split_paths(&OsString::from(path)).collect::<Vec<_>>();
        let homebrew_bin = PathBuf::from("/opt/homebrew/bin");
        assert_eq!(
            parsed
                .iter()
                .filter(|entry| **entry == homebrew_bin)
                .count(),
            1
        );
        assert!(parsed.contains(&PathBuf::from("/opt/homebrew/sbin")));
        assert!(parsed.contains(&PathBuf::from("/usr/local/bin")));
        assert!(parsed.contains(&PathBuf::from("/usr/local/sbin")));
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::{expand_windows_env_vars, normalized_windows_path_env};
    use std::{ffi::OsString, path::PathBuf};

    #[test]
    fn normalized_path_env_merges_registry_and_tool_paths() {
        let raw = OsString::from(r"C:\Windows\System32");
        let path = normalized_windows_path_env(
            Some(raw.as_os_str()),
            [OsString::from(r"C:\ProgramData\scoop\shims;C:\Tools\bin")],
            [
                PathBuf::from(r"C:\Users\me\scoop\shims"),
                PathBuf::from(r"C:\Users\me\.cargo\bin"),
                PathBuf::from(r"C:\Users\me\.bun\bin"),
            ],
        )
        .expect("normalized path");
        let parsed = std::env::split_paths(&OsString::from(path)).collect::<Vec<_>>();

        assert!(parsed.contains(&PathBuf::from(r"C:\Windows\System32")));
        assert!(parsed.contains(&PathBuf::from(r"C:\ProgramData\scoop\shims")));
        assert!(parsed.contains(&PathBuf::from(r"C:\Tools\bin")));
        assert!(parsed.contains(&PathBuf::from(r"C:\Users\me\scoop\shims")));
        assert!(parsed.contains(&PathBuf::from(r"C:\Users\me\.cargo\bin")));
        assert!(parsed.contains(&PathBuf::from(r"C:\Users\me\.bun\bin")));
    }

    #[test]
    fn normalized_path_env_deduplicates_windows_paths_case_insensitively() {
        let raw = OsString::from(r"C:\Tools;c:\tools");
        let path = normalized_windows_path_env(
            Some(raw.as_os_str()),
            [OsString::from(r"C:\TOOLS")],
            [PathBuf::from(r"c:\TOOLS")],
        )
        .expect("normalized path");
        let parsed = std::env::split_paths(&OsString::from(path)).collect::<Vec<_>>();

        assert_eq!(
            parsed
                .iter()
                .filter(|entry| entry.to_string_lossy().eq_ignore_ascii_case(r"C:\Tools"))
                .count(),
            1
        );
    }

    #[test]
    fn normalized_path_env_ignores_trailing_separators_and_slashes() {
        let raw = OsString::from(r"C:\Tools\;c:/tools;C:\;c:");
        let path = normalized_windows_path_env(
            Some(raw.as_os_str()),
            Vec::<OsString>::new(),
            Vec::<PathBuf>::new(),
        )
        .expect("normalized path");
        let parsed = std::env::split_paths(&OsString::from(path)).collect::<Vec<_>>();

        assert_eq!(
            parsed
                .iter()
                .filter(|entry| {
                    entry
                        .to_string_lossy()
                        .trim_end_matches('\\')
                        .eq_ignore_ascii_case(r"C:\Tools")
                })
                .count(),
            1
        );
        // A drive root and a bare drive (its current directory) differ.
        assert!(parsed.contains(&PathBuf::from(r"C:\")));
        assert!(parsed.contains(&PathBuf::from("c:")));
    }

    #[test]
    fn expand_windows_env_vars_keeps_non_unicode_code_units() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let raw = OsString::from_wide(&[0xD800, b'\\' as u16, b'%' as u16, b'%' as u16]);
        let expanded = expand_windows_env_vars(raw.as_os_str());
        assert_eq!(
            expanded.encode_wide().collect::<Vec<_>>(),
            [0xD800, b'\\' as u16, b'%' as u16, b'%' as u16]
        );
    }

    #[test]
    fn expand_windows_env_vars_preserves_unknown_variables() {
        let expanded =
            expand_windows_env_vars(OsString::from(r"%TERMY_MISSING_VAR%\bin").as_os_str());
        assert_eq!(expanded, OsString::from(r"%TERMY_MISSING_VAR%\bin"));
    }
}
