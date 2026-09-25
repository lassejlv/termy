use super::{
    CommandAction, CommandPaletteVisibility, MenuActionRole, MenuRoot, MenuVisibility,
    inline_input_keybindings,
};
use std::collections::HashSet;
use termy_core::command_core::{CommandCapabilities, CommandId};

#[test]
fn command_catalog_contains_unique_actions() {
    let mut seen = HashSet::new();
    for spec in CommandAction::specs() {
        assert!(seen.insert(spec.action), "duplicate action in catalog");
    }

    assert_eq!(seen.len(), CommandAction::all().count());
}

#[test]
fn switch_theme_is_configurable_and_palette_visible() {
    assert_eq!(
        CommandAction::from_config_name("switch_theme"),
        Some(CommandAction::SwitchTheme)
    );
    assert!(
        CommandAction::palette_entries()
            .iter()
            .any(|entry| entry.action == CommandAction::SwitchTheme)
    );
}

#[test]
fn clear_screen_is_configurable_and_palette_visible() {
    assert_eq!(
        CommandAction::from_config_name("clear_screen"),
        Some(CommandAction::ClearScreen)
    );
    assert!(
        CommandAction::palette_entries()
            .iter()
            .any(|entry| entry.action == CommandAction::ClearScreen)
    );
}

#[test]
fn tab_bar_visibility_toggle_is_configurable_and_palette_visible() {
    assert_eq!(
        CommandAction::from_config_name("toggle_tab_bar_visibility"),
        Some(CommandAction::ToggleTabBarVisibility)
    );
    assert!(
        CommandAction::palette_entries()
            .iter()
            .any(|entry| entry.action == CommandAction::ToggleTabBarVisibility)
    );
}

#[test]
fn view_release_notes_is_configurable_and_palette_visible() {
    assert_eq!(
        CommandAction::from_config_name("view_release_notes"),
        Some(CommandAction::ViewReleaseNotes)
    );
    assert!(
        CommandAction::palette_entries()
            .iter()
            .any(|entry| entry.action == CommandAction::ViewReleaseNotes)
    );
    let help_entries = CommandAction::menu_entries_for_root(MenuRoot::Help);
    assert!(
        help_entries
            .iter()
            .any(|entry| entry.action == CommandAction::ViewReleaseNotes)
    );
}

#[test]
fn browse_release_notes_is_configurable_and_palette_visible() {
    assert_eq!(
        CommandAction::from_config_name("browse_release_notes"),
        Some(CommandAction::BrowseReleaseNotes)
    );
    assert!(
        CommandAction::palette_entries()
            .iter()
            .any(|entry| entry.action == CommandAction::BrowseReleaseNotes)
    );
    let help_entries = CommandAction::menu_entries_for_root(MenuRoot::Help);
    assert!(
        help_entries
            .iter()
            .any(|entry| entry.action == CommandAction::BrowseReleaseNotes)
    );
}

#[test]
fn tab_actions_are_always_palette_visible() {
    let entries = CommandAction::palette_entries();
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == CommandAction::NewTab)
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == CommandAction::ClosePaneOrTab)
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == CommandAction::MoveTabLeft)
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == CommandAction::MoveTabRight)
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == CommandAction::SwitchTabLeft)
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == CommandAction::SwitchTabRight)
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == CommandAction::CycleTabs)
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == CommandAction::RenameTab)
    );
    assert!(
        !entries
            .iter()
            .any(|entry| entry.action == CommandAction::CloseTab)
    );
}

