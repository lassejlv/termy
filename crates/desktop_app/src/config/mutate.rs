use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

#[cfg(test)]
use std::cell::RefCell;

use fs4::fs_std::FileExt;
use serde::{Deserialize, de::IgnoredAny};
use termy_core::config_core::{
    AppConfig, ColorSettingId, ColorSettingUpdate, Rgb8, RootSettingId, SHELL_DECIDE_THEME_ID,
    TaskConfig, apply_color_updates, color_setting_from_key, color_setting_spec, parse_theme_id,
    prettify_config_contents, remove_raw_root_key as remove_raw_root_key_entry,
    remove_root_setting as remove_root_setting_entry, replace_keybind_lines, upsert_root_setting,
};

use super::ConfigIoError;
#[cfg(test)]
use super::DEFAULT_CONFIG;
use super::io::{ensure_config_file, notify_config_changed, write_atomic};

static CONFIG_UPDATE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResetThemeReferences {
    pub theme: bool,
    pub theme_light: bool,
    pub theme_dark: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ImportedThemeJsonValue {
    String(String),
    Ignored(IgnoredAny),
}

#[cfg(test)]
thread_local! {
    static TEST_CONFIG_PATH_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn config_file_for_update() -> Result<PathBuf, ConfigIoError> {
    #[cfg(test)]
    {
        if let Some(path) = TEST_CONFIG_PATH_OVERRIDE
            .with(|override_path| override_path.borrow().as_ref().map(PathBuf::clone))
        {
            if !path.exists() {
                let parent = path
                    .parent()
                    .ok_or_else(|| ConfigIoError::InvalidConfigPath(path.clone()))?;
                fs::create_dir_all(parent).map_err(|source| ConfigIoError::CreateDir {
                    path: parent.to_path_buf(),
                    source,
                })?;
                write_atomic(&path, DEFAULT_CONFIG)?;
            }
            return Ok(path);
        }
    }

    ensure_config_file()
}

fn update_config_contents<R>(
    updater: impl FnOnce(&str) -> Result<(String, R), String>,
) -> Result<R, String> {
    let _process_guard = CONFIG_UPDATE_LOCK
        .lock()
        .map_err(|_| "Config update lock was poisoned by a previous failed update".to_string())?;
    let config_path = config_file_for_update().map_err(|error| error.to_string())?;
    let lock_path = config_path.with_extension("lock");
    let lock_path_display = lock_path.display().to_string();
    let process_lock_file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|source| {
            format!("Failed to open config lock file '{lock_path_display}': {source}")
        })?;
    process_lock_file.lock_exclusive().map_err(|source| {
        format!("Failed to lock config lock file '{lock_path_display}': {source}")
    })?;

    let mut config_lock_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&config_path)
        .map_err(|source| ConfigIoError::ReadConfig {
            path: config_path.clone(),
            source,
        })
        .map_err(|error| error.to_string())?;
    config_lock_file.lock_exclusive().map_err(|source| {
        format!(
            "Failed to lock config file '{}': {}",
            config_path.display(),
            source
        )
    })?;

    let mut existing = String::new();
    config_lock_file
        .read_to_string(&mut existing)
        .map_err(|source| ConfigIoError::ReadConfig {
            path: config_path.clone(),
            source,
        })
        .map_err(|error| error.to_string())?;
    config_lock_file.unlock().map_err(|source| {
        format!(
            "Failed to unlock config file '{}': {}",
            config_path.display(),
            source
        )
    })?;
    drop(config_lock_file);

    let (updated, result) = updater(&existing)?;
    write_atomic(&config_path, &updated).map_err(|error| error.to_string())?;
    notify_config_changed();
    process_lock_file.unlock().map_err(|source| {
        format!("Failed to unlock config lock file '{lock_path_display}': {source}")
    })?;
    Ok(result)
}

