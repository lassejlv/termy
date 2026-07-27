//! Most-recently-used tracking for command palette rows.
//!
//! Executed commands are remembered (most recent first) so the palette can
//! surface them without a query and boost them while filtering. Only rows with
//! a stable identity are tracked: built-in commands keyed by their config name
//! and plugin commands keyed by `plugin:<plugin_id>:<command_id>`.

use super::state::{CommandPaletteItem, CommandPaletteItemKind};
use std::path::{Path, PathBuf};

pub(super) const RECENTS_FILE: &str = "command-palette-recents.json";
const MAX_RECENTS: usize = 50;
/// Score added to the most recent entry while filtering; later entries get
/// progressively less so relevance still leads.
const RECENT_BONUS_MAX: i32 = 24;
const RECENT_BONUS_STEP: i32 = 2;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in super::super) struct CommandPaletteRecents {
    /// Command keys, most recently executed first.
    keys: Vec<String>,
}

impl CommandPaletteRecents {
    pub(in super::super) fn load() -> Self {
        let Some(path) = recents_path() else {
            return Self::default();
        };
        Self::load_from_path(&path)
    }

    pub(super) fn load_from_path(path: &Path) -> Self {
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                log::warn!("Failed to read command palette recents: {error}");
                return Self::default();
            }
        };

        match serde_json::from_str::<Vec<String>>(&contents) {
            Ok(mut keys) => {
                keys.truncate(MAX_RECENTS);
                Self { keys }
            }
            Err(error) => {
                log::warn!("Failed to parse command palette recents: {error}");
                Self::default()
            }
        }
    }

    /// Moves `key` to the front and persists the list. Persistence failures are
    /// logged and otherwise ignored: recents are a convenience, not state the
    /// palette depends on.
    pub(super) fn record(&mut self, key: String) {
        self.push(key);
        if let Some(path) = recents_path()
            && let Err(error) = self.save_to_path(&path)
        {
            log::warn!("Failed to save command palette recents: {error}");
        }
    }

    pub(super) fn push(&mut self, key: String) {
        self.keys.retain(|existing| existing != &key);
        self.keys.insert(0, key);
        self.keys.truncate(MAX_RECENTS);
    }

    pub(super) fn save_to_path(&self, path: &Path) -> std::io::Result<()> {
        let contents = serde_json::to_string(&self.keys)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        std::fs::write(path, contents)
    }

    /// Position in the recents list, 0 being the most recent.
    pub(super) fn rank(&self, key: &str) -> Option<usize> {
        self.keys.iter().position(|existing| existing == key)
    }

    pub(super) fn rank_for_item(&self, item: &CommandPaletteItem) -> Option<usize> {
        self.rank(&recent_key_for_item(item)?)
    }

    pub(super) fn bonus_for_item(&self, item: &CommandPaletteItem) -> i32 {
        match self.rank_for_item(item) {
            Some(rank) => {
                (RECENT_BONUS_MAX - (rank as i32).saturating_mul(RECENT_BONUS_STEP)).max(0)
            }
            None => 0,
        }
    }
}

/// Stable identity for rows worth remembering, or `None` for rows whose
/// meaning depends on transient state (themes, sessions, layouts, tasks).
pub(super) fn recent_key_for_item(item: &CommandPaletteItem) -> Option<String> {
    match &item.kind {
        CommandPaletteItemKind::Command(action) => {
            Some(action.to_command_id().config_name().to_string())
        }
        CommandPaletteItemKind::PluginCommand {
            plugin_id,
            command_id,
            ..
        } => Some(format!("plugin:{plugin_id}:{command_id}")),
        _ => None,
    }
}

fn recents_path() -> Option<PathBuf> {
    let config_path = termy_config_core::config_path()?;
    Some(config_path.parent()?.join(RECENTS_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_view::CommandAction;
    use tempfile::tempdir;

    fn command_item(action: CommandAction) -> CommandPaletteItem {
        CommandPaletteItem::command_with_state("Title", "keywords", action, true, None)
    }

    #[test]
    fn push_moves_existing_key_to_front_without_duplicating() {
        let mut recents = CommandPaletteRecents::default();
        recents.push("new_tab".to_string());
        recents.push("close_tab".to_string());
        recents.push("new_tab".to_string());

        assert_eq!(recents.keys, vec!["new_tab", "close_tab"]);
        assert_eq!(recents.rank("new_tab"), Some(0));
        assert_eq!(recents.rank("close_tab"), Some(1));
        assert_eq!(recents.rank("split_pane_vertical"), None);
    }

    #[test]
    fn push_caps_stored_history() {
        let mut recents = CommandPaletteRecents::default();
        for index in 0..(MAX_RECENTS + 10) {
            recents.push(format!("command_{index}"));
        }

        assert_eq!(recents.keys.len(), MAX_RECENTS);
        assert_eq!(recents.keys[0], format!("command_{}", MAX_RECENTS + 9));
    }

    #[test]
    fn bonus_decays_with_rank_and_never_goes_negative() {
        let mut recents = CommandPaletteRecents::default();
        for index in 0..30 {
            recents.push(format!("command_{index}"));
        }
        recents.push("new_tab".to_string());

        let mut item = command_item(CommandAction::NewTab);
        assert_eq!(recents.bonus_for_item(&item), RECENT_BONUS_MAX);

        recents.push("close_tab".to_string());
        assert_eq!(
            recents.bonus_for_item(&item),
            RECENT_BONUS_MAX - RECENT_BONUS_STEP
        );

        item = command_item(CommandAction::ZoomReset);
        assert_eq!(recents.bonus_for_item(&item), 0);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join(RECENTS_FILE);

        let mut recents = CommandPaletteRecents::default();
        recents.push("new_tab".to_string());
        recents.push("close_tab".to_string());
        recents.save_to_path(&path).expect("save");

        let loaded = CommandPaletteRecents::load_from_path(&path);
        assert_eq!(loaded, recents);
    }

    #[test]
    fn missing_or_corrupt_file_loads_empty_history() {
        let dir = tempdir().expect("tempdir");
        let missing = dir.path().join(RECENTS_FILE);
        assert_eq!(
            CommandPaletteRecents::load_from_path(&missing),
            CommandPaletteRecents::default()
        );

        let corrupt = dir.path().join("corrupt.json");
        std::fs::write(&corrupt, "{ not json").expect("write");
        assert_eq!(
            CommandPaletteRecents::load_from_path(&corrupt),
            CommandPaletteRecents::default()
        );
    }

    #[test]
    fn plugin_commands_get_namespaced_keys() {
        let item = CommandPaletteItem {
            title: "Do Thing".to_string(),
            keywords: String::new(),
            enabled: true,
            status_hint: None,
            tmux_status_hint: None,
            kind: CommandPaletteItemKind::PluginCommand {
                plugin_id: "acme".to_string(),
                command_id: "do-thing".to_string(),
                icon: termy_plugin_runtime::PluginIcon::Command,
            },
        };

        assert_eq!(
            recent_key_for_item(&item).as_deref(),
            Some("plugin:acme:do-thing")
        );
    }
}
