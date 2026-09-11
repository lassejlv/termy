use super::search::SettingMetadata;
use super::*;
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

const FALLBACK_SETTING_METADATA: SettingMetadata = SettingMetadata {
    key: "__missing_setting_metadata__",
    section: SettingsSection::Advanced,
    title: "Unknown setting",
    description: "Metadata unavailable for this setting.",
    keywords: &[],
};

static LOGGED_MISSING_METADATA_KEYS: LazyLock<Mutex<HashSet<&'static str>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

impl SettingsWindow {
    pub(super) fn setting_metadata_or_fallback(key: &'static str) -> &'static SettingMetadata {
        if let Some(metadata) = Self::setting_metadata(key) {
            return metadata;
        }

        let mut logged = LOGGED_MISSING_METADATA_KEYS
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if logged.insert(key) {
            log::error!("Missing settings metadata for key '{key}'");
        }

        &FALLBACK_SETTING_METADATA
    }

    pub(super) fn render_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w_full()
            .child(match self.active_section {
                SettingsSection::Appearance => {
                    self.render_appearance_section(cx).into_any_element()
                }
                SettingsSection::Terminal => self.render_terminal_section(cx).into_any_element(),
                SettingsSection::Ssh => self.render_ssh_section(cx).into_any_element(),
                SettingsSection::Tabs => self.render_tabs_section(cx).into_any_element(),
                SettingsSection::ThemeStore => {
                    self.render_theme_store_section(cx).into_any_element()
                }
                SettingsSection::Plugins => self.render_plugins_section(cx).into_any_element(),
                SettingsSection::Advanced => self.render_advanced_section(cx).into_any_element(),
                SettingsSection::Colors => self.render_colors_section(cx).into_any_element(),
                SettingsSection::Keybindings => {
                    self.render_keybindings_section(cx).into_any_element()
                }
            })
            .into_any_element()
    }

    pub(super) fn render_appearance_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let background_blur = self.config.background_blur;
        let background_opacity_cells = self.config.background_opacity_cells;
        let theme = self.config.theme.clone();
        let theme_mode_value = self.editable_field_value(EditableField::ThemeMode);
        let theme_light = self.config.theme_light.clone();
        let theme_dark = self.config.theme_dark.clone();
        #[cfg(target_os = "macos")]
        let app_icon = self.config.app_icon;
        let theme_mode_is_system = matches!(
            self.config.theme_mode,
            termy_config_core::AppearanceMode::System
        );
        let chrome_contrast = self.config.chrome_contrast;
        let font_family = self.config.font_family.clone();
        let ui_font_family = self.config.ui_font_family.clone();
        let font_size = self.config.font_size;
        let line_height = self.config.line_height;
        let padding_x = self.config.padding_x;
        let padding_y = self.config.padding_y;
        let theme_meta = Self::setting_metadata_or_fallback("theme");
        let theme_mode_meta = Self::setting_metadata_or_fallback("theme_mode");
        let theme_light_meta = Self::setting_metadata_or_fallback("theme_light");
        let theme_dark_meta = Self::setting_metadata_or_fallback("theme_dark");
        #[cfg(target_os = "macos")]
        let app_icon_meta = Self::setting_metadata_or_fallback("app_icon");
        let opacity_meta = Self::setting_metadata_or_fallback("background_opacity");
        let font_family_meta = Self::setting_metadata_or_fallback("font_family");
        let ui_font_family_meta = Self::setting_metadata_or_fallback("ui_font_family");
        let font_size_meta = Self::setting_metadata_or_fallback("font_size");
        let line_height_meta = Self::setting_metadata_or_fallback("line_height");
        let padding_x_meta = Self::setting_metadata_or_fallback("padding_x");
        let padding_y_meta = Self::setting_metadata_or_fallback("padding_y");

        let mut theme_rows = vec![self.render_editable_row(
            "theme_mode",
            EditableField::ThemeMode,
            theme_mode_meta.title,
            theme_mode_meta.description,
            theme_mode_value,
            cx,
        )];
        if theme_mode_is_system {
            theme_rows.push(self.render_editable_row(
                "theme_light",
                EditableField::ThemeLight,
                theme_light_meta.title,
                theme_light_meta.description,
                theme_light,
                cx,
            ));
            theme_rows.push(self.render_editable_row(
                "theme_dark",
                EditableField::ThemeDark,
                theme_dark_meta.title,
                theme_dark_meta.description,
                theme_dark,
                cx,
            ));
        } else {
            theme_rows.push(self.render_editable_row(
                "theme",
                EditableField::Theme,
                theme_meta.title,
                theme_meta.description,
                theme,
                cx,
            ));
        }
        let theme_group = self.render_settings_group("Theme", theme_rows);

        let mut window_rows = Vec::new();
        #[cfg(target_os = "macos")]
        window_rows.push(
            self.render_editable_row(
                "app_icon",
                EditableField::AppIcon,
                app_icon_meta.title,
                app_icon_meta.description,
                match app_icon {
                    termy_config_core::AppIcon::TermyDefault => "default",
                    termy_config_core::AppIcon::TermyOld => "old",
                }
                .to_string(),
                cx,
            ),
        );
        window_rows.extend([
            self.render_root_bool_setting_row(
                "chrome_contrast",
                "chrome-contrast-toggle",
                RootSettingId::ChromeContrast,
                chrome_contrast,
                "Saved",
                cx,
            ),
            self.render_root_bool_setting_row(
                "background_blur",
                "blur-toggle",
                RootSettingId::BackgroundBlur,
                background_blur,
                "Saved",
                cx,
            ),
            self.render_background_opacity_row(
                "background_opacity",
                opacity_meta.title,
                opacity_meta.description,
                cx,
            ),
            self.render_root_bool_setting_row(
                "background_opacity_cells",
                "background-opacity-cells-toggle",
                RootSettingId::BackgroundOpacityCells,
                background_opacity_cells,
                "Saved",
                cx,
            ),
        ]);
        let window_group = self.render_settings_group("Window", window_rows);