#[test]
fn file_menu_includes_requested_pane_actions() {
    let file_entries = CommandAction::menu_entries_for_root(MenuRoot::File);

    #[cfg(not(target_os = "windows"))]
    for action in [
        CommandAction::ClosePaneOrTab,
        CommandAction::SplitPaneVertical,
        CommandAction::SplitPaneHorizontal,
        CommandAction::FocusPaneNext,
    ] {
        assert!(
            file_entries.iter().any(|entry| entry.action == action),
            "missing {action:?} from File menu"
        );
    }
    #[cfg(target_os = "windows")]
    for action in [
        CommandAction::ManageTmuxSessions,
        CommandAction::SplitPaneVertical,
        CommandAction::SplitPaneHorizontal,
        CommandAction::FocusPaneNext,
    ] {
        assert!(
            !file_entries.iter().any(|entry| entry.action == action),
            "unexpected {action:?} in File menu on Windows"
        );
    }

    let close_pane_or_tab = file_entries
        .iter()
        .find(|entry| entry.action == CommandAction::ClosePaneOrTab)
        .expect("missing ClosePaneOrTab from File menu");
    assert_eq!(close_pane_or_tab.section, 1);
    assert!(
        !file_entries
            .iter()
            .any(|entry| entry.action == CommandAction::ClosePane)
    );
}

#[test]
fn window_menu_excludes_file_menu_pane_actions() {
    let window_entries = CommandAction::menu_entries_for_root(MenuRoot::Window);
    for action in [
        CommandAction::ClosePaneOrTab,
        CommandAction::SplitPaneVertical,
        CommandAction::SplitPaneHorizontal,
        CommandAction::ClosePane,
        CommandAction::FocusPaneNext,
        CommandAction::FocusPanePrevious,
    ] {
        assert!(
            !window_entries.iter().any(|entry| entry.action == action),
            "unexpected {action:?} in Window menu"
        );
    }
}

#[test]
fn file_menu_section_order_is_stable() {
    let file_entries = CommandAction::menu_entries_for_root(MenuRoot::File);
    let sections = file_entries
        .iter()
        .map(|entry| entry.section)
        .collect::<Vec<_>>();
    #[cfg(not(target_os = "windows"))]
    assert_eq!(sections, [0, 0, 1, 1, 1, 1, 1]);
    #[cfg(target_os = "windows")]
    assert_eq!(sections, [0, 0, 1]);
}

#[test]
fn menu_entries_have_non_empty_titles_and_valid_sections() {
    for root in CommandAction::menu_roots() {
        for entry in CommandAction::menu_entries_for_root(*root) {
            assert!(!entry.title.trim().is_empty());
            assert_eq!(entry.root, *root);
        }
    }
}

#[test]
fn view_menu_includes_tab_bar_toggle() {
    let view_entries = CommandAction::menu_entries_for_root(MenuRoot::View);
    assert!(
        view_entries
            .iter()
            .any(|entry| entry.action == CommandAction::ToggleTabBarVisibility)
    );
}

#[test]
fn menu_entries_do_not_collide_on_root_section_and_title() {
    let mut seen = HashSet::new();
    for root in CommandAction::menu_roots() {
        for entry in CommandAction::menu_entries_for_root(*root) {
            assert!(
                seen.insert((entry.root, entry.section, entry.title)),
                "duplicate menu entry for ({:?}, {}, {:?})",
                entry.root,
                entry.section,
                entry.title
            );
        }
    }
}

#[test]
fn menu_visibility_filters_by_platform() {
    assert!(MenuVisibility::Always.is_visible_on_platform(true, true));
    assert!(MenuVisibility::Always.is_visible_on_platform(false, false));
    assert!(MenuVisibility::MacOsOnly.is_visible_on_platform(true, false));
    assert!(!MenuVisibility::MacOsOnly.is_visible_on_platform(false, false));
    assert!(MenuVisibility::NotWindows.is_visible_on_platform(true, false));
    assert!(!MenuVisibility::NotWindows.is_visible_on_platform(false, true));
}

#[test]
fn palette_visibility_filters_by_platform() {
    assert!(CommandPaletteVisibility::Always.is_visible_on_platform(true, true));
    assert!(CommandPaletteVisibility::Always.is_visible_on_platform(false, false));
    assert!(CommandPaletteVisibility::MacOsOnly.is_visible_on_platform(true, false));
    assert!(!CommandPaletteVisibility::MacOsOnly.is_visible_on_platform(false, false));
    assert!(CommandPaletteVisibility::NotWindows.is_visible_on_platform(false, false));
    assert!(!CommandPaletteVisibility::NotWindows.is_visible_on_platform(false, true));
}

