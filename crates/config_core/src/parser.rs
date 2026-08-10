use std::collections::{BTreeMap, HashMap};

use crate::color_keys::{ColorEntryError, apply_color_entry};
use crate::constants::{
    MAX_LINE_HEIGHT, MAX_MOUSE_SCROLL_MULTIPLIER, MAX_PANE_FOCUS_STRENGTH, MAX_SCROLLBACK_HISTORY,
    MAX_SIDEBAR_WIDTH, MIN_LINE_HEIGHT, MIN_MOUSE_SCROLL_MULTIPLIER, MIN_SIDEBAR_WIDTH,
    SHELL_DECIDE_THEME_ID, VALID_SECTIONS,
};
use crate::diagnostics::{ConfigDiagnostic, ConfigDiagnosticKind, ConfigParseReport};
use crate::schema::{RootSettingId, root_setting_from_key, root_setting_spec};
use crate::types::{
    AppConfig, AppearanceMode, CursorStyle, KeybindConfigLine, NativeTabPlacement, PaneFocusEffect,
    TabBarPosition, TabCloseVisibility, TabTitleMode, TabTitleSource, TabWidthMode, TaskConfig,
    TerminalScrollbarStyle, TerminalScrollbarVisibility, ThemeId, WindowsShell, WorkingDirFallback,
};

#[derive(Default)]
struct PendingTaskConfig {
    first_line: usize,
    command: Option<String>,
    layout: Option<String>,
    working_dir: Option<String>,
    keybind: Option<KeybindConfigLine>,
}

enum TaskKeyParseError {
    InvalidTaskName,
}

impl AppConfig {
    pub fn from_contents(contents: &str) -> Self {
        Self::from_contents_with_report(contents).config
    }

