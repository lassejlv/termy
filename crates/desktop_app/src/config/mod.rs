mod error;
mod io;
mod mutate;
mod preview;

use crate::theme_library;
use std::time::Duration;
use std::{
    collections::HashSet,
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

pub use error::ConfigIoError;
#[cfg(target_os = "windows")]
pub(crate) use io::shell_execute_open;
pub(crate) use io::write_atomic_unsynced;
pub use io::{ensure_config_file, open_config_file, subscribe_config_changes};
pub use mutate::{
    import_colors_from_json, prettify_config_file, remove_root_setting, set_color_setting,
    set_keybind_lines, set_root_setting, set_theme_in_config, upsert_task,
};
pub use preview::{
    BackgroundOpacityPreview, current_background_opacity_preview, effective_background_opacity,
    publish_background_opacity_preview, subscribe_background_opacity_preview,
    synced_background_opacity_preview,
};
pub use termy_core::config_core::{
    AppConfig, AppearanceMode, ClipboardReadPolicy, ConfigDiagnostic, ConfigDiagnosticKind,
    CursorStyle, CustomColors, PaneFocusEffect, SHELL_DECIDE_THEME_ID, SystemAppearance,
    TabBarPosition, TabCloseVisibility, TabStyle, TabTitleConfig, TabTitleSource, TabWidthMode,
    TaskConfig, TermProgram, TerminalScrollbarStyle, TerminalScrollbarVisibility, WindowsShell,
    WorkingDirFallback, resolve_active_theme,
};

pub fn system_appearance_from_window(appearance: gpui_kit::WindowAppearance) -> SystemAppearance {
    match appearance {
        gpui_kit::WindowAppearance::Light | gpui_kit::WindowAppearance::VibrantLight => {
            SystemAppearance::Light
        }
        gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark => {
            SystemAppearance::Dark
        }
    }
}

pub struct LoadedConfig {
    pub path: PathBuf,
    pub config: AppConfig,
    pub diagnostics: Vec<ConfigDiagnostic>,
    pub fingerprint: u64,
}

pub struct RuntimeConfigLoad {
    pub config: AppConfig,
    pub path: Option<PathBuf>,
    pub fingerprint: Option<u64>,
    pub loaded_from_disk: bool,
}

pub(crate) const DEFAULT_CONFIG: &str = termy_core::config_core::DEFAULT_CONFIG_TEMPLATE;

fn load_from_path(path: PathBuf) -> Result<LoadedConfig, ConfigIoError> {
    let original_contents =
        fs::read_to_string(&path).map_err(|source| ConfigIoError::ReadConfig {
            path: path.clone(),
            source,
        })?;
    let contents = migrate_legacy_builtin_theme_in_contents(&original_contents);
    if contents != original_contents {
        io::write_atomic(&path, &contents)?;
        io::notify_config_changed();
    }
    let fingerprint = config_fingerprint_from_bytes(contents.as_bytes());
    let report = AppConfig::from_contents_with_report(&contents);

    Ok(LoadedConfig {
        path,
        config: report.config,
        diagnostics: report.diagnostics,
        fingerprint,
    })
}

pub fn log_parse_diagnostics(diagnostics: &[ConfigDiagnostic]) {
    for diagnostic in diagnostics {
        log::warn!(
            "Config diagnostic line {} [{:?}]: {}",
            diagnostic.line_number,
            diagnostic.kind,
            diagnostic.message
        );
    }
}

pub fn summarize_parse_diagnostics(diagnostics: &[ConfigDiagnostic]) -> Option<String> {
    if diagnostics.is_empty() {
        return None;
    }

    let first = &diagnostics[0];
    Some(format!(
        "Config has {} warning(s). First: line {} [{}] {}",
        diagnostics.len(),
        first.line_number,
        diagnostic_kind_label(first.kind),
        first.message
    ))
}

/// A one-click fix offered by the config diagnostics toast.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigFix {
    /// Remove every occurrence of an unknown root key.
    RemoveUnknownKey(String),
    /// Remove all but the last (effective) occurrence of a duplicated root key.
    RemoveDuplicateKey(String),
}

