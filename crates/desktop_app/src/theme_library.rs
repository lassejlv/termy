//! The themes the app offers: the built-in collection from `termy_core`, plus
//! local theme files at `<config dir>/themes/<id>.json`. A local file wins over
//! a built-in theme with the same id, so a customized copy can replace it.

use crate::config;
use std::path::PathBuf;
use termy_core::themes::{ThemeColors, builtin_theme, builtin_themes};

fn local_themes_dir() -> Option<PathBuf> {
    let config_path = config::ensure_config_file().ok()?;
    Some(config_path.parent()?.join("themes"))
}

/// Ids of the local theme files, sorted.
pub(crate) fn local_theme_ids() -> Vec<String> {
    local_themes_dir().map_or_else(Vec::new, |dir| termy_core::themes::local_theme_ids(&dir))
}

fn local_theme_colors(theme_id: &str) -> Option<ThemeColors> {
    termy_core::themes::load_local_theme(&local_themes_dir()?, theme_id)
}

/// Colors of a theme: its local file, else the built-in theme.
pub(crate) fn theme_colors(theme_id: &str) -> Option<ThemeColors> {
    local_theme_colors(theme_id).or_else(|| builtin_theme(theme_id))
}

/// Every theme id the app can apply: built-in themes in their own order,
/// then local themes that are not built in.
pub(crate) fn available_theme_ids() -> Vec<String> {
    let mut ids: Vec<String> = builtin_themes()
        .iter()
        .map(|theme| theme.id.to_string())
        .collect();
    ids.extend(
        local_theme_ids()
            .into_iter()
            .filter(|id| builtin_theme(id).is_none()),
    );
    ids
}