    pub fn from_contents_with_report(contents: &str) -> ConfigParseReport {
        let mut config = Self::default();
        let mut diagnostics = Vec::new();
        let mut tab_title_priority_overridden = false;
        let mut in_colors_section = false;
        let mut in_non_root_section = false;
        let mut seen_root_keys: HashMap<RootSettingId, usize> = HashMap::new();
        let mut task_entries: BTreeMap<String, PendingTaskConfig> = BTreeMap::new();

        for (line_number, line) in contents.lines().enumerate() {
            let line_number = line_number + 1;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                let section = line[1..line.len() - 1].trim().to_ascii_lowercase();
                in_colors_section = section == "colors";
                in_non_root_section = true;
                if !VALID_SECTIONS.iter().any(|valid| *valid == section) {
                    diagnostics.push(ConfigDiagnostic {
                        line_number,
                        kind: ConfigDiagnosticKind::UnknownSection,
                        message: format!("Unknown section '[{section}]'"),
                    });
                }
                continue;
            }

            if in_non_root_section && !in_colors_section {
                continue;
            }

            let Some((raw_key, raw_value)) = line.split_once('=') else {
                diagnostics.push(ConfigDiagnostic {
                    line_number,
                    kind: ConfigDiagnosticKind::InvalidSyntax,
                    message: "Invalid syntax: expected 'key = value'".to_string(),
                });
                continue;
            };
            let key = raw_key.trim();
            let value = raw_value.trim();

            if key.is_empty() {
                diagnostics.push(ConfigDiagnostic {
                    line_number,
                    kind: ConfigDiagnosticKind::InvalidSyntax,
                    message: "Invalid syntax: missing key before '='".to_string(),
                });
                continue;
            }

            if in_colors_section
                && root_setting_from_key(key).is_none()
                && !is_ignored_removed_root_key(key)
            {
                match apply_color_entry(&mut config.colors, key, value) {
                    Ok(()) => {}
                    Err(ColorEntryError::UnknownKey) => diagnostics.push(ConfigDiagnostic {
                        line_number,
                        kind: ConfigDiagnosticKind::UnknownColorKey,
                        message: format!("Unknown color key '{key}'"),
                    }),
                    Err(ColorEntryError::InvalidValue) => diagnostics.push(ConfigDiagnostic {
                        line_number,
                        kind: ConfigDiagnosticKind::InvalidValue,
                        message: format!(
                            "Invalid color value '{value}' for '{key}': expected #RRGGBB"
                        ),
                    }),
                }
                continue;
            }

            match parse_task_key(key) {
                Ok(Some((task_name, task_field))) => {
                    if task_field.eq_ignore_ascii_case("command") {
                        let task = task_entries
                            .entry(task_name.to_string())
                            .or_insert_with(|| PendingTaskConfig {
                                first_line: line_number,
                                ..PendingTaskConfig::default()
                            });
                        if let Some(parsed) = parse_string_field(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "a non-empty task command",
                        ) {
                            task.command = Some(parsed);
                        }
                    } else if task_field.eq_ignore_ascii_case("layout") {
                        let task = task_entries
                            .entry(task_name.to_string())
                            .or_insert_with(|| PendingTaskConfig {
                                first_line: line_number,
                                ..PendingTaskConfig::default()
                            });
                        task.layout = parse_optional_string_value(value);
                    } else if task_field.eq_ignore_ascii_case("working_dir") {
                        let task = task_entries
                            .entry(task_name.to_string())
                            .or_insert_with(|| PendingTaskConfig {
                                first_line: line_number,
                                ..PendingTaskConfig::default()
                            });
                        task.working_dir = parse_optional_string_value(value);
                    } else if task_field.eq_ignore_ascii_case("keybind") {
                        let task = task_entries
                            .entry(task_name.to_string())
                            .or_insert_with(|| PendingTaskConfig {
                                first_line: line_number,
                                ..PendingTaskConfig::default()
                            });
                        if let Some(parsed) = parse_string_field(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "a non-empty keybind trigger",
                        ) {
                            task.keybind = Some(KeybindConfigLine {
                                line_number,
                                value: parsed,
                            });
                        }
                    } else {
                        diagnostics.push(ConfigDiagnostic {
                            line_number,
                            kind: ConfigDiagnosticKind::InvalidValue,
                            message: format!(
                                "Invalid task field '{task_field}' for '{key}': expected command, layout, working_dir, or keybind"
                            ),
                        });
                    }
                    continue;
                }
                Err(TaskKeyParseError::InvalidTaskName) => {
                    diagnostics.push(ConfigDiagnostic {
                        line_number,
                        kind: ConfigDiagnosticKind::InvalidValue,
                        message: format!(
                            "Invalid task key '{key}': task names must not contain '.'"
                        ),
                    });
                    continue;
                }
                Ok(None) => {}
            }

            if is_ignored_removed_root_key(key) {
                continue;
            }

            let Some(root_key) = root_setting_from_key(key) else {
                diagnostics.push(ConfigDiagnostic {
                    line_number,
                    kind: ConfigDiagnosticKind::UnknownRootKey,
                    message: format!("Unknown root key '{key}'"),
                });
                continue;
            };

            if !root_setting_spec(root_key).repeatable {
                if let Some(first_line) = seen_root_keys.get(&root_key).copied() {
                    diagnostics.push(ConfigDiagnostic {
                        line_number,
                        kind: ConfigDiagnosticKind::DuplicateRootKey,
                        message: format!(
                            "Duplicate root key '{key}' (first defined on line {first_line})"
                        ),
                    });
                } else {
                    seen_root_keys.insert(root_key, line_number);
                }
            }

            match root_key {
                RootSettingId::Theme => {
                    if let Some(theme) = parse_theme_id(value) {
                        config.theme = theme;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "a non-empty theme id",
                        );
                    }
                }
                RootSettingId::ThemeMode => {
                    if let Some(parsed) = AppearanceMode::from_str(value) {
                        config.theme_mode = parsed;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: manual, system",
                        );
                    }
                }
                RootSettingId::ThemeLight => {
                    if let Some(theme) = parse_theme_id(value) {
                        config.theme_light = theme;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "a non-empty theme id",
                        );
                    }
                }
                RootSettingId::ThemeDark => {
                    if let Some(theme) = parse_theme_id(value) {
                        config.theme_dark = theme;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "a non-empty theme id",
                        );
                    }
                }
                RootSettingId::AutoUpdate => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.auto_update = parsed;
                    }
                }
                RootSettingId::TmuxEnabled => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.tmux_enabled = parsed;
                    }
                }
                RootSettingId::TmuxPersistence => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.tmux_persistence = parsed;
                    }
                }
                RootSettingId::TmuxExclusive => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.tmux_exclusive = parsed;
                    }
                }
                RootSettingId::NativeTabPersistence => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.native_tab_persistence = parsed;
                    }
                }
                RootSettingId::NativeLayoutAutosave => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.native_layout_autosave = parsed;
                    }
                }
                RootSettingId::NativeBufferPersistence => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.native_buffer_persistence = parsed;
                    }
                }
                RootSettingId::ShowDebugOverlay => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.show_debug_overlay = parsed;
                    }
                }
                RootSettingId::SimpleMode => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.simple_mode = parsed;
                    }
                }
                RootSettingId::OnboardingComplete => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.onboarding_complete = parsed;
                    }
                }
                RootSettingId::TmuxBinary => {
                    if let Some(parsed) = parse_string_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a non-empty string",
                    ) {
                        config.tmux_binary = parsed;
                    }
                }
                RootSettingId::TmuxCommandPrefix => {
                    if value.trim().eq_ignore_ascii_case("none") {
                        config.tmux_command_prefix = None;
                    } else if value.is_empty() {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "a non-empty command prefix or none",
                        );
                    } else {
                        config.tmux_command_prefix = Some(value.to_string());
                    }
                }
                RootSettingId::TmuxShowActivePaneBorder => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.tmux_show_active_pane_border = parsed;
                    }
                }
                RootSettingId::WorkingDir => {
                    if value.trim().eq_ignore_ascii_case("none") {
                        config.working_dir = None;
                    } else if value.is_empty() {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "a non-empty path value",
                        );
                    } else {
                        config.working_dir = Some(value.to_string());
                    }
                }
                RootSettingId::WorkingDirFallback => {
                    if let Some(fallback) = WorkingDirFallback::from_str(value) {
                        config.working_dir_fallback = fallback;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: home, process",
                        );
                    }
                }
                RootSettingId::AppIcon => {
                    if let Some(parsed) = crate::types::AppIcon::from_str(value) {
                        config.app_icon = parsed;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: default, old",
                        );
                    }
                }
                RootSettingId::WarnOnQuit => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.warn_on_quit = parsed;
                    }
                }
                RootSettingId::WarnOnQuitWithRunningProcess => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.warn_on_quit_with_running_process = parsed;
                    }
                }
                RootSettingId::ChromeContrast => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.chrome_contrast = parsed;
                    }
                }
                RootSettingId::TabTitlePriority => {
                    if let Some(priority) = parse_tab_title_priority(value) {
                        config.tab_title.priority = priority;
                        tab_title_priority_overridden = true;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "a comma-separated list of tab title sources",
                        );
                    }
                }
                RootSettingId::TabTitleMode => {
                    if let Some(mode) = TabTitleMode::from_str(value) {
                        config.tab_title.mode = mode;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: smart, shell, explicit, static",
                        );
                    }
                }
                RootSettingId::TabTitleFallback => {
                    if let Some(parsed) = parse_string_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a non-empty string",
                    ) {
                        config.tab_title.fallback = parsed;
                    }
                }
                RootSettingId::TabTitleExplicitPrefix => {
                    if let Some(parsed) = parse_string_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a non-empty string",
                    ) {
                        config.tab_title.explicit_prefix = parsed;
                    }
                }
                RootSettingId::TabTitleShellIntegration => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.tab_title.shell_integration = parsed;
                    }
                }
                RootSettingId::TabTitlePromptFormat => {
                    if let Some(parsed) = parse_string_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a non-empty string",
                    ) {
                        config.tab_title.prompt_format = parsed;
                    }
                }
                RootSettingId::TabTitleCommandFormat => {
                    if let Some(parsed) = parse_string_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a non-empty string",
                    ) {
                        config.tab_title.command_format = parsed;
                    }
                }
                RootSettingId::TabCloseVisibility => {
                    if let Some(parsed) = TabCloseVisibility::from_str(value) {
                        config.tab_close_visibility = parsed;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: active_hover, hover, always",
                        );
                    }
                }
                RootSettingId::TabWidthMode => {
                    if let Some(parsed) = TabWidthMode::from_str(value) {
                        config.tab_width_mode = parsed;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: uniform, stable, active_grow, active_grow_sticky",
                        );
                    }
                }
                RootSettingId::TabBarPosition => {
                    if let Some(parsed) = TabBarPosition::from_str(value) {
                        config.tab_bar_position = parsed;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: top, right",
                        );
                    }
                }
                RootSettingId::NativeTabPlacement => {
                    if let Some(parsed) = NativeTabPlacement::from_str(value) {
                        config.native_tab_placement = parsed;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: native_tabbar, sidebar",
                        );
                    }
                }
                RootSettingId::TabSwitchModifierHints => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.tab_switch_modifier_hints = parsed;
                    }
                }
                RootSettingId::AutoHideTabbar => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.auto_hide_tabbar = parsed;
                    }
                }
                RootSettingId::SidebarEnabled => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.sidebar_enabled = parsed;
                    }
                }
                RootSettingId::SidebarWidth => {
                    if let Some(parsed) =
                        parse_positive_f32_field(&mut diagnostics, line_number, key, value)
                    {
                        config.sidebar_width = parsed.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
                    }
                }
                RootSettingId::ShowTermyInTitlebar => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.show_termy_in_titlebar = parsed;
                    }
                }
                RootSettingId::WindowsShell => {
                    if let Some(parsed) = WindowsShell::from_str(value) {
                        config.windows_shell = parsed;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: cmd, powershell, git_bash",
                        );
                    }
                }
                RootSettingId::Shell => {
                    config.shell = parse_optional_string_value(value);
                }
                RootSettingId::Term => {
                    if let Some(parsed) = parse_string_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a non-empty string",
                    ) {
                        config.term = parsed;
                    }
                }
                RootSettingId::Colorterm => {
                    config.colorterm = parse_optional_string_value(value);
                }
                RootSettingId::MacosOptionAsAlt => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.macos_option_as_alt = parsed;
                    }
                }
                RootSettingId::WindowWidth => {
                    if let Some(parsed) =
                        parse_positive_f32_field(&mut diagnostics, line_number, key, value)
                    {
                        config.window_width = parsed;
                    }
                }
                RootSettingId::WindowHeight => {
                    if let Some(parsed) =
                        parse_positive_f32_field(&mut diagnostics, line_number, key, value)
                    {
                        config.window_height = parsed;
                    }
                }
                RootSettingId::InspectorHeight => {
                    if let Some(parsed) =
                        parse_positive_f32_field(&mut diagnostics, line_number, key, value)
                    {
                        config.inspector_height = parsed;
                    }
                }
                RootSettingId::FontFamily => {
                    if let Some(parsed) = parse_string_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a non-empty string",
                    ) {
                        config.font_family = parsed;
                    }
                }
                RootSettingId::UiFontFamily => {
                    if let Some(parsed) = parse_string_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a non-empty string",
                    ) {
                        config.ui_font_family = parsed;
                    }
                }
                RootSettingId::FontSize => {
                    if let Some(parsed) =
                        parse_positive_f32_field(&mut diagnostics, line_number, key, value)
                    {
                        config.font_size = parsed;
                    }
                }
                RootSettingId::LineHeight => {
                    if let Some(parsed) = parse_f32_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a finite number between 0.8 and 2.5",
                    ) {
                        if parsed.is_finite()
                            && (MIN_LINE_HEIGHT..=MAX_LINE_HEIGHT).contains(&parsed)
                        {
                            config.line_height = parsed;
                        } else {
                            push_invalid_value(
                                &mut diagnostics,
                                line_number,
                                key,
                                value,
                                "a finite number between 0.8 and 2.5",
                            );
                        }
                    }
                }
                RootSettingId::CursorStyle => {
                    if let Some(parsed) = CursorStyle::from_str(value) {
                        config.cursor_style = parsed;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: line, block",
                        );
                    }
                }
                RootSettingId::CursorBlink => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.cursor_blink = parsed;
                    }
                }
                RootSettingId::BackgroundOpacity => {
                    if let Some(parsed) = parse_f32_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a number between 0.0 and 1.0",
                    ) {
                        if parsed.is_finite() {
                            config.background_opacity = parsed.clamp(0.0, 1.0);
                        } else {
                            push_invalid_value(
                                &mut diagnostics,
                                line_number,
                                key,
                                value,
                                "a number between 0.0 and 1.0",
                            );
                        }
                    }
                }
                RootSettingId::BackgroundOpacityCells => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.background_opacity_cells = parsed;
                    }
                }
                RootSettingId::BackgroundBlur => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.background_blur = parsed;
                    }
                }
                RootSettingId::PaddingX => {
                    if let Some(parsed) =
                        parse_non_negative_f32_field(&mut diagnostics, line_number, key, value)
                    {
                        config.padding_x = parsed;
                    }
                }
                RootSettingId::PaddingY => {
                    if let Some(parsed) =
                        parse_non_negative_f32_field(&mut diagnostics, line_number, key, value)
                    {
                        config.padding_y = parsed;
                    }
                }
                RootSettingId::MouseScrollMultiplier => {
                    if let Some(parsed) = parse_finite_f32_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a finite number",
                    ) {
                        config.mouse_scroll_multiplier =
                            parsed.clamp(MIN_MOUSE_SCROLL_MULTIPLIER, MAX_MOUSE_SCROLL_MULTIPLIER);
                    }
                }
                RootSettingId::ScrollbarVisibility => {
                    if let Some(parsed) = TerminalScrollbarVisibility::from_str(value) {
                        config.terminal_scrollbar_visibility = parsed;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: off, always, on_scroll",
                        );
                    }
                }
                RootSettingId::ScrollbarStyle => {
                    if let Some(parsed) = TerminalScrollbarStyle::from_str(value) {
                        config.terminal_scrollbar_style = parsed;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: neutral, muted_theme, theme",
                        );
                    }
                }
                RootSettingId::ScrollbackHistory => {
                    if let Some(parsed) =
                        parse_usize_field(&mut diagnostics, line_number, key, value)
                    {
                        config.scrollback_history = parsed.min(MAX_SCROLLBACK_HISTORY);
                    }
                }
                RootSettingId::InactiveTabScrollback => {
                    if value.trim().eq_ignore_ascii_case("none") {
                        config.inactive_tab_scrollback = None;
                    } else if let Some(parsed) =
                        parse_usize_field(&mut diagnostics, line_number, key, value)
                    {
                        config.inactive_tab_scrollback = Some(parsed.min(MAX_SCROLLBACK_HISTORY));
                    }
                }
                RootSettingId::PaneFocusEffect => {
                    if let Some(parsed) = PaneFocusEffect::from_str(value) {
                        config.pane_focus_effect = parsed;
                    } else {
                        push_invalid_value(
                            &mut diagnostics,
                            line_number,
                            key,
                            value,
                            "one of: off, soft_spotlight, cinematic, minimal",
                        );
                    }
                }
                RootSettingId::PaneFocusStrength => {
                    if let Some(parsed) = parse_f32_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a number between 0.0 and 2.0",
                    ) {
                        if parsed.is_finite() {
                            config.pane_focus_strength = parsed.clamp(0.0, MAX_PANE_FOCUS_STRENGTH);
                        } else {
                            push_invalid_value(
                                &mut diagnostics,
                                line_number,
                                key,
                                value,
                                "a number between 0.0 and 2.0",
                            );
                        }
                    }
                }
                RootSettingId::CopyOnSelect => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.copy_on_select = parsed;
                    }
                }
                RootSettingId::CopyOnSelectToast => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.copy_on_select_toast = parsed;
                    }
                }
                RootSettingId::CommandPaletteShowKeybinds => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.command_palette_show_keybinds = parsed;
                    }
                }
                RootSettingId::ShellIntegrationEnabled => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.shell_integration_enabled = parsed;
                    }
                }
                RootSettingId::ProgressIndicatorEnabled => {
                    if let Some(parsed) =
                        parse_bool_field(&mut diagnostics, line_number, key, value)
                    {
                        config.progress_indicator_enabled = parsed;
                    }
                }
                RootSettingId::Keybind => {
                    if let Some(parsed) = parse_string_field(
                        &mut diagnostics,
                        line_number,
                        key,
                        value,
                        "a keybind directive",
                    ) {
                        config.keybind_lines.push(KeybindConfigLine {
                            line_number,
                            value: parsed,
                        });
                    }
                }
            }
        }

        if !tab_title_priority_overridden {
            config.tab_title.priority = config.tab_title.mode.default_priority();
        }

        for (task_name, task) in task_entries {
            let Some(command) = task.command else {
                diagnostics.push(ConfigDiagnostic {
                    line_number: task.first_line,
                    kind: ConfigDiagnosticKind::InvalidValue,
                    message: format!("Task '{task_name}' is missing required field 'command'"),
                });
                continue;
            };

            config.tasks.push(TaskConfig {
                name: task_name,
                command,
                layout: task.layout,
                working_dir: task.working_dir,
                keybind: task.keybind,
            });
        }

        ConfigParseReport::new(config, diagnostics)
    }
}

