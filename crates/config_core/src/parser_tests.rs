use crate::{
    AppConfig, AppIcon, AppearanceMode, ConfigDiagnosticKind, ConfigParseReport, CursorStyle,
    DEFAULT_LINE_HEIGHT, PaneFocusEffect, Rgb8, RootSettingId, RootSettingValueKind,
    TabCloseVisibility, TabTitleMode, TabTitleSource, TabWidthMode, TerminalScrollbarStyle,
    TerminalScrollbarVisibility, WindowsShell, WorkingDirFallback, root_setting_specs,
};

fn parse(input: &str) -> AppConfig {
    AppConfig::from_contents(input)
}

fn parse_report(input: &str) -> ConfigParseReport {
    AppConfig::from_contents_with_report(input)
}

#[test]
fn defaults_enable_tmux_persistence_and_raise_pane_focus_strength() {
    let defaults = parse("");
    assert!(defaults.tmux_persistence);
    assert!(!defaults.native_tab_persistence);
    assert!(!defaults.background_opacity_cells);
    assert!(!defaults.chrome_contrast);
    assert!((defaults.pane_focus_strength - 0.6).abs() < f32::EPSILON);
    assert!((defaults.line_height - DEFAULT_LINE_HEIGHT).abs() < f32::EPSILON);
}

#[test]
fn native_tab_persistence_parses_as_boolean() {
    assert!(parse("native_tab_persistence = true\n").native_tab_persistence);
    assert!(!parse("native_tab_persistence = false\n").native_tab_persistence);
}

#[test]
fn native_layout_autosave_parses_as_boolean() {
    assert!(parse("native_layout_autosave = true\n").native_layout_autosave);
    assert!(!parse("native_layout_autosave = false\n").native_layout_autosave);
}

#[test]
fn native_buffer_persistence_parses_as_boolean() {
    assert!(parse("native_buffer_persistence = true\n").native_buffer_persistence);
    assert!(!parse("native_buffer_persistence = false\n").native_buffer_persistence);
}

#[test]
fn simple_mode_parses_as_boolean_and_defaults_off() {
    assert!(!parse("").simple_mode);
    assert!(parse("simple_mode = true\n").simple_mode);
    assert!(!parse("simple_mode = false\n").simple_mode);
}

#[test]
fn app_icon_parses_default_and_old_values() {
    assert_eq!(parse("").app_icon, AppIcon::TermyDefault);
    assert_eq!(
        parse("app_icon = default\n").app_icon,
        AppIcon::TermyDefault
    );
    assert_eq!(parse("app_icon = old\n").app_icon, AppIcon::TermyOld);
    assert_eq!(parse("app_icon = Termy Old\n").app_icon, AppIcon::TermyOld);
}

#[test]
fn invalid_app_icon_keeps_default_and_reports_diagnostic() {
    let report = parse_report("app_icon = neon\n");

    assert_eq!(report.config.app_icon, AppIcon::TermyDefault);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(
        report.diagnostics[0].kind,
        ConfigDiagnosticKind::InvalidValue
    );
}

#[test]
fn invalid_simple_mode_keeps_default_and_reports_diagnostic() {
    let report = parse_report("simple_mode = sometimes\n");

    assert!(!report.config.simple_mode);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(
        report.diagnostics[0].kind,
        ConfigDiagnosticKind::InvalidValue
    );
}

#[test]
fn from_contents_with_report_captures_diagnostics() {
    let report = parse_report(
        "bad syntax line\n\
         typo_key = true\n\
         [colors]\n\
         unknown = #ffffff\n\
         foreground = #nothex\n\
         [tab_title]\n\
         font_size = nope\n\
         font_size = 12\n",
    );

    assert_eq!(report.config.font_size, AppConfig::default().font_size);
    assert_eq!(report.diagnostics.len(), 4);
    assert_eq!(
        report.diagnostics[0].kind,
        ConfigDiagnosticKind::InvalidSyntax
    );
    assert_eq!(
        report.diagnostics[1].kind,
        ConfigDiagnosticKind::UnknownRootKey
    );
    assert_eq!(
        report.diagnostics[2].kind,
        ConfigDiagnosticKind::UnknownColorKey
    );
    assert_eq!(
        report.diagnostics[3].kind,
        ConfigDiagnosticKind::InvalidValue
    );
}

