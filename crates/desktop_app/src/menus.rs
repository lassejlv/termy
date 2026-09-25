use crate::commands::{CommandAction, CommandMenuEntry, MenuRoot};
#[cfg(target_os = "macos")]
use gpui::SystemMenuType;
use gpui::{Menu, MenuItem};
use termy_core::command_core::{
    CommandAvailability, CommandCapabilities, CommandUnavailableReason,
};

const INSTALL_CLI_TITLE: &str = "Install CLI";
const INSTALL_CLI_INSTALLED_TITLE: &str = "Install CLI (Installed)";

pub(crate) fn app_menus(
    install_cli_available: bool,
    tmux_enabled: bool,
    simple_mode: bool,
) -> Vec<Menu> {
    let capabilities = CommandCapabilities {
        tmux_runtime_active: tmux_enabled,
        install_cli_available,
    };

    CommandAction::menu_roots()
        .iter()
        .copied()
        .map(|root| build_menu(root, capabilities, simple_mode))
        .collect()
}

fn build_menu(root: MenuRoot, capabilities: CommandCapabilities, simple_mode: bool) -> Menu {
    let entries =
        CommandAction::menu_entries_for_root_for_runtime(root, capabilities.tmux_runtime_active);
    let mut items = Vec::new();

    #[cfg(target_os = "macos")]
    if root == MenuRoot::App {
        items.push(MenuItem::os_submenu("Services", SystemMenuType::Services));
        if !entries.is_empty() {
            items.push(MenuItem::separator());
        }
    }

    append_menu_entries(&mut items, &entries, capabilities, simple_mode);

    Menu {
        name: root.title().into(),
        items,
    }
}

fn append_menu_entries(
    items: &mut Vec<MenuItem>,
    entries: &[CommandMenuEntry],
    capabilities: CommandCapabilities,
    simple_mode: bool,
) {
    let mut previous_section = None;

    for entry in entries {
        if simple_mode && entry.action == CommandAction::ToggleCommandPalette {
            continue;
        }

        let availability = entry.action.availability(capabilities);
        let Some(title) = menu_item_title(entry, availability) else {
            continue;
        };

        if let Some(section) = previous_section
            && section != entry.section
        {
            items.push(MenuItem::separator());
        }

        items.push(entry.action.to_menu_item(title, entry.role));
        previous_section = Some(entry.section);
    }
}

fn menu_item_title(
    entry: &CommandMenuEntry,
    availability: CommandAvailability,
) -> Option<&'static str> {
    if availability.enabled {
        if entry.action == CommandAction::InstallCli {
            return Some(INSTALL_CLI_TITLE);
        }
        return Some(entry.title);
    }

    match availability.reason {
        Some(CommandUnavailableReason::RequiresTmuxRuntime) => None,
        Some(CommandUnavailableReason::InstallCliAlreadyInstalled) => {
            Some(INSTALL_CLI_INSTALLED_TITLE)
        }
        None => unreachable!("disabled command must include an unavailable reason"),
    }
}

#[cfg(test)]
mod tests {
    use super::{INSTALL_CLI_INSTALLED_TITLE, INSTALL_CLI_TITLE, app_menus};
    use crate::commands::CommandAction;
    use gpui::{MenuItem, OsAction};
    use termy_core::command_core::{CommandCapabilities, CommandUnavailableReason};

    #[test]
    fn top_level_menu_order_is_stable() {
        let names = app_menus(true, true, false)
            .into_iter()
            .map(|menu| menu.name.to_string())
            .collect::<Vec<_>>();

        assert_eq!(names, ["Termy", "File", "Edit", "View", "Window", "Help"]);
    }

    #[test]
    fn app_menu_includes_services_only_on_macos() {
        let app_menu = app_menus(true, true, false)
            .into_iter()
            .find(|menu| menu.name.as_ref() == "Termy")
            .expect("missing Termy menu");

        let has_services = app_menu
            .items
            .iter()
            .any(|item| matches!(item, MenuItem::SystemMenu(_)));

        #[cfg(target_os = "macos")]
        assert!(has_services);

        #[cfg(not(target_os = "macos"))]
        assert!(!has_services);
    }

    #[test]
    fn edit_menu_copy_and_paste_use_os_actions() {
        let edit_menu = app_menus(true, true, false)
            .into_iter()
            .find(|menu| menu.name.as_ref() == "Edit")
            .expect("missing Edit menu");

        let mut copy_os_action = None;
        let mut paste_os_action = None;

        for item in &edit_menu.items {
            if let MenuItem::Action {
                name, os_action, ..
            } = item
            {
                match name.as_ref() {
                    "Copy" => copy_os_action = *os_action,
                    "Paste" => paste_os_action = *os_action,
                    _ => {}
                }
            }
        }

        assert!(matches!(copy_os_action, Some(OsAction::Copy)));
        assert!(matches!(paste_os_action, Some(OsAction::Paste)));
    }

