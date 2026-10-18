//! What the native Settings window shows, built from the config schema, and
//! the config writes behind its edits.

use crate::colors::TerminalColors;
use crate::config::{self, AppConfig, SystemAppearance};
use crate::settings_view::SettingsSection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use termy_core::config_core::{
    AppearanceMode, ColorSettingId, Rgb8, RootSettingId, RootSettingValueKind,
    SettingsSection as SchemaSection, color_setting_from_key, color_setting_specs,
    root_setting_default_value, root_setting_enum_choices, root_setting_from_key,
    root_setting_spec, root_setting_specs,
};
use termy_core::themes::ThemeColors;

/// A sidebar page of the native window.
struct PageSpec {
    id: &'static str,
    title: &'static str,
    symbol: &'static str,
    content: PageContent,
}

enum PageContent {
    Schema(SchemaSection),
    Colors,
    Themes,
    /// Drawn by Termy's own Settings window.
    External(SettingsSection),
}

const THEMES_PAGE: &str = "themes";

const PAGES: &[PageSpec] = &[
    PageSpec {
        id: "general",
        title: "General",
        symbol: "gearshape",
        content: PageContent::Schema(SchemaSection::Advanced),
    },
    PageSpec {
        id: "appearance",
        title: "Appearance",
        symbol: "paintbrush",
        content: PageContent::Schema(SchemaSection::Appearance),
    },
    PageSpec {
        id: THEMES_PAGE,
        title: "Themes",
        symbol: "swatchpalette",
        content: PageContent::Themes,
    },
    PageSpec {
        id: "terminal",
        title: "Terminal",
        symbol: "terminal",
        content: PageContent::Schema(SchemaSection::Terminal),
    },
    PageSpec {
        id: "tabs",
        title: "Tabs",
        symbol: "square.on.square",
        content: PageContent::Schema(SchemaSection::Tabs),
    },
    PageSpec {
        id: "colors",
        title: "Colors",
        symbol: "paintpalette",
        content: PageContent::Colors,
    },
    PageSpec {
        id: "keybindings",
        title: "Keyboard Shortcuts",
        symbol: "keyboard",
        content: PageContent::External(SettingsSection::Keybindings),
    },
    PageSpec {
        id: "plugins",
        title: "Plugins",
        symbol: "puzzlepiece.extension",
        content: PageContent::External(SettingsSection::Plugins),
    },
    PageSpec {
        id: "ssh",
        title: "SSH",
        symbol: "network",
        content: PageContent::External(SettingsSection::Ssh),
    },
];

/// The native page for a Settings section, or `None` when Termy's own window
/// draws that section.
pub(super) fn page_for_section(section: SettingsSection) -> Option<&'static str> {
    let schema = match section {
        SettingsSection::Advanced => SchemaSection::Advanced,
        SettingsSection::Appearance => SchemaSection::Appearance,
        SettingsSection::Terminal => SchemaSection::Terminal,
        SettingsSection::Tabs => SchemaSection::Tabs,
        SettingsSection::Colors => return Some("colors"),
        SettingsSection::Keybindings | SettingsSection::Plugins | SettingsSection::Ssh => {
            return None;
        }
    };
    PAGES
        .iter()
        .find(|page| matches!(page.content, PageContent::Schema(section) if section == schema))
        .map(|page| page.id)
}

/// The Settings section an external page opens in Termy's own window.
pub(super) fn external_section(page: &str) -> Option<SettingsSection> {
    PAGES
        .iter()
        .find(|spec| spec.id == page)
        .and_then(|spec| match spec.content {
            PageContent::External(section) => Some(section),
            _ => None,
        })
}

#[derive(Serialize)]
pub(super) struct Model {
    pages: Vec<Page>,
}

#[derive(Serialize)]
struct Page {
    id: &'static str,
    title: &'static str,
    symbol: &'static str,
    external: bool,
    groups: Vec<Group>,
    #[serde(skip_serializing_if = "Option::is_none")]
    themes: Option<ThemesPage>,
}

/// The theme browser: every theme, and which ones the config uses.
#[derive(Serialize)]
struct ThemesPage {
    /// Whether the theme follows the system appearance (light and dark slots).
    follows_system: bool,
    /// The appearance the system has now, `light` or `dark`.
    appearance: &'static str,
    theme: String,
    theme_light: String,
    theme_dark: String,
    themes: Vec<ThemeCard>,
}

