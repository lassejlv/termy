use super::*;

#[cfg(target_os = "macos")]
#[test]
fn mac_secondary_shortcuts_map_to_line_editing_sequences() {
    let secondary = Modifiers {
        platform: true,
        ..Default::default()
    };

    assert_eq!(
        keystroke_to_input(
            &keystroke("left", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x01".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("home", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x01".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("right", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x05".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("end", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x05".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("backspace", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x15".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("delete", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x0b".to_vec())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn mac_alt_shortcuts_map_to_word_editing_sequences() {
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };

    assert_eq!(
        keystroke_to_input(
            &keystroke("left", alt),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x1bb".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("right", alt),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x1bf".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("backspace", alt),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x1b\x7f".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("delete", alt),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x1bd".to_vec())
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn non_macos_secondary_shortcuts_map_to_native_word_sequences() {
    let secondary = Modifiers {
        control: true,
        ..Default::default()
    };

    assert_eq!(
        keystroke_to_input(
            &keystroke("left", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x1bb".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("right", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x1bf".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("backspace", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x17".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("delete", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x1bd".to_vec())
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn non_macos_secondary_shortcuts_do_not_remap_in_alternate_screen() {
    // Without the remap, Ctrl+arrow/Delete use xterm's modifier parameter.
    let secondary = Modifiers {
        control: true,
        ..Default::default()
    };

    assert_eq!(
        keystroke_to_input(
            &keystroke("left", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            false,
        ),
        Some(b"\x1b[1;5D".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("right", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            false,
        ),
        Some(b"\x1b[1;5C".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("backspace", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            false,
        ),
        Some(vec![0x7f])
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("delete", secondary),
            TerminalKeyEventKind::Press,
            press_mode(),
            false,
        ),
        Some(b"\x1b[3;5~".to_vec())
    );
}

#[test]
fn plain_special_key_sequences_remain_unchanged() {
    let none = Modifiers::default();

    assert_eq!(
        keystroke_to_input(
            &keystroke("backspace", none),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(vec![0x7f])
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("delete", none),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x1b[3~".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("left", none),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x1b[D".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("right", none),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x1b[C".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("home", none),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x1b[H".to_vec())
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("end", none),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(b"\x1b[F".to_vec())
    );
}

#[test]
fn control_letter_mappings_remain_unchanged() {
    let control = Modifiers {
        control: true,
        ..Default::default()
    };

    assert_eq!(
        keystroke_to_input(
            &keystroke("a", control),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(vec![0x01])
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("c", control),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(vec![0x03])
    );
    assert_eq!(
        keystroke_to_input(
            &keystroke("z", control),
            TerminalKeyEventKind::Press,
            press_mode(),
            true,
        ),
        Some(vec![0x1a])
    );
}

#[test]
fn keyboard_mode_detects_report_all_and_event_types() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    terminal.feed_output(b"\x1b[>10u");
    let mode = terminal.keyboard_mode();
    assert!(mode.report_all_keys_as_esc());
    assert!(mode.report_event_types());
    assert!(mode.enhanced_reporting_active());
}

#[test]
fn keyboard_mode_augment_only_flags_do_not_activate_enhanced_reporting() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    terminal.feed_output(b"\x1b[>20u");
    let mode = terminal.keyboard_mode();
    assert!(mode.report_alternate_keys());
    assert!(mode.report_associated_text());
    assert!(!mode.enhanced_reporting_active());
}

#[test]
fn env_overrides_set_term_by_default() {
    let env = terminal_environment_overrides(None, &TerminalRuntimeConfig::default());
    assert_eq!(env.get("TERM").map(String::as_str), Some(DEFAULT_TERM));
}

#[test]
fn env_overrides_advertise_ghostty_progress_capability() {
    let env = terminal_environment_overrides(None, &TerminalRuntimeConfig::default());
    assert_eq!(
        env.get("TERM_PROGRAM").map(String::as_str),
        Some(GHOSTTY_COMPAT_TERM_PROGRAM)
    );
    assert_eq!(
        env.get("TERM_PROGRAM_VERSION").map(String::as_str),
        Some(GHOSTTY_COMPAT_TERM_PROGRAM_VERSION)
    );
    assert_eq!(
        env.get("TERMY_TERM_PROGRAM").map(String::as_str),
        Some(TERMY_TERM_PROGRAM)
    );
}

#[test]
fn env_overrides_allow_disabling_colorterm() {
    let config = TerminalRuntimeConfig {
        colorterm: None,
        ..TerminalRuntimeConfig::default()
    };
    let env = terminal_environment_overrides(None, &config);
    assert!(!env.contains_key("COLORTERM"));
}

#[test]
fn env_overrides_merge_host_environment_last() {
    let config = TerminalRuntimeConfig {
        environment: HashMap::from([
            ("CMUX_SOCKET_PATH".to_string(), "/tmp/cmux.sock".to_string()),
            ("TERM_PROGRAM".to_string(), "cmux".to_string()),
        ]),
        ..TerminalRuntimeConfig::default()
    };
    let env = terminal_environment_overrides(None, &config);
    assert_eq!(
        env.get("CMUX_SOCKET_PATH").map(String::as_str),
        Some("/tmp/cmux.sock")
    );
    assert_eq!(env.get("TERM_PROGRAM").map(String::as_str), Some("cmux"));
}

#[test]
fn explicit_shell_path_wins() {
    assert_eq!(resolve_shell_path(Some("/bin/custom")), "/bin/custom");
    let config = TerminalRuntimeConfig {
        shell: Some("/bin/custom".to_string()),
        windows_shell: WindowsShell::PowerShell,
        ..TerminalRuntimeConfig::default()
    };
    let launch = resolve_terminal_launch(&config, None).expect("configured shell launch");
    assert_eq!(launch.program, "/bin/custom");
    assert_eq!(config.resolved_shell_program(), "/bin/custom");
}

#[cfg(target_os = "macos")]
#[test]
fn configured_unix_shell_keeps_macos_interactive_login_arguments() {
    let resolved = resolve_terminal_launch(
        &TerminalRuntimeConfig {
            shell: Some("/opt/custom/bin/zsh".to_string()),
            ..TerminalRuntimeConfig::default()
        },
        None,
    )
    .expect("configured shell launch");

    assert_eq!(resolved.program, "/opt/custom/bin/zsh");
    assert_eq!(resolved.args, ["-i", "-l"]);
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn configured_unix_shell_keeps_non_macos_interactive_arguments() {
    let resolved = resolve_terminal_launch(
        &TerminalRuntimeConfig {
            shell: Some("/opt/custom/bin/zsh".to_string()),
            ..TerminalRuntimeConfig::default()
        },
        None,
    )
    .expect("configured shell launch");

    assert_eq!(resolved.program, "/opt/custom/bin/zsh");
    assert_eq!(resolved.args, ["-i"]);
}

#[test]
fn typed_program_launch_keeps_arguments_out_of_the_shell() {
    let launch = TerminalLaunch::Program {
        program: "ssh".to_string(),
        args: vec![
            "-i".to_string(),
            "/tmp/key; touch /tmp/should-not-exist".to_string(),
            "--".to_string(),
            "example.com".to_string(),
        ],
    };
    let resolved = resolve_terminal_launch(&TerminalRuntimeConfig::default(), Some(&launch))
        .expect("typed launch");
    assert_eq!(resolved.program, "ssh");
    assert_eq!(
        resolved.args,
        vec![
            "-i".to_string(),
            "/tmp/key; touch /tmp/should-not-exist".to_string(),
            "--".to_string(),
            "example.com".to_string(),
        ]
    );
}

#[cfg(unix)]
#[test]
fn existing_startup_commands_still_use_the_unix_shell() {
    let launch = TerminalLaunch::ShellCommand("printf existing-behavior".to_string());
    let resolved = resolve_terminal_launch(&TerminalRuntimeConfig::default(), Some(&launch))
        .expect("shell command launch");
    assert_eq!(resolved.program, "/bin/sh");
    assert_eq!(
        resolved.args,
        vec!["-c".to_string(), "printf existing-behavior".to_string()]
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_shell_setting_selects_powershell() {
    let launch = default_shell_launch(&TerminalRuntimeConfig {
        windows_shell: WindowsShell::PowerShell,
        ..TerminalRuntimeConfig::default()
    });

    assert_eq!(launch.program, "powershell.exe");
    assert_eq!(launch.args, vec!["-NoLogo".to_string()]);
}

#[cfg(target_os = "windows")]
#[test]
fn windows_shell_setting_selects_powershell_core() {
    let launch = default_shell_launch(&TerminalRuntimeConfig {
        windows_shell: WindowsShell::PowerShellCore,
        ..TerminalRuntimeConfig::default()
    });

    assert_eq!(launch.program, "pwsh.exe");
    assert_eq!(launch.args, vec!["-NoLogo".to_string()]);
}

#[cfg(target_os = "windows")]
#[test]
fn public_resolver_preserves_every_windows_shell_launch() {
    let cases = [
        (WindowsShell::Cmd, windows_cmd_path(), Vec::new()),
        (
            WindowsShell::PowerShell,
            "powershell.exe".to_string(),
            vec!["-NoLogo".to_string()],
        ),
        (
            WindowsShell::PowerShellCore,
            "pwsh.exe".to_string(),
            vec!["-NoLogo".to_string()],
        ),
        (
            WindowsShell::GitBash,
            windows_git_bash_path(),
            vec!["--login".to_string(), "-i".to_string()],
        ),
    ];

    for (windows_shell, program, args) in cases {
        let resolved = resolve_terminal_launch(
            &TerminalRuntimeConfig {
                windows_shell,
                ..TerminalRuntimeConfig::default()
            },
            None,
        )
        .expect("Windows shell launch");
        assert_eq!(resolved.program, program);
        assert_eq!(resolved.args, args);
    }
}

#[cfg(target_os = "windows")]
#[test]
fn public_resolver_preserves_every_windows_startup_command_launch() {
    let command = "echo startup";
    let cases = [
        (
            WindowsShell::Cmd,
            windows_cmd_path(),
            vec![
                "/D".to_string(),
                "/S".to_string(),
                "/C".to_string(),
                command.to_string(),
            ],
        ),
        (
            WindowsShell::PowerShell,
            "powershell.exe".to_string(),
            vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                command.to_string(),
            ],
        ),
        (
            WindowsShell::PowerShellCore,
            "pwsh.exe".to_string(),
            vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                command.to_string(),
            ],
        ),
        (
            WindowsShell::GitBash,
            windows_git_bash_path(),
            vec!["-lc".to_string(), command.to_string()],
        ),
    ];
    let launch = TerminalLaunch::ShellCommand(command.to_string());

    for (windows_shell, program, args) in cases {
        let resolved = resolve_terminal_launch(
            &TerminalRuntimeConfig {
                windows_shell,
                ..TerminalRuntimeConfig::default()
            },
            Some(&launch),
        )
        .expect("Windows startup command launch");
        assert_eq!(resolved.program, program);
        assert_eq!(resolved.args, args);
    }
}

#[cfg(target_os = "windows")]
#[test]
fn public_resolver_preserves_custom_windows_shell_startup_behavior() {
    let runtime_config = TerminalRuntimeConfig {
        shell: Some(r"C:\Tools\custom-shell.exe".to_string()),
        windows_shell: WindowsShell::PowerShellCore,
        ..TerminalRuntimeConfig::default()
    };

    let interactive =
        resolve_terminal_launch(&runtime_config, None).expect("custom Windows shell launch");
    assert_eq!(interactive.program, r"C:\Tools\custom-shell.exe");
    assert!(interactive.args.is_empty());

    let startup = TerminalLaunch::ShellCommand("echo startup".to_string());
    let command = resolve_terminal_launch(&runtime_config, Some(&startup))
        .expect("custom Windows startup command launch");
    assert_eq!(command.program, windows_cmd_path());
    assert_eq!(command.args, ["/D", "/S", "/C", "echo startup"]);
}

#[cfg(target_os = "windows")]
#[test]
fn custom_windows_shell_runs_startup_commands_with_its_own_command_flag() {
    let startup = TerminalLaunch::ShellCommand(r#"echo "quoted" & dir"#.to_string());
    let cases = [
        (
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            vec![
                "-NoLogo",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                r#"echo "quoted" & dir"#,
            ],
        ),
        (
            r"C:\Program Files\Git\bin\bash.exe",
            vec!["-lc", r#"echo "quoted" & dir"#],
        ),
        (
            r"D:\tools\CMD.EXE",
            vec!["/D", "/S", "/C", r#"echo "quoted" & dir"#],
        ),
    ];
    for (shell, args) in cases {
        let resolved = resolve_terminal_launch(
            &TerminalRuntimeConfig {
                shell: Some(shell.to_string()),
                ..TerminalRuntimeConfig::default()
            },
            Some(&startup),
        )
        .expect("custom Windows startup command launch");
        assert_eq!(resolved.program, shell);
        assert_eq!(resolved.args, args);
    }
}

#[cfg(target_os = "windows")]
#[test]
fn windows_shell_family_classifies_executable_names() {
    for (program, family) in [
        (r"C:\Windows\System32\cmd.exe", WindowsShellFamily::Cmd),
        ("pwsh", WindowsShellFamily::PowerShell),
        ("PowerShell.EXE", WindowsShellFamily::PowerShell),
        (r"C:\msys64\usr\bin\zsh.exe", WindowsShellFamily::Posix),
        ("bash.exe. ", WindowsShellFamily::Posix),
        (r"C:\Tools\nu.exe", WindowsShellFamily::Other),
    ] {
        assert_eq!(windows_shell_family(program), family, "{program}");
    }
}

#[cfg(target_os = "windows")]
#[test]
fn automatic_windows_shell_prefers_powershell_and_falls_back_to_cmd() {
    let launch = default_shell_launch(&TerminalRuntimeConfig::default());
    match windows_shell_family(&launch.program) {
        WindowsShellFamily::PowerShell => assert_eq!(launch.args, ["-NoLogo"]),
        WindowsShellFamily::Cmd => assert!(launch.args.is_empty()),
        family => panic!("unexpected automatic shell {family:?}: {}", launch.program),
    }
}

#[cfg(target_os = "windows")]
#[test]
fn git_bash_is_never_the_wsl_launcher() {
    let bash = windows_git_bash_path().to_ascii_lowercase();
    let system_root = std::env::var("SystemRoot")
        .unwrap_or_else(|_| r"C:\Windows".to_string())
        .to_ascii_lowercase();
    assert!(!bash.starts_with(&system_root), "{bash}");
    assert!(!bash.contains(r"\microsoft\windowsapps\"), "{bash}");
    assert!(bash.ends_with(r"\bash.exe"), "{bash}");
}

#[cfg(target_os = "windows")]
#[test]
fn user_path_replaces_normalized_path_case_insensitively() {
    let config = TerminalRuntimeConfig {
        environment: HashMap::from([("Path".to_string(), r"C:\only".to_string())]),
        ..TerminalRuntimeConfig::default()
    };
    let env = terminal_environment_overrides(None, &config);
    let paths = env
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("PATH"))
        .collect::<Vec<_>>();
    assert_eq!(paths, [(&"Path".to_string(), &r"C:\only".to_string())]);
}

#[cfg(target_os = "windows")]
#[test]
fn windows_shell_integration_wraps_interactive_powershell_and_cmd() {
    let enabled = TabTitleShellIntegration {
        enabled: true,
        explicit_prefix: String::new(),
    };
    let mut pwsh = ResolvedTerminalLaunch {
        program: r"C:\Program Files\PowerShell\7\pwsh.exe".to_string(),
        args: vec!["-NoLogo".to_string()],
    };
    let mut environment = HashMap::new();
    apply_windows_shell_integration(&mut pwsh, &mut environment, Some(&enabled));
    assert_eq!(pwsh.args[..3], ["-NoLogo", "-NoExit", "-Command"]);
    assert!(pwsh.args[3].contains("133;A"));
    assert!(!pwsh.args[3].contains('"'));

    let mut disabled_pwsh = ResolvedTerminalLaunch {
        program: "pwsh.exe".to_string(),
        args: vec!["-NoLogo".to_string()],
    };
    apply_windows_shell_integration(&mut disabled_pwsh, &mut environment, None);
    assert_eq!(disabled_pwsh.args, ["-NoLogo"]);

    let mut cmd = ResolvedTerminalLaunch {
        program: windows_cmd_path(),
        args: Vec::new(),
    };
    let mut cmd_environment = HashMap::new();
    apply_windows_shell_integration(&mut cmd, &mut cmd_environment, Some(&enabled));
    if !inherited_prompt_is_custom(std::env::var_os("PROMPT").as_deref()) {
        assert!(cmd_environment["PROMPT"].contains("$e]133;A"));
    }
    let mut user_environment = HashMap::from([("prompt".to_string(), "$G".to_string())]);
    apply_windows_shell_integration(&mut cmd, &mut user_environment, Some(&enabled));
    assert!(!user_environment.contains_key("PROMPT"));
    assert!(cmd.args.is_empty());

    assert!(!inherited_prompt_is_custom(None));
    assert!(!inherited_prompt_is_custom(Some(std::ffi::OsStr::new(
        "$p$g"
    ))));
    assert!(inherited_prompt_is_custom(Some(std::ffi::OsStr::new(
        "$T $P$G"
    ))));
}

#[cfg(target_os = "windows")]
#[test]
fn windows_startup_commands_use_selected_shell() {
    let launch = super::windows_startup_command_shell(WindowsShell::GitBash, "echo hi");

    assert_eq!(launch.args, vec!["-lc".to_string(), "echo hi".to_string()]);
}