pub fn set_root_setting(setting: RootSettingId, value: &str) -> Result<(), String> {
    update_config_contents(|existing| Ok((upsert_root_setting(existing, setting, value), ())))
}

pub fn remove_root_setting(setting: RootSettingId) -> Result<(), String> {
    update_config_contents(|existing| Ok((remove_root_setting_entry(existing, setting), ())))
}

pub fn remove_raw_root_key_from_config(key: &str) -> Result<(), String> {
    let key = key.to_string();
    update_config_contents(move |existing| Ok((remove_raw_root_key_entry(existing, &key), ())))
}

pub fn set_theme_in_config(theme_id: &str) -> Result<String, String> {
    let theme = parse_theme_id(theme_id).ok_or_else(|| "Invalid theme id".to_string())?;
    set_root_setting(RootSettingId::Theme, &theme)?;
    Ok(format!("Theme set to {theme}"))
}

pub fn reset_theme_references_in_config(theme_id: &str) -> Result<ResetThemeReferences, String> {
    let theme_id = parse_theme_id(theme_id).ok_or_else(|| "Invalid theme id".to_string())?;
    update_config_contents(move |existing| {
        let config = AppConfig::from_contents(existing);
        let reset = ResetThemeReferences {
            theme: config.theme.eq_ignore_ascii_case(&theme_id),
            theme_light: config.theme_light.eq_ignore_ascii_case(&theme_id),
            theme_dark: config.theme_dark.eq_ignore_ascii_case(&theme_id),
        };
        let mut updated = existing.to_string();
        if reset.theme {
            updated = upsert_root_setting(&updated, RootSettingId::Theme, SHELL_DECIDE_THEME_ID);
        }
        if reset.theme_light {
            updated = upsert_root_setting(&updated, RootSettingId::ThemeLight, "termy-light");
        }
        if reset.theme_dark {
            updated = upsert_root_setting(&updated, RootSettingId::ThemeDark, "termy");
        }
        Ok((updated, reset))
    })
}

pub fn set_color_setting(color: ColorSettingId, value: Option<&str>) -> Result<(), String> {
    if let Some(value) = value
        && Rgb8::from_hex(value).is_none()
    {
        return Err(format!(
            "Invalid hex color for '{}': {}",
            color_setting_spec(color).key,
            value
        ));
    }

    let updates = vec![ColorSettingUpdate {
        id: color,
        value: value.map(ToString::to_string),
    }];
    update_config_contents(|existing| Ok((apply_color_updates(existing, &updates), ())))
}

pub fn set_keybind_lines(lines: &[String]) -> Result<(), String> {
    update_config_contents(|existing| Ok((replace_keybind_lines(existing, lines), ())))
}

pub fn prettify_config_file() -> Result<String, String> {
    update_config_contents(|existing| {
        let prettified = prettify_config_contents(existing);
        Ok((prettified.clone(), prettified))
    })
}

pub fn upsert_task(task: TaskConfig) -> Result<(), String> {
    let task_name = task.name.trim().to_string();
    let command = task.command.trim().to_string();
    if task_name.is_empty() {
        return Err("Task name is required".to_string());
    }
    if command.is_empty() {
        return Err("Task command is required".to_string());
    }

    update_config_contents(|existing| Ok((upsert_task_lines(existing, &task), ())))
}