#[derive(Serialize)]
struct ThemeCard {
    id: String,
    name: String,
    background: String,
    foreground: String,
    /// Red, green, yellow, blue, magenta and cyan.
    accents: Vec<String>,
    dark: bool,
}

/// A theme the window offers, with its display name and colors.
pub(super) struct ThemeEntry {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) colors: ThemeColors,
}

#[derive(Serialize)]
struct Group {
    id: String,
    title: String,
    rows: Vec<Row>,
}

#[derive(Serialize)]
struct Row {
    key: &'static str,
    title: &'static str,
    description: &'static str,
    keywords: &'static [&'static str],
    control: Control,
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    choices: Option<Vec<Choice>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    step: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    overridden: Option<bool>,
    /// Links only: the text shown for the value, and the page it opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Control {
    Toggle,
    Picker,
    Text,
    Number,
    Slider,
    Color,
    /// Shows the value and opens another page to change it.
    Link,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Choice {
    value: String,
    label: String,
}

/// Inputs for [`Model::build`] that do not come from the config file.
pub(super) struct ModelSources<'a> {
    pub(super) fonts: &'a [String],
    pub(super) themes: &'a [ThemeEntry],
    pub(super) appearance: SystemAppearance,
    pub(super) errors: &'a HashMap<String, String>,
}

impl Model {
    pub(super) fn build(config: &AppConfig, sources: &ModelSources<'_>) -> Self {
        let pages = PAGES
            .iter()
            .map(|spec| Page {
                id: spec.id,
                title: spec.title,
                symbol: spec.symbol,
                external: matches!(spec.content, PageContent::External(_)),
                groups: match spec.content {
                    PageContent::Schema(section) => schema_groups(config, section, sources),
                    PageContent::Colors => color_groups(config, sources),
                    PageContent::Themes | PageContent::External(_) => Vec::new(),
                },
                themes: matches!(spec.content, PageContent::Themes)
                    .then(|| themes_page(config, sources)),
            })
            .collect();
        Self { pages }
    }
}

/// Settings the window leaves out: bookkeeping values, the keybinding list
/// (its own page), Windows-only options, and whichever theme rows the theme
/// mode does not use.
fn root_setting_visible(config: &AppConfig, id: RootSettingId) -> bool {
    match id {
        RootSettingId::OnboardingComplete
        | RootSettingId::InspectorHeight
        | RootSettingId::Keybind
        | RootSettingId::WindowsShell => false,
        RootSettingId::Theme => config.theme_mode == AppearanceMode::Manual,
        RootSettingId::ThemeLight | RootSettingId::ThemeDark => {
            config.theme_mode == AppearanceMode::System
        }
        _ => true,
    }
}

/// Bounded numbers that read better as a slider: (min, max, step).
fn slider_range(id: RootSettingId) -> Option<(f64, f64, f64)> {
    match id {
        RootSettingId::BackgroundOpacity => Some((0.0, 1.0, 0.01)),
        RootSettingId::LineHeight => Some((0.8, 2.5, 0.05)),
        RootSettingId::PaneFocusStrength => Some((0.0, 2.0, 0.05)),
        _ => None,
    }
}

fn schema_groups(
    config: &AppConfig,
    section: SchemaSection,
    sources: &ModelSources<'_>,
) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for spec in root_setting_specs()
        .iter()
        .filter(|spec| spec.section == section && root_setting_visible(config, spec.id))
    {
        let row = root_row(config, spec.id, sources);
        match groups.iter_mut().find(|group| group.id == spec.group) {
            Some(group) => group.rows.push(row),
            None => groups.push(Group {
                id: spec.group.to_string(),
                title: group_title(spec.group),
                rows: vec![row],
            }),
        }
    }
    groups
}

