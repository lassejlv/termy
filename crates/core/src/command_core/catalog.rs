#[macro_export]
macro_rules! termy_command_catalog {
    ($visitor:ident) => {
        $visitor! {
            (NewTab, "new_tab"),
            (CloseTab, "close_tab"),
            (ClosePaneOrTab, "close_pane_or_tab"),
            (MoveTabLeft, "move_tab_left"),
            (MoveTabRight, "move_tab_right"),
            (SwitchTabLeft, "switch_tab_left"),
            (SwitchTabRight, "switch_tab_right"),
            (CycleTabs, "cycle_tabs"),
            (SwitchToTab1, "switch_to_tab_1"),
            (SwitchToTab2, "switch_to_tab_2"),
            (SwitchToTab3, "switch_to_tab_3"),
            (SwitchToTab4, "switch_to_tab_4"),
            (SwitchToTab5, "switch_to_tab_5"),
            (SwitchToTab6, "switch_to_tab_6"),
            (SwitchToTab7, "switch_to_tab_7"),
            (SwitchToTab8, "switch_to_tab_8"),
            (SwitchToTab9, "switch_to_tab_9"),
            (ManageTmuxSessions, "manage_tmux_sessions"),
            (ManageSavedLayouts, "manage_saved_layouts"),
            (RunTask, "run_task"),
            (SplitPaneVertical, "split_pane_vertical"),
            (SplitPaneHorizontal, "split_pane_horizontal"),
            (ClosePane, "close_pane"),
            (FocusPaneLeft, "focus_pane_left"),
            (FocusPaneRight, "focus_pane_right"),
            (FocusPaneUp, "focus_pane_up"),
            (FocusPaneDown, "focus_pane_down"),
            (FocusPaneNext, "focus_pane_next"),
            (FocusPanePrevious, "focus_pane_previous"),
            (ResizePaneLeft, "resize_pane_left"),
            (ResizePaneRight, "resize_pane_right"),
            (ResizePaneUp, "resize_pane_up"),
            (ResizePaneDown, "resize_pane_down"),
            (TogglePaneZoom, "toggle_pane_zoom"),
            (MinimizeWindow, "minimize_window"),
            (RenameTab, "rename_tab"),
            (AppInfo, "app_info"),
            (RestartApp, "restart_app"),
            (OpenConfig, "open_config"),
            (PrettifyConfig, "prettify_config"),
            (OpenSettings, "open_settings"),
            (ImportColors, "import_colors"),
            (SwitchTheme, "switch_theme"),
            (ZoomIn, "zoom_in"),
            (ZoomOut, "zoom_out"),
            (ZoomReset, "zoom_reset"),
            (OpenSearch, "open_search"),
            (CheckForUpdates, "check_for_updates"),
            (Quit, "quit"),
            (ToggleCommandPalette, "toggle_command_palette"),
            (Copy, "copy"),
            (Paste, "paste"),
            (SelectAll, "select_all"),
            (ClearScreen, "clear_screen"),
            (CloseSearch, "close_search"),
            (SearchNext, "search_next"),
            (SearchPrevious, "search_previous"),
            (ToggleSearchCaseSensitive, "toggle_search_case_sensitive"),
            (ToggleSearchRegex, "toggle_search_regex"),
            (InstallCli, "install_cli"),
            (ToggleTabBarVisibility, "toggle_tab_bar_visibility"),
            (ToggleWorkspaceSidebar, "toggle_workspace_sidebar"),
            (ToggleInspector, "toggle_inspector"),
        }
    };
}

macro_rules! define_command_catalog {
    ($(($variant:ident, $config_name:literal),)+) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum CommandId {
            $($variant,)+
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct CommandSpec {
            pub id: CommandId,
            pub config_name: &'static str,
        }

        const COMMAND_IDS: &[CommandId] = &[
            $(CommandId::$variant,)+
        ];

        const COMMAND_CONFIG_NAMES: &[&str] = &[
            $($config_name,)+
        ];

        const COMMAND_SPECS: &[CommandSpec] = &[
            $(CommandSpec {
                id: CommandId::$variant,
                config_name: $config_name,
            },)+
        ];

        pub fn command_specs() -> &'static [CommandSpec] {
            COMMAND_SPECS
        }

        impl CommandId {
            pub fn from_config_name(name: &str) -> Option<Self> {
                let normalized = normalize_config_name(name);
                match normalized.as_str() {
                    $($config_name => Some(Self::$variant),)+
                    _ => None,
                }
            }

            pub fn config_name(self) -> &'static str {
                match self {
                    $(Self::$variant => $config_name,)+
                }
            }

            pub const fn is_tmux_only(self) -> bool {
                let _ = self;
                false
            }

            pub fn all() -> impl std::iter::ExactSizeIterator<Item = Self> + Clone {
                COMMAND_IDS.iter().copied()
            }

            pub fn all_config_names() -> impl std::iter::ExactSizeIterator<Item = &'static str> {
                COMMAND_CONFIG_NAMES.iter().copied()
            }
        }
    };
}

termy_command_catalog!(define_command_catalog);

fn normalize_config_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::{CommandId, command_specs};

    #[test]
    fn command_catalog_has_unique_config_names() {
        let mut names = command_specs()
            .iter()
            .map(|spec| spec.config_name)
            .collect::<Vec<_>>();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), command_specs().len());
    }

    #[test]
    fn from_config_name_accepts_dash_and_underscore() {
        assert_eq!(
            CommandId::from_config_name("toggle-command-palette"),
            Some(CommandId::ToggleCommandPalette)
        );
        assert_eq!(
            CommandId::from_config_name("toggle_command_palette"),
            Some(CommandId::ToggleCommandPalette)
        );
    }

    #[test]
    fn tmux_only_command_set_is_stable() {
        let mut actual = CommandId::all()
            .filter(|id| id.is_tmux_only())
            .map(CommandId::config_name)
            .collect::<Vec<_>>();
        actual.sort_unstable();

        let expected: Vec<&str> = vec![];

        assert_eq!(actual, expected);
    }
}
