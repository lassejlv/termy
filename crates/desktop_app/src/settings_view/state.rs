use super::*;

const MAX_THEME_SUGGESTIONS: usize = 16;
const MAX_FONT_SUGGESTIONS: usize = 200;
const PANE_FOCUS_MAX: f32 = 2.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum EditableField {
    Theme,
    ThemeMode,
    ThemeLight,
    ThemeDark,
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    AppIcon,
    BackgroundOpacity,
    FontFamily,
    UiFontFamily,
    FontSize,
    LineHeight,
    PaddingX,
    PaddingY,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    WindowsShell,
    Shell,
    Term,
    Colorterm,
    TmuxBinary,
    TmuxCommandPrefix,
    ScrollbackHistory,
    InactiveTabScrollback,
    ScrollMultiplier,
    CursorStyle,
    ScrollbarVisibility,
    ScrollbarStyle,
    PaneFocusEffect,
    PaneFocusStrength,
    TabFallbackTitle,
    TabTitlePriority,
    TabTitleMode,
    TabTitleExplicitPrefix,
    TabTitlePromptFormat,
    TabTitleCommandFormat,
    TabCloseVisibility,
    TabWidthMode,
    TabBarPosition,
    SidebarWidth,
    WorkingDirectory,
    WorkingDirFallback,
    WindowWidth,
    WindowHeight,
    Color(termy_config_core::ColorSettingId),
}

#[derive(Clone, Debug)]
pub(super) struct ActiveTextInput {
    pub(super) field: EditableField,
    pub(super) state: TextInputState,
    pub(super) selecting: bool,
}

#[derive(Clone, Debug)]
pub(super) struct DropdownOption {
    pub(super) value: String,
    pub(super) label: String,
    pub(super) show_raw_value: bool,
}

impl DropdownOption {
    pub(super) fn raw(value: String) -> Self {
        Self {
            label: value.clone(),
            value,
            show_raw_value: false,
        }
    }

    pub(super) fn labeled(value: String, label: String, show_raw_value: bool) -> Self {
        Self {
            value,
            label,
            show_raw_value,
        }
    }

    pub(super) fn display_text(&self) -> String {
        if !self.show_raw_value || self.label == self.value {
            self.label.clone()
        } else {
            format!("{} ({})", self.label, self.value)
        }
    }
}