fn root_row(config: &AppConfig, id: RootSettingId, sources: &ModelSources<'_>) -> Row {
    let spec = root_setting_spec(id);
    let value = root_setting_default_value(config, id).unwrap_or_default();
    let mut row = Row {
        key: spec.key,
        title: spec.title,
        description: spec.description,
        keywords: spec.keywords,
        control: Control::Text,
        value,
        choices: None,
        min: None,
        max: None,
        step: None,
        overridden: None,
        display: None,
        target: None,
        error: sources.errors.get(spec.key).cloned(),
    };
    match spec.value_kind {
        RootSettingValueKind::Boolean => row.control = Control::Toggle,
        RootSettingValueKind::Enum => {
            let choices = root_setting_enum_choices(id)
                .unwrap_or_default()
                .iter()
                .map(|choice| Choice {
                    value: choice.value.to_string(),
                    label: choice.label.to_string(),
                })
                .collect();
            row.control = Control::Picker;
            row.choices = Some(with_current_choice(choices, &row.value));
        }
        RootSettingValueKind::Numeric => match slider_range(id) {
            Some((min, max, step)) => {
                row.control = Control::Slider;
                row.min = Some(min);
                row.max = Some(max);
                row.step = Some(step);
            }
            None => row.control = Control::Number,
        },
        RootSettingValueKind::Text => {}
        RootSettingValueKind::Special => {
            let options = match id {
                RootSettingId::Theme | RootSettingId::ThemeLight | RootSettingId::ThemeDark => {
                    row.control = Control::Link;
                    row.display = Some(theme_name(sources.themes, &row.value));
                    row.target = Some(THEMES_PAGE);
                    None
                }
                RootSettingId::FontFamily | RootSettingId::UiFontFamily => {
                    let mut choices: Vec<Choice> = sources
                        .fonts
                        .iter()
                        .map(|font| Choice {
                            value: font.clone(),
                            label: font.clone(),
                        })
                        .collect();
                    // Keep the default selectable even when it is a hidden
                    // system font the list leaves out.
                    if let Some(default) = root_setting_default_value(&AppConfig::default(), id)
                        && !choices.iter().any(|choice| choice.value == default)
                    {
                        choices.insert(
                            0,
                            Choice {
                                label: format!("{default} (Default)"),
                                value: default,
                            },
                        );
                    }
                    Some(choices)
                }
                _ => None,
            };
            if let Some(choices) = options {
                row.control = Control::Picker;
                row.choices = Some(with_current_choice(choices, &row.value));
            }
        }
    }
    row
}

/// A picker must list its current value, even one the list does not know.
fn with_current_choice(mut choices: Vec<Choice>, current: &str) -> Vec<Choice> {
    if !current.is_empty() && !choices.iter().any(|choice| choice.value == current) {
        choices.insert(
            0,
            Choice {
                value: current.to_string(),
                label: current.to_string(),
            },
        );
    }
    choices
}