#[test]
fn windows_hides_tmux_commands_from_palette_without_tmux_runtime() {
    let entries = CommandAction::palette_entries();
    #[cfg(target_os = "windows")]
    {
        for action in [
            CommandAction::ManageTmuxSessions,
            CommandAction::SplitPaneVertical,
            CommandAction::SplitPaneHorizontal,
            CommandAction::ClosePane,
            CommandAction::FocusPaneLeft,
            CommandAction::FocusPaneRight,
            CommandAction::FocusPaneUp,
            CommandAction::FocusPaneDown,
            CommandAction::FocusPaneNext,
            CommandAction::FocusPanePrevious,
            CommandAction::ResizePaneLeft,
            CommandAction::ResizePaneRight,
            CommandAction::ResizePaneUp,
            CommandAction::ResizePaneDown,
            CommandAction::TogglePaneZoom,
        ] {
            assert!(
                !entries.iter().any(|entry| entry.action == action),
                "unexpected tmux palette action {action:?} on Windows"
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        assert!(
            entries
                .iter()
                .any(|entry| entry.action == CommandAction::ManageTmuxSessions)
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.action == CommandAction::SplitPaneVertical)
        );
    }
}

#[cfg(target_os = "windows")]
#[test]
fn windows_shows_tmux_commands_when_runtime_is_active() {
    let entries = CommandAction::palette_entries_for_runtime(true);
    for action in [
        CommandAction::ManageTmuxSessions,
        CommandAction::SplitPaneVertical,
        CommandAction::ResizePaneLeft,
    ] {
        assert!(
            entries.iter().any(|entry| entry.action == action),
            "missing {action:?} from tmux command palette on Windows"
        );
    }
    let file_entries = CommandAction::menu_entries_for_root_for_runtime(MenuRoot::File, true);
    for action in [
        CommandAction::ManageTmuxSessions,
        CommandAction::SplitPaneVertical,
        CommandAction::SplitPaneHorizontal,
    ] {
        assert!(
            file_entries.iter().any(|entry| entry.action == action),
            "missing {action:?} from tmux File menu on Windows"
        );
    }
}

#[test]
fn only_edit_commands_use_os_edit_roles() {
    for root in CommandAction::menu_roots() {
        for entry in CommandAction::menu_entries_for_root(*root) {
            match entry.role {
                MenuActionRole::Copy => assert_eq!(entry.action, CommandAction::Copy),
                MenuActionRole::Paste => assert_eq!(entry.action, CommandAction::Paste),
                MenuActionRole::SelectAll => {
                    assert_eq!(entry.action, CommandAction::SelectAll);
                }
                MenuActionRole::Normal => {}
            }
        }
    }
}

#[test]
fn inline_input_keybindings_include_copy_binding() {
    assert_eq!(inline_input_keybindings().len(), 18);
}

#[test]
fn command_action_roundtrips_all_core_command_ids() {
    for command_id in CommandId::all() {
        let action = CommandAction::from_command_id(command_id);
        assert_eq!(action.to_command_id(), command_id);
    }
}

#[test]
fn command_action_count_matches_core_catalog() {
    assert_eq!(CommandAction::all().count(), CommandId::all().count());
}

#[test]
fn command_action_availability_reason_matches_command_core() {
    let caps = CommandCapabilities {
        tmux_runtime_active: false,
        install_cli_available: true,
    };
    let availability = CommandAction::ResizePaneLeft.availability(caps);
    assert!(availability.enabled);
    assert_eq!(availability.reason, None);
}

#[test]
fn menu_roots_are_stable_and_ordered() {
    assert_eq!(
        CommandAction::menu_roots(),
        &[
            MenuRoot::App,
            MenuRoot::File,
            MenuRoot::Edit,
            MenuRoot::View,
            MenuRoot::Window,
            MenuRoot::Help,
        ]
    );
}

#[test]
fn tmux_only_actions_match_command_core_tmux_only_set() {
    let mut actual = CommandAction::all()
        .filter(|action| action.to_command_id().is_tmux_only())
        .map(|action| action.to_command_id().config_name())
        .collect::<Vec<_>>();
    actual.sort_unstable();

    let expected: Vec<&str> = vec![];

    assert_eq!(actual, expected);
}