#[test]
fn removed_notification_settings_are_ignored_without_diagnostics() {
    let report = parse_report(
        "notifications_enabled = true\n\
         notification_min_duration = 10\n\
         notify_only_unfocused = false\n",
    );

    assert!(report.diagnostics.is_empty());
}

#[test]
fn removed_browser_tabs_setting_is_ignored_without_diagnostics() {
    let report = parse_report("browser_tabs_enabled = true\n");

    assert!(report.diagnostics.is_empty());
}

#[test]
fn non_color_sections_do_not_mutate_root_keys() {
    let defaults = AppConfig::default();
    let report = parse_report(
        "[tab_title]\n\
         font_size = 18\n\
         cursor_blink = false\n\
         [unknown]\n\
         font_size = 20\n",
    );

    assert_eq!(report.config.font_size, defaults.font_size);
    assert_eq!(report.config.cursor_blink, defaults.cursor_blink);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(
        report.diagnostics[0].kind,
        ConfigDiagnosticKind::UnknownSection
    );
}

#[test]
fn duplicate_root_key_diagnostics_use_canonical_key_groups() {
    let report = parse_report(
        "working_dir_fallback = home\n\
         default_working_dir = process\n",
    );
    assert_eq!(
        report.config.working_dir_fallback,
        WorkingDirFallback::Process
    );
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(
        report.diagnostics[0].kind,
        ConfigDiagnosticKind::DuplicateRootKey
    );
}

#[test]
fn optional_fields_accept_none_sentinel() {
    let report = parse_report(
        "working_dir = none\n\
         inactive_tab_scrollback = none\n",
    );
    assert!(report.diagnostics.is_empty());
    assert_eq!(report.config.working_dir, None);
    assert_eq!(report.config.inactive_tab_scrollback, None);
}

#[test]
fn tab_title_mode_sets_default_priority() {
    let config = parse(
        "tab_title_mode = static\n\
         tab_title_fallback = Session\n",
    );

    assert_eq!(config.tab_title.mode, TabTitleMode::Static);
    assert_eq!(
        config.tab_title.priority,
        vec![TabTitleSource::Manual, TabTitleSource::Fallback]
    );
    assert_eq!(config.tab_title.fallback, "Session");
}

#[test]
fn tab_title_priority_overrides_mode() {
    let config = parse(
        "tab_title_mode = static\n\
         tab_title_priority = shell, explicit, fallback\n\
         tab_title_fallback = Session\n\
         tab_title_explicit_prefix = termy:custom:\n\
         tab_title_shell_integration = false\n\
         tab_title_prompt_format = cwd:{cwd}\n\
         tab_title_command_format = run:{command}\n",
    );

    assert_eq!(config.tab_title.mode, TabTitleMode::Static);
    assert_eq!(
        config.tab_title.priority,
        vec![
            TabTitleSource::Shell,
            TabTitleSource::Explicit,
            TabTitleSource::Fallback
        ]
    );
    assert_eq!(config.tab_title.fallback, "Session");
    assert_eq!(config.tab_title.explicit_prefix, "termy:custom:");
    assert!(!config.tab_title.shell_integration);
    assert_eq!(config.tab_title.prompt_format, "cwd:{cwd}");
    assert_eq!(config.tab_title.command_format, "run:{command}");
}