        let font_rows = vec![
            self.render_editable_row(
                "font_family",
                EditableField::FontFamily,
                font_family_meta.title,
                font_family_meta.description,
                font_family,
                cx,
            ),
            self.render_editable_row(
                "ui_font_family",
                EditableField::UiFontFamily,
                ui_font_family_meta.title,
                ui_font_family_meta.description,
                ui_font_family,
                cx,
            ),
            self.render_editable_row(
                "font_size",
                EditableField::FontSize,
                font_size_meta.title,
                font_size_meta.description,
                format!("{}px", font_size as i32),
                cx,
            ),
            self.render_editable_row(
                "line_height",
                EditableField::LineHeight,
                line_height_meta.title,
                line_height_meta.description,
                format!("{line_height:.2}"),
                cx,
            ),
            self.render_editable_row(
                "padding_x",
                EditableField::PaddingX,
                padding_x_meta.title,
                padding_x_meta.description,
                format!("{}px", padding_x as i32),
                cx,
            ),
            self.render_editable_row(
                "padding_y",
                EditableField::PaddingY,
                padding_y_meta.title,
                padding_y_meta.description,
                format!("{}px", padding_y as i32),
                cx,
            ),
        ];
        let typography_group = self.render_settings_group("Typography & spacing", font_rows);