    #[test]
    fn separators_are_inserted_only_between_sections() {
        for menu in app_menus(true, true, false) {
            if menu.items.is_empty() {
                continue;
            }

            assert!(!matches!(menu.items.first(), Some(MenuItem::Separator)));
            assert!(!matches!(menu.items.last(), Some(MenuItem::Separator)));

            let mut previous_was_separator = false;
            for item in &menu.items {
                if matches!(item, MenuItem::Separator) {
                    assert!(!previous_was_separator);
                    previous_was_separator = true;
                } else {
                    previous_was_separator = false;
                }
            }
        }
    }

    #[test]
    fn install_cli_menu_title_reflects_install_state() {
        let help_menu_available = app_menus(true, true, false)
            .into_iter()
            .find(|menu| menu.name.as_ref() == "Help")
            .expect("missing Help menu");
        let help_menu_installed = app_menus(false, true, false)
            .into_iter()
            .find(|menu| menu.name.as_ref() == "Help")
            .expect("missing Help menu");

        let install_cli_titles = |menu: &gpui::Menu| {
            menu.items
                .iter()
                .filter_map(|item| {
                    let MenuItem::Action { name, .. } = item else {
                        return None;
                    };
                    let title = name.as_ref();
                    if title == INSTALL_CLI_TITLE || title == INSTALL_CLI_INSTALLED_TITLE {
                        Some(title.to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(
            install_cli_titles(&help_menu_available),
            [INSTALL_CLI_TITLE]
        );
        assert_eq!(
            install_cli_titles(&help_menu_installed),
            [INSTALL_CLI_INSTALLED_TITLE]
        );

        let availability = CommandAction::InstallCli.availability(CommandCapabilities {
            tmux_runtime_active: true,
            install_cli_available: false,
        });
        assert_eq!(
            availability.reason,
            Some(CommandUnavailableReason::InstallCliAlreadyInstalled)
        );
    }

    #[test]
    fn simple_mode_hides_command_palette_menu_item_but_keeps_settings_entries() {
        let all_menu_titles = app_menus(true, true, true)
            .into_iter()
            .flat_map(|menu| menu.items)
            .filter_map(|item| {
                let MenuItem::Action { name, .. } = item else {
                    return None;
                };
                Some(name.as_ref().to_string())
            })
            .collect::<Vec<_>>();

        assert!(
            !all_menu_titles
                .iter()
                .any(|title| title == "Command Palette")
        );
        assert!(all_menu_titles.iter().any(|title| title == "Settings..."));
        assert!(
            all_menu_titles
                .iter()
                .any(|title| title == "Open Config File...")
        );
    }

    #[test]
    fn file_menu_keeps_native_pane_actions_when_tmux_is_disabled() {
        let caps = CommandCapabilities {
            tmux_runtime_active: false,
            install_cli_available: true,
        };
        for action in [
            CommandAction::SplitPaneVertical,
            CommandAction::SplitPaneHorizontal,
            CommandAction::FocusPaneNext,
        ] {
            let availability = action.availability(caps);
            assert!(availability.enabled);
            assert_eq!(availability.reason, None);
        }
        let close_pane_or_tab_availability = CommandAction::ClosePaneOrTab.availability(caps);
        assert!(close_pane_or_tab_availability.enabled);
        assert_eq!(close_pane_or_tab_availability.reason, None);

        let file_menu = app_menus(true, false, false)
            .into_iter()
            .find(|menu| menu.name.as_ref() == "File")
            .expect("missing File menu");

        let labels = file_menu
            .items
            .iter()
            .map(|item| match item {
                MenuItem::Action { name, .. } => name.as_ref().to_string(),
                MenuItem::Separator => "<separator>".to_string(),
                MenuItem::Submenu(_) | MenuItem::SystemMenu(_) => "<non-action>".to_string(),
            })
            .collect::<Vec<_>>();

        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            labels,
            vec![
                "New Tab",
                "Rename Tab",
                "<separator>",
                "Close Pane or Tab",
                "Tmux Sessions",
                "Split Pane Vertical",
                "Split Pane Horizontal",
                "Focus Next Pane",
            ]
            .into_iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
        );
        #[cfg(target_os = "windows")]
        assert_eq!(
            labels,
            vec!["New Tab", "Rename Tab", "<separator>", "Close Pane or Tab"]
                .into_iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn pane_actions_remain_visible_when_tmux_is_disabled() {
        let all_menu_titles = app_menus(true, false, false)
            .into_iter()
            .flat_map(|menu| menu.items)
            .filter_map(|item| {
                let MenuItem::Action { name, .. } = item else {
                    return None;
                };
                Some(name.as_ref().to_string())
            })
            .collect::<Vec<_>>();

        assert!(
            all_menu_titles
                .iter()
                .any(|title| title.as_str() == "Toggle Pane Zoom")
        );
    }
}