#[test]
fn enum_keys_parse_table_driven() {
    let tab_close_cases = [
        ("hover", TabCloseVisibility::Hover),
        ("activehover", TabCloseVisibility::ActiveHover),
        ("always", TabCloseVisibility::Always),
    ];
    for (input, expected) in tab_close_cases {
        let config = parse(&format!("tab_close_visibility = {input}\n"));
        assert_eq!(config.tab_close_visibility, expected);
    }
    assert_eq!(
        parse("tab_close_visibility = invalid\n").tab_close_visibility,
        TabCloseVisibility::Hover
    );

    let tab_width_cases = [
        ("stable", TabWidthMode::Stable),
        ("activegrow", TabWidthMode::ActiveGrow),
        ("active_grow_sticky", TabWidthMode::ActiveGrowSticky),
        ("uniform", TabWidthMode::Uniform),
    ];
    for (input, expected) in tab_width_cases {
        let config = parse(&format!("tab_width_mode = {input}\n"));
        assert_eq!(config.tab_width_mode, expected);
    }
    assert_eq!(
        parse("tab_width_mode = invalid\n").tab_width_mode,
        TabWidthMode::Uniform
    );

    let cursor_style_cases = [("line", CursorStyle::Line), ("bar", CursorStyle::Line)];
    for (input, expected) in cursor_style_cases {
        let config = parse(&format!("cursor_style = {input}\n"));
        assert_eq!(config.cursor_style, expected);
    }
    assert_eq!(
        parse("cursor_style = block\n").cursor_style,
        CursorStyle::Block
    );

    let scrollbar_visibility_cases = [
        ("off", TerminalScrollbarVisibility::Off),
        ("always", TerminalScrollbarVisibility::Always),
        ("on_scroll", TerminalScrollbarVisibility::OnScroll),
    ];
    for (input, expected) in scrollbar_visibility_cases {
        let config = parse(&format!("scrollbar_visibility = {input}\n"));
        assert_eq!(config.terminal_scrollbar_visibility, expected);
    }
    assert_eq!(
        parse("scrollbar_visibility = nope\n").terminal_scrollbar_visibility,
        TerminalScrollbarVisibility::OnScroll
    );
    // `scrollbar_visibility` is the supported key; the legacy
    // `terminal_scrollbar_visibility` key should be ignored.
    assert_eq!(
        parse("terminal_scrollbar_visibility = always\n").terminal_scrollbar_visibility,
        TerminalScrollbarVisibility::OnScroll
    );

    let scrollbar_style_cases = [
        ("neutral", TerminalScrollbarStyle::Neutral),
        ("muted_theme", TerminalScrollbarStyle::MutedTheme),
        ("theme", TerminalScrollbarStyle::Theme),
    ];
    for (input, expected) in scrollbar_style_cases {
        let config = parse(&format!("scrollbar_style = {input}\n"));
        assert_eq!(config.terminal_scrollbar_style, expected);
    }
    assert_eq!(
        parse("terminal_scrollbar_style = theme\n").terminal_scrollbar_style,
        TerminalScrollbarStyle::Neutral
    );

    let pane_focus_effect_cases = [
        ("off", PaneFocusEffect::Off),
        ("soft_spotlight", PaneFocusEffect::SoftSpotlight),
        ("cinematic", PaneFocusEffect::Cinematic),
        ("minimal", PaneFocusEffect::Minimal),
    ];
    for (input, expected) in pane_focus_effect_cases {
        let config = parse(&format!("pane_focus_effect = {input}\n"));
        assert_eq!(config.pane_focus_effect, expected);
    }
    assert_eq!(
        parse("pane_focus_effect = unknown\n").pane_focus_effect,
        PaneFocusEffect::SoftSpotlight
    );

    let fallback_cases = [
        ("home", WorkingDirFallback::Home),
        ("process", WorkingDirFallback::Process),
    ];
    for (input, expected) in fallback_cases {
        let config = parse(&format!("working_dir_fallback = {input}\n"));
        assert_eq!(config.working_dir_fallback, expected);
    }
    assert_eq!(
        parse("working_dir_fallback = invalid\n").working_dir_fallback,
        WorkingDirFallback::Home
    );

    let windows_shell_cases = [
        ("cmd", WindowsShell::Cmd),
        ("powershell", WindowsShell::PowerShell),
        ("pwsh", WindowsShell::PowerShellCore),
        ("powershell-7", WindowsShell::PowerShellCore),
        ("git-bash", WindowsShell::GitBash),
    ];
    for (input, expected) in windows_shell_cases {
        let config = parse(&format!("windows_shell = {input}\n"));
        assert_eq!(config.windows_shell, expected);
    }
    assert_eq!(
        parse("windows_shell = invalid\n").windows_shell,
        WindowsShell::Cmd
    );

    assert!(parse("chrome_contrast = true\n").chrome_contrast);
    assert!(parse("chrome_contrast = 1\n").chrome_contrast);
    assert!(!parse("chrome_contrast = false\n").chrome_contrast);
    assert!(!parse("chrome_contrast = 0\n").chrome_contrast);
}

#[test]
fn invalid_chrome_contrast_emits_diagnostic_and_keeps_default() {
    let report = parse_report("chrome_contrast = louder\n");
    assert!(!report.config.chrome_contrast);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == ConfigDiagnosticKind::InvalidValue)
    );
}