pub fn parse_theme_id(value: &str) -> Option<ThemeId> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let normalized = termy_theme_core::normalize_theme_id(value);
    if matches!(
        normalized.as_str(),
        "shell" | "shell-decide" | "let-shell-decide"
    ) {
        return Some(SHELL_DECIDE_THEME_ID.to_string());
    }

    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn is_ignored_removed_root_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "browser_tabs_enabled"
            | "notifications_enabled"
            | "notification_min_duration"
            | "notify_only_unfocused"
    )
}

fn parse_task_key(key: &str) -> Result<Option<(&str, &str)>, TaskKeyParseError> {
    let trimmed = key.trim();
    let Some(rest) = trimmed.strip_prefix("task.") else {
        return Ok(None);
    };
    let Some((task_name, field)) = rest.rsplit_once('.') else {
        return Ok(None);
    };
    let task_name = task_name.trim();
    let field = field.trim();
    if task_name.is_empty() || field.is_empty() {
        return Ok(None);
    }
    if task_name.contains('.') {
        return Err(TaskKeyParseError::InvalidTaskName);
    }
    Ok(Some((task_name, field)))
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_bool_field(
    diagnostics: &mut Vec<ConfigDiagnostic>,
    line_number: usize,
    key: &str,
    value: &str,
) -> Option<bool> {
    parse_bool(value).or_else(|| {
        push_invalid_value(diagnostics, line_number, key, value, "a boolean value");
        None
    })
}

fn parse_f32_field(
    diagnostics: &mut Vec<ConfigDiagnostic>,
    line_number: usize,
    key: &str,
    value: &str,
    expected: &str,
) -> Option<f32> {
    value.parse::<f32>().ok().or_else(|| {
        push_invalid_value(diagnostics, line_number, key, value, expected);
        None
    })
}

fn parse_positive_f32_field(
    diagnostics: &mut Vec<ConfigDiagnostic>,
    line_number: usize,
    key: &str,
    value: &str,
) -> Option<f32> {
    let parsed = parse_finite_f32_field(diagnostics, line_number, key, value, "a positive number")?;
    if parsed > 0.0 {
        Some(parsed)
    } else {
        push_invalid_value(diagnostics, line_number, key, value, "a positive number");
        None
    }
}

fn parse_non_negative_f32_field(
    diagnostics: &mut Vec<ConfigDiagnostic>,
    line_number: usize,
    key: &str,
    value: &str,
) -> Option<f32> {
    let parsed = parse_finite_f32_field(diagnostics, line_number, key, value, "a number >= 0")?;
    if parsed >= 0.0 {
        Some(parsed)
    } else {
        push_invalid_value(diagnostics, line_number, key, value, "a number >= 0");
        None
    }
}

fn parse_finite_f32_field(
    diagnostics: &mut Vec<ConfigDiagnostic>,
    line_number: usize,
    key: &str,
    value: &str,
    expected: &str,
) -> Option<f32> {
    let parsed = parse_f32_field(diagnostics, line_number, key, value, expected)?;
    if parsed.is_finite() {
        Some(parsed)
    } else {
        push_invalid_value(diagnostics, line_number, key, value, expected);
        None
    }
}

fn parse_usize_field(
    diagnostics: &mut Vec<ConfigDiagnostic>,
    line_number: usize,
    key: &str,
    value: &str,
) -> Option<usize> {
    value.parse::<usize>().ok().or_else(|| {
        push_invalid_value(diagnostics, line_number, key, value, "a positive integer");
        None
    })
}

fn parse_string_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let unquoted = if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    let unquoted = unquoted.trim();
    if unquoted.is_empty() {
        return None;
    }

    Some(unquoted.to_string())
}

