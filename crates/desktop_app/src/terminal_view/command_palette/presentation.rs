//! Per-row presentation lookups: which icon a palette row paints and which
//! category label it carries.
//!
//! Categories group the flat command catalog so a scan of the Commands list
//! reads as sections ("Panes", "Tabs", "Search") instead of one long wall.
//! They are only rendered where rows come from mixed sources; single-purpose
//! modes (themes, sessions, layouts, tasks) would repeat one label per row.

use super::plugins;
use super::state::{CommandPaletteItem, CommandPaletteItemKind};
use termy_command_core::CommandId;

pub(super) fn command_icon_path(id: CommandId) -> &'static str {
    use termy_command_core::CommandId::*;
    match id {
        NewTab => "icons/command_palette/new-tab.svg",
        CloseTab | ClosePane | ClosePaneOrTab => "icons/command_palette/close-tab.svg",
        MoveTabLeft | SwitchTabLeft => "icons/command_palette/tab-left.svg",
        MoveTabRight | SwitchTabRight | CycleTabs => "icons/command_palette/tab-right.svg",
        // Jumping to a numbered tab is not directional, so it does not borrow
        // the move-right glyph.
        SwitchToTab1 | SwitchToTab2 | SwitchToTab3 | SwitchToTab4 | SwitchToTab5 | SwitchToTab6
        | SwitchToTab7 | SwitchToTab8 | SwitchToTab9 => "icons/settings/tabs.svg",
        RenameTab => "icons/command_palette/rename.svg",
        SplitPaneVertical => "icons/command_palette/split-right.svg",
        SplitPaneHorizontal => "icons/command_palette/split-down.svg",
        FocusPaneLeft | FocusPaneRight | FocusPaneUp | FocusPaneDown | FocusPaneNext
        | FocusPanePrevious => "icons/command_palette/focus-pane.svg",
        ResizePaneLeft | ResizePaneRight | ResizePaneUp | ResizePaneDown => {
            "icons/command_palette/resize-pane.svg"
        }
        TogglePaneZoom => "icons/command_palette/zoom-pane.svg",
        MinimizeWindow => "icons/command_palette/minimize.svg",
        ManageTmuxSessions => "icons/settings/terminal.svg",
        ManageSavedLayouts => "icons/command_palette/layout.svg",
        RunTask => "icons/command_palette/play.svg",
        AppInfo => "icons/command_palette/info.svg",
        RestartApp => "icons/command_palette/restart.svg",
        OpenConfig | PrettifyConfig | OpenSettings => "icons/settings/advanced.svg",
        ImportColors => "icons/settings/colors.svg",
        SwitchTheme => "icons/settings/themes.svg",
        ZoomIn => "icons/command_palette/zoom-in.svg",
        ZoomOut => "icons/command_palette/zoom-out.svg",
        ZoomReset => "icons/command_palette/zoom-reset.svg",
        OpenSearch
        | CloseSearch
        | SearchNext
        | SearchPrevious
        | ToggleSearchCaseSensitive
        | ToggleSearchRegex => "icons/settings/search.svg",
        CheckForUpdates => "icons/command_palette/check-update.svg",
        Quit => "icons/command_palette/power.svg",
        ToggleCommandPalette => "icons/command_palette/command.svg",
        Copy | Paste | SelectAll => "icons/command_palette/clipboard.svg",
        ClearScreen => "icons/settings/reset.svg",
        InstallCli => "icons/command_palette/cli.svg",
        ToggleTabBarVisibility => "icons/settings/tabs.svg",
        ToggleWorkspaceSidebar => "icons/command_palette/sidebar.svg",
        ToggleInspector => "icons/settings/advanced.svg",
    }
}