impl ActiveTextInput {
    pub(super) fn new(field: EditableField, text: String) -> Self {
        Self {
            field,
            state: TextInputState::new(text),
            selecting: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FieldCodec {
    Theme,
    FontFamily,
    Enum,
    Numeric,
    Text,
    Color,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct NumericStepSpec {
    pub(super) delta: f32,
    pub(super) min: f32,
    pub(super) max: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct FieldSpec {
    pub(super) root_setting: Option<RootSettingId>,
    pub(super) codec: FieldCodec,
    pub(super) dropdown_click_only: bool,
    pub(super) numeric_step: Option<NumericStepSpec>,
}

impl SettingsWindow {
    pub(super) fn is_secret_field(field: EditableField) -> bool {
        let _ = field;
        false
    }

    pub(super) fn parse_tab_title_source_token(
        token: &str,
    ) -> Option<termy_config_core::TabTitleSource> {
        match token.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(termy_config_core::TabTitleSource::Manual),
            "explicit" => Some(termy_config_core::TabTitleSource::Explicit),
            "shell" | "app" | "terminal" => Some(termy_config_core::TabTitleSource::Shell),
            "fallback" | "default" => Some(termy_config_core::TabTitleSource::Fallback),
            _ => None,
        }
    }

    pub(super) fn custom_color_for_id(
        &self,
        id: termy_config_core::ColorSettingId,
    ) -> Option<termy_config_core::Rgb8> {
        let colors = &self.config.colors;
        match id {
            termy_config_core::ColorSettingId::Foreground => colors.foreground,
            termy_config_core::ColorSettingId::Background => colors.background,
            termy_config_core::ColorSettingId::Cursor => colors.cursor,
            termy_config_core::ColorSettingId::Black => colors.ansi[0],
            termy_config_core::ColorSettingId::Red => colors.ansi[1],
            termy_config_core::ColorSettingId::Green => colors.ansi[2],
            termy_config_core::ColorSettingId::Yellow => colors.ansi[3],
            termy_config_core::ColorSettingId::Blue => colors.ansi[4],
            termy_config_core::ColorSettingId::Magenta => colors.ansi[5],
            termy_config_core::ColorSettingId::Cyan => colors.ansi[6],
            termy_config_core::ColorSettingId::White => colors.ansi[7],
            termy_config_core::ColorSettingId::BrightBlack => colors.ansi[8],
            termy_config_core::ColorSettingId::BrightRed => colors.ansi[9],
            termy_config_core::ColorSettingId::BrightGreen => colors.ansi[10],
            termy_config_core::ColorSettingId::BrightYellow => colors.ansi[11],
            termy_config_core::ColorSettingId::BrightBlue => colors.ansi[12],
            termy_config_core::ColorSettingId::BrightMagenta => colors.ansi[13],
            termy_config_core::ColorSettingId::BrightCyan => colors.ansi[14],
            termy_config_core::ColorSettingId::BrightWhite => colors.ansi[15],
        }
    }

    pub(super) fn set_custom_color_for_id(
        &mut self,
        id: termy_config_core::ColorSettingId,
        value: Option<termy_config_core::Rgb8>,
    ) {
        let colors = &mut self.config.colors;
        match id {
            termy_config_core::ColorSettingId::Foreground => colors.foreground = value,
            termy_config_core::ColorSettingId::Background => colors.background = value,
            termy_config_core::ColorSettingId::Cursor => colors.cursor = value,
            termy_config_core::ColorSettingId::Black => colors.ansi[0] = value,
            termy_config_core::ColorSettingId::Red => colors.ansi[1] = value,
            termy_config_core::ColorSettingId::Green => colors.ansi[2] = value,
            termy_config_core::ColorSettingId::Yellow => colors.ansi[3] = value,
            termy_config_core::ColorSettingId::Blue => colors.ansi[4] = value,
            termy_config_core::ColorSettingId::Magenta => colors.ansi[5] = value,
            termy_config_core::ColorSettingId::Cyan => colors.ansi[6] = value,
            termy_config_core::ColorSettingId::White => colors.ansi[7] = value,
            termy_config_core::ColorSettingId::BrightBlack => colors.ansi[8] = value,
            termy_config_core::ColorSettingId::BrightRed => colors.ansi[9] = value,
            termy_config_core::ColorSettingId::BrightGreen => colors.ansi[10] = value,
            termy_config_core::ColorSettingId::BrightYellow => colors.ansi[11] = value,
            termy_config_core::ColorSettingId::BrightBlue => colors.ansi[12] = value,
            termy_config_core::ColorSettingId::BrightMagenta => colors.ansi[13] = value,
            termy_config_core::ColorSettingId::BrightCyan => colors.ansi[14] = value,
            termy_config_core::ColorSettingId::BrightWhite => colors.ansi[15] = value,
        }
    }

    pub(super) fn default_root_setting_value(setting: RootSettingId) -> Option<String> {
        let defaults = AppConfig::default();
        root_setting_default_value(&defaults, setting)
    }

    fn root_setting_section(section: SettingsSection) -> Option<CoreSettingsSection> {
        match section {
            SettingsSection::Appearance => Some(CoreSettingsSection::Appearance),
            SettingsSection::Terminal => Some(CoreSettingsSection::Terminal),
            SettingsSection::Tabs => Some(CoreSettingsSection::Tabs),
            SettingsSection::Advanced => Some(CoreSettingsSection::Advanced),
            SettingsSection::ThemeStore
            | SettingsSection::Plugins
            | SettingsSection::Ssh
            | SettingsSection::Colors
            | SettingsSection::Keybindings => None,
        }
    }

    fn section_root_settings(section: SettingsSection) -> impl Iterator<Item = RootSettingId> {
        let section = Self::root_setting_section(section);
        root_setting_specs()
            .iter()
            .filter(move |spec| {
                Some(spec.section) == section
                    && !spec.repeatable
                    && Self::root_setting_visible_in_current_settings(spec.id)
            })
            .map(|spec| spec.id)
    }

    pub(super) fn root_setting_visible_in_current_settings(setting: RootSettingId) -> bool {
        if matches!(setting, RootSettingId::AppIcon) {
            return cfg!(target_os = "macos");
        }

        // Inspector height is remembered UI state (saved when the inspector
        // pane is resized), not a setting with a dedicated row.
        if matches!(setting, RootSettingId::InspectorHeight) {
            return false;
        }

        // tmux settings stay visible on Windows: the runtime is reachable
        // there through tmux_command_prefix (e.g. `wsl.exe -e`).
        if cfg!(target_os = "windows") {
            return true;
        }

        !matches!(setting, RootSettingId::WindowsShell)
    }

    fn is_root_setting_at_default(&self, setting: RootSettingId) -> bool {
        let current = root_setting_default_value(&self.config, setting);
        let default = Self::default_root_setting_value(setting);
        current == default
    }

    pub(super) fn is_setting_at_default(&self, setting_key: &str) -> bool {
        if let Some(setting) = root_setting_from_key(setting_key) {
            return self.is_root_setting_at_default(setting);
        }
        if let Some(color_setting) = color_setting_from_key(setting_key) {
            return self.custom_color_for_id(color_setting).is_none();
        }
        false
    }

    pub(super) fn section_has_non_default_values(&self, section: SettingsSection) -> bool {
        match section {
            SettingsSection::Colors => color_setting_specs()
                .iter()
                .any(|spec| self.custom_color_for_id(spec.id).is_some()),
            SettingsSection::ThemeStore | SettingsSection::Plugins | SettingsSection::Ssh => false,
            SettingsSection::Keybindings => !self.config.keybind_lines.is_empty(),
            _ => Self::section_root_settings(section)
                .any(|setting| !self.is_root_setting_at_default(setting)),
        }
    }

    pub(super) fn reset_root_setting_to_default(
        &mut self,
        setting: RootSettingId,
    ) -> Result<(), String> {
        if let Some(default_value) = Self::default_root_setting_value(setting) {
            config::set_root_setting(setting, &default_value)
        } else {
            config::remove_root_setting(setting)
        }
    }

    pub(super) fn reset_setting_to_default(
        &mut self,
        setting_key: &'static str,
        cx: &mut Context<Self>,
    ) {
        let result = if let Some(setting) = root_setting_from_key(setting_key) {
            self.reset_root_setting_to_default(setting)
        } else if let Some(color_setting) = color_setting_from_key(setting_key) {
            config::set_color_setting(color_setting, None)
        } else {
            return;
        };

        match result {
            Ok(()) => {
                let _ = self.reload_config_if_changed(cx);
                self.active_input = None;
                self.capturing_action = None;
                crate::ui::toast::success("Reset to default");
                cx.notify();
            }
            Err(error) => crate::ui::toast::error(error),
        }
    }

    pub(super) fn confirm_reset_section_to_defaults(
        &mut self,
        section: SettingsSection,
        cx: &mut Context<Self>,
    ) {
        let section_name = match section {
            SettingsSection::Appearance => "Appearance",
            SettingsSection::Terminal => "Terminal",
            SettingsSection::Tabs => "Tabs",
            SettingsSection::Advanced => "General",
            SettingsSection::Colors => "Colors",
            SettingsSection::Keybindings => "Keyboard shortcuts",
            SettingsSection::ThemeStore | SettingsSection::Plugins | SettingsSection::Ssh => return,
        };
        let title = "Reset Section";
        let message = format!(
            "Are you sure you want to reset all {section_name} settings to their default values?"
        );

        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let confirmed = termy_native_sdk::confirm(title, &message);
            if !confirmed {
                return;
            }

            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.reset_section_to_defaults(section, cx);
                })
            });
        })
        .detach();
    }

    pub(super) fn confirm_reset_setting_to_default(
        &mut self,
        setting_key: &'static str,
        cx: &mut Context<Self>,
    ) {
        let title = "Reset Setting";
        let message = "Are you sure you want to reset this setting to its default value?";

        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let confirmed = termy_native_sdk::confirm(title, message);
            if !confirmed {
                return;
            }

            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.reset_setting_to_default(setting_key, cx);
                })
            });
        })
        .detach();
    }

    pub(super) fn reset_section_to_defaults(
        &mut self,
        section: SettingsSection,
        cx: &mut Context<Self>,
    ) {
        if section == SettingsSection::Keybindings {
            self.reset_keybinds_to_defaults(cx);
            return;
        }

        let result = match section {
            SettingsSection::Appearance
            | SettingsSection::Terminal
            | SettingsSection::Tabs
            | SettingsSection::Advanced => Self::section_root_settings(section)
                .try_for_each(|setting| self.reset_root_setting_to_default(setting)),
            SettingsSection::Colors => color_setting_specs()
                .iter()
                .try_for_each(|spec| config::set_color_setting(spec.id, None)),
            SettingsSection::ThemeStore | SettingsSection::Plugins | SettingsSection::Ssh => Ok(()),
            SettingsSection::Keybindings => Ok(()),
        };

        match result {
            Ok(()) => {
                let _ = self.reload_config_if_changed(cx);
                self.active_input = None;
                self.capturing_action = None;
                crate::ui::toast::success("Section reset to defaults");
                cx.notify();
            }
            Err(error) => crate::ui::toast::error(error),
        }
    }

    pub(super) fn field_spec(field: EditableField) -> FieldSpec {
        match field {
            EditableField::Theme
            | EditableField::ThemeMode
            | EditableField::ThemeLight
            | EditableField::ThemeDark
            | EditableField::AppIcon
            | EditableField::BackgroundOpacity
            | EditableField::FontFamily
            | EditableField::UiFontFamily
            | EditableField::FontSize
            | EditableField::LineHeight
            | EditableField::PaddingX
            | EditableField::PaddingY => Self::appearance_field_spec(field),
            EditableField::WindowsShell
            | EditableField::Shell
            | EditableField::Term
            | EditableField::Colorterm
            | EditableField::TmuxBinary
            | EditableField::TmuxCommandPrefix
            | EditableField::ScrollbackHistory
            | EditableField::InactiveTabScrollback
            | EditableField::ScrollMultiplier
            | EditableField::CursorStyle
            | EditableField::ScrollbarVisibility
            | EditableField::ScrollbarStyle
            | EditableField::PaneFocusEffect
            | EditableField::PaneFocusStrength => Self::terminal_field_spec(field),
            EditableField::TabFallbackTitle
            | EditableField::TabTitlePriority
            | EditableField::TabTitleMode
            | EditableField::TabTitleExplicitPrefix
            | EditableField::TabTitlePromptFormat
            | EditableField::TabTitleCommandFormat
            | EditableField::TabCloseVisibility
            | EditableField::TabWidthMode
            | EditableField::TabBarPosition
            | EditableField::SidebarWidth => Self::tabs_field_spec(field),
            EditableField::WorkingDirectory
            | EditableField::WorkingDirFallback
            | EditableField::WindowWidth
            | EditableField::WindowHeight => Self::advanced_field_spec(field),
            EditableField::Color(_) => FieldSpec {
                root_setting: None,
                codec: FieldCodec::Color,
                dropdown_click_only: false,
                numeric_step: None,
            },
        }
    }

    pub(super) fn appearance_field_spec(field: EditableField) -> FieldSpec {
        match field {
            EditableField::Theme => FieldSpec {
                root_setting: Some(RootSettingId::Theme),
                codec: FieldCodec::Theme,
                dropdown_click_only: false,
                numeric_step: None,
            },
            EditableField::ThemeMode => Self::enum_field_spec(RootSettingId::ThemeMode),
            EditableField::ThemeLight => FieldSpec {
                root_setting: Some(RootSettingId::ThemeLight),
                codec: FieldCodec::Theme,
                dropdown_click_only: false,
                numeric_step: None,
            },
            EditableField::ThemeDark => FieldSpec {
                root_setting: Some(RootSettingId::ThemeDark),
                codec: FieldCodec::Theme,
                dropdown_click_only: false,
                numeric_step: None,
            },
            EditableField::AppIcon => Self::enum_field_spec(RootSettingId::AppIcon),
            EditableField::BackgroundOpacity => FieldSpec {
                root_setting: Some(RootSettingId::BackgroundOpacity),
                codec: FieldCodec::Numeric,
                dropdown_click_only: false,
                numeric_step: Some(NumericStepSpec {
                    delta: 0.05,
                    min: 0.0,
                    max: 1.0,
                }),
            },
            EditableField::FontFamily => FieldSpec {
                root_setting: Some(RootSettingId::FontFamily),
                codec: FieldCodec::FontFamily,
                dropdown_click_only: false,
                numeric_step: None,
            },
            EditableField::UiFontFamily => FieldSpec {
                root_setting: Some(RootSettingId::UiFontFamily),
                codec: FieldCodec::FontFamily,
                dropdown_click_only: false,
                numeric_step: None,
            },
            EditableField::FontSize => FieldSpec {
                root_setting: Some(RootSettingId::FontSize),
                codec: FieldCodec::Numeric,
                dropdown_click_only: false,
                numeric_step: Some(NumericStepSpec {
                    delta: 1.0,
                    min: 1.0,
                    max: 4096.0,
                }),
            },
            EditableField::LineHeight => FieldSpec {
                root_setting: Some(RootSettingId::LineHeight),
                codec: FieldCodec::Numeric,
                dropdown_click_only: false,
                numeric_step: Some(NumericStepSpec {
                    delta: 0.05,
                    min: termy_config_core::MIN_LINE_HEIGHT,
                    max: termy_config_core::MAX_LINE_HEIGHT,
                }),
            },
            EditableField::PaddingX => FieldSpec {
                root_setting: Some(RootSettingId::PaddingX),
                codec: FieldCodec::Numeric,
                dropdown_click_only: false,
                numeric_step: Some(NumericStepSpec {
                    delta: 1.0,
                    min: 0.0,
                    max: 4096.0,
                }),
            },
            EditableField::PaddingY => FieldSpec {
                root_setting: Some(RootSettingId::PaddingY),
                codec: FieldCodec::Numeric,
                dropdown_click_only: false,
                numeric_step: Some(NumericStepSpec {
                    delta: 1.0,
                    min: 0.0,
                    max: 4096.0,
                }),
            },
            _ => unreachable!("invalid appearance field"),
        }
    }

    pub(super) fn terminal_field_spec(field: EditableField) -> FieldSpec {
        match field {
            EditableField::WindowsShell => Self::enum_field_spec(RootSettingId::WindowsShell),
            EditableField::Shell => Self::text_field_spec(Some(RootSettingId::Shell)),
            EditableField::Term => Self::text_field_spec(Some(RootSettingId::Term)),
            EditableField::Colorterm => Self::text_field_spec(Some(RootSettingId::Colorterm)),
            EditableField::TmuxBinary => Self::text_field_spec(Some(RootSettingId::TmuxBinary)),
            EditableField::TmuxCommandPrefix => {
                Self::text_field_spec(Some(RootSettingId::TmuxCommandPrefix))
            }
            EditableField::ScrollbackHistory => Self::numeric_field_spec(
                RootSettingId::ScrollbackHistory,
                NumericStepSpec {
                    delta: 100.0,
                    min: 0.0,
                    max: 100_000.0,
                },
            ),
            EditableField::InactiveTabScrollback => Self::numeric_field_spec(
                RootSettingId::InactiveTabScrollback,
                NumericStepSpec {
                    delta: 100.0,
                    min: 0.0,
                    max: 100_000.0,
                },
            ),
            EditableField::ScrollMultiplier => Self::numeric_field_spec(
                RootSettingId::MouseScrollMultiplier,
                NumericStepSpec {
                    delta: 0.1,
                    min: 0.1,
                    max: 1000.0,
                },
            ),
            EditableField::CursorStyle => Self::enum_field_spec(RootSettingId::CursorStyle),
            EditableField::ScrollbarVisibility => {
                Self::enum_field_spec(RootSettingId::ScrollbarVisibility)
            }
            EditableField::ScrollbarStyle => Self::enum_field_spec(RootSettingId::ScrollbarStyle),
            EditableField::PaneFocusEffect => Self::enum_field_spec(RootSettingId::PaneFocusEffect),
            EditableField::PaneFocusStrength => Self::numeric_field_spec(
                RootSettingId::PaneFocusStrength,
                NumericStepSpec {
                    delta: 0.05,
                    min: 0.0,
                    max: PANE_FOCUS_MAX,
                },
            ),
            _ => unreachable!("invalid terminal field"),
        }
    }

    pub(super) fn tabs_field_spec(field: EditableField) -> FieldSpec {
        match field {
            EditableField::TabFallbackTitle => {
                Self::text_field_spec(Some(RootSettingId::TabTitleFallback))
            }
            EditableField::TabTitlePriority => {
                Self::text_field_spec(Some(RootSettingId::TabTitlePriority))
            }
            EditableField::TabTitleMode => Self::enum_field_spec(RootSettingId::TabTitleMode),
            EditableField::TabTitleExplicitPrefix => {
                Self::text_field_spec(Some(RootSettingId::TabTitleExplicitPrefix))
            }
            EditableField::TabTitlePromptFormat => {
                Self::text_field_spec(Some(RootSettingId::TabTitlePromptFormat))
            }
            EditableField::TabTitleCommandFormat => {
                Self::text_field_spec(Some(RootSettingId::TabTitleCommandFormat))
            }
            EditableField::TabCloseVisibility => {
                Self::enum_field_spec(RootSettingId::TabCloseVisibility)
            }
            EditableField::TabWidthMode => Self::enum_field_spec(RootSettingId::TabWidthMode),
            EditableField::TabBarPosition => Self::enum_field_spec(RootSettingId::TabBarPosition),
            EditableField::SidebarWidth => Self::numeric_field_spec(
                RootSettingId::SidebarWidth,
                NumericStepSpec {
                    delta: 10.0,
                    min: termy_config_core::MIN_SIDEBAR_WIDTH,
                    max: termy_config_core::MAX_SIDEBAR_WIDTH,
                },
            ),
            _ => unreachable!("invalid tabs field"),
        }
    }

    pub(super) fn advanced_field_spec(field: EditableField) -> FieldSpec {
        match field {
            EditableField::WorkingDirectory => {
                Self::text_field_spec(Some(RootSettingId::WorkingDir))
            }
            EditableField::WorkingDirFallback => {
                Self::enum_field_spec(RootSettingId::WorkingDirFallback)
            }
            EditableField::WindowWidth => Self::numeric_field_spec(
                RootSettingId::WindowWidth,
                NumericStepSpec {
                    delta: 10.0,
                    min: 100.0,
                    max: 10000.0,
                },
            ),
            EditableField::WindowHeight => Self::numeric_field_spec(
                RootSettingId::WindowHeight,
                NumericStepSpec {
                    delta: 10.0,
                    min: 100.0,
                    max: 10000.0,
                },
            ),
            _ => unreachable!("invalid advanced field"),
        }
    }

    pub(super) fn text_field_spec(root_setting: Option<RootSettingId>) -> FieldSpec {
        FieldSpec {
            root_setting,
            codec: FieldCodec::Text,
            dropdown_click_only: false,
            numeric_step: None,
        }
    }

    pub(super) fn numeric_field_spec(
        root_setting: RootSettingId,
        numeric_step: NumericStepSpec,
    ) -> FieldSpec {
        FieldSpec {
            root_setting: Some(root_setting),
            codec: FieldCodec::Numeric,
            dropdown_click_only: false,
            numeric_step: Some(numeric_step),
        }
    }

    pub(super) fn enum_field_spec(root_setting: RootSettingId) -> FieldSpec {
        FieldSpec {
            root_setting: Some(root_setting),
            codec: FieldCodec::Enum,
            dropdown_click_only: true,
            numeric_step: None,
        }
    }

    pub(super) fn root_setting_for_editable_field(field: EditableField) -> Option<RootSettingId> {
        Self::field_spec(field).root_setting
    }

    pub(super) fn enum_root_setting_for_field(field: EditableField) -> Option<RootSettingId> {
        let setting = Self::root_setting_for_editable_field(field)?;
        (root_setting_value_kind(setting) == RootSettingValueKind::Enum).then_some(setting)
    }

    pub(super) fn field_uses_dropdown(field: EditableField) -> bool {
        matches!(
            Self::field_spec(field).codec,
            FieldCodec::Theme | FieldCodec::FontFamily | FieldCodec::Enum
        )
    }

    pub(super) fn field_uses_click_only_dropdown(field: EditableField) -> bool {
        Self::field_spec(field).dropdown_click_only
    }

    pub(super) fn dropdown_option_for_enum_choice(
        setting: RootSettingId,
        value: &str,
        label: &str,
    ) -> DropdownOption {
        let show_raw_value = !matches!(setting, RootSettingId::AppIcon);
        DropdownOption::labeled(value.to_string(), label.to_string(), show_raw_value)
    }

    pub(super) fn normalize_dropdown_query_token(value: &str) -> String {
        value
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|ch| !matches!(ch, '_' | '-' | ' ' | '+'))
            .collect()
    }

    pub(super) fn filtered_enum_suggestions(
        &self,
        field: EditableField,
        query: &str,
    ) -> Vec<DropdownOption> {
        let Some(setting) = Self::enum_root_setting_for_field(field) else {
            return Vec::new();
        };
        let Some(choices) = root_setting_enum_choices(setting) else {
            return Vec::new();
        };

        let mut options = choices
            .iter()
            .map(|choice| {
                Self::dropdown_option_for_enum_choice(setting, choice.value, choice.label)
            })
            .collect::<Vec<_>>();

        let trimmed_query = query.trim();
        let normalized_query = trimmed_query.to_ascii_lowercase();
        let normalized_compact = Self::normalize_dropdown_query_token(trimmed_query);
        if normalized_query.is_empty() {
            let current_value = self.editable_field_value(field);
            if let Some(index) = options
                .iter()
                .position(|option| option.value.eq_ignore_ascii_case(&current_value))
            {
                let selected = options.remove(index);
                options.insert(0, selected);
            } else if !current_value.trim().is_empty() {
                options.insert(0, DropdownOption::raw(current_value));
            }
            return options;
        }

        let mut matched = options
            .into_iter()
            .filter(|option| {
                let value_lower = option.value.to_ascii_lowercase();
                let label_lower = option.label.to_ascii_lowercase();
                let value_compact = Self::normalize_dropdown_query_token(&option.value);
                let label_compact = Self::normalize_dropdown_query_token(&option.label);
                value_lower.contains(&normalized_query)
                    || label_lower.contains(&normalized_query)
                    || (!normalized_compact.is_empty()
                        && (value_compact.contains(&normalized_compact)
                            || label_compact.contains(&normalized_compact)))
            })
            .collect::<Vec<_>>();

        if !trimmed_query.is_empty()
            && !matched.iter().any(|option| {
                option.value.eq_ignore_ascii_case(trimmed_query)
                    || Self::normalize_dropdown_query_token(&option.value) == normalized_compact
            })
        {
            matched.insert(0, DropdownOption::raw(trimmed_query.to_string()));
        }

        matched
    }

    pub(super) fn dropdown_options_for_field(
        &self,
        field: EditableField,
        query: &str,
    ) -> Vec<DropdownOption> {
        if matches!(
            field,
            EditableField::Theme | EditableField::ThemeLight | EditableField::ThemeDark
        ) {
            return self
                .filtered_theme_suggestions(query)
                .into_iter()
                .map(DropdownOption::raw)
                .collect();
        }
        if matches!(
            field,
            EditableField::FontFamily | EditableField::UiFontFamily
        ) {
            return self
                .filtered_font_suggestions(query)
                .into_iter()
                .map(DropdownOption::raw)
                .collect();
        }
        self.filtered_enum_suggestions(field, query)
    }

    pub(super) fn dropdown_display_value(&self, field: EditableField, raw_value: &str) -> String {
        let Some(setting) = Self::enum_root_setting_for_field(field) else {
            return raw_value.to_string();
        };
        let Some(choices) = root_setting_enum_choices(setting) else {
            return raw_value.to_string();
        };
        let Some(choice) = choices
            .iter()
            .find(|choice| choice.value.eq_ignore_ascii_case(raw_value))
        else {
            return raw_value.to_string();
        };
        Self::dropdown_option_for_enum_choice(setting, choice.value, choice.label).display_text()
    }

    pub(super) fn apply_dropdown_selection(
        &mut self,
        field: EditableField,
        selected_value: &str,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.apply_editable_field(field, selected_value) {
            crate::ui::toast::error(error);
            return;
        }
        self.active_input = None;
        crate::ui::toast::success("Saved");
        cx.notify();
    }

    pub(super) fn commit_dropdown_selection(
        &mut self,
        field: EditableField,
        query: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if !Self::field_uses_dropdown(field) {
            return false;
        }

        let dropdown_query = if Self::field_uses_click_only_dropdown(field) {
            ""
        } else {
            query
        };
        let Some(first_option) = self
            .dropdown_options_for_field(field, dropdown_query)
            .into_iter()
            .next()
        else {
            self.cancel_active_input(cx);
            return true;
        };

        self.apply_dropdown_selection(field, &first_option.value, cx);
        true
    }

    pub(super) fn editable_field_value(&self, field: EditableField) -> String {
        match field {
            EditableField::Theme => self.config.theme.clone(),
            EditableField::ThemeMode => match self.config.theme_mode {
                termy_config_core::AppearanceMode::Manual => "manual",
                termy_config_core::AppearanceMode::System => "system",
            }
            .to_string(),
            EditableField::ThemeLight => self.config.theme_light.clone(),
            EditableField::ThemeDark => self.config.theme_dark.clone(),
            EditableField::AppIcon => match self.config.app_icon {
                termy_config_core::AppIcon::TermyDefault => "default",
                termy_config_core::AppIcon::TermyOld => "old",
            }
            .to_string(),
            EditableField::BackgroundOpacity => format!(
                "{}",
                (self.effective_background_opacity() * 100.0).round() as i32
            ),
            EditableField::FontFamily => self.config.font_family.clone(),
            EditableField::UiFontFamily => self.config.ui_font_family.clone(),
            EditableField::FontSize => format!("{}", self.config.font_size.round() as i32),
            EditableField::LineHeight => format_line_height(self.config.line_height),
            EditableField::PaddingX => format!("{}", self.config.padding_x.round() as i32),
            EditableField::PaddingY => format!("{}", self.config.padding_y.round() as i32),
            EditableField::WindowsShell => match self.config.windows_shell {
                termy_config_core::WindowsShell::Cmd => "cmd",
                termy_config_core::WindowsShell::PowerShell => "powershell",
                termy_config_core::WindowsShell::PowerShellCore => "pwsh",
                termy_config_core::WindowsShell::GitBash => "git_bash",
            }
            .to_string(),
            EditableField::Shell => self.config.shell.clone().unwrap_or_default(),
            EditableField::Term => self.config.term.clone(),
            EditableField::Colorterm => self.config.colorterm.clone().unwrap_or_default(),
            EditableField::TmuxBinary => self.config.tmux_binary.clone(),
            EditableField::TmuxCommandPrefix => {
                self.config.tmux_command_prefix.clone().unwrap_or_default()
            }
            EditableField::ScrollbackHistory => self.config.scrollback_history.to_string(),
            EditableField::InactiveTabScrollback => self
                .config
                .inactive_tab_scrollback
                .map(|value| value.to_string())
                .unwrap_or_default(),
            EditableField::ScrollMultiplier => {
                format!("{:.3}", self.config.mouse_scroll_multiplier)
            }
            EditableField::CursorStyle => match self.config.cursor_style {
                termy_config_core::CursorStyle::Line => "line",
                termy_config_core::CursorStyle::Block => "block",
            }
            .to_string(),
            EditableField::ScrollbarVisibility => match self.config.terminal_scrollbar_visibility {
                termy_config_core::TerminalScrollbarVisibility::Off => "off",
                termy_config_core::TerminalScrollbarVisibility::Always => "always",
                termy_config_core::TerminalScrollbarVisibility::OnScroll => "on_scroll",
            }
            .to_string(),
            EditableField::ScrollbarStyle => match self.config.terminal_scrollbar_style {
                termy_config_core::TerminalScrollbarStyle::Neutral => "neutral",
                termy_config_core::TerminalScrollbarStyle::MutedTheme => "muted_theme",
                termy_config_core::TerminalScrollbarStyle::Theme => "theme",
            }
            .to_string(),
            EditableField::PaneFocusEffect => match self.config.pane_focus_effect {
                termy_config_core::PaneFocusEffect::Off => "off",
                termy_config_core::PaneFocusEffect::SoftSpotlight => "soft_spotlight",
                termy_config_core::PaneFocusEffect::Cinematic => "cinematic",
                termy_config_core::PaneFocusEffect::Minimal => "minimal",
            }
            .to_string(),
            EditableField::PaneFocusStrength => {
                format!("{}", self.pane_focus_strength_display_percent())
            }
            EditableField::TabFallbackTitle => self.config.tab_title.fallback.clone(),
            EditableField::TabTitlePriority => self
                .config
                .tab_title
                .priority
                .iter()
                .map(|source| match source {
                    termy_config_core::TabTitleSource::Manual => "manual",
                    termy_config_core::TabTitleSource::Explicit => "explicit",
                    termy_config_core::TabTitleSource::Shell => "shell",
                    termy_config_core::TabTitleSource::Fallback => "fallback",
                })
                .collect::<Vec<_>>()
                .join(", "),
            EditableField::TabTitleMode => match self.config.tab_title.mode {
                termy_config_core::TabTitleMode::Smart => "smart",
                termy_config_core::TabTitleMode::Shell => "shell",
                termy_config_core::TabTitleMode::Explicit => "explicit",
                termy_config_core::TabTitleMode::Static => "static",
            }
            .to_string(),
            EditableField::TabTitleExplicitPrefix => self.config.tab_title.explicit_prefix.clone(),
            EditableField::TabTitlePromptFormat => self.config.tab_title.prompt_format.clone(),
            EditableField::TabTitleCommandFormat => self.config.tab_title.command_format.clone(),
            EditableField::TabCloseVisibility => match self.config.tab_close_visibility {
                termy_config_core::TabCloseVisibility::ActiveHover => "active_hover",
                termy_config_core::TabCloseVisibility::Hover => "hover",
                termy_config_core::TabCloseVisibility::Always => "always",
            }
            .to_string(),
            EditableField::TabWidthMode => match self.config.tab_width_mode {
                termy_config_core::TabWidthMode::Stable => "stable",
                termy_config_core::TabWidthMode::ActiveGrow => "active_grow",
                termy_config_core::TabWidthMode::ActiveGrowSticky => "active_grow_sticky",
                termy_config_core::TabWidthMode::Uniform => "uniform",
            }
            .to_string(),
            EditableField::TabBarPosition => match self.config.tab_bar_position {
                termy_config_core::TabBarPosition::Top => "top",
                termy_config_core::TabBarPosition::Right => "right",
            }
            .to_string(),
            EditableField::SidebarWidth => {
                format!("{}", self.config.sidebar_width.round() as i32)
            }
            EditableField::WorkingDirectory => self.config.working_dir.clone().unwrap_or_default(),
            EditableField::WorkingDirFallback => match self.config.working_dir_fallback {
                termy_config_core::WorkingDirFallback::Home => "home",
                termy_config_core::WorkingDirFallback::Process => "process",
            }
            .to_string(),
            EditableField::WindowWidth => format!("{}", self.config.window_width.round() as i32),
            EditableField::WindowHeight => format!("{}", self.config.window_height.round() as i32),
            EditableField::Color(id) => self
                .custom_color_for_id(id)
                .map(|rgb| format!("#{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b))
                .unwrap_or_default(),
        }
    }

    pub(super) fn pane_focus_strength_display_percent(&self) -> i32 {
        // Normalize internal 0.0..=PANE_FOCUS_MAX strength to a cleaner 0..=100 UI scale
        // without changing the stored value range or step behavior.
        ((self.config.pane_focus_strength.clamp(0.0, PANE_FOCUS_MAX) / PANE_FOCUS_MAX) * 100.0)
            .round() as i32
    }

    pub(super) fn begin_editing_field(
        &mut self,
        field: EditableField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.blur_sidebar_search();
        self.capturing_action = None;
        self.active_input = Some(ActiveTextInput::new(
            field,
            self.editable_field_value(field),
        ));
        self.focus_handle.focus(window);
        cx.notify();
    }

    pub(super) fn is_numeric_field(field: EditableField) -> bool {
        Self::field_spec(field).numeric_step.is_some()
    }

    pub(super) fn uses_text_input_for_field(field: EditableField) -> bool {
        !Self::is_numeric_field(field) && !Self::field_uses_click_only_dropdown(field)
    }

    pub(super) fn step_numeric_field(
        &mut self,
        field: EditableField,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(step) = Self::field_spec(field).numeric_step else {
            crate::ui::toast::error("Invalid numeric setting");
            return;
        };
        let result = match field {
            EditableField::BackgroundOpacity => {
                let next = (self.config.background_opacity + (delta as f32 * step.delta))
                    .clamp(step.min, step.max);
                self.clear_background_opacity_preview();
                self.persist_background_opacity(next)
            }
            EditableField::FontSize => {
                let next =
                    (self.config.font_size + (delta as f32 * step.delta)).clamp(step.min, step.max);
                self.config.font_size = next;
                config::set_root_setting(
                    termy_config_core::RootSettingId::FontSize,
                    &next.to_string(),
                )
            }
            EditableField::LineHeight => {
                let next = (self.config.line_height + (delta as f32 * step.delta))
                    .clamp(step.min, step.max);
                let result = config::set_root_setting(
                    termy_config_core::RootSettingId::LineHeight,
                    &format_line_height(next),
                );
                if result.is_ok() {
                    self.config.line_height = next;
                }
                result
            }
            EditableField::PaddingX => {
                let next =
                    (self.config.padding_x + (delta as f32 * step.delta)).clamp(step.min, step.max);
                self.config.padding_x = next;
                config::set_root_setting(
                    termy_config_core::RootSettingId::PaddingX,
                    &next.to_string(),
                )
            }
            EditableField::PaddingY => {
                let next =
                    (self.config.padding_y + (delta as f32 * step.delta)).clamp(step.min, step.max);
                self.config.padding_y = next;
                config::set_root_setting(
                    termy_config_core::RootSettingId::PaddingY,
                    &next.to_string(),
                )
            }
            EditableField::ScrollbackHistory => {
                let next = (self.config.scrollback_history as f32 + (delta as f32 * step.delta))
                    .round()
                    .clamp(step.min, step.max) as usize;
                self.config.scrollback_history = next;
                config::set_root_setting(
                    termy_config_core::RootSettingId::ScrollbackHistory,
                    &next.to_string(),
                )
            }
            EditableField::InactiveTabScrollback => {
                let current = self.config.inactive_tab_scrollback.unwrap_or(0);
                let next = (current as f32 + (delta as f32 * step.delta))
                    .round()
                    .clamp(step.min, step.max) as usize;
                self.config.inactive_tab_scrollback = Some(next);
                config::set_root_setting(
                    termy_config_core::RootSettingId::InactiveTabScrollback,
                    &next.to_string(),
                )
            }
            EditableField::ScrollMultiplier => {
                let next = (self.config.mouse_scroll_multiplier + (delta as f32 * step.delta))
                    .clamp(step.min, step.max);
                self.config.mouse_scroll_multiplier = next;
                config::set_root_setting(
                    termy_config_core::RootSettingId::MouseScrollMultiplier,
                    &format!("{next:.3}"),
                )
            }
            EditableField::PaneFocusStrength => {
                let next = (self.config.pane_focus_strength + (delta as f32 * step.delta))
                    .clamp(step.min, step.max);
                self.config.pane_focus_strength = next;
                config::set_root_setting(
                    termy_config_core::RootSettingId::PaneFocusStrength,
                    &format!("{next:.3}"),
                )
            }
            EditableField::SidebarWidth => {
                let next = (self.config.sidebar_width + (delta as f32 * step.delta))
                    .clamp(step.min, step.max);
                self.config.sidebar_width = next;
                config::set_root_setting(
                    termy_config_core::RootSettingId::SidebarWidth,
                    &next.to_string(),
                )
            }
            EditableField::WindowWidth => {
                let next = (self.config.window_width + (delta as f32 * step.delta))
                    .clamp(step.min, step.max);
                self.config.window_width = next;
                config::set_root_setting(
                    termy_config_core::RootSettingId::WindowWidth,
                    &next.to_string(),
                )
            }
            EditableField::WindowHeight => {
                let next = (self.config.window_height + (delta as f32 * step.delta))
                    .clamp(step.min, step.max);
                self.config.window_height = next;
                config::set_root_setting(
                    termy_config_core::RootSettingId::WindowHeight,
                    &next.to_string(),
                )
            }
            _ => Err(format!("Unsupported numeric field: {field:?}")),
        };

        if let Err(error) = result {
            crate::ui::toast::error(error);
        } else {
            crate::ui::toast::success("Saved");
        }
        self.active_input = None;
        cx.notify();
    }

    pub(super) fn ordered_theme_ids_for_settings(&self) -> Vec<String> {
        let mut theme_ids: Vec<String> = self
            .theme_store_installed_versions
            .keys()
            .cloned()
            .collect();
        theme_ids.push("shell-decide".to_string());

        for current in [
            &self.config.theme,
            &self.config.theme_light,
            &self.config.theme_dark,
        ] {
            if !theme_ids.iter().any(|theme| theme == current) {
                theme_ids.push(current.clone());
            }
        }

        theme_ids.sort_unstable();
        theme_ids.dedup();
        theme_ids
    }

    pub(super) fn ordered_font_families_for_settings(&self) -> Vec<String> {
        self.available_font_families.clone()
    }

    pub(super) fn filtered_theme_suggestions(&self, query: &str) -> Vec<String> {
        let normalized = query.trim().to_ascii_lowercase();
        let themes = self.ordered_theme_ids_for_settings();

        if normalized.is_empty() {
            return themes.into_iter().take(MAX_THEME_SUGGESTIONS).collect();
        }

        let mut matched = Vec::new();
        let mut rest = Vec::new();
        for theme in themes {
            let lower = theme.to_ascii_lowercase();
            if lower.contains(&normalized) || lower.replace('-', " ").contains(&normalized) {
                matched.push(theme);
            } else {
                rest.push(theme);
            }
        }
        matched.extend(rest);
        matched.into_iter().take(MAX_THEME_SUGGESTIONS).collect()
    }

    pub(super) fn filtered_font_suggestions(&self, query: &str) -> Vec<String> {
        let normalized = query.trim().to_ascii_lowercase();
        let fonts = self.ordered_font_families_for_settings();
        let selected_font = self.config.font_family.trim().to_ascii_lowercase();

        // When the dropdown first opens, the input text equals the selected font.
        // Treat that like an empty query so users can browse the full installed list.
        if normalized.is_empty() || normalized == selected_font {
            return fonts.into_iter().take(MAX_FONT_SUGGESTIONS).collect();
        }

        fonts
            .into_iter()
            .filter(|font| font.to_ascii_lowercase().contains(&normalized))
            .take(MAX_FONT_SUGGESTIONS)
            .collect()
    }

    pub(super) fn commit_active_input(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.active_input.take() else {
            return;
        };

        if let Err(error) = self.apply_editable_field(input.field, input.state.text()) {
            crate::ui::toast::error(error);
            self.active_input = Some(input);
        } else {
            crate::ui::toast::success("Saved");
        }
        cx.notify();
    }

    pub(super) fn cancel_active_input(&mut self, cx: &mut Context<Self>) {
        self.active_input = None;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{EditableField, FieldCodec, SettingsWindow};

    #[test]
    fn field_spec_covers_all_editable_fields() {
        let fields = vec![
            EditableField::Theme,
            EditableField::ThemeMode,
            EditableField::ThemeLight,
            EditableField::ThemeDark,
            EditableField::AppIcon,
            EditableField::BackgroundOpacity,
            EditableField::FontFamily,
            EditableField::FontSize,
            EditableField::LineHeight,
            EditableField::PaddingX,
            EditableField::PaddingY,
            EditableField::WindowsShell,
            EditableField::Shell,
            EditableField::Term,
            EditableField::Colorterm,
            EditableField::TmuxBinary,
            EditableField::TmuxCommandPrefix,
            EditableField::ScrollbackHistory,
            EditableField::InactiveTabScrollback,
            EditableField::ScrollMultiplier,
            EditableField::CursorStyle,
            EditableField::ScrollbarVisibility,
            EditableField::ScrollbarStyle,
            EditableField::PaneFocusEffect,
            EditableField::PaneFocusStrength,
            EditableField::TabFallbackTitle,
            EditableField::TabTitlePriority,
            EditableField::TabTitleMode,
            EditableField::TabTitleExplicitPrefix,
            EditableField::TabTitlePromptFormat,
            EditableField::TabTitleCommandFormat,
            EditableField::TabCloseVisibility,
            EditableField::TabWidthMode,
            EditableField::TabBarPosition,
            EditableField::SidebarWidth,
            EditableField::WorkingDirectory,
            EditableField::WorkingDirFallback,
            EditableField::WindowWidth,
            EditableField::WindowHeight,
            EditableField::Color(termy_config_core::ColorSettingId::Foreground),
        ];

        for field in fields {
            let spec = SettingsWindow::field_spec(field);
            if matches!(field, EditableField::Color(_)) {
                assert_eq!(spec.codec, FieldCodec::Color);
                assert!(spec.root_setting.is_none());
            } else {
                assert!(spec.root_setting.is_some());
            }
        }
    }

    #[test]
    fn enum_fields_are_click_only_dropdowns() {
        let enum_fields = [
            EditableField::WindowsShell,
            EditableField::CursorStyle,
            EditableField::ScrollbarVisibility,
            EditableField::ScrollbarStyle,
            EditableField::PaneFocusEffect,
            EditableField::AppIcon,
            EditableField::TabTitleMode,
            EditableField::TabCloseVisibility,
            EditableField::TabWidthMode,
            EditableField::TabBarPosition,
            EditableField::WorkingDirFallback,
        ];

        for field in enum_fields {
            let spec = SettingsWindow::field_spec(field);
            assert_eq!(spec.codec, FieldCodec::Enum);
            assert!(spec.dropdown_click_only);
        }
    }

    #[test]
    fn app_icon_dropdown_displays_user_facing_labels_only() {
        let option = SettingsWindow::dropdown_option_for_enum_choice(
            termy_config_core::RootSettingId::AppIcon,
            "default",
            "Termy Default",
        );

        assert_eq!(option.display_text(), "Termy Default");
    }

    #[test]
    fn numeric_step_specs_are_defined_for_numeric_fields() {
        let numeric_fields = [
            EditableField::BackgroundOpacity,
            EditableField::FontSize,
            EditableField::LineHeight,
            EditableField::PaddingX,
            EditableField::PaddingY,
            EditableField::ScrollbackHistory,
            EditableField::InactiveTabScrollback,
            EditableField::ScrollMultiplier,
            EditableField::PaneFocusStrength,
            EditableField::SidebarWidth,
            EditableField::WindowWidth,
            EditableField::WindowHeight,
        ];

        for field in numeric_fields {
            let spec = SettingsWindow::field_spec(field);
            assert_eq!(spec.codec, FieldCodec::Numeric);
            assert!(spec.numeric_step.is_some());
        }

        let line_height_spec = SettingsWindow::field_spec(EditableField::LineHeight);
        let step = line_height_spec
            .numeric_step
            .expect("missing line-height step");
        assert!((step.delta - 0.05).abs() < f32::EPSILON);
        assert!((step.min - termy_config_core::MIN_LINE_HEIGHT).abs() < f32::EPSILON);
        assert!((step.max - termy_config_core::MAX_LINE_HEIGHT).abs() < f32::EPSILON);
    }

    #[test]
    fn color_hex_parser_accepts_valid_and_rejects_invalid_values() {
        assert!(termy_config_core::Rgb8::from_hex("#12ab34").is_some());
        assert!(termy_config_core::Rgb8::from_hex("#12AB34").is_some());
        assert!(termy_config_core::Rgb8::from_hex("12ab34").is_some());
        assert!(termy_config_core::Rgb8::from_hex("#zzzzzz").is_none());
    }
}