fn bool_root_setting_value(config: &AppConfig, setting: RootSettingId) -> Option<bool> {
    match setting {
        RootSettingId::AutoUpdate => Some(config.auto_update),
        RootSettingId::TmuxEnabled => Some(config.tmux_enabled),
        RootSettingId::TmuxPersistence => Some(config.tmux_persistence),
        RootSettingId::TmuxExclusive => Some(config.tmux_exclusive),
        RootSettingId::NativeTabPersistence => Some(config.native_tab_persistence),
        RootSettingId::NativeLayoutAutosave => Some(config.native_layout_autosave),
        RootSettingId::NativeBufferPersistence => Some(config.native_buffer_persistence),
        RootSettingId::ShowDebugOverlay => Some(config.show_debug_overlay),
        RootSettingId::SimpleMode => Some(config.simple_mode),
        RootSettingId::OnboardingComplete => Some(config.onboarding_complete),
        RootSettingId::TmuxShowActivePaneBorder => Some(config.tmux_show_active_pane_border),
        RootSettingId::WarnOnQuit => Some(config.warn_on_quit),
        RootSettingId::WarnOnQuitWithRunningProcess => {
            Some(config.warn_on_quit_with_running_process)
        }
        RootSettingId::TabTitleShellIntegration => Some(config.tab_title.shell_integration),
        RootSettingId::TabSwitchModifierHints => Some(config.tab_switch_modifier_hints),
        RootSettingId::AutoHideTabbar => Some(config.auto_hide_tabbar),
        RootSettingId::SidebarEnabled => Some(config.sidebar_enabled),
        RootSettingId::SidebarWidth => None,
        RootSettingId::ShowTermyInTitlebar => Some(config.show_termy_in_titlebar),
        RootSettingId::MacosOptionAsAlt => Some(config.macos_option_as_alt),
        RootSettingId::CursorBlink => Some(config.cursor_blink),
        RootSettingId::BackgroundOpacityCells => Some(config.background_opacity_cells),
        RootSettingId::BackgroundBlur => Some(config.background_blur),
        RootSettingId::ChromeContrast => Some(config.chrome_contrast),
        RootSettingId::CopyOnSelect => Some(config.copy_on_select),
        RootSettingId::CopyOnSelectToast => Some(config.copy_on_select_toast),
        RootSettingId::CommandPaletteShowKeybinds => Some(config.command_palette_show_keybinds),
        RootSettingId::ShellIntegrationEnabled => Some(config.shell_integration_enabled),
        RootSettingId::ProgressIndicatorEnabled => Some(config.progress_indicator_enabled),
        _ => None,
    }
}

#[test]
fn bool_root_settings_parse_table_driven_from_schema() {
    let defaults = AppConfig::default();
    let bool_specs = root_setting_specs()
        .iter()
        .filter(|spec| spec.value_kind == RootSettingValueKind::Boolean && !spec.repeatable)
        .collect::<Vec<_>>();

    for spec in bool_specs {
        let key = spec.key;
        assert!(
            bool_root_setting_value(&defaults, spec.id).is_some(),
            "missing bool accessor for {key}"
        );

        let enabled = parse(&format!("{key} = true\n"));
        assert_eq!(bool_root_setting_value(&enabled, spec.id), Some(true));

        let enabled_numeric = parse(&format!("{key} = 1\n"));
        assert_eq!(
            bool_root_setting_value(&enabled_numeric, spec.id),
            Some(true)
        );

        let disabled = parse(&format!("{key} = false\n"));
        assert_eq!(bool_root_setting_value(&disabled, spec.id), Some(false));

        let disabled_numeric = parse(&format!("{key} = 0\n"));
        assert_eq!(
            bool_root_setting_value(&disabled_numeric, spec.id),
            Some(false)
        );

        let invalid = parse(&format!("{key} = maybe\n"));
        assert_eq!(
            bool_root_setting_value(&invalid, spec.id),
            bool_root_setting_value(&defaults, spec.id)
        );
    }
}

