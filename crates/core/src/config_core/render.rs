use std::collections::{BTreeMap, HashMap};

use crate::config_core::color_keys::canonical_color_key;
use crate::config_core::schema::{
    COLOR_SETTING_KEYS, RootSettingId, canonical_root_key, root_setting_specs,
};

pub const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("default_config.txt");

const SECTION_ORDER: &[&str] = &["colors", "tab_title"];

pub fn prettify_config_contents(contents: &str) -> String {
    let mut root_settings: HashMap<String, String> = HashMap::new();
    let mut keybinds = Vec::new();
    let mut colors: HashMap<String, String> = HashMap::new();
    let mut other_sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut current_section: Option<String> = None;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section_name = trimmed[1..trimmed.len() - 1].trim().to_ascii_lowercase();
            current_section = Some(section_name);
            continue;
        }

        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };

        let key = key.trim();
        let value = value.trim().to_string();

        match current_section.as_deref() {
            None => {
                if key.eq_ignore_ascii_case("keybind") {
                    keybinds.push(value);
                } else {
                    let root_key = canonical_root_key(key)
                        .map_or_else(|| canonical_unknown_key(key), ToString::to_string);
                    root_settings.insert(root_key, value);
                }
            }
            Some("colors") => {
                let color_key = canonical_color_key(key)
                    .map_or_else(|| canonical_unknown_key(key), ToString::to_string);
                colors.insert(color_key, value);
            }
            Some(section) => {
                other_sections
                    .entry(section.to_string())
                    .or_default()
                    .insert(canonical_unknown_key(key), value);
            }
        }
    }

    let mut output = String::new();

    for spec in root_setting_specs() {
        if spec.id == RootSettingId::Keybind {
            continue;
        }
        if let Some(value) = root_settings.remove(spec.key) {
            let key = spec.key;
            output.push_str(&format!("{key} = {value}\n"));
        }
    }

    let mut remaining_root_keys: Vec<_> = root_settings.into_iter().collect();
    remaining_root_keys.sort_by(|left, right| left.0.cmp(&right.0));
    for (key, value) in remaining_root_keys {
        output.push_str(&format!("{key} = {value}\n"));
    }

    if !keybinds.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        for keybind in keybinds {
            output.push_str(&format!("keybind = {keybind}\n"));
        }
    }

    if !colors.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("[colors]\n");

        for key in COLOR_SETTING_KEYS {
            if let Some(value) = colors.remove(*key) {
                output.push_str(&format!("{key} = {value}\n"));
            }
        }

        let mut extra_colors: Vec<_> = colors.into_iter().collect();
        extra_colors.sort_by(|left, right| left.0.cmp(&right.0));
        for (key, value) in extra_colors {
            output.push_str(&format!("{key} = {value}\n"));
        }
    }

    for section_name in SECTION_ORDER {
        if *section_name == "colors" {
            continue;
        }

        let Some(section_values) = other_sections.remove(*section_name) else {
            continue;
        };
        append_section(&mut output, section_name, section_values);
    }

    for (section_name, section_values) in other_sections {
        append_section(&mut output, &section_name, section_values);
    }

    output
}

fn append_section(
    output: &mut String,
    section_name: &str,
    section_values: BTreeMap<String, String>,
) {
    if !output.is_empty() {
        output.push('\n');
    }
    output.push('[');
    output.push_str(section_name);
    output.push_str("]\n");

    for (key, value) in section_values {
        output.push_str(&format!("{key} = {value}\n"));
    }
}

fn canonical_unknown_key(key: &str) -> String {
    key.trim().to_ascii_lowercase().replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_CONFIG_TEMPLATE, prettify_config_contents};

    #[test]
    fn default_template_is_non_empty() {
        assert!(!DEFAULT_CONFIG_TEMPLATE.trim().is_empty());
    }

    #[test]
    fn prettify_orders_root_keybind_and_colors() {
        let input = "\
# comment\n\
keybind = cmd-w=close_pane_or_tab\n\
font_size = 16\n\
theme = nord\n\
[colors]\n\
color2 = #00ff00\n\
foreground = #ffffff\n\
";

        let output = prettify_config_contents(input);

        assert!(output.starts_with("theme = nord\nfont_size = 16\n\nkeybind = cmd-w=close_pane_or_tab\n\n[colors]\nforeground = #ffffff\ngreen = #00ff00\n"));
    }

    #[test]
    fn prettify_normalizes_legacy_aliases() {
        let output = prettify_config_contents("default_working_dir = process\nscrollback = 3000\n");
        assert!(output.contains("working_dir_fallback = process"));
        assert!(output.contains("scrollback_history = 3000"));
    }
}