pub(super) fn palette_item_icon_path(item: &CommandPaletteItem) -> &'static str {
    match &item.kind {
        CommandPaletteItemKind::Command(action) => command_icon_path(action.to_command_id()),
        CommandPaletteItemKind::PluginCommand { icon, .. }
        | CommandPaletteItemKind::PluginInputSubmit { icon }
        | CommandPaletteItemKind::PluginInputOption { icon, .. } => {
            plugins::plugin_icon_path(*icon)
        }
        CommandPaletteItemKind::Theme(_) => "icons/settings/themes.svg",
        CommandPaletteItemKind::SshHost { .. } | CommandPaletteItemKind::ManageSshHosts => {
            "icons/settings/ssh.svg"
        }
        CommandPaletteItemKind::TmuxSessionAttachOrSwitch { .. }
        | CommandPaletteItemKind::TmuxSessionCreateAndAttach { .. }
        | CommandPaletteItemKind::TmuxSessionDetachCurrent
        | CommandPaletteItemKind::TmuxSessionOpenRenameMode
        | CommandPaletteItemKind::TmuxSessionOpenKillMode
        | CommandPaletteItemKind::TmuxSessionRenameSelect { .. }
        | CommandPaletteItemKind::TmuxSessionRenameApply { .. }
        | CommandPaletteItemKind::TmuxSessionKill { .. } => "icons/settings/terminal.svg",
        CommandPaletteItemKind::SavedLayoutOpen { .. }
        | CommandPaletteItemKind::SavedLayoutOpenTasksMode { .. }
        | CommandPaletteItemKind::SavedLayoutOpenSaveMode
        | CommandPaletteItemKind::SavedLayoutSaveAs { .. }
        | CommandPaletteItemKind::SavedLayoutOpenRenameMode
        | CommandPaletteItemKind::SavedLayoutRenameSelect { .. }
        | CommandPaletteItemKind::SavedLayoutRenameApply { .. }
        | CommandPaletteItemKind::SavedLayoutOpenDeleteMode
        | CommandPaletteItemKind::SavedLayoutDelete { .. } => "icons/command_palette/layout.svg",
        CommandPaletteItemKind::TaskOpenCreateGlobalMode
        | CommandPaletteItemKind::TaskOpenCreateLayoutMode { .. }
        | CommandPaletteItemKind::TaskOpenSaveCurrentCommandGlobalMode
        | CommandPaletteItemKind::TaskOpenSaveCurrentCommandLayoutMode { .. }
        | CommandPaletteItemKind::TaskCreate { .. }
        | CommandPaletteItemKind::Task { .. } => "icons/command_palette/play.svg",
        CommandPaletteItemKind::AppInfoEntry { .. } => "icons/command_palette/info.svg",
        CommandPaletteItemKind::AppInfoCopyAll { .. } => "icons/command_palette/clipboard.svg",
    }
}

/// Group label for a built-in command. Exhaustive on purpose: a new command
/// fails to compile until it is filed under a category.
pub(super) fn command_category(id: CommandId) -> &'static str {
    use termy_command_core::CommandId::*;
    match id {
        NewTab
        | CloseTab
        | RenameTab
        | MoveTabLeft
        | MoveTabRight
        | SwitchTabLeft
        | SwitchTabRight
        | CycleTabs
        | SwitchToTab1
        | SwitchToTab2
        | SwitchToTab3
        | SwitchToTab4
        | SwitchToTab5
        | SwitchToTab6
        | SwitchToTab7
        | SwitchToTab8
        | SwitchToTab9
        | ToggleTabBarVisibility => "Tabs",
        SplitPaneVertical | SplitPaneHorizontal | ClosePane | ClosePaneOrTab | FocusPaneLeft
        | FocusPaneRight | FocusPaneUp | FocusPaneDown | FocusPaneNext | FocusPanePrevious
        | ResizePaneLeft | ResizePaneRight | ResizePaneUp | ResizePaneDown | TogglePaneZoom => {
            "Panes"
        }
        MinimizeWindow | ToggleWorkspaceSidebar => "Window",
        ManageTmuxSessions | ManageSavedLayouts | RunTask => "Sessions",
        OpenSearch
        | CloseSearch
        | SearchNext
        | SearchPrevious
        | ToggleSearchCaseSensitive
        | ToggleSearchRegex => "Search",
        Copy | Paste | SelectAll | ClearScreen => "Edit",
        SwitchTheme | ImportColors | ZoomIn | ZoomOut | ZoomReset => "Appearance",
        OpenConfig | PrettifyConfig | OpenSettings | InstallCli => "Settings",
        AppInfo | RestartApp | CheckForUpdates | Quit | ToggleCommandPalette | ToggleInspector => {
            "App"
        }
    }
}

/// Modifier glyphs GPUI writes with no separator (macOS, and the platform key
/// on Linux/Windows).
const MODIFIER_GLYPHS: &[char] = &['^', '⌃', '⌥', '⌘', '⇧', '❖', '⊞'];
/// Modifier words GPUI writes with a trailing dash on non-macOS platforms.
const MODIFIER_WORDS: &[(&str, &str)] = &[
    ("ctrl-", "Ctrl"),
    ("alt-", "Alt"),
    ("shift-", "Shift"),
    ("super-", "Super"),
    ("cmd-", "Cmd"),
    ("win-", "Win"),
    ("fn-", "Fn"),
];

/// Splits a keybinding label into one group of keycaps per keystroke, so the
/// renderer can draw `⇧⌘K` as three caps instead of one wide chip.
///
/// GPUI joins keystrokes with a space and writes modifiers either as bare
/// glyphs (`⇧⌘K`) or as dash-suffixed words (`ctrl-shift-k`), depending on the
/// platform; both forms are handled here.
pub(super) fn shortcut_keycaps(label: &str) -> Vec<Vec<String>> {
    label
        .split_whitespace()
        .map(keystroke_keycaps)
        .filter(|caps| !caps.is_empty())
        .collect()
}