/// The diagnostics toast currently shown for the config file, if any.
struct DiagnosticsToast {
    fingerprint: u64,
    toast_id: u64,
    fixes: Vec<ConfigFix>,
}

/// The config is reloaded from many places (each window, settings, tmux), so
/// the toast is only shown again when the diagnostics themselves change. This
/// also bounds the pending fixes to the latest toast.
static DIAGNOSTICS_TOAST: LazyLock<Mutex<Option<DiagnosticsToast>>> =
    LazyLock::new(|| Mutex::new(None));

fn diagnostics_fingerprint(diagnostics: &[ConfigDiagnostic]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for diagnostic in diagnostics {
        diagnostic.line_number.hash(&mut hasher);
        diagnostic_kind_label(diagnostic.kind).hash(&mut hasher);
        diagnostic.message.hash(&mut hasher);
    }
    hasher.finish()
}

fn fixes_for_diagnostics(diagnostics: &[ConfigDiagnostic]) -> Vec<ConfigFix> {
    let mut fixes = Vec::new();
    for diagnostic in diagnostics {
        // Messages look like "Unknown root key 'key'" and
        // "Duplicate root key 'key' (first defined on line N)".
        let fix = match diagnostic.kind {
            ConfigDiagnosticKind::UnknownRootKey => diagnostic
                .message
                .strip_prefix("Unknown root key '")
                .and_then(|rest| rest.split_once('\''))
                .map(|(key, _)| ConfigFix::RemoveUnknownKey(key.to_string())),
            ConfigDiagnosticKind::DuplicateRootKey => diagnostic
                .message
                .strip_prefix("Duplicate root key '")
                .and_then(|rest| rest.split_once('\''))
                .map(|(key, _)| ConfigFix::RemoveDuplicateKey(key.to_string())),
            _ => None,
        };
        if let Some(fix) = fix
            && !fixes.contains(&fix)
        {
            fixes.push(fix);
        }
    }
    fixes
}

pub fn show_parse_diagnostics_toast(diagnostics: &[ConfigDiagnostic]) {
    let mut state = DIAGNOSTICS_TOAST.lock().expect("diagnostics toast lock");
    show_parse_diagnostics_toast_with_state(
        &mut state,
        diagnostics,
        |summary, action_label| {
            crate::ui::toast::enqueue_actionable_toast_with_id(
                crate::ui::toast::ToastKind::Warning,
                summary,
                Some(Duration::from_secs(10)),
                action_label,
            )
        },
        crate::ui::toast::dismiss_toast,
    );
}

fn show_parse_diagnostics_toast_with_state(
    state: &mut Option<DiagnosticsToast>,
    diagnostics: &[ConfigDiagnostic],
    enqueue: impl FnOnce(String, Option<String>) -> u64,
    mut dismiss: impl FnMut(u64),
) {
    let Some(summary) = summarize_parse_diagnostics(diagnostics) else {
        if let Some(previous) = state.take() {
            dismiss(previous.toast_id);
        }
        return;
    };

    let fingerprint = diagnostics_fingerprint(diagnostics);
    if state
        .as_ref()
        .is_some_and(|previous| previous.fingerprint == fingerprint)
    {
        return;
    }
    if let Some(previous) = state.take() {
        dismiss(previous.toast_id);
    }

    let fixes = fixes_for_diagnostics(diagnostics);
    let action_label = if fixes.is_empty() {
        None
    } else {
        Some("Fix".to_string())
    };

    let toast_id = enqueue(summary, action_label);

    *state = Some(DiagnosticsToast {
        fingerprint,
        toast_id,
        fixes,
    });
}

/// Execute the fix registered for a toast (remove unknown/duplicate keys from config).
/// Returns true if any fix was applied.
pub fn execute_fix_for_toast(toast_id: u64) -> bool {
    let fixes = {
        let mut state = DIAGNOSTICS_TOAST.lock().expect("diagnostics toast lock");
        match state.as_mut() {
            Some(toast) if toast.toast_id == toast_id => std::mem::take(&mut toast.fixes),
            _ => return false,
        }
    };
    let mut fixed_any = false;
    for fix in &fixes {
        let result = match fix {
            ConfigFix::RemoveUnknownKey(key) => mutate::remove_raw_root_key_from_config(key),
            ConfigFix::RemoveDuplicateKey(key) => {
                mutate::remove_duplicate_root_key_from_config(key)
            }
        };
        match result {
            Ok(_) => fixed_any = true,
            Err(e) => log::warn!("Failed to fix config: {fix:?}: {e}"),
        }
    }
    fixed_any
}