#[test]
fn numeric_keys_parse_table_driven() {
    let defaults = parse("");

    let positive_float_cases = [
        ("window_width", 1100.0, defaults.window_width),
        ("window_height", 700.0, defaults.window_height),
        ("sidebar_width", 240.0, defaults.sidebar_width),
        ("font_size", 16.0, defaults.font_size),
    ];
    for (key, expected, fallback) in positive_float_cases {
        let valid = parse(&format!("{key} = {expected}\n"));
        let parsed = match key {
            "window_width" => valid.window_width,
            "window_height" => valid.window_height,
            "sidebar_width" => valid.sidebar_width,
            "font_size" => valid.font_size,
            _ => unreachable!(),
        };
        assert_eq!(parsed, expected);

        let invalid = parse(&format!("{key} = -1\n"));
        let parsed = match key {
            "window_width" => invalid.window_width,
            "window_height" => invalid.window_height,
            "sidebar_width" => invalid.sidebar_width,
            "font_size" => invalid.font_size,
            _ => unreachable!(),
        };
        assert_eq!(parsed, fallback);
    }

    assert_eq!(
        parse("sidebar_width = 80\n").sidebar_width,
        crate::constants::MIN_SIDEBAR_WIDTH
    );
    assert_eq!(
        parse("sidebar_width = 999\n").sidebar_width,
        crate::constants::MAX_SIDEBAR_WIDTH
    );

    assert_eq!(parse("line_height = 0.8\n").line_height, 0.8);
    assert_eq!(parse("line_height = 1.25\n").line_height, 1.25);
    assert_eq!(parse("line_height = 2.5\n").line_height, 2.5);
    assert_eq!(
        parse("line_height = 0.79\n").line_height,
        defaults.line_height
    );
    assert_eq!(
        parse("line_height = 2.51\n").line_height,
        defaults.line_height
    );
    assert_eq!(
        parse("line_height = inf\n").line_height,
        defaults.line_height
    );
    assert_eq!(
        parse("line_height = NaN\n").line_height,
        defaults.line_height
    );

    let non_negative_float_cases = [("padding_x", 2.0), ("padding_y", 4.0)];
    for (key, expected) in non_negative_float_cases {
        let valid = parse(&format!("{key} = {expected}\n"));
        let parsed = match key {
            "padding_x" => valid.padding_x,
            "padding_y" => valid.padding_y,
            _ => unreachable!(),
        };
        assert_eq!(parsed, expected);

        let invalid = parse(&format!("{key} = -1\n"));
        let parsed = match key {
            "padding_x" => invalid.padding_x,
            "padding_y" => invalid.padding_y,
            _ => unreachable!(),
        };
        let default_value = match key {
            "padding_x" => defaults.padding_x,
            "padding_y" => defaults.padding_y,
            _ => unreachable!(),
        };
        assert_eq!(parsed, default_value);
    }

    assert_eq!(parse("background_opacity = -0.5\n").background_opacity, 0.0);
    assert_eq!(parse("background_opacity = 4\n").background_opacity, 1.0);
    assert!(parse("background_opacity_cells = true\n").background_opacity_cells);
    assert!(!parse("background_opacity_cells = false\n").background_opacity_cells);
    let nan_opacity = parse_report("background_opacity = NaN\n");
    assert_eq!(
        nan_opacity.config.background_opacity,
        defaults.background_opacity
    );
    assert!(
        nan_opacity
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == ConfigDiagnosticKind::InvalidValue)
    );
    // The removed/unsupported alias should be ignored, so the default remains.
    assert_eq!(
        parse("transparent_background_opacity = 0.2\n").background_opacity,
        defaults.background_opacity
    );

    assert_eq!(
        parse("pane_focus_strength = -0.5\n").pane_focus_strength,
        0.0
    );
    assert_eq!(parse("pane_focus_strength = 4\n").pane_focus_strength, 2.0);
    let nan_pane_focus_strength = parse_report("pane_focus_strength = NaN\n");
    assert_eq!(
        nan_pane_focus_strength.config.pane_focus_strength,
        defaults.pane_focus_strength
    );
    assert!(
        nan_pane_focus_strength
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == ConfigDiagnosticKind::InvalidValue)
    );

    assert_eq!(
        parse("mouse_scroll_multiplier = -1\n").mouse_scroll_multiplier,
        0.1
    );
    assert_eq!(
        parse("mouse_scroll_multiplier = 20000\n").mouse_scroll_multiplier,
        1_000.0
    );

    assert_eq!(
        parse("scrollback_history = 5000\n").scrollback_history,
        5000
    );
    assert_eq!(parse("scrollback = 3000\n").scrollback_history, 3000);
    assert_eq!(
        parse("scrollback_history = 200000\n").scrollback_history,
        20_000
    );

    assert_eq!(parse("font_size = inf\n").font_size, defaults.font_size);
    assert_eq!(parse("padding_x = NaN\n").padding_x, defaults.padding_x);
}