/// "TAB TITLES" → "Tab Titles"; short acronyms such as "UI" stay as is.
fn group_title(group: &str) -> String {
    group
        .split_whitespace()
        .map(|word| {
            if word.len() <= 2 {
                return word.to_string();
            }
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// "tokyo-night" → "Tokyo Night", for themes without a display name.
pub(super) fn theme_label(theme: &str) -> String {
    theme
        .split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn theme_name(themes: &[ThemeEntry], id: &str) -> String {
    themes
        .iter()
        .find(|theme| theme.id == id)
        .map_or_else(|| theme_label(id), |theme| theme.name.clone())
}

fn themes_page(config: &AppConfig, sources: &ModelSources<'_>) -> ThemesPage {
    let to_hex = |color: Rgb8| hex(color.r, color.g, color.b);
    ThemesPage {
        follows_system: config.theme_mode == AppearanceMode::System,
        appearance: match sources.appearance {
            SystemAppearance::Light => "light",
            SystemAppearance::Dark => "dark",
        },
        theme: config.theme.clone(),
        theme_light: config.theme_light.clone(),
        theme_dark: config.theme_dark.clone(),
        themes: sources
            .themes
            .iter()
            .map(|theme| ThemeCard {
                id: theme.id.clone(),
                name: theme.name.clone(),
                background: to_hex(theme.colors.background),
                foreground: to_hex(theme.colors.foreground),
                accents: theme.colors.ansi[1..=6]
                    .iter()
                    .copied()
                    .map(to_hex)
                    .collect(),
                dark: is_dark(theme.colors.background),
            })
            .collect(),
    }
}

/// Relative luminance below the midpoint reads as a dark theme.
fn is_dark(color: Rgb8) -> bool {
    let luminance =
        0.2126 * f64::from(color.r) + 0.7152 * f64::from(color.g) + 0.0722 * f64::from(color.b);
    luminance < 128.0
}

fn color_groups(config: &AppConfig, sources: &ModelSources<'_>) -> Vec<Group> {
    let effective = TerminalColors::from_config(config, sources.appearance);
    let mut groups = vec![
        Group {
            id: "terminal".to_string(),
            title: "Terminal".to_string(),
            rows: Vec::new(),
        },
        Group {
            id: "ansi".to_string(),
            title: "ANSI".to_string(),
            rows: Vec::new(),
        },
        Group {
            id: "bright".to_string(),
            title: "Bright ANSI".to_string(),
            rows: Vec::new(),
        },
    ];
    for spec in color_setting_specs() {
        let custom = custom_color(config, spec.id);
        let value = custom.map_or_else(
            || {
                let color = effective_color(&effective, spec.id);
                hex(
                    (color.r * 255.0).round() as u8,
                    (color.g * 255.0).round() as u8,
                    (color.b * 255.0).round() as u8,
                )
            },
            |color| hex(color.r, color.g, color.b),
        );
        let group = match ansi_index(spec.id) {
            None => 0,
            Some(index) if index < 8 => 1,
            Some(_) => 2,
        };
        groups[group].rows.push(Row {
            key: spec.key,
            title: spec.title,
            description: spec.description,
            keywords: spec.keywords,
            control: Control::Color,
            value,
            choices: None,
            min: None,
            max: None,
            step: None,
            overridden: Some(custom.is_some()),
            display: None,
            target: None,
            error: sources.errors.get(spec.key).cloned(),
        });
    }
    groups
}

fn ansi_index(id: ColorSettingId) -> Option<usize> {
    let index = color_setting_specs()
        .iter()
        .position(|spec| spec.id == id)?;
    // The specs list foreground, background and cursor before the 16 ANSI
    // colors.
    index.checked_sub(3)
}

fn custom_color(config: &AppConfig, id: ColorSettingId) -> Option<termy_core::config_core::Rgb8> {
    let colors = &config.colors;
    match id {
        ColorSettingId::Foreground => colors.foreground,
        ColorSettingId::Background => colors.background,
        ColorSettingId::Cursor => colors.cursor,
        id => ansi_index(id).and_then(|index| colors.ansi[index]),
    }
}

fn effective_color(colors: &TerminalColors, id: ColorSettingId) -> gpui_kit::Rgba {
    match id {
        ColorSettingId::Foreground => colors.foreground,
        ColorSettingId::Background => colors.background,
        ColorSettingId::Cursor => colors.cursor,
        id => ansi_index(id).map_or(colors.foreground, |index| colors.ansi[index]),
    }
}

fn hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// What the window reports back.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Event {
    Set {
        key: String,
        value: String,
    },
    Reset {
        key: String,
    },
    OpenSection {
        section: String,
    },
    /// The window wants the model again (it was restored, or the system
    /// appearance changed).
    Refresh,
}

/// Write one edited setting. Values go through the config parser first, so
/// the window accepts exactly what the config file does and writes the
/// canonical spelling.
pub(super) fn apply_set(key: &str, value: &str, fonts: &[String]) -> Result<(), String> {
    let value = value.trim();
    // A config value is one line; a pasted newline would write a second one.
    if value.contains(['\n', '\r']) {
        return Err("Value must be a single line".to_string());
    }
    if let Some(id) = color_setting_from_key(key) {
        let color = termy_core::config_core::Rgb8::from_hex(value)
            .ok_or_else(|| "Color must be #RRGGBB".to_string())?;
        return config::set_color_setting(id, Some(&hex(color.r, color.g, color.b)));
    }
    let id = root_setting_from_key(key).ok_or_else(|| format!("Unknown setting {key}"))?;
    let spec = root_setting_spec(id);
    match id {
        RootSettingId::Theme => {
            if value.is_empty() {
                return Err("Theme cannot be empty".to_string());
            }
            return config::set_theme_in_config(value).map(drop);
        }
        RootSettingId::FontFamily | RootSettingId::UiFontFamily => {
            // The default may be a hidden system font no font list includes,
            // such as `.SystemUIFont`.
            let default = root_setting_default_value(&AppConfig::default(), id);
            let font = if default.as_deref() == Some(value) {
                value.to_string()
            } else {
                crate::font_families::canonical_available_font_family(value, fonts)
                    .ok_or_else(|| format!("Font family {value} is not installed"))?
            };
            return config::set_root_setting(id, &font);
        }
        _ => {}
    }
    if value.is_empty() && spec.value_kind == RootSettingValueKind::Text {
        return clear_text_setting(id);
    }

    let (canonical, parsed) = parse_root_value(id, value)?;
    config::set_root_setting(id, &canonical)?;
    if id == RootSettingId::AppIcon {
        crate::app_icon::apply(parsed.app_icon);
    }
    Ok(())
}

/// An emptied text field: `none` for an optional setting (no shell override,
/// no COLORTERM), else the setting's default.
fn clear_text_setting(id: RootSettingId) -> Result<(), String> {
    let optional = parse_root_value(id, "none")
        .is_ok_and(|(_, parsed)| root_setting_default_value(&parsed, id).is_none());
    if optional {
        config::set_root_setting(id, "none")
    } else {
        config::remove_root_setting(id)
    }
}

/// Run `value` through the config parser: its error for a bad value, else
/// the canonical spelling and the parsed config.
fn parse_root_value(id: RootSettingId, value: &str) -> Result<(String, AppConfig), String> {
    let key = root_setting_spec(id).key;
    let report = AppConfig::from_contents_with_report(&format!("{key} = {value}\n"));
    if let Some(diagnostic) = report.diagnostics.first() {
        return Err(diagnostic.message.clone());
    }
    let canonical =
        root_setting_default_value(&report.config, id).unwrap_or_else(|| value.to_string());
    Ok((canonical, report.config))
}

/// Return a setting to its default: a color to its theme color, anything
/// else to the default config value.
pub(super) fn apply_reset(key: &str) -> Result<(), String> {
    if let Some(id) = color_setting_from_key(key) {
        return config::set_color_setting(id, None);
    }
    let id = root_setting_from_key(key).ok_or_else(|| format!("Unknown setting {key}"))?;
    match root_setting_default_value(&AppConfig::default(), id) {
        Some(default) => config::set_root_setting(id, &default),
        None => config::remove_root_setting(id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources<'a>(errors: &'a HashMap<String, String>) -> ModelSources<'a> {
        ModelSources {
            fonts: &[],
            themes: &[],
            appearance: SystemAppearance::Dark,
            errors,
        }
    }

    fn model_json(config: &AppConfig) -> serde_json::Value {
        let errors = HashMap::new();
        serde_json::to_value(Model::build(config, &sources(&errors))).expect("model serializes")
    }

    fn row<'a>(model: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        model["pages"]
            .as_array()?
            .iter()
            .flat_map(|page| page["groups"].as_array().into_iter().flatten())
            .flat_map(|group| group["rows"].as_array().into_iter().flatten())
            .find(|row| row["key"] == key)
    }

    #[test]
    fn every_settings_section_has_a_native_or_external_page() {
        for section in [
            SettingsSection::Appearance,
            SettingsSection::Terminal,
            SettingsSection::Tabs,
            SettingsSection::Advanced,
            SettingsSection::Colors,
        ] {
            assert!(page_for_section(section).is_some(), "{section:?}");
        }
        for section in [
            SettingsSection::Keybindings,
            SettingsSection::Plugins,
            SettingsSection::Ssh,
        ] {
            assert_eq!(page_for_section(section), None);
            let page = PAGES
                .iter()
                .find(|page| matches!(page.content, PageContent::External(s) if s == section))
                .expect("external page");
            assert_eq!(external_section(page.id), Some(section));
        }
    }

    #[test]
    fn model_maps_value_kinds_to_controls() {
        let model = model_json(&AppConfig::default());
        assert_eq!(row(&model, "cursor_blink").unwrap()["control"], "toggle");
        assert_eq!(row(&model, "cursor_style").unwrap()["control"], "picker");
        assert_eq!(
            row(&model, "background_opacity").unwrap()["control"],
            "slider"
        );
        assert_eq!(row(&model, "font_size").unwrap()["control"], "number");
        assert_eq!(row(&model, "term").unwrap()["control"], "text");
        assert_eq!(row(&model, "theme").unwrap()["control"], "link");
        assert_eq!(row(&model, "theme").unwrap()["target"], "themes");
        assert_eq!(row(&model, "red").unwrap()["control"], "color");
        assert_eq!(row(&model, "red").unwrap()["overridden"], false);
    }

    #[test]
    fn model_hides_bookkeeping_and_unused_theme_rows() {
        let mut config = AppConfig {
            theme_mode: AppearanceMode::Manual,
            ..AppConfig::default()
        };
        let model = model_json(&config);
        for hidden in [
            "onboarding_complete",
            "inspector_height",
            "keybind",
            "theme_light",
        ] {
            assert!(row(&model, hidden).is_none(), "{hidden} should be hidden");
        }
        assert!(row(&model, "theme").is_some());

        config.theme_mode = AppearanceMode::System;
        let model = model_json(&config);
        assert!(row(&model, "theme").is_none());
        assert!(row(&model, "theme_dark").is_some());
    }

    #[test]
    fn pickers_keep_an_unknown_current_value() {
        let config = AppConfig {
            font_family: "Missing Mono".to_string(),
            ..AppConfig::default()
        };
        let model = model_json(&config);
        let choices = row(&model, "font_family").unwrap()["choices"]
            .as_array()
            .unwrap();
        assert_eq!(choices[0]["value"], "Missing Mono");
    }

    #[test]
    fn root_values_are_validated_and_canonicalized_by_the_parser() {
        assert!(parse_root_value(RootSettingId::FontSize, "abc").is_err());
        assert!(parse_root_value(RootSettingId::CursorStyle, "triangle").is_err());
        assert_eq!(
            parse_root_value(RootSettingId::CursorBlink, "true")
                .unwrap()
                .0,
            "true"
        );
        assert_eq!(
            parse_root_value(RootSettingId::CursorStyle, "Block")
                .unwrap()
                .0,
            "block"
        );
    }

    #[test]
    fn themes_page_lists_every_theme_with_its_colors() {
        let errors = HashMap::new();
        let themes = [ThemeEntry {
            id: "dracula".to_string(),
            name: "Dracula".to_string(),
            colors: termy_core::themes::builtin_theme("dracula").unwrap(),
        }];
        let config = AppConfig {
            theme: "dracula".to_string(),
            ..AppConfig::default()
        };
        let model = serde_json::to_value(Model::build(
            &config,
            &ModelSources {
                themes: &themes,
                ..sources(&errors)
            },
        ))
        .unwrap();
        let page = model["pages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|page| page["id"] == "themes")
            .unwrap();
        assert_eq!(page["external"], false);
        let card = &page["themes"]["themes"][0];
        assert_eq!(card["name"], "Dracula");
        assert_eq!(card["background"], "#282a36");
        assert_eq!(card["accents"].as_array().unwrap().len(), 6);
        assert_eq!(card["dark"], true);
        assert_eq!(page["themes"]["theme"], "dracula");
        assert_eq!(row(&model, "theme").unwrap()["display"], "Dracula");
    }

    #[test]
    fn font_pickers_offer_the_default_font() {
        let config = AppConfig {
            ui_font_family: "Menlo".to_string(),
            ..AppConfig::default()
        };
        let model = model_json(&config);
        let default = AppConfig::default().ui_font_family;
        let choices = row(&model, "ui_font_family").unwrap()["choices"]
            .as_array()
            .unwrap();
        assert!(
            choices
                .iter()
                .any(|choice| choice["value"] == default.as_str())
        );
    }

    #[test]
    fn group_and_theme_labels_read_naturally() {
        assert_eq!(group_title("TAB TITLES"), "Tab Titles");
        assert_eq!(group_title("UI"), "UI");
        assert_eq!(theme_label("tokyo-night"), "Tokyo Night");
    }

    #[test]
    fn events_decode_from_the_window_json() {
        let set: Event =
            serde_json::from_str(r#"{"kind":"set","key":"font_size","value":"14"}"#).unwrap();
        assert_eq!(
            set,
            Event::Set {
                key: "font_size".to_string(),
                value: "14".to_string()
            }
        );
        let open: Event =
            serde_json::from_str(r#"{"kind":"open_section","section":"ssh"}"#).unwrap();
        assert_eq!(
            open,
            Event::OpenSection {
                section: "ssh".to_string()
            }
        );
    }
}
