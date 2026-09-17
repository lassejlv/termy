mod color_keys;
mod constants;
mod diagnostics;
mod document;
mod parser;
mod path;
mod render;
mod schema;
mod types;

pub use color_keys::{ColorEntryError, apply_color_entry, canonical_color_key};
pub use constants::{
    DEFAULT_FONT_FAMILY, DEFAULT_LINE_HEIGHT, DEFAULT_SIDEBAR_WIDTH, MAX_LINE_HEIGHT,
    MAX_SIDEBAR_WIDTH, MIN_LINE_HEIGHT, MIN_SIDEBAR_WIDTH, SHELL_DECIDE_THEME_ID, VALID_SECTIONS,
};
pub use diagnostics::{ConfigDiagnostic, ConfigDiagnosticKind, ConfigParseReport};
pub use document::{
    ColorSettingUpdate, apply_color_updates, remove_raw_root_key, remove_root_setting,
    replace_keybind_lines, upsert_root_setting,
};
pub use parser::parse_theme_id;
pub use path::config_path;
pub use render::{DEFAULT_CONFIG_TEMPLATE, prettify_config_contents};
pub use schema::ROOT_SETTING_ALL_KEYS as VALID_ROOT_KEYS;
pub use schema::{
    COLOR_SETTING_KEYS, COLOR_SETTING_SPECS, ColorSettingId, ColorSettingSpec, EnumChoice,
    ROOT_SETTING_ALL_KEYS, ROOT_SETTING_KEYS, ROOT_SETTING_SPECS, RootSettingId, RootSettingSpec,
    RootSettingValueKind, SettingsSection, canonical_color_key as schema_canonical_color_key,
    canonical_root_key as schema_canonical_root_key, color_setting_from_key, color_setting_spec,
    color_setting_specs, root_setting_default_value, root_setting_enum_choices,
    root_setting_from_key, root_setting_spec, root_setting_specs, root_setting_value_kind,
};
pub use types::{
    AppConfig, AppIcon, AppearanceMode, CursorStyle, CustomColors, KeybindConfigLine,
    PaneFocusEffect, Rgb8, SystemAppearance, TabBarPosition, TabCloseVisibility, TabTitleConfig,
    TabTitleMode, TabTitleSource, TabWidthMode, TaskConfig, TerminalScrollbarStyle,
    TerminalScrollbarVisibility, ThemeId, WindowsShell, WorkingDirFallback, resolve_active_theme,
};

pub fn format_line_height(value: f32) -> String {
    format!("{value:.2}")
}

#[cfg(test)]
mod parser_tests;