pub fn import_colors_from_json(json_path: &Path) -> Result<String, String> {
    let contents =
        fs::read_to_string(json_path).map_err(|e| format!("Failed to read file: {e}"))?;

    let colors: BTreeMap<String, ImportedThemeJsonValue> =
        serde_json::from_str(&contents).map_err(|e| format!("Invalid JSON: {e}"))?;

    let mut updates_by_id: HashMap<ColorSettingId, String> = HashMap::new();
    for (key, value) in colors {
        if key.starts_with('$') {
            continue;
        }

        let Some(id) = color_setting_from_key(&key) else {
            continue;
        };

        let ImportedThemeJsonValue::String(hex) = value else {
            return Err(format!("Color '{key}' must be a hex string"));
        };

        if Rgb8::from_hex(&hex).is_none() {
            return Err(format!("Invalid hex color for '{key}': {hex}"));
        }

        let is_canonical_key = key.eq_ignore_ascii_case(color_setting_spec(id).key);
        match updates_by_id.get_mut(&id) {
            Some(existing_hex) if is_canonical_key => *existing_hex = hex,
            Some(_) => {}
            None => {
                updates_by_id.insert(id, hex);
            }
        }
    }

    if updates_by_id.is_empty() {
        return Err("No valid colors found in JSON".to_string());
    }

    let color_count = updates_by_id.len();
    let updates = updates_by_id
        .into_iter()
        .map(|(id, value)| ColorSettingUpdate {
            id,
            value: Some(value),
        })
        .collect::<Vec<_>>();
    update_config_contents(|existing| Ok((apply_color_updates(existing, &updates), ())))?;
    Ok(format!("Imported {color_count} colors"))
}