#[test]
fn runtime_env_options_parse() {
    let config = parse(
        "term = screen-256color\n\
         shell = /bin/zsh\n\
         working_dir_fallback = process\n\
         colorterm = none\n",
    );

    assert_eq!(config.term, "screen-256color");
    assert_eq!(config.shell.as_deref(), Some("/bin/zsh"));
    assert_eq!(config.working_dir_fallback, WorkingDirFallback::Process);
    assert!(config.colorterm.is_none());
}

#[test]
fn tmux_runtime_options_parse() {
    let config = parse(
        "tmux_enabled = true\n\
         tmux_persistence = true\n\
         tmux_exclusive = true\n\
         tmux_show_active_pane_border = true\n\
         tmux_binary = /opt/homebrew/bin/tmux\n\
         working_dir_fallback = process\n",
    );

    assert!(config.tmux_enabled);
    assert!(config.tmux_persistence);
    assert!(config.tmux_exclusive);
    assert!(config.tmux_show_active_pane_border);
    assert_eq!(config.tmux_binary, "/opt/homebrew/bin/tmux");
    assert_eq!(config.working_dir_fallback, WorkingDirFallback::Process);
}

#[test]
fn tmux_command_prefix_parses_none_resets_and_empty_is_invalid() {
    let config = parse("tmux_command_prefix = wsl.exe -e\n");
    assert_eq!(config.tmux_command_prefix.as_deref(), Some("wsl.exe -e"));
    assert_eq!(
        config.tmux_command_prefix_argv(),
        vec!["wsl.exe".to_string(), "-e".to_string()]
    );

    let config = parse("tmux_command_prefix = ssh myhost\ntmux_command_prefix = none\n");
    assert!(config.tmux_command_prefix.is_none());
    assert!(config.tmux_command_prefix_argv().is_empty());

    let report = parse_report("tmux_command_prefix =\n");
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(
        report.diagnostics[0].kind,
        ConfigDiagnosticKind::InvalidValue
    );
    assert!(report.config.tmux_command_prefix.is_none());
}

#[test]
fn removed_tmux_persist_scrollback_key_produces_unknown_root_key_diagnostic() {
    let report = parse_report("tmux_persist_scrollback = true\n");
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(
        report.diagnostics[0].kind,
        ConfigDiagnosticKind::UnknownRootKey
    );
}

#[test]
fn removed_tmux_session_name_key_produces_unknown_root_key_diagnostic() {
    let report = parse_report("tmux_session_name = work\n");
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(
        report.diagnostics[0].kind,
        ConfigDiagnosticKind::UnknownRootKey
    );
}

#[test]
fn keybind_lines_are_collected_in_order_with_line_numbers() {
    let config = parse(
        "# ignore comments\n\
         keybind = cmd-p=toggle_command_palette\n\
         keybind = cmd-c=copy\n\
         keybind = cmd-c=unbind\n\
         keybind = clear\n",
    );

    assert_eq!(config.keybind_lines.len(), 4);
    assert_eq!(config.keybind_lines[0].line_number, 2);
    assert_eq!(
        config.keybind_lines[0].value,
        "cmd-p=toggle_command_palette"
    );
    assert_eq!(config.keybind_lines[1].line_number, 3);
    assert_eq!(config.keybind_lines[1].value, "cmd-c=copy");
    assert_eq!(config.keybind_lines[2].line_number, 4);
    assert_eq!(config.keybind_lines[2].value, "cmd-c=unbind");
    assert_eq!(config.keybind_lines[3].line_number, 5);
    assert_eq!(config.keybind_lines[3].value, "clear");
}

#[test]
fn removed_hide_titlebar_buttons_key_is_ignored_as_unknown() {
    let configured = parse(
        "hide_titlebar_buttons = true\n\
         warn_on_quit = true\n\
         warn_on_quit_with_running_process = false\n",
    );
    assert!(configured.warn_on_quit);
    assert!(!configured.warn_on_quit_with_running_process);
}

