use super::{OPEN_TAB_HERE_LABEL, posix_single_quote};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(super) fn register(executable: &Path) -> Result<(), String> {
    let exe = executable
        .canonicalize()
        .unwrap_or_else(|_| executable.to_path_buf());
    let quoted_exe = posix_single_quote(&exe.to_string_lossy());
    let home = dirs_home()?;

    write_executable_script(
        &home
            .join(".local/share/nautilus/scripts")
            .join(OPEN_TAB_HERE_LABEL),
        &nautilus_script(&quoted_exe),
    )?;
    write_executable_script(
        &home
            .join(".local/share/nemo/scripts")
            .join(OPEN_TAB_HERE_LABEL),
        &nautilus_script(&quoted_exe),
    )?;
    write_executable_script(
        &home
            .join(".local/share/caja/scripts")
            .join(OPEN_TAB_HERE_LABEL),
        &nautilus_script(&quoted_exe),
    )?;
    write_text_if_unmanaged(
        &home.join(".local/share/nemo/actions/termy-open-tab.nemo_action"),
        Path::new("/usr/share/nemo/actions/termy-open-tab.nemo_action"),
        &nemo_action(&quoted_exe),
    )?;
    write_text_if_unmanaged(
        &home.join(".local/share/kio/servicemenus/termy-open-tab.desktop"),
        Path::new("/usr/share/kio/servicemenus/termy-open-tab.desktop"),
        &kde_servicemenu(&quoted_exe),
    )?;
    write_text_if_unmanaged(
        &home.join(".local/share/kservices5/ServiceMenus/termy-open-tab.desktop"),
        Path::new("/usr/share/kservices5/ServiceMenus/termy-open-tab.desktop"),
        &kde_servicemenu(&quoted_exe),
    )?;
    Ok(())
}

pub(crate) fn nautilus_script(quoted_exe: &str) -> String {
    format!(
        "#!/usr/bin/env bash\n\
         set -euo pipefail\n\
         paths=()\n\
         if (($# > 0)); then\n\
         \tpaths=(\"$@\")\n\
         elif [[ -n \"${{NAUTILUS_SCRIPT_SELECTED_FILE_PATHS:-}}\" ]]; then\n\
         \twhile IFS= read -r line; do\n\
         \t\t[[ -n \"$line\" ]] && paths+=(\"$line\")\n\
         \tdone <<< \"$NAUTILUS_SCRIPT_SELECTED_FILE_PATHS\"\n\
         elif [[ -n \"${{NEMO_SCRIPT_SELECTED_FILE_PATHS:-}}\" ]]; then\n\
         \twhile IFS= read -r line; do\n\
         \t\t[[ -n \"$line\" ]] && paths+=(\"$line\")\n\
         \tdone <<< \"$NEMO_SCRIPT_SELECTED_FILE_PATHS\"\n\
         fi\n\
         target=\"${{paths[0]:-}}\"\n\
         if [[ -z \"$target\" ]]; then\n\
         \techo \"No folder selected\" >&2\n\
         \texit 1\n\
         fi\n\
         if [[ -f \"$target\" ]]; then\n\
         \ttarget=\"$(dirname \"$target\")\"\n\
         fi\n\
         exec {quoted_exe} --new-tab --working-directory \"$target\"\n"
    )
}

pub(crate) fn nemo_action(quoted_exe: &str) -> String {
    format!(
        "[Nemo Action]\n\
         Name={OPEN_TAB_HERE_LABEL}\n\
         Comment=Open a new Termy tab in this folder\n\
         Exec={quoted_exe} --new-tab --working-directory %F\n\
         Icon-Name=termy\n\
         Selection=any\n\
         Extensions=dir;\n\
         Quote=double\n"
    )
}

pub(crate) fn kde_servicemenu(quoted_exe: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Service\n\
         X-KDE-ServiceTypes=KonqPopupMenu/Plugin\n\
         MimeType=inode/directory;\n\
         Actions=openTabHere;\n\
         X-KDE-Priority=TopLevel\n\
         \n\
         [Desktop Action openTabHere]\n\
         Name={OPEN_TAB_HERE_LABEL}\n\
         Icon=termy\n\
         Exec={quoted_exe} --new-tab --working-directory %f\n"
    )
}

fn dirs_home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_string())
}

fn write_text(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn write_text_if_unmanaged(
    user_path: &Path,
    system_path: &Path,
    contents: &str,
) -> Result<(), String> {
    if system_path.is_file() {
        return Ok(());
    }
    write_text(user_path, contents)
}

fn write_executable_script(path: &Path, contents: &str) -> Result<(), String> {
    write_text(path, contents)?;
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("failed to chmod {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{kde_servicemenu, nautilus_script, nemo_action};

    #[test]
    fn scripts_invoke_termy_with_working_directory() {
        let quoted = "'/usr/bin/termy'";
        assert!(
            nautilus_script(quoted).contains("exec '/usr/bin/termy' --new-tab --working-directory")
        );
        assert!(
            nemo_action(quoted).contains("Exec='/usr/bin/termy' --new-tab --working-directory %F")
        );
        assert!(
            kde_servicemenu(quoted)
                .contains("Exec='/usr/bin/termy' --new-tab --working-directory %f")
        );
        assert!(kde_servicemenu(quoted).contains("inode/directory"));
        assert!(nemo_action(quoted).contains("Open new Termy tab here"));
    }

    #[test]
    fn skips_user_copy_when_a_system_file_already_exists() {
        let temp = std::env::temp_dir().join(format!(
            "termy-file-manager-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).expect("temp dir");
        let system_path = temp.join("system.desktop");
        let user_path = temp.join("user.desktop");
        std::fs::write(&system_path, "system").expect("system file");
        super::write_text_if_unmanaged(&user_path, &system_path, "user").expect("skip system");
        assert!(!user_path.exists());
        std::fs::remove_file(&system_path).expect("remove system");
        super::write_text_if_unmanaged(&user_path, &system_path, "user").expect("write user");
        assert_eq!(
            std::fs::read_to_string(&user_path).expect("read user"),
            "user"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }
}