fn upsert_task_lines(contents: &str, task: &TaskConfig) -> String {
    let task_name = task.name.trim();
    let prefix = format!("task.{task_name}.");
    let mut out = Vec::new();
    let mut in_root = true;
    let mut first_task_index = None;
    let mut first_section_index = None;

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
            && raw_key.trim().starts_with(prefix.as_str())
        {
            if first_task_index.is_none() {
                first_task_index = Some(out.len());
            }
            continue;
        }

        out.push(line.to_string());
    }

    let insert_index = first_task_index
        .or(first_section_index)
        .unwrap_or(out.len());
    let mut insertion = vec![format!(
        "task.{task_name}.command = {}",
        task.command.trim()
    )];
    if let Some(layout) = task
        .layout
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        insertion.push(format!("task.{task_name}.layout = {layout}"));
    }
    if let Some(working_dir) = task
        .working_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        insertion.push(format!("task.{task_name}.working_dir = {working_dir}"));
    }
    if let Some(keybind) = task
        .keybind
        .as_ref()
        .map(|keybind| keybind.value.trim())
        .filter(|value| !value.is_empty())
    {
        insertion.push(format!("task.{task_name}.keybind = {keybind}"));
    }
    out.splice(insert_index..insert_index, insertion);

    if out.is_empty() {
        String::new()
    } else {
        let mut result = out.join("\n");
        result.push('\n');
        result
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{TEST_CONFIG_PATH_OVERRIDE, write_atomic};
    use super::{import_colors_from_json, reset_theme_references_in_config, upsert_task_lines};
    use crate::config::TaskConfig;

    struct ConfigPathOverrideGuard {
        previous: Option<PathBuf>,
    }

    impl ConfigPathOverrideGuard {
        fn set(path: &Path) -> Self {
            let previous = TEST_CONFIG_PATH_OVERRIDE
                .with(|override_path| override_path.replace(Some(path.to_path_buf())));
            Self { previous }
        }
    }

    impl Drop for ConfigPathOverrideGuard {
        fn drop(&mut self) {
            let previous = self.previous.take();
            TEST_CONFIG_PATH_OVERRIDE.with(|override_path| {
                override_path.replace(previous);
            });
        }
    }

    fn with_temp_config_file_inner(
        initial_contents: Option<&str>,
        test: impl FnOnce(&Path, &Path),
    ) {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_dir = temp_dir.path().join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let config_path = config_dir.join("config.txt");
        write_atomic(
            &config_path,
            initial_contents.unwrap_or(crate::config::DEFAULT_CONFIG),
        )
        .expect("write config");

        let _restore_guard = ConfigPathOverrideGuard::set(&config_path);
        test(temp_dir.path(), &config_path);
    }

    fn with_temp_config_file(test: impl FnOnce(&Path, &Path)) {
        with_temp_config_file_inner(None, test);
    }

    #[test]
    fn with_temp_config_file_restores_override_after_panic() {
        let before = TEST_CONFIG_PATH_OVERRIDE.with(|override_path| override_path.borrow().clone());
        let result = std::panic::catch_unwind(|| {
            with_temp_config_file_inner(None, |_, _| panic!("intentional panic"));
        });
        assert!(result.is_err());
        assert_eq!(
            TEST_CONFIG_PATH_OVERRIDE.with(|override_path| override_path.borrow().clone()),
            before
        );
    }

    #[test]
    fn import_colors_json_accepts_aliases_and_canonical_keys() {
        with_temp_config_file(|temp_dir, _| {
            let json_path = temp_dir.join("colors.json");
            std::fs::write(
                &json_path,
                "{\n  \"foreground\": \"#112233\",\n  \"color1\": \"#445566\",\n  \"red\": \"#778899\"\n}\n",
            )
            .expect("write json");

            let result = import_colors_from_json(&json_path).expect("import colors");
            assert!(result.contains("Imported"));
        });
    }

    #[test]
    fn import_colors_json_ignores_unknown_metadata_values() {
        with_temp_config_file(|temp_dir, _| {
            let json_path = temp_dir.join("colors.json");
            std::fs::write(
                &json_path,
                "{\n  \"$schema\": \"./theme.schema.json\",\n  \"metadata\": {\"name\": \"ignored\"},\n  \"foreground\": \"#112233\"\n}\n",
            )
            .expect("write json");

            let result = import_colors_from_json(&json_path).expect("import colors");
            assert!(result.contains("Imported 1 colors"));
        });
    }

    #[test]
    fn import_colors_json_rejects_known_non_string_values() {
        with_temp_config_file(|temp_dir, _| {
            let json_path = temp_dir.join("colors.json");
            std::fs::write(&json_path, "{\n  \"foreground\": 42\n}\n").expect("write json");

            let error = import_colors_from_json(&json_path).expect_err("reject color");
            assert_eq!(error, "Color 'foreground' must be a hex string");
        });
    }

    #[test]
    fn reset_theme_references_updates_all_system_slots_in_one_write() {
        with_temp_config_file_inner(
            Some(
                "theme = catppuccin-macchiato\n\
                 theme_mode = system\n\
                 theme_light = catppuccin-macchiato\n\
                 theme_dark = catppuccin-macchiato\n\
                 font_size = 15\n",
            ),
            |_, config_path| {
                let reset = reset_theme_references_in_config("catppuccin-macchiato")
                    .expect("reset theme references");
                assert_eq!(
                    reset,
                    super::ResetThemeReferences {
                        theme: true,
                        theme_light: true,
                        theme_dark: true,
                    }
                );

                let contents = std::fs::read_to_string(config_path).expect("read config");
                let config = termy_core::config_core::AppConfig::from_contents(&contents);
                assert_eq!(config.theme, "shell-decide");
                assert_eq!(config.theme_light, "termy-light");
                assert_eq!(config.theme_dark, "termy");
                assert_eq!(config.font_size, 15.0);
            },
        );
    }

    #[test]
    fn upsert_task_lines_replaces_existing_task_block() {
        let input = "theme = termy\ntask.build.command = cargo test\ntask.build.layout = app\n[colors]\nforeground = #fff\n";
        let output = upsert_task_lines(
            input,
            &TaskConfig {
                name: "build".to_string(),
                command: "cargo build".to_string(),
                layout: Some("dashboard".to_string()),
                working_dir: None,
                keybind: Some(termy_core::config_core::KeybindConfigLine {
                    line_number: 99,
                    value: "secondary-shift-b".to_string(),
                }),
            },
        );

        assert!(output.contains("task.build.command = cargo build\n"));
        assert!(output.contains("task.build.layout = dashboard\n"));
        assert!(output.contains("task.build.keybind = secondary-shift-b\n"));
        assert!(!output.contains("cargo test"));
        assert!(output.contains("[colors]\nforeground = #fff\n"));
    }
}