// This uses Rust's default SipHash-based hasher and is only for in-process
// change detection. Do not persist or compare this fingerprint across
// processes/toolchain versions; use a stable hash algorithm for that.
pub fn config_fingerprint(path: &Path) -> Option<u64> {
    let contents = fs::read(path).ok()?;
    Some(config_fingerprint_from_bytes(&contents))
}

fn config_fingerprint_from_bytes(contents: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);
    hasher.finish()
}

pub fn report_config_error_once(
    previous_error: &mut Option<String>,
    error_context: &'static str,
    error: &ConfigIoError,
) {
    let error_message = error.to_string();
    if previous_error.as_deref() == Some(error_message.as_str()) {
        return;
    }

    log::error!("{error_context}: {error_message}");
    crate::ui::toast::error(error_message.clone());
    *previous_error = Some(error_message);
}

pub fn load_runtime_config(
    previous_error: &mut Option<String>,
    error_context: &'static str,
) -> RuntimeConfigLoad {
    let path = match ensure_config_file() {
        Ok(path) => path,
        Err(error) => {
            report_config_error_once(previous_error, error_context, &error);
            return RuntimeConfigLoad {
                config: AppConfig::default(),
                path: None,
                fingerprint: None,
                loaded_from_disk: false,
            };
        }
    };
    match load_from_path(path.clone()) {
        Ok(loaded) => {
            log_parse_diagnostics(&loaded.diagnostics);
            show_parse_diagnostics_toast(&loaded.diagnostics);
            *previous_error = None;
            RuntimeConfigLoad {
                config: loaded.config,
                path: Some(loaded.path),
                fingerprint: Some(loaded.fingerprint),
                loaded_from_disk: true,
            }
        }
        Err(error) => {
            report_config_error_once(previous_error, error_context, &error);
            RuntimeConfigLoad {
                config: AppConfig::default(),
                path: Some(path),
                fingerprint: None,
                loaded_from_disk: false,
            }
        }
    }
}

fn diagnostic_kind_label(kind: ConfigDiagnosticKind) -> &'static str {
    match kind {
        ConfigDiagnosticKind::UnknownSection => "unknown-section",
        ConfigDiagnosticKind::UnknownRootKey => "unknown-root-key",
        ConfigDiagnosticKind::UnknownColorKey => "unknown-color-key",
        ConfigDiagnosticKind::InvalidSyntax => "invalid-syntax",
        ConfigDiagnosticKind::InvalidValue => "invalid-value",
        ConfigDiagnosticKind::DuplicateRootKey => "duplicate-root-key",
    }
}

fn migrate_legacy_builtin_theme_in_contents(contents: &str) -> String {
    let installed_theme_ids: HashSet<String> =
        theme_library::local_theme_ids().into_iter().collect();
    migrate_legacy_builtin_theme_in_contents_with_installed_ids(contents, &installed_theme_ids)
}

fn migrate_legacy_builtin_theme_in_contents_with_installed_ids(
    contents: &str,
    installed_theme_ids: &HashSet<String>,
) -> String {
    let report = AppConfig::from_contents(contents);
    // A theme that resolves as written (built in or a local file) is never
    // rewritten, even when it looks like a legacy alias such as `tokyonight`.
    if termy_core::themes::builtin_theme(&report.theme).is_some()
        || installed_theme_ids.contains(&termy_core::themes::normalize_theme_id(&report.theme))
    {
        return contents.to_string();
    }
    let Some(canonical_legacy_theme) = canonical_legacy_builtin_theme_id(&report.theme) else {
        return contents.to_string();
    };

    // Legacy aliases move to their canonical id when that resolves, and to
    // shell-decide otherwise.
    let next_theme = if termy_core::themes::builtin_theme(canonical_legacy_theme).is_some()
        || installed_theme_ids.contains(canonical_legacy_theme)
    {
        canonical_legacy_theme.to_string()
    } else {
        SHELL_DECIDE_THEME_ID.to_string()
    };

    if report.theme == next_theme {
        return contents.to_string();
    }

    termy_core::config_core::upsert_root_setting(
        contents,
        termy_core::config_core::RootSettingId::Theme,
        &next_theme,
    )
}