fn keystroke_keycaps(keystroke: &str) -> Vec<String> {
    let mut caps = Vec::new();
    let mut rest = keystroke;

    loop {
        if let Some((prefix, label)) = MODIFIER_WORDS
            .iter()
            .find(|(prefix, _)| rest.len() > prefix.len() && rest.starts_with(*prefix))
        {
            caps.push((*label).to_string());
            rest = &rest[prefix.len()..];
            continue;
        }

        let Some(first) = rest.chars().next() else {
            break;
        };
        // A lone modifier glyph is the whole keystroke (someone bound ⌘ by
        // itself), so it stays as the key rather than being eaten as a prefix.
        if MODIFIER_GLYPHS.contains(&first) && rest.chars().count() > 1 {
            caps.push(first.to_string());
            rest = &rest[first.len_utf8()..];
            continue;
        }

        break;
    }

    if !rest.is_empty() {
        caps.push(rest.to_string());
    }
    caps
}

/// Category label for a row, or `None` for rows whose mode already says what
/// they are.
pub(super) fn palette_item_category(item: &CommandPaletteItem) -> Option<String> {
    match &item.kind {
        CommandPaletteItemKind::Command(action) => {
            Some(command_category(action.to_command_id()).to_string())
        }
        CommandPaletteItemKind::PluginCommand { plugin_id, .. } => Some(plugin_id.clone()),
        CommandPaletteItemKind::SshHost { .. } | CommandPaletteItemKind::ManageSshHosts => {
            Some("SSH".to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_view::CommandAction;

    #[test]
    fn every_command_has_an_embedded_icon_and_a_category() {
        for id in CommandId::all() {
            let icon = command_icon_path(id);
            let embedded = gpui::AssetSource::load(&crate::asset_source::EmbeddedAssets, icon)
                .ok()
                .flatten()
                .is_some();
            assert!(
                embedded,
                "icon {icon} for {} is not embedded",
                id.config_name()
            );
            assert!(
                !command_category(id).is_empty(),
                "missing category for {}",
                id.config_name()
            );
        }
    }

    #[test]
    fn categories_stay_a_small_scannable_set() {
        let mut categories: Vec<&str> = CommandId::all().map(command_category).collect();
        categories.sort_unstable();
        categories.dedup();

        assert_eq!(
            categories,
            vec![
                "App",
                "Appearance",
                "Edit",
                "Panes",
                "Search",
                "Sessions",
                "Settings",
                "Tabs",
                "Window",
            ]
        );
    }

    #[test]
    fn plugin_rows_use_their_plugin_id_as_category() {
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

        assert_eq!(palette_item_category(&item).as_deref(), Some("acme"));
    }

    #[test]
    fn non_command_rows_have_no_category() {
        let theme = CommandPaletteItem::theme("nord".to_string(), false);
        assert_eq!(palette_item_category(&theme), None);

        let task = CommandPaletteItem::task("build", "cargo build", None, None);
        assert_eq!(palette_item_category(&task), None);
    }

    #[test]
    fn keycaps_split_macos_glyph_shortcuts() {
        assert_eq!(shortcut_keycaps("⌘K"), vec![vec!["⌘", "K"]]);
        assert_eq!(shortcut_keycaps("⇧⌘K"), vec![vec!["⇧", "⌘", "K"]]);
        assert_eq!(
            shortcut_keycaps("^⌥⌘⇧K"),
            vec![vec!["^", "⌥", "⌘", "⇧", "K"]]
        );
    }

    #[test]
    fn keycaps_split_dash_separated_shortcuts() {
        assert_eq!(shortcut_keycaps("ctrl-k"), vec![vec!["Ctrl", "k"]]);
        assert_eq!(
            shortcut_keycaps("ctrl-shift-k"),
            vec![vec!["Ctrl", "Shift", "k"]]
        );
    }

    #[test]
    fn keycaps_group_multi_keystroke_bindings() {
        assert_eq!(
            shortcut_keycaps("⌘K ⌘T"),
            vec![vec!["⌘", "K"], vec!["⌘", "T"]]
        );
    }

    #[test]
    fn keycaps_keep_a_lone_modifier_as_the_key() {
        assert_eq!(shortcut_keycaps("⌘"), vec![vec!["⌘"]]);
        assert_eq!(shortcut_keycaps(""), Vec::<Vec<String>>::new());
    }

    #[test]
    fn keycaps_keep_symbol_keys_intact() {
        assert_eq!(shortcut_keycaps("⌘-"), vec![vec!["⌘", "-"]]);
        assert_eq!(shortcut_keycaps("⌘↑"), vec![vec!["⌘", "↑"]]);
        assert_eq!(shortcut_keycaps("ctrl--"), vec![vec!["Ctrl", "-"]]);
    }

    #[test]
    fn split_commands_are_filed_under_panes() {
        let split = CommandPaletteItem::command_with_state(
            "Split Pane Vertical",
            "split",
            CommandAction::SplitPaneVertical,
            true,
            None,
        );

        assert_eq!(palette_item_category(&split).as_deref(), Some("Panes"));
    }
}