#[test]
fn task_entries_parse_into_named_tasks() {
    let config = parse(
        "task.build.command = cargo build\n\
         task.build.working_dir = crates/cli\n\
         task.build.keybind = secondary-shift-b\n\
         task.dev_server.layout = dashboard\n\
         task.dev_server.command = cargo run\n",
    );

    assert_eq!(config.tasks.len(), 2);
    assert_eq!(config.tasks[0].name, "build");
    assert_eq!(config.tasks[0].command, "cargo build");
    assert_eq!(config.tasks[0].working_dir.as_deref(), Some("crates/cli"));
    assert_eq!(config.tasks[0].layout, None);
    assert_eq!(
        config.tasks[0]
            .keybind
            .as_ref()
            .map(|keybind| (keybind.line_number, keybind.value.as_str())),
        Some((3, "secondary-shift-b"))
    );

    assert_eq!(config.tasks[1].name, "dev_server");
    assert_eq!(config.tasks[1].layout.as_deref(), Some("dashboard"));
    assert_eq!(config.tasks[1].command, "cargo run");
    assert_eq!(config.tasks[1].keybind, None);
}

#[test]
fn task_without_command_reports_invalid_value() {
    let report = parse_report("task.build.layout = dev\n");
    assert!(report.config.tasks.is_empty());
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(
        report.diagnostics[0].kind,
        ConfigDiagnosticKind::InvalidValue
    );
    assert!(
        report.diagnostics[0]
            .message
            .contains("missing required field 'command'")
    );
}

#[test]
fn invalid_task_field_does_not_create_missing_command_diagnostic() {
    let report = parse_report("task.build.invalid = nope\n");

    assert!(report.config.tasks.is_empty());
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(
        report.diagnostics[0].kind,
        ConfigDiagnosticKind::InvalidValue
    );
    assert!(report.diagnostics[0].message.contains("Invalid task field"));
}

#[test]
fn dotted_task_name_reports_clear_diagnostic() {
    let report = parse_report("task.my.task.command = cargo build\n");

    assert!(report.config.tasks.is_empty());
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(
        report.diagnostics[0].kind,
        ConfigDiagnosticKind::InvalidValue
    );
    assert!(
        report.diagnostics[0]
            .message
            .contains("task names must not contain '.'")
    );
}

#[test]
fn custom_colors_parse() {
    let config = parse(
        "theme = termy\n\
         \n\
         [colors]\n\
         foreground = #e7ebf5\n\
         background = #0b1020\n\
         cursor = #a7e9a3\n\
         black = #0b1020\n\
         red = #f1b8c5\n\
         color10 = #00ff00\n\
         brightblack = #101010\n",
    );

    let fg = config.colors.foreground.expect("foreground color");
    assert_eq!(
        fg,
        Rgb8 {
            r: 0xe7,
            g: 0xeb,
            b: 0xf5
        }
    );

    let bg = config.colors.background.expect("background color");
    assert_eq!(
        bg,
        Rgb8 {
            r: 0x0b,
            g: 0x10,
            b: 0x20
        }
    );

    assert!(config.colors.cursor.is_some());
    assert!(config.colors.ansi[0].is_some());
    assert!(config.colors.ansi[1].is_some());
    assert!(config.colors.ansi[10].is_some());
    assert!(config.colors.ansi[8].is_some());
    assert!(config.colors.ansi[2].is_none());
}

#[test]
fn root_settings_in_colors_section_parse_for_legacy_settings_writes() {
    let report = parse_report(
        "theme = termy\n\
         [colors]\n\
         theme_mode = system\n\
         theme_dark = tokyo-night\n\
         foreground = #e7ebf5\n",
    );

    assert_eq!(report.config.theme_mode, AppearanceMode::System);
    assert_eq!(report.config.theme_dark, "tokyo-night");
    assert!(report.config.colors.foreground.is_some());
    assert!(report.diagnostics.is_empty());
}

#[test]
fn shell_decide_theme_aliases_canonicalize() {
    let config = parse("theme = shell\n");
    assert_eq!(config.theme, "shell-decide");

    let config = parse("theme = let shell decide\n");
    assert_eq!(config.theme, "shell-decide");
}

#[test]
fn theme_ids_are_normalized_without_builtin_aliases() {
    let config = parse("theme = gruvbox\n");
    assert_eq!(config.theme, "gruvbox");

    let config = parse("theme = tokyonight\n");
    assert_eq!(config.theme, "tokyonight");
}
