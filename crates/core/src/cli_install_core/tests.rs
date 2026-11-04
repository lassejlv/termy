use super::*;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::os::unix::fs::PermissionsExt;
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
fn install_cli_profile_paths_match_shell() {
    let home = TempDir::new().expect("home");
    let home = home.path();
    assert_eq!(
        install_cli_profile_path(InstallShell::Zsh, home),
        home.join(".zshrc")
    );
    assert_eq!(
        install_cli_profile_path(InstallShell::Fish, home),
        home.join(".config/fish/config.fish")
    );
    #[cfg(target_os = "macos")]
    assert_eq!(
        install_cli_profile_path(InstallShell::Bash, home),
        home.join(".bash_profile")
    );
    // Linux bash keeps `.bashrc` even when a login file exists.
    std::fs::write(home.join(".profile"), "").expect("write .profile");
    #[cfg(target_os = "macos")]
    assert_eq!(
        install_cli_profile_path(InstallShell::Bash, home),
        home.join(".profile")
    );
    #[cfg(target_os = "linux")]
    assert_eq!(
        install_cli_profile_path(InstallShell::Bash, home),
        home.join(".bashrc")
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn bash_login_profile_creates_bash_profile_only_when_none_exists() {
    let home = TempDir::new().expect("home");
    assert_eq!(
        bash_login_profile_path(home.path()),
        home.path().join(".bash_profile")
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn bash_login_profile_follows_bash_lookup_order() {
    let home = TempDir::new().expect("home");
    let home = home.path();

    std::fs::write(home.join(".profile"), "export A=1\n").expect("write .profile");
    assert_eq!(bash_login_profile_path(home), home.join(".profile"));

    std::fs::write(home.join(".bash_login"), "").expect("write .bash_login");
    assert_eq!(bash_login_profile_path(home), home.join(".bash_login"));

    std::fs::write(home.join(".bash_profile"), "").expect("write .bash_profile");
    assert_eq!(bash_login_profile_path(home), home.join(".bash_profile"));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn bash_login_profile_skips_a_dangling_bash_profile_link() {
    let home = TempDir::new().expect("home");
    let home = home.path();
    std::os::unix::fs::symlink(home.join("missing"), home.join(".bash_profile")).expect("symlink");
    std::fs::write(home.join(".profile"), "export A=1\n").expect("write .profile");

    assert_eq!(bash_login_profile_path(home), home.join(".profile"));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn bash_login_profile_update_does_not_shadow_profile() {
    let home = TempDir::new().expect("home");
    let home = home.path();
    std::fs::write(home.join(".profile"), "export A=1\n").expect("write .profile");

    let profile = bash_login_profile_path(home);
    let block = install_cli_profile_block(InstallShell::Bash, "/tmp/bin");
    assert!(ensure_install_cli_profile_block(&profile, &block).expect("update profile"));

    assert!(!home.join(".bash_profile").exists());
    let contents = std::fs::read_to_string(home.join(".profile")).expect("read .profile");
    assert!(contents.starts_with("export A=1\n"));
    assert!(contents.contains("# >>> termy cli path >>>"));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn install_cli_profile_append_is_idempotent() {
    let marker = "# >>> termy cli path >>>";
    let block = install_cli_profile_block(InstallShell::Zsh, "/tmp/bin");
    let once = install_cli_profile_block_addition("", marker, &block).unwrap();
    assert!(once.contains(marker));
    assert!(once.ends_with('\n'));
    assert!(install_cli_profile_block_addition(&once, marker, &block).is_none());
    assert_eq!(
        install_cli_profile_block_addition("alias ll='ls -l'", marker, &block).unwrap(),
        format!("\n{once}")
    );
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
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
        .expect("chmod target");
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
    assert_eq!(file_mode(&target), 0o600);
    assert!(!dotfiles.join("zshrc.tmp").exists());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn file_mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o7777
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn profile_update_keeps_mode_and_hard_links() {
    use std::os::unix::fs::MetadataExt;

    let home = TempDir::new().expect("home");
    let profile = home.path().join(".zshrc");
    std::fs::write(&profile, "export EDITOR=vi").expect("write profile");
    std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o600))
        .expect("chmod profile");
    let hard_link = home.path().join("zshrc-backup");
    std::fs::hard_link(&profile, &hard_link).expect("hard link");
    let inode = std::fs::metadata(&profile).expect("metadata").ino();

    let block = install_cli_profile_block(InstallShell::Zsh, "/tmp/bin");
    assert!(ensure_install_cli_profile_block(&profile, &block).expect("update profile"));

    assert_eq!(file_mode(&profile), 0o600);
    assert_eq!(std::fs::metadata(&profile).expect("metadata").ino(), inode);
    let contents = std::fs::read_to_string(&profile).expect("read profile");
    assert_eq!(contents, format!("export EDITOR=vi\n{block}\n"));
    assert_eq!(
        std::fs::read_to_string(&hard_link).expect("read hard link"),
        contents
    );

    // A second install leaves the file exactly as it is.
    assert!(!ensure_install_cli_profile_block(&profile, &block).expect("second update"));
    assert_eq!(
        std::fs::read_to_string(&profile).expect("read profile"),
        contents
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn profile_update_leaves_a_sibling_tmp_path_alone() {
    let home = TempDir::new().expect("home");
    let profile = home.path().join(".zshrc");
    std::fs::write(&profile, "export EDITOR=vi\n").expect("write profile");
    let tmp = home.path().join(".zshrc.tmp");
    std::fs::write(&tmp, "user data\n").expect("write tmp");
    // A link where a fixed temp name used to be written must not be
    // followed either.
    let victim = home.path().join("victim");
    std::fs::write(&victim, "keep me\n").expect("write victim");
    let config_tmp = home.path().join("config.tmp");
    std::os::unix::fs::symlink(&victim, &config_tmp).expect("symlink");
    let fish_profile = home.path().join("config.fish");
    std::fs::write(&fish_profile, "set -gx EDITOR vi\n").expect("write fish profile");

    let block = install_cli_profile_block(InstallShell::Zsh, "/tmp/bin");
    assert!(ensure_install_cli_profile_block(&profile, &block).expect("update profile"));
    let fish_block = install_cli_profile_block(InstallShell::Fish, "/tmp/bin");
    assert!(
        ensure_install_cli_profile_block(&fish_profile, &fish_block).expect("update fish profile")
    );

    assert_eq!(
        std::fs::read_to_string(&tmp).expect("read tmp"),
        "user data\n"
    );
    assert_eq!(
        std::fs::read_to_string(&victim).expect("read victim"),
        "keep me\n"
    );
    assert!(
        std::fs::symlink_metadata(&config_tmp)
            .expect("link metadata")
            .file_type()
            .is_symlink()
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn profile_update_creates_a_missing_profile() {
    let home = TempDir::new().expect("home");
    let profile = home.path().join(".config").join("fish").join("config.fish");

    let block = install_cli_profile_block(InstallShell::Fish, "/tmp/bin");
    assert!(ensure_install_cli_profile_block(&profile, &block).expect("create profile"));

    assert_eq!(
        std::fs::read_to_string(&profile).expect("read profile"),
        format!("{block}\n")
    );
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