        div()
            .flex()
            .flex_col()
            .gap(px(CARD_GAP))
            .child(self.render_section_header(
                "Appearance",
                "Customize the look and feel",
                SettingsSection::Appearance,
                cx,
            ))
            .child(theme_group)
            .child(window_group)
            .child(typography_group)
    }

    pub(super) fn render_settings_group(
        &self,
        title: impl Into<SharedString>,
        rows: Vec<AnyElement>,
    ) -> AnyElement {
        // Card chrome, label, and the hairlines between rows all come from the
        // design system now; the rows themselves stay owned by this view.
        termy_ui::SettingsGroup::new(title.into())
            .children(rows)
            .into_any_element()
    }

    pub(super) fn render_terminal_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let section = div()
            .flex()
            .flex_col()
            .gap(px(CARD_GAP))
            .child(self.render_section_header(
                "Terminal",
                "Configure terminal behavior",
                SettingsSection::Terminal,
                cx,
            ))
            .child(self.render_terminal_cursor_group(cx));

        #[cfg(target_os = "macos")]
        let section = section.child(self.render_terminal_keyboard_group(cx));

        let section = section.child(self.render_terminal_shell_group(cx));

        let section = section.child(self.render_terminal_tmux_group(cx));

        section
            .child(self.render_terminal_scrolling_group(cx))
            .child(self.render_terminal_clipboard_group(cx))
            .child(self.render_terminal_ui_group(cx))
    }

    pub(super) fn render_terminal_cursor_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let cursor_style_meta = Self::setting_metadata_or_fallback("cursor_style");
        let cursor_blink = self.config.cursor_blink;

        let rows = vec![
            self.render_root_bool_setting_row(
                "cursor_blink",
                "cursor_blink-toggle",
                RootSettingId::CursorBlink,
                cursor_blink,
                "Saved",
                cx,
            ),
            self.render_editable_row(
                "cursor_style",
                EditableField::CursorStyle,
                cursor_style_meta.title,
                cursor_style_meta.description,
                self.editable_field_value(EditableField::CursorStyle),
                cx,
            ),
        ];
        self.render_settings_group("Cursor", rows)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn render_terminal_keyboard_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let macos_option_as_alt = self.config.macos_option_as_alt;
        let rows = vec![self.render_root_bool_setting_row(
            "macos_option_as_alt",
            "macos_option_as_alt-toggle",
            RootSettingId::MacosOptionAsAlt,
            macos_option_as_alt,
            "Saved",
            cx,
        )];
        self.render_settings_group("Keyboard", rows)
    }

    pub(super) fn render_terminal_shell_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        #[cfg(target_os = "windows")]
        let windows_shell_meta = Self::setting_metadata_or_fallback("windows_shell");
        let shell_meta = Self::setting_metadata_or_fallback("shell");
        let term_meta = Self::setting_metadata_or_fallback("term");
        let colorterm_meta = Self::setting_metadata_or_fallback("colorterm");
        #[cfg(target_os = "windows")]
        let shell = self
            .config
            .shell
            .clone()
            .unwrap_or_else(|| "Uses Windows Shell".to_string());
        #[cfg(not(target_os = "windows"))]
        let shell = self
            .config
            .shell
            .clone()
            .unwrap_or_else(|| "System default".to_string());
        let term = self.config.term.clone();
        let colorterm = self
            .config
            .colorterm
            .clone()
            .unwrap_or_else(|| "Disabled".to_string());

        let mut rows = Vec::new();
        #[cfg(target_os = "windows")]
        rows.push(self.render_editable_row(
            "windows_shell",
            EditableField::WindowsShell,
            windows_shell_meta.title,
            windows_shell_meta.description,
            self.editable_field_value(EditableField::WindowsShell),
            cx,
        ));

        rows.extend([
            self.render_editable_row(
                "shell",
                EditableField::Shell,
                shell_meta.title,
                shell_meta.description,
                shell,
                cx,
            ),
            self.render_editable_row(
                "term",
                EditableField::Term,
                term_meta.title,
                term_meta.description,
                term,
                cx,
            ),
            self.render_editable_row(
                "colorterm",
                EditableField::Colorterm,
                colorterm_meta.title,
                colorterm_meta.description,
                colorterm,
                cx,
            ),
        ]);
        self.render_settings_group("Shell environment", rows)
    }

    pub(super) fn render_terminal_tmux_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let binary_meta = Self::setting_metadata_or_fallback("tmux_binary");
        let command_prefix_meta = Self::setting_metadata_or_fallback("tmux_command_prefix");
        let tmux_enabled = self.config.tmux_enabled;
        let tmux_persistence = self.config.tmux_persistence;
        let tmux_exclusive = self.config.tmux_exclusive;
        let tmux_show_active_pane_border = self.config.tmux_show_active_pane_border;
        let binary = self.config.tmux_binary.clone();
        let command_prefix = self.config.tmux_command_prefix.clone().unwrap_or_default();

        let mut rows = vec![self.render_root_bool_setting_row(
            "tmux_enabled",
            "tmux_enabled-toggle",
            RootSettingId::TmuxEnabled,
            tmux_enabled,
            "Saved. Use Tmux Sessions to switch runtime now.",
            cx,
        )];

        if tmux_enabled {
            rows.push(self.render_root_bool_setting_row(
                "tmux_persistence",
                "tmux_persistence-toggle",
                RootSettingId::TmuxPersistence,
                tmux_persistence,
                "Saved",
                cx,
            ));
            rows.push(self.render_root_bool_setting_row(
                "tmux_exclusive",
                "tmux_exclusive-toggle",
                RootSettingId::TmuxExclusive,
                tmux_exclusive,
                "Saved",
                cx,
            ));
            rows.push(self.render_root_bool_setting_row(
                "tmux_show_active_pane_border",
                "tmux_show_active_pane_border-toggle",
                RootSettingId::TmuxShowActivePaneBorder,
                tmux_show_active_pane_border,
                "Saved",
                cx,
            ));
            rows.push(self.render_editable_row(
                "tmux_binary",
                EditableField::TmuxBinary,
                binary_meta.title,
                binary_meta.description,
                binary,
                cx,
            ));
            rows.push(self.render_editable_row(
                "tmux_command_prefix",
                EditableField::TmuxCommandPrefix,
                command_prefix_meta.title,
                command_prefix_meta.description,
                command_prefix,
                cx,
            ));
        }

        self.render_settings_group("tmux", rows)
    }

    pub(super) fn render_terminal_scrolling_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let scrollback_meta = Self::setting_metadata_or_fallback("scrollback_history");
        let scroll_mult_meta = Self::setting_metadata_or_fallback("mouse_scroll_multiplier");
        let inactive_scrollback_meta =
            Self::setting_metadata_or_fallback("inactive_tab_scrollback");
        let scrollbar_visibility_meta = Self::setting_metadata_or_fallback("scrollbar_visibility");
        let scrollbar_style_meta = Self::setting_metadata_or_fallback("scrollbar_style");
        let scrollback = self.config.scrollback_history;
        let inactive_scrollback = self.config.inactive_tab_scrollback.unwrap_or(0);
        let scroll_mult = self.config.mouse_scroll_multiplier;

        let rows = vec![
            self.render_editable_row(
                "scrollback_history",
                EditableField::ScrollbackHistory,
                scrollback_meta.title,
                scrollback_meta.description,
                format!("{scrollback} lines"),
                cx,
            ),
            self.render_editable_row(
                "inactive_tab_scrollback",
                EditableField::InactiveTabScrollback,
                inactive_scrollback_meta.title,
                inactive_scrollback_meta.description,
                format!("{inactive_scrollback} lines"),
                cx,
            ),
            self.render_editable_row(
                "mouse_scroll_multiplier",
                EditableField::ScrollMultiplier,
                scroll_mult_meta.title,
                scroll_mult_meta.description,
                format!("{scroll_mult}x"),
                cx,
            ),
            self.render_editable_row(
                "scrollbar_visibility",
                EditableField::ScrollbarVisibility,
                scrollbar_visibility_meta.title,
                scrollbar_visibility_meta.description,
                self.editable_field_value(EditableField::ScrollbarVisibility),
                cx,
            ),
            self.render_editable_row(
                "scrollbar_style",
                EditableField::ScrollbarStyle,
                scrollbar_style_meta.title,
                scrollbar_style_meta.description,
                self.editable_field_value(EditableField::ScrollbarStyle),
                cx,
            ),
        ];
        self.render_settings_group("Scrolling", rows)
    }

    pub(super) fn render_terminal_clipboard_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let copy_on_select = self.config.copy_on_select;
        let copy_on_select_toast = self.config.copy_on_select_toast;

        let rows = vec![
            self.render_root_bool_setting_row(
                "copy_on_select",
                "copy_on_select-toggle",
                RootSettingId::CopyOnSelect,
                copy_on_select,
                "Saved",
                cx,
            ),
            self.render_root_bool_setting_row(
                "copy_on_select_toast",
                "copy_on_select_toast-toggle",
                RootSettingId::CopyOnSelectToast,
                copy_on_select_toast,
                "Saved",
                cx,
            ),
        ];
        self.render_settings_group("Clipboard", rows)
    }

    pub(super) fn render_terminal_ui_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let pane_focus_effect_meta = Self::setting_metadata_or_fallback("pane_focus_effect");
        let pane_focus_strength_meta = Self::setting_metadata_or_fallback("pane_focus_strength");
        let pane_focus_strength_percent = self.pane_focus_strength_display_percent();
        let command_palette_show_keybinds = self.config.command_palette_show_keybinds;

        let rows = vec![
            self.render_editable_row(
                "pane_focus_effect",
                EditableField::PaneFocusEffect,
                pane_focus_effect_meta.title,
                pane_focus_effect_meta.description,
                self.editable_field_value(EditableField::PaneFocusEffect),
                cx,
            ),
            self.render_editable_row(
                "pane_focus_strength",
                EditableField::PaneFocusStrength,
                pane_focus_strength_meta.title,
                pane_focus_strength_meta.description,
                format!("{pane_focus_strength_percent}%"),
                cx,
            ),
            self.render_root_bool_setting_row(
                "command_palette_show_keybinds",
                "command_palette_show_keybinds-toggle",
                RootSettingId::CommandPaletteShowKeybinds,
                command_palette_show_keybinds,
                "Saved",
                cx,
            ),
        ];
        self.render_settings_group("Terminal interface", rows)
    }

    pub(super) fn render_tabs_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(CARD_GAP))
            .child(self.render_section_header(
                "Tabs",
                "Configure tab behavior and titles",
                SettingsSection::Tabs,
                cx,
            ))
            .child(self.render_tabs_title_group(cx))
            .child(self.render_tabs_strip_group(cx))
            .child(self.render_tabs_sidebar_group(cx))
            .child(self.render_tabs_titlebar_group(cx))
    }

    pub(super) fn render_theme_store_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.ensure_theme_store_themes_loaded(cx);

        let toolbar = self.render_theme_store_toolbar(cx);
        let status = self.render_theme_store_status(cx);
        let grid = self.render_theme_store_grid(cx);

        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(self.render_section_header(
                "Themes",
                "Browse and install community themes",
                SettingsSection::ThemeStore,
                cx,
            ))
            .child(toolbar)
            .when_some(status, |s, status| s.child(status))
            .child(grid)
    }

    pub(super) fn render_tabs_title_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let shell_integration = self.config.tab_title.shell_integration;
        let fallback = self.config.tab_title.fallback.clone();
        let title_priority = self.editable_field_value(EditableField::TabTitlePriority);
        let explicit_prefix = self.config.tab_title.explicit_prefix.clone();
        let prompt_format = self.config.tab_title.prompt_format.clone();
        let command_format = self.config.tab_title.command_format.clone();
        let title_mode_meta = Self::setting_metadata_or_fallback("tab_title_mode");
        let fallback_meta = Self::setting_metadata_or_fallback("tab_title_fallback");
        let title_priority_meta = Self::setting_metadata_or_fallback("tab_title_priority");
        let explicit_prefix_meta = Self::setting_metadata_or_fallback("tab_title_explicit_prefix");
        let prompt_format_meta = Self::setting_metadata_or_fallback("tab_title_prompt_format");
        let command_format_meta = Self::setting_metadata_or_fallback("tab_title_command_format");

        let rows = vec![
            self.render_editable_row(
                "tab_title_mode",
                EditableField::TabTitleMode,
                title_mode_meta.title,
                title_mode_meta.description,
                self.editable_field_value(EditableField::TabTitleMode),
                cx,
            ),
            self.render_root_bool_setting_row(
                "tab_title_shell_integration",
                "tab_title_shell_integration-toggle",
                RootSettingId::TabTitleShellIntegration,
                shell_integration,
                "Saved",
                cx,
            ),
            self.render_editable_row(
                "tab_title_fallback",
                EditableField::TabFallbackTitle,
                fallback_meta.title,
                fallback_meta.description,
                fallback,
                cx,
            ),
            self.render_editable_row(
                "tab_title_priority",
                EditableField::TabTitlePriority,
                title_priority_meta.title,
                title_priority_meta.description,
                title_priority,
                cx,
            ),
            self.render_editable_row(
                "tab_title_explicit_prefix",
                EditableField::TabTitleExplicitPrefix,
                explicit_prefix_meta.title,
                explicit_prefix_meta.description,
                explicit_prefix,
                cx,
            ),
            self.render_editable_row(
                "tab_title_prompt_format",
                EditableField::TabTitlePromptFormat,
                prompt_format_meta.title,
                prompt_format_meta.description,
                prompt_format,
                cx,
            ),
            self.render_editable_row(
                "tab_title_command_format",
                EditableField::TabTitleCommandFormat,
                command_format_meta.title,
                command_format_meta.description,
                command_format,
                cx,
            ),
        ];

        self.render_settings_group("Tab titles", rows)
    }

    pub(super) fn render_tabs_strip_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let close_visibility = self.editable_field_value(EditableField::TabCloseVisibility);
        let width_mode = self.editable_field_value(EditableField::TabWidthMode);
        let bar_position = self.editable_field_value(EditableField::TabBarPosition);
        let show_switch_hints = self.config.tab_switch_modifier_hints;
        let auto_hide_tabbar = self.config.auto_hide_tabbar;
        let close_visibility_meta = Self::setting_metadata_or_fallback("tab_close_visibility");
        let width_mode_meta = Self::setting_metadata_or_fallback("tab_width_mode");
        let bar_position_meta = Self::setting_metadata_or_fallback("tab_bar_position");
        let mut rows = vec![
            self.render_editable_row(
                "tab_bar_position",
                EditableField::TabBarPosition,
                bar_position_meta.title,
                bar_position_meta.description,
                bar_position,
                cx,
            ),
            self.render_editable_row(
                "tab_close_visibility",
                EditableField::TabCloseVisibility,
                close_visibility_meta.title,
                close_visibility_meta.description,
                close_visibility,
                cx,
            ),
            self.render_editable_row(
                "tab_width_mode",
                EditableField::TabWidthMode,
                width_mode_meta.title,
                width_mode_meta.description,
                width_mode,
                cx,
            ),
            self.render_root_bool_setting_row(
                "tab_switch_modifier_hints",
                "tab_switch_modifier_hints-toggle",
                RootSettingId::TabSwitchModifierHints,
                show_switch_hints,
                "Saved",
                cx,
            ),
        ];

        rows.push(self.render_root_bool_setting_row(
            "auto_hide_tabbar",
            "auto_hide_tabbar-toggle",
            RootSettingId::AutoHideTabbar,
            auto_hide_tabbar,
            "Saved",
            cx,
        ));

        self.render_settings_group("Tab strip", rows)
    }

    pub(super) fn render_tabs_sidebar_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let sidebar_enabled = self.config.sidebar_enabled;
        let sidebar_width = self.config.sidebar_width;
        let sidebar_width_meta = Self::setting_metadata_or_fallback("sidebar_width");
        let rows = vec![
            self.render_root_bool_setting_row(
                "sidebar_enabled",
                "sidebar_enabled-toggle",
                RootSettingId::SidebarEnabled,
                sidebar_enabled,
                "Saved",
                cx,
            ),
            self.render_editable_row(
                "sidebar_width",
                EditableField::SidebarWidth,
                sidebar_width_meta.title,
                sidebar_width_meta.description,
                format!("{}px", sidebar_width as i32),
                cx,
            ),
        ];

        self.render_settings_group("Workspace sidebar", rows)
    }

    pub(super) fn render_tabs_titlebar_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let show_termy = self.config.show_termy_in_titlebar;
        let rows = vec![self.render_root_bool_setting_row(
            "show_termy_in_titlebar",
            "show_termy_in_titlebar-toggle",
            RootSettingId::ShowTermyInTitlebar,
            show_termy,
            "Saved",
            cx,
        )];

        self.render_settings_group("Title bar", rows)
    }

    pub(super) fn render_theme_store_toolbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let border_color = self.border_color();
        let bg_input = self.bg_input();
        let hover_bg = self.bg_hover();
        let text_primary = self.text_primary();
        let text_secondary = self.text_secondary();
        let text_muted = self.text_muted();
        let accent = self.accent();
        let store_url = "https://github.com/termy-org/themes";
        let query_text = self.theme_store_search_state.text().to_string();
        let has_query = !query_text.trim().is_empty();
        let is_search_active = self.theme_store_search_active;
        let normalized_query = query_text.trim().to_ascii_lowercase();
        let total_theme_count = self.theme_store_themes.len();
        let installed_theme_count = self.theme_store_installed_versions.len();

        let search_content = if is_search_active {
            let font = Font {
                family: self.config.ui_font_family.clone().into(),
                ..gpui::font("")
            };
            TextInputElement::new(
                cx.entity(),
                self.focus_handle.clone(),
                font,
                px(SETTINGS_INPUT_TEXT_SIZE),
                text_secondary.into(),
                self.accent_with_alpha(0.3).into(),
                TextInputAlignment::Left,
            )
            .into_any_element()
        } else if has_query {
            div()
                .text_size(px(SETTINGS_INPUT_TEXT_SIZE))
                .text_color(text_secondary)
                .child(query_text)
                .into_any_element()
        } else {
            div()
                .text_size(px(SETTINGS_INPUT_TEXT_SIZE))
                .text_color(text_muted)
                .child("Search themes...")
                .into_any_element()
        };

        let search_input = div()
            .id("theme-store-search-input")
            .h(px(32.0))
            .px_3()
            .rounded(px(SETTINGS_INPUT_RADIUS))
            .bg(bg_input)
            .border_1()
            .border_color(if is_search_active {
                accent
            } else {
                border_color
            })
            .overflow_hidden()
            .cursor_text()
            .flex_1()
            .min_w(px(220.0))
            .flex()
            .items_center()
            .child(
                div()
                    .w_full()
                    .h(px(20.0))
                    .overflow_hidden()
                    .child(search_content),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    view.active_input = None;
                    view.sidebar_search_active = false;
                    view.theme_store_search_active = true;
                    let index = view
                        .theme_store_search_state
                        .character_index_for_point(event.position);
                    if event.modifiers.shift {
                        view.theme_store_search_state.select_to_utf16(index);
                    } else if event.click_count >= 3 {
                        view.theme_store_search_state.select_all();
                    } else if event.click_count == 2 {
                        view.theme_store_search_state.select_token_at_utf16(index);
                    } else {
                        view.theme_store_search_state.set_cursor_utf16(index);
                    }
                    view.theme_store_search_selecting = event.click_count == 1;
                    view.focus_handle.focus(window);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _window, cx| {
                if !view.theme_store_search_selecting || !event.dragging() {
                    return;
                }
                let index = view
                    .theme_store_search_state
                    .character_index_for_point(event.position);
                view.theme_store_search_state.select_to_utf16(index);
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _event: &MouseUpEvent, _window, cx| {
                    if view.theme_store_search_selecting {
                        view.theme_store_search_selecting = false;
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _event: &MouseUpEvent, _window, cx| {
                    if view.theme_store_search_selecting {
                        view.theme_store_search_selecting = false;
                        cx.notify();
                    }
                }),
            );

        let filtered_count = if has_query {
            self.theme_store_themes
                .iter()
                .filter(|theme| {
                    let haystack = format!(
                        "{} {} {}",
                        theme.name.to_ascii_lowercase(),
                        theme.slug.to_ascii_lowercase(),
                        theme.description.to_ascii_lowercase()
                    );
                    haystack.contains(&normalized_query)
                })
                .count()
        } else {
            total_theme_count
        };
        let availability_label = if self.theme_store_loading {
            "Syncing".to_string()
        } else if self.theme_store_error.is_some() {
            "Unavailable".to_string()
        } else if has_query {
            format!(
                "{filtered_count} result{}",
                if filtered_count == 1 { "" } else { "s" }
            )
        } else {
            format!("{total_theme_count} available")
        };

        let installed_chip: Option<AnyElement> = if installed_theme_count > 0 {
            Some(
                div()
                    .h(px(26.0))
                    .px_2()
                    .rounded(px(SETTINGS_INPUT_RADIUS))
                    .text_xs()
                    .text_color(text_muted)
                    .whitespace_nowrap()
                    .flex()
                    .items_center()
                    .child(format!("· {installed_theme_count} installed"))
                    .into_any_element(),
            )
        } else {
            None
        };

        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(search_input)
            .child(
                div()
                    .text_xs()
                    .text_color(text_secondary)
                    .whitespace_nowrap()
                    .child(availability_label),
            )
            .when_some(installed_chip, |s, chip| s.child(chip))
            .child(div().flex_1())
            .child(
                div()
                    .id("theme-store-open-link")
                    .h(px(28.0))
                    .px_3()
                    .rounded(px(SETTINGS_BUTTON_RADIUS))
                    .border_1()
                    .border_color(border_color)
                    .bg(bg_input)
                    .text_xs()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(text_secondary)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(move |s| s.bg(hover_bg).text_color(text_primary))
                    .child("Open repo ↗")
                    .on_click(cx.listener(move |_view, _, _, cx| {
                        if let Err(error) = SettingsWindow::open_url(store_url) {
                            crate::ui::toast::error(error);
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_theme_store_status(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let bg_elevated = self.bg_elevated();
        let border_color = self.border_color();
        let text_primary = self.text_primary();
        let text_muted = self.text_muted();
        let accent = self.accent();
        let active_border = self.accent_with_alpha(0.55);
        let bg_card = self.bg_card();
        let accent_hover = self.accent_with_alpha(0.8);
        let button_text = self.contrasting_text_for_fill(accent, bg_card);
        let button_hover_text = self.contrasting_text_for_fill(accent_hover, bg_card);

        if self.theme_store_loading {
            return Some(
                div()
                    .py(px(CARD_ROW_PADDING_Y + 2.0))
                    .px(px(CARD_ROW_PADDING_X))
                    .rounded(px(SETTINGS_CARD_RADIUS))
                    .bg(bg_elevated)
                    .border_1()
                    .border_color(border_color)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(text_primary)
                                    .child("Syncing theme registry"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(text_muted)
                                    .child("Fetching the latest themes from GitHub."),
                            ),
                    )
                    .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(accent))
                    .into_any_element(),
            );
        }
        if let Some(error) = self.theme_store_error.clone() {
            return Some(
                div()
                    .py(px(CARD_ROW_PADDING_Y + 2.0))
                    .px(px(CARD_ROW_PADDING_X))
                    .rounded(px(SETTINGS_CARD_RADIUS))
                    .bg(bg_elevated)
                    .border_1()
                    .border_color(active_border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(text_primary)
                                    .child("Registry did not respond"),
                            )
                            .child(div().text_xs().text_color(text_muted).child(error)),
                    )
                    .child(
                        div()
                            .id("theme-store-retry-btn")
                            .h(px(28.0))
                            .px_3()
                            .rounded(px(SETTINGS_BUTTON_RADIUS))
                            .bg(accent)
                            .text_xs()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(button_text)
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(move |s| s.bg(accent_hover).text_color(button_hover_text))
                            .child("Retry")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.theme_store_loaded = false;
                                view.refresh_theme_store_themes(cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        if self.theme_store_from_cache {
            return Some(
                div()
                    .py(px(8.0))
                    .px(px(CARD_ROW_PADDING_X))
                    .rounded(px(SETTINGS_INPUT_RADIUS))
                    .bg(self.accent_with_alpha(0.08))
                    .text_xs()
                    .text_color(text_muted)
                    .child("Showing cached themes — registry unreachable.")
                    .into_any_element(),
            );
        }
        None
    }

    pub(super) fn render_theme_store_grid(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let bg_elevated = self.bg_elevated();
        let border_color = self.border_color();
        let text_primary = self.text_primary();
        let text_muted = self.text_muted();

        if self.theme_store_themes.is_empty()
            && (self.theme_store_loading || self.theme_store_error.is_some())
        {
            return div().into_any_element();
        }

        let query_text = self.theme_store_search_state.text().to_string();
        let normalized_query = query_text.trim().to_ascii_lowercase();

        if self.theme_store_themes.is_empty() {
            return div()
                .py(px(28.0))
                .px(px(CARD_ROW_PADDING_X))
                .rounded(px(SETTINGS_CARD_RADIUS))
                .bg(bg_elevated)
                .border_1()
                .border_color(border_color)
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(text_primary)
                        .child("No themes yet"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(text_muted)
                        .child("Registry is reachable but has no published themes."),
                )
                .into_any_element();
        }

        let filtered_themes: Vec<ThemeStoreTheme> = self
            .theme_store_themes
            .iter()
            .filter(|theme| {
                if normalized_query.is_empty() {
                    return true;
                }
                let haystack = format!(
                    "{} {} {}",
                    theme.name.to_ascii_lowercase(),
                    theme.slug.to_ascii_lowercase(),
                    theme.description.to_ascii_lowercase()
                );
                haystack.contains(&normalized_query)
            })
            .take(30)
            .cloned()
            .collect();

        if filtered_themes.is_empty() {
            return div()
                .py(px(28.0))
                .px(px(CARD_ROW_PADDING_X))
                .rounded(px(SETTINGS_CARD_RADIUS))
                .bg(bg_elevated)
                .border_1()
                .border_color(border_color)
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(text_primary)
                        .child("No matching themes"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(text_muted)
                        .child("Try a different name, slug, or style keyword."),
                )
                .into_any_element();
        }

        let cards: Vec<AnyElement> = filtered_themes
            .into_iter()
            .map(|theme| self.render_theme_store_card(theme, cx))
            .collect();

        div()
            .flex()
            .flex_wrap()
            .justify_center()
            .gap(px(10.0))
            .children(cards)
            .into_any_element()
    }

    pub(super) fn render_theme_store_card(
        &self,
        theme: ThemeStoreTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bg_elevated = self.bg_elevated();
        let bg_input = self.bg_input();
        let hover_bg = self.bg_hover();
        let border_color = self.border_color();
        let text_primary = self.text_primary();
        let text_secondary = self.text_secondary();
        let text_muted = self.text_muted();
        let accent = self.accent();
        let accent_hover = self.accent_with_alpha(0.85);
        let bg_card = self.bg_card();
        let button_text = self.contrasting_text_for_fill(accent, bg_card);
        let button_hover_text = self.contrasting_text_for_fill(accent_hover, bg_card);
        let active_border = self.accent_with_alpha(0.50);

        let version_label = theme
            .latest_version
            .clone()
            .unwrap_or_else(|| "n/a".to_string());
        let slug_key = theme.slug.to_ascii_lowercase();
        let installed_version = self.theme_store_installed_versions.get(&slug_key).cloned();
        let installed_any = installed_version.is_some();
        let is_installed = installed_version.as_ref().is_some_and(|version| {
            theme
                .latest_version
                .as_deref()
                .is_none_or(|latest| version.eq_ignore_ascii_case(latest))
        });
        let has_update = installed_any && !is_installed;
        let description = if theme.description.trim().is_empty() {
            "No description provided.".to_string()
        } else {
            theme.description.clone()
        };

        let install_theme = theme.clone();
        let uninstall_slug = theme.slug.clone();

        let action_button: AnyElement = if is_installed {
            div()
                .id(SharedString::from(format!(
                    "theme-store-uninstall-{}",
                    theme.slug
                )))
                .flex_1()
                .h(px(28.0))
                .px_3()
                .rounded(px(SETTINGS_BUTTON_RADIUS))
                .border_1()
                .border_color(border_color)
                .bg(bg_input)
                .text_xs()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(text_secondary)
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(hover_bg).text_color(text_primary))
                .child("Uninstall")
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.uninstall_theme_store_theme(&uninstall_slug, cx);
                    cx.notify();
                }))
                .into_any_element()
        } else {
            let label = if has_update { "Update" } else { "Install" };
            div()
                .id(SharedString::from(format!(
                    "theme-store-install-{}",
                    theme.slug
                )))
                .flex_1()
                .h(px(28.0))
                .px_3()
                .rounded(px(SETTINGS_BUTTON_RADIUS))
                .bg(accent)
                .text_xs()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(button_text)
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(accent_hover).text_color(button_hover_text))
                .child(label)
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.confirm_install_theme_store_theme(install_theme.clone(), cx);
                }))
                .into_any_element()
        };

        let status_dot_color = if is_installed {
            accent
        } else if has_update {
            self.accent_with_alpha(0.55)
        } else {
            border_color
        };

        let swatch_colors: Vec<Rgba> = {
            let resolved = crate::theme_store::load_installed_theme_colors(&theme.slug)
                .or_else(|| crate::theme_store::load_installed_theme_colors(&theme.name));
            if let Some(colors) = resolved {
                let palette_indices = [1usize, 2, 4, 3, 5, 6, 14];
                palette_indices
                    .iter()
                    .map(|&i| {
                        let c = colors.ansi[i];
                        Rgba {
                            r: c.r as f32 / 255.0,
                            g: c.g as f32 / 255.0,
                            b: c.b as f32 / 255.0,
                            a: 1.0,
                        }
                    })
                    .collect()
            } else {
                let mut muted = text_muted;
                muted.a *= 0.35;
                vec![muted; 7]
            }
        };

        let mut swatch_row = div().flex().items_center().gap(px(5.0));
        for color in swatch_colors {
            swatch_row = swatch_row.child(div().w(px(11.0)).h(px(11.0)).rounded_full().bg(color));
        }

        let theme_name = theme.name;
        let theme_slug = theme.slug;

        div()
            .flex_grow()
            .flex_shrink()
            .flex_basis(px(240.0))
            .min_w(px(240.0))
            .max_w(px(360.0))
            .bg(bg_elevated)
            .border_1()
            .border_color(if is_installed || has_update {
                active_border
            } else {
                border_color
            })
            .rounded(px(SETTINGS_CARD_RADIUS))
            .overflow_hidden()
            .flex()
            .flex_col()
            .p(px(14.0))
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded_full()
                            .bg(status_dot_color),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(text_primary)
                            .overflow_hidden()
                            .child(theme_name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(text_muted)
                            .whitespace_nowrap()
                            .child(format!("v{version_label}")),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(text_muted)
                    .overflow_hidden()
                    .child(theme_slug),
            )
            .child(swatch_row)
            .child(
                div()
                    .text_xs()
                    .text_color(text_secondary)
                    .line_height(px(16.0))
                    .overflow_hidden()
                    .child(description),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(action_button),
            )
            .into_any_element()
    }

    pub(super) fn render_advanced_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let working_dir = self
            .config
            .working_dir
            .clone()
            .unwrap_or_else(|| "Not set".to_string());
        let working_dir_fallback = self.editable_field_value(EditableField::WorkingDirFallback);
        let always_warn_on_quit = self.config.warn_on_quit;
        let warn_on_quit_with_running_process = self.config.warn_on_quit_with_running_process;
        let native_tab_persistence = self.config.native_tab_persistence;
        let native_layout_autosave = self.config.native_layout_autosave;
        let native_buffer_persistence = self.config.native_buffer_persistence;
        let show_debug_overlay = self.config.show_debug_overlay;
        let simple_mode = self.config.simple_mode;
        let window_width = self.config.window_width;
        let window_height = self.config.window_height;
        let bg_card = self.bg_card();
        let border_color = self.border_color();
        let text_muted = self.text_muted();
        let text_secondary = self.text_secondary();
        let accent = self.accent();
        let accent_hover = self.accent_with_alpha(0.8);
        let button_text = self.contrasting_text_for_fill(accent, bg_card);
        let button_hover_text = self.contrasting_text_for_fill(accent_hover, bg_card);
        let working_dir_meta = Self::setting_metadata_or_fallback("working_dir");
        let working_dir_fallback_meta = Self::setting_metadata_or_fallback("working_dir_fallback");
        let window_width_meta = Self::setting_metadata_or_fallback("window_width");
        let window_height_meta = Self::setting_metadata_or_fallback("window_height");
        let config_path_display = self
            .config_path
            .as_ref()
            .map(|path| path.display().to_string())
            .filter(|path| !path.trim().is_empty())
            .unwrap_or_else(|| "config path not set".to_string());

        let startup_rows = vec![
            self.render_editable_row(
                "working_dir",
                EditableField::WorkingDirectory,
                working_dir_meta.title,
                working_dir_meta.description,
                working_dir,
                cx,
            ),
            self.render_editable_row(
                "working_dir_fallback",
                EditableField::WorkingDirFallback,
                working_dir_fallback_meta.title,
                working_dir_fallback_meta.description,
                working_dir_fallback,
                cx,
            ),
            self.render_root_bool_setting_row(
                "native_tab_persistence",
                "native-tab-persistence-toggle",
                RootSettingId::NativeTabPersistence,
                native_tab_persistence,
                "Saved",
                cx,
            ),
            self.render_root_bool_setting_row(
                "native_layout_autosave",
                "native-layout-autosave-toggle",
                RootSettingId::NativeLayoutAutosave,
                native_layout_autosave,
                "Saved",
                cx,
            ),
            self.render_root_bool_setting_row(
                "native_buffer_persistence",
                "native-buffer-persistence-toggle",
                RootSettingId::NativeBufferPersistence,
                native_buffer_persistence,
                "Saved",
                cx,
            ),
        ];
        let startup_group = self.render_settings_group("Startup", startup_rows);

        let safety_rows = vec![
            self.render_root_bool_setting_row(
                "warn_on_quit",
                "warn_on_quit-toggle",
                RootSettingId::WarnOnQuit,
                always_warn_on_quit,
                "Saved",
                cx,
            ),
            self.render_root_bool_setting_row(
                "warn_on_quit_with_running_process",
                "warn_on_quit_with_running_process-toggle",
                RootSettingId::WarnOnQuitWithRunningProcess,
                warn_on_quit_with_running_process,
                "Saved",
                cx,
            ),
        ];
        let safety_group = self.render_settings_group("Safety", safety_rows);

        let window_rows = vec![
            self.render_editable_row(
                "window_width",
                EditableField::WindowWidth,
                window_width_meta.title,
                window_width_meta.description,
                format!("{}px", window_width as i32),
                cx,
            ),
            self.render_editable_row(
                "window_height",
                EditableField::WindowHeight,
                window_height_meta.title,
                window_height_meta.description,
                format!("{}px", window_height as i32),
                cx,
            ),
        ];
        let window_group = self.render_settings_group("Window defaults", window_rows);

        let auto_update = self.config.auto_update;
        let behavior_rows = vec![
            self.render_root_bool_setting_row(
                "auto_update",
                "auto_update-toggle",
                RootSettingId::AutoUpdate,
                auto_update,
                "Saved",
                cx,
            ),
            self.render_root_bool_setting_row(
                "simple_mode",
                "simple_mode-toggle",
                RootSettingId::SimpleMode,
                simple_mode,
                "Saved",
                cx,
            ),
            self.render_root_bool_setting_row(
                "show_debug_overlay",
                "show_debug_overlay-toggle",
                RootSettingId::ShowDebugOverlay,
                show_debug_overlay,
                "Saved",
                cx,
            ),
        ];
        let behavior_group = self.render_settings_group("App behavior", behavior_rows);

        let config_file_card = div()
            .py_4()
            .px_4()
            .rounded(px(SETTINGS_INPUT_RADIUS))
            .bg(bg_card)
            .border_1()
            .border_color(border_color)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(text_muted)
                    .child("Edit the config file directly for settings not shown here:"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(text_secondary)
                    .child(config_path_display),
            )
            .child(
                div()
                    .id("open-config-btn")
                    .mt_2()
                    .px_4()
                    .py_2()
                    .rounded(px(SETTINGS_INPUT_RADIUS))
                    .bg(accent)
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(button_text)
                    .cursor_pointer()
                    .hover(move |s| s.bg(accent_hover).text_color(button_hover_text))
                    .child("Open config file")
                    .on_click(cx.listener(|_view, _, _, cx| {
                        if let Err(error) = crate::config::open_config_file() {
                            log::error!("Failed to open config file from settings: {error}");
                            crate::ui::toast::error(error.to_string());
                        }
                        cx.notify();
                    })),
            )
            .into_any_element();

        let config_group = self.render_settings_group("Config file", vec![config_file_card]);

        div()
            .flex()
            .flex_col()
            .gap(px(CARD_GAP))
            .child(self.render_section_header(
                "General",
                "Startup, windows, updates, and app behavior",
                SettingsSection::Advanced,
                cx,
            ))
            .child(startup_group)
            .child(safety_group)
            .child(window_group)
            .child(behavior_group)
            .child(config_group)
    }
}
