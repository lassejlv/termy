use std::collections::{HashMap, HashSet};

use crate::config_core::schema::{
    ColorSettingId, RootSettingId, color_setting_from_key, color_setting_spec,
    root_setting_from_key, root_setting_spec,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorSettingUpdate {
    pub id: ColorSettingId,
    pub value: Option<String>,
}

pub fn upsert_root_setting(contents: &str, setting: RootSettingId, value: &str) -> String {
    rewrite(contents, |contents| {
        upsert_root_setting_lines(contents, setting, value)
    })
}

fn upsert_root_setting_lines(contents: &str, setting: RootSettingId, value: &str) -> String {
    let spec = root_setting_spec(setting);
    let mut out = Vec::new();
    let mut replaced = false;
    let mut inserted_before_first_section = false;
    let mut in_root = true;
    let mut in_colors = false;

    for line in contents.lines() {
        let trimmed = line.trim();
        let is_section_header = trimmed.starts_with('[') && trimmed.ends_with(']');

        if is_section_header {
            if !replaced && !inserted_before_first_section {
                out.push(format!("{} = {}", spec.key, value));
                replaced = true;
                inserted_before_first_section = true;
            }
            in_root = false;
            in_colors = trimmed[1..trimmed.len() - 1]
                .trim()
                .eq_ignore_ascii_case("colors");
            out.push(line.to_string());
            continue;
        }

        if (in_root || in_colors)
            && let Some((raw_key, _)) = line.split_once('=')
            && root_setting_from_key(raw_key.trim()) == Some(setting)
        {
            if in_root && !replaced {
                out.push(format!("{} = {}", spec.key, value));
                replaced = true;
            }
            continue;
        }

        out.push(line.to_string());
    }

    if !replaced && !inserted_before_first_section {
        out.push(format!("{} = {}", spec.key, value));
    }

    join_lines(out)
}

pub fn remove_root_setting(contents: &str, setting: RootSettingId) -> String {
    rewrite(contents, |contents| {
        remove_root_setting_lines(contents, setting)
    })
}

fn remove_root_setting_lines(contents: &str, setting: RootSettingId) -> String {
    let mut out = Vec::new();
    let mut in_root = true;
    let mut in_colors = false;

    for line in contents.lines() {
        let trimmed = line.trim();
        let is_section_header = trimmed.starts_with('[') && trimmed.ends_with(']');
        if is_section_header {
            in_root = false;
            in_colors = trimmed[1..trimmed.len() - 1]
                .trim()
                .eq_ignore_ascii_case("colors");
            out.push(line.to_string());
            continue;
        }

        if (in_root || in_colors)
            && let Some((raw_key, _)) = line.split_once('=')
            && root_setting_from_key(raw_key.trim()) == Some(setting)
        {
            continue;
        }

        out.push(line.to_string());
    }

    join_lines(out)
}

/// Remove all root-level lines whose key matches `key` (case-insensitive).
/// Used to remove unknown/removed config keys that don't map to a `RootSettingId`.
pub fn remove_raw_root_key(contents: &str, key: &str) -> String {
    rewrite(contents, |contents| {
        remove_raw_root_key_lines(contents, key)
    })
}

fn remove_raw_root_key_lines(contents: &str, key: &str) -> String {
    let mut out = Vec::new();
    let mut in_root = true;

    for line in contents.lines() {
        let trimmed = line.trim();
        let is_section_header = trimmed.starts_with('[') && trimmed.ends_with(']');
        if is_section_header {
            in_root = false;
            out.push(line.to_string());
            continue;
        }

        if in_root
            && let Some((raw_key, _)) = line.split_once('=')
            && raw_key.trim().eq_ignore_ascii_case(key)
        {
            continue;
        }

        out.push(line.to_string());
    }

    join_lines(out)
}

pub fn replace_keybind_lines(contents: &str, keybind_lines: &[String]) -> String {
    rewrite(contents, |contents| {
        replace_keybind_lines_in(contents, keybind_lines)
    })
}

fn replace_keybind_lines_in(contents: &str, keybind_lines: &[String]) -> String {
    let mut out = Vec::new();
    let mut in_root = true;
    let mut first_section_index = None;
    let mut first_keybind_index = None;

    for line in contents.lines() {
        let trimmed = line.trim();
        let is_section_header = trimmed.starts_with('[') && trimmed.ends_with(']');
        if is_section_header {
            if first_section_index.is_none() {
                first_section_index = Some(out.len());
            }
            in_root = false;
            out.push(line.to_string());
            continue;
        }

        if in_root
            && let Some((raw_key, _)) = line.split_once('=')
            && root_setting_from_key(raw_key.trim()) == Some(RootSettingId::Keybind)
        {
            if first_keybind_index.is_none() {
                first_keybind_index = Some(out.len());
            }
            continue;
        }

        out.push(line.to_string());
    }

    let insert_index = first_keybind_index
        .or(first_section_index)
        .unwrap_or(out.len());
    let mut insertion = Vec::with_capacity(keybind_lines.len());
    for line in keybind_lines {
        insertion.push(format!("keybind = {}", line.trim()));
    }
    out.splice(insert_index..insert_index, insertion);

    join_lines(out)
}

pub fn apply_color_updates(contents: &str, updates: &[ColorSettingUpdate]) -> String {
    rewrite(contents, |contents| {
        apply_color_updates_lines(contents, updates)
    })
}

fn apply_color_updates_lines(contents: &str, updates: &[ColorSettingUpdate]) -> String {
    if updates.is_empty() {
        return normalize_newline(contents);
    }

    let mut updates_by_id: HashMap<ColorSettingId, Option<&str>> = HashMap::new();
    for update in updates {
        updates_by_id.insert(update.id, update.value.as_deref());
    }

    let mut out = Vec::new();
    let mut in_colors = false;
    let mut saw_colors_section = false;
    let mut inserted_ids = HashSet::new();

    for line in contents.lines() {
        let trimmed = line.trim();
        let is_section_header = trimmed.starts_with('[') && trimmed.ends_with(']');
        if is_section_header {
            if in_colors {
                append_missing_color_updates(&mut out, &updates_by_id, &mut inserted_ids);
            }

            let section = trimmed[1..trimmed.len() - 1].trim().to_ascii_lowercase();
            if section == "colors" {
                if saw_colors_section {
                    in_colors = true;
                    out.push("[colors]".to_string());
                    continue;
                }
                saw_colors_section = true;
                in_colors = true;
                out.push("[colors]".to_string());
                continue;
            }

            in_colors = false;
            out.push(line.to_string());
            continue;
        }

        if in_colors {
            if let Some((raw_key, raw_value)) = line.split_once('=')
                && let Some(id) = color_setting_from_key(raw_key.trim())
            {
                if let Some(value) = updates_by_id.get(&id) {
                    if inserted_ids.contains(&id) {
                        continue;
                    }
                    if let Some(value) = value {
                        out.push(format!("{} = {}", color_setting_spec(id).key, value.trim()));
                    }
                    inserted_ids.insert(id);
                    continue;
                }

                out.push(format!(
                    "{} = {}",
                    color_setting_spec(id).key,
                    raw_value.trim()
                ));
                continue;
            }
            out.push(line.to_string());
            continue;
        }

        out.push(line.to_string());
    }

    if in_colors {
        append_missing_color_updates(&mut out, &updates_by_id, &mut inserted_ids);
    } else if !saw_colors_section {
        if !out.is_empty() {
            out.push(String::new());
        }
        out.push("[colors]".to_string());
        append_missing_color_updates(&mut out, &updates_by_id, &mut inserted_ids);
    }

    join_lines(out)
}

fn append_missing_color_updates(
    out: &mut Vec<String>,
    updates_by_id: &HashMap<ColorSettingId, Option<&str>>,
    inserted_ids: &mut HashSet<ColorSettingId>,
) {
    for (id, value) in updates_by_id {
        if inserted_ids.contains(id) {
            continue;
        }
        if let Some(value) = value {
            out.push(format!(
                "{} = {}",
                color_setting_spec(*id).key,
                value.trim()
            ));
            inserted_ids.insert(*id);
        }
    }
}

/// Rewrite config `contents` with `rewrite_lines`, which sees the text without
/// a leading byte-order mark (Windows editors add one) and may end lines with
/// `\n`. The result is written without the mark, and keeps CRLF line endings
/// when the file used them.
pub(super) fn rewrite(contents: &str, rewrite_lines: impl FnOnce(&str) -> String) -> String {
    let contents = strip_bom(contents);
    let output = rewrite_lines(contents);
    if uses_crlf(contents) {
        output.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        output
    }
}

/// `contents` without a leading UTF-8 byte-order mark.
pub(super) fn strip_bom(contents: &str) -> &str {
    contents.strip_prefix('\u{feff}').unwrap_or(contents)
}

/// Whether the first line ends with CRLF.
fn uses_crlf(contents: &str) -> bool {
    contents
        .find('\n')
        .is_some_and(|index| contents[..index].ends_with('\r'))
}

fn normalize_newline(contents: &str) -> String {
    if contents.is_empty() {
        String::new()
    } else if contents.ends_with('\n') {
        contents.to_string()
    } else {
        format!("{contents}\n")
    }
}

fn join_lines(lines: Vec<String>) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use crate::config_core::schema::{ColorSettingId, RootSettingId};

    use super::{
        ColorSettingUpdate, apply_color_updates, remove_raw_root_key, remove_root_setting,
        replace_keybind_lines, upsert_root_setting,
    };

    #[test]
    fn rewrites_drop_a_leading_bom_and_keep_crlf_line_endings() {
        let input =
            "\u{feff}theme = termy\r\nscrollback = 3000\r\n[colors]\r\nforeground = #fff\r\n";
        assert_eq!(
            upsert_root_setting(input, RootSettingId::Theme, "nord"),
            "theme = nord\r\nscrollback = 3000\r\n[colors]\r\nforeground = #fff\r\n"
        );
        assert_eq!(
            remove_root_setting(input, RootSettingId::ScrollbackHistory),
            "theme = termy\r\n[colors]\r\nforeground = #fff\r\n"
        );
        assert_eq!(
            remove_raw_root_key(input, "theme"),
            "scrollback = 3000\r\n[colors]\r\nforeground = #fff\r\n"
        );
        assert_eq!(
            apply_color_updates(input, &[]),
            "theme = termy\r\nscrollback = 3000\r\n[colors]\r\nforeground = #fff\r\n"
        );
        let output = replace_keybind_lines(input, &["cmd-p=toggle_command_palette".to_string()]);
        assert!(!output.starts_with('\u{feff}'));
        assert!(output.contains("keybind = cmd-p=toggle_command_palette\r\n"));
        assert!(!output.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn rewrites_keep_lf_line_endings() {
        let input = "\u{feff}theme = termy\n";
        let output = upsert_root_setting(input, RootSettingId::Theme, "nord");
        assert_eq!(output, "theme = nord\n");
    }

    #[test]
    fn upsert_root_setting_canonicalizes_alias_and_preserves_comments() {
        let input = "# top\nscrollback = 3000\n# after\n";
        let output = upsert_root_setting(input, RootSettingId::ScrollbackHistory, "5000");
        assert_eq!(output, "# top\nscrollback_history = 5000\n# after\n");
    }

    #[test]
    fn remove_root_setting_removes_all_matching_root_entries() {
        let input =
            "theme = termy\nshell = /bin/zsh\nshell = /bin/bash\n[colors]\nforeground = #fff\n";
        let output = remove_root_setting(input, RootSettingId::Shell);
        assert_eq!(output, "theme = termy\n[colors]\nforeground = #fff\n");
    }

    #[test]
    fn root_setting_writes_clean_misplaced_color_section_entries() {
        let input = "theme = termy\n[colors]\ntheme_mode = system\nforeground = #fff\n";
        let output = upsert_root_setting(input, RootSettingId::ThemeMode, "manual");
        assert_eq!(
            output,
            "theme = termy\ntheme_mode = manual\n[colors]\nforeground = #fff\n"
        );
    }

    #[test]
    fn remove_root_setting_cleans_misplaced_color_section_entries() {
        let input = "theme = termy\n[colors]\ntheme_mode = system\nforeground = #fff\n";
        let output = remove_root_setting(input, RootSettingId::ThemeMode);
        assert_eq!(output, "theme = termy\n[colors]\nforeground = #fff\n");
    }

    #[test]
    fn replace_keybind_lines_rewrites_root_keybinds_only() {
        let input = "theme = termy\nkeybind = cmd-c=copy\n[colors]\nforeground = #fff\n";
        let output = replace_keybind_lines(input, &["cmd-p=toggle_command_palette".to_string()]);
        assert!(output.contains("keybind = cmd-p=toggle_command_palette"));
        assert!(output.contains("[colors]\nforeground = #fff\n"));
    }

    #[test]
    fn apply_color_updates_preserves_other_color_lines() {
        let input = "theme = termy\n[colors]\nforeground = #111111\nred = #222222\n";
        let output = apply_color_updates(
            input,
            &[ColorSettingUpdate {
                id: ColorSettingId::Foreground,
                value: Some("#abcdef".to_string()),
            }],
        );
        assert!(output.contains("foreground = #abcdef"));
        assert!(output.contains("red = #222222"));
    }

    #[test]
    fn apply_color_updates_handles_duplicate_colors_sections() {
        let input = "theme = termy\n[colors]\nforeground = #111111\n[colors]\nred = #222222\n";
        let output = apply_color_updates(
            input,
            &[ColorSettingUpdate {
                id: ColorSettingId::Foreground,
                value: Some("#abcdef".to_string()),
            }],
        );
        assert!(output.contains("[colors]\nforeground = #abcdef\n[colors]\nred = #222222\n"));
    }
}