fn canonical_legacy_builtin_theme_id(theme_id: &str) -> Option<&'static str> {
    let normalized = termy_core::themes::normalize_theme_id(theme_id);
    let lookup = normalized.replace('-', "");

    match lookup.as_str() {
        "termy" | "default" => Some("termy"),
        "tokyonight" => Some("tokyo-night"),
        "catppuccin" | "catppuccinmocha" => Some("catppuccin-mocha"),
        "dracula" => Some("dracula"),
        "gruvbox" | "gruvboxdark" => Some("gruvbox-dark"),
        "nord" => Some("nord"),
        "solarized" | "solarizeddark" => Some("solarized-dark"),
        "one" | "onedark" => Some("one-dark"),
        "monokai" => Some("monokai"),
        "material" | "materialdark" => Some("material-dark"),
        "palenight" => Some("palenight"),
        "tomorrow" | "tomorrownight" => Some("tomorrow-night"),
        "oceanic" | "oceanicnext" => Some("oceanic-next"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses_without_diagnostics() {
        let report = AppConfig::from_contents_with_report(DEFAULT_CONFIG);
        assert!(
            report.diagnostics.is_empty(),
            "expected no diagnostics but got: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn default_config_template_matches_default_struct() {
        let parsed = AppConfig::from_contents(DEFAULT_CONFIG);
        assert_eq!(parsed, AppConfig::default());
    }

    #[test]
    fn gpui_window_appearance_maps_light_and_dark_variants() {
        assert_eq!(
            system_appearance_from_window(gpui_kit::WindowAppearance::Light),
            SystemAppearance::Light
        );
        assert_eq!(
            system_appearance_from_window(gpui_kit::WindowAppearance::VibrantLight),
            SystemAppearance::Light
        );
        assert_eq!(
            system_appearance_from_window(gpui_kit::WindowAppearance::Dark),
            SystemAppearance::Dark
        );
        assert_eq!(
            system_appearance_from_window(gpui_kit::WindowAppearance::VibrantDark),
            SystemAppearance::Dark
        );
    }

    #[test]
    fn default_config_root_keys_are_known() {
        let mut in_section = false;

        for line in DEFAULT_CONFIG.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_section = true;
                continue;
            }
            if in_section {
                continue;
            }

            let (key, _) = trimmed
                .split_once('=')
                .expect("active config line must contain '='");
            let key = key.trim();
            assert!(
                termy_core::config_core::VALID_ROOT_KEYS
                    .iter()
                    .any(|valid| valid.eq_ignore_ascii_case(key)),
                "unknown root key in DEFAULT_CONFIG: {key}"
            );
        }
    }

    #[test]
    fn default_config_sections_are_known() {
        for line in DEFAULT_CONFIG.lines() {
            let trimmed = line.trim();
            if !(trimmed.starts_with('[') && trimmed.ends_with(']')) {
                continue;
            }
            let section_name = trimmed[1..trimmed.len() - 1].trim();
            assert!(
                termy_core::config_core::VALID_SECTIONS
                    .iter()
                    .any(|section| section.eq_ignore_ascii_case(section_name)),
                "unknown section in DEFAULT_CONFIG: {section_name}"
            );
        }
    }

    #[test]
    fn keeps_earlier_builtin_theme_ids_that_ship_again() {
        for theme in ["monokai", "tokyo-night", "palenight"] {
            let input = format!("theme = {theme}\nfont_size = 14\n");
            let output = migrate_legacy_builtin_theme_in_contents_with_installed_ids(
                &input,
                &HashSet::new(),
            );
            assert_eq!(output, input);
        }
    }

    #[test]
    fn keeps_legacy_ids_that_are_now_builtin_themes() {
        for input in [
            "theme = dracula\nfont_size = 14\n",
            "theme = tokyonight\nfont_size = 14\n",
        ] {
            let output =
                migrate_legacy_builtin_theme_in_contents_with_installed_ids(input, &HashSet::new());
            assert_eq!(output, input);
        }
    }

    #[test]
    fn migrates_legacy_alias_to_builtin_canonical_id() {
        let output = migrate_legacy_builtin_theme_in_contents_with_installed_ids(
            "theme = catppuccin\nfont_size = 14\n",
            &HashSet::new(),
        );
        assert_eq!(output, "theme = catppuccin-mocha\nfont_size = 14\n");
    }

    #[test]
    fn keeps_builtin_termy_theme_unchanged() {
        let input = "theme = termy\nfont_size = 14\n";
        let output =
            migrate_legacy_builtin_theme_in_contents_with_installed_ids(input, &HashSet::new());
        assert_eq!(output, input);
    }

    #[test]
    fn default_config_template_is_not_migrated() {
        let output = migrate_legacy_builtin_theme_in_contents_with_installed_ids(
            DEFAULT_CONFIG,
            &HashSet::new(),
        );
        assert_eq!(output, DEFAULT_CONFIG);
    }

    #[test]
    fn migrates_legacy_default_theme_alias_to_builtin_termy() {
        let output = migrate_legacy_builtin_theme_in_contents_with_installed_ids(
            "theme = default\nfont_size = 14\n",
            &HashSet::new(),
        );
        assert_eq!(output, "theme = termy\nfont_size = 14\n");
    }

    fn diagnostic(kind: ConfigDiagnosticKind, message: &str) -> ConfigDiagnostic {
        ConfigDiagnostic {
            line_number: 3,
            kind,
            message: message.to_string(),
        }
    }

    #[test]
    fn duplicate_and_unknown_root_keys_are_fixable() {
        let report =
            AppConfig::from_contents_with_report("font_size = 12\nfont_size = 14\nbogus = 1\n");
        assert_eq!(
            fixes_for_diagnostics(&report.diagnostics),
            vec![
                ConfigFix::RemoveDuplicateKey("font_size".to_string()),
                ConfigFix::RemoveUnknownKey("bogus".to_string()),
            ]
        );
    }

    #[test]
    fn identical_diagnostics_show_a_single_toast() {
        let diagnostics = vec![diagnostic(
            ConfigDiagnosticKind::UnknownRootKey,
            "Unknown root key 'bogus'",
        )];
        let mut state = None;
        let mut next_id = 0;
        let mut dismissed = Vec::new();
        let mut show = |state: &mut Option<DiagnosticsToast>, diagnostics: &[ConfigDiagnostic]| {
            show_parse_diagnostics_toast_with_state(
                state,
                diagnostics,
                |_, _| {
                    next_id += 1;
                    next_id
                },
                |id| dismissed.push(id),
            );
        };

        show(&mut state, &diagnostics);
        let first_id = state.as_ref().expect("toast shown").toast_id;
        show(&mut state, &diagnostics);
        assert_eq!(state.as_ref().expect("toast kept").toast_id, first_id);

        let changed = vec![diagnostic(
            ConfigDiagnosticKind::UnknownRootKey,
            "Unknown root key 'other'",
        )];
        show(&mut state, &changed);
        let second = state.as_ref().expect("new toast shown");
        assert_ne!(second.toast_id, first_id);
        assert_eq!(
            second.fixes,
            vec![ConfigFix::RemoveUnknownKey("other".to_string())]
        );

        let second_id = second.toast_id;
        show(&mut state, &[]);
        assert!(state.is_none());
        assert_eq!(dismissed, vec![first_id, second_id]);
    }

    #[test]
    fn migrates_legacy_builtin_alias_to_installed_slug() {
        let output = migrate_legacy_builtin_theme_in_contents_with_installed_ids(
            "theme = one\nfont_size = 14\n",
            &HashSet::from([String::from("one-dark")]),
        );
        assert_eq!(output, "theme = one-dark\nfont_size = 14\n");
    }
}