fn parse_string_field(
    diagnostics: &mut Vec<ConfigDiagnostic>,
    line_number: usize,
    key: &str,
    value: &str,
    expected: &str,
) -> Option<String> {
    parse_string_value(value).or_else(|| {
        push_invalid_value(diagnostics, line_number, key, value, expected);
        None
    })
}

fn parse_optional_string_value(value: &str) -> Option<String> {
    let parsed = parse_string_value(value)?;
    let normalized = parsed.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "none" | "unset" | "default" | "auto") {
        return None;
    }
    Some(parsed)
}

fn parse_tab_title_priority(value: &str) -> Option<Vec<TabTitleSource>> {
    let mut priority = Vec::new();
    for token in value.split(',') {
        let Some(source) = TabTitleSource::from_str(token) else {
            continue;
        };

        if !priority.contains(&source) {
            priority.push(source);
        }
    }

    if priority.is_empty() {
        return None;
    }

    Some(priority)
}

fn push_invalid_value(
    diagnostics: &mut Vec<ConfigDiagnostic>,
    line_number: usize,
    key: &str,
    value: &str,
    expected: &str,
) {
    diagnostics.push(ConfigDiagnostic {
        line_number,
        kind: ConfigDiagnosticKind::InvalidValue,
        message: format!("Invalid value '{value}' for '{key}': expected {expected}"),
    });
}
