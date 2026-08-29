use gpui::{FocusHandle, KeyBinding, MenuItem, OsAction, Window, actions};
use termy_command_core::{CommandAvailability, CommandCapabilities, CommandId};

const GLOBAL_CONTEXT: Option<&str> = None;
const TERMINAL_CONTEXT: Option<&str> = Some("Terminal");
const INLINE_INPUT_CONTEXT: Option<&str> = Some("InlineInput");

#[derive(Clone, Debug, PartialEq, Eq, gpui::Action)]
#[action(namespace = termy, no_json)]
pub struct RunNamedTask {
    pub task_name: String,
}

impl RunNamedTask {
    pub fn into_key_binding(self, trigger: &str) -> KeyBinding {
        KeyBinding::new(trigger, self, TERMINAL_CONTEXT)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, gpui::Action)]
#[action(namespace = termy, no_json)]
pub struct RunPluginCommand {
    pub plugin_id: String,
    pub command_id: String,
}

impl RunPluginCommand {
    pub fn into_key_binding(self, trigger: &str) -> KeyBinding {
        KeyBinding::new(trigger, self, TERMINAL_CONTEXT)
    }

    pub fn keybinding_label(&self, window: &Window, focus_handle: &FocusHandle) -> Option<String> {
        window
            .bindings_for_action_in(self, focus_handle)
            .into_iter()
            .next()
            .map(|binding| {
                binding
                    .keystrokes()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
    }
}

pub type MenuSection = u8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPaletteVisibility {
    Always,
    MacOsOnly,
    NotWindows,
}

impl CommandPaletteVisibility {
    pub fn is_visible(self) -> bool {
        self.is_visible_on_platform(cfg!(target_os = "macos"), cfg!(target_os = "windows"))
    }

    pub const fn is_visible_on_platform(self, is_macos: bool, is_windows: bool) -> bool {
        match self {
            Self::Always => true,
            Self::MacOsOnly => is_macos,
            Self::NotWindows => !is_windows,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuRoot {
    App,
    File,
    Edit,
    View,
    Window,
    Help,
}

impl MenuRoot {
    pub const fn title(self) -> &'static str {
        match self {
            Self::App => "Termy",
            Self::File => "File",
            Self::Edit => "Edit",
            Self::View => "View",
            Self::Window => "Window",
            Self::Help => "Help",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuVisibility {
    Always,
    MacOsOnly,
    NotWindows,
}

impl MenuVisibility {
    pub fn is_visible(self) -> bool {
        self.is_visible_on_platform(cfg!(target_os = "macos"), cfg!(target_os = "windows"))
    }

    pub const fn is_visible_on_platform(self, is_macos: bool, is_windows: bool) -> bool {
        match self {
            Self::Always => true,
            Self::MacOsOnly => is_macos,
            Self::NotWindows => !is_windows,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuActionRole {
    Normal,
    Copy,
    Paste,
    SelectAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandPaletteSpec {
    pub title: &'static str,
    pub keywords: &'static str,
    pub visibility: CommandPaletteVisibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandPaletteEntry {
    pub action: CommandAction,
    pub title: &'static str,
    pub keywords: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandMenuSpec {
    pub root: MenuRoot,
    pub section: MenuSection,
    pub title: &'static str,
    pub visibility: MenuVisibility,
    pub role: MenuActionRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandMenuEntry {
    pub action: CommandAction,
    pub root: MenuRoot,
    pub section: MenuSection,
    pub title: &'static str,
    pub role: MenuActionRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    pub action: CommandAction,
    pub context: Option<&'static str>,
    pub palette: Option<CommandPaletteSpec>,
    pub menu: Option<CommandMenuSpec>,
}

const MENU_ROOTS: [MenuRoot; 6] = [
    MenuRoot::App,
    MenuRoot::File,
    MenuRoot::Edit,
    MenuRoot::View,
    MenuRoot::Window,
    MenuRoot::Help,
];

const fn palette(
    title: &'static str,
    keywords: &'static str,
    visibility: CommandPaletteVisibility,
) -> CommandPaletteSpec {
    CommandPaletteSpec {
        title,
        keywords,
        visibility,
    }
}

const fn menu(
    root: MenuRoot,
    section: MenuSection,
    title: &'static str,
    visibility: MenuVisibility,
    role: MenuActionRole,
) -> CommandMenuSpec {
    CommandMenuSpec {
        root,
        section,
        title,
        visibility,
        role,
    }
}

const fn command(
    action: CommandAction,
    context: Option<&'static str>,
    palette: Option<CommandPaletteSpec>,
    menu: Option<CommandMenuSpec>,
) -> CommandSpec {
    CommandSpec {
        action,
        context,
        palette,
        menu,
    }
}

macro_rules! define_commands {
    ($(($variant:ident, $context:expr, $palette:expr, $menu:expr)),+ $(,)?) => {
        actions!(
            termy,
            [$( $variant, )+]
        );

        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum CommandAction {
            $( $variant, )+
        }

        const COMMAND_SPECS: &[CommandSpec] = &[
            $(command(CommandAction::$variant, $context, $palette, $menu),)+
        ];

        impl CommandAction {
            #[cfg(test)]
            pub fn specs() -> &'static [CommandSpec] {
                COMMAND_SPECS
            }

            #[cfg(test)]
            pub fn all() -> impl std::iter::ExactSizeIterator<Item = Self> + Clone {
                COMMAND_SPECS.iter().map(|spec| spec.action)
            }

            #[allow(dead_code)]
            pub fn from_config_name(name: &str) -> Option<Self> {
                CommandId::from_config_name(name).map(Self::from_command_id)
            }

            #[allow(dead_code)]
            pub fn all_config_names() -> impl std::iter::ExactSizeIterator<Item = &'static str> {
                CommandId::all_config_names()
            }

            pub fn palette_entries() -> Vec<CommandPaletteEntry> {
                COMMAND_SPECS
                    .iter()
                    .filter_map(|spec| {
                        let palette = spec.palette?;
                        if !palette.visibility.is_visible() {
                            return None;
                        }

                        Some(CommandPaletteEntry {
                            action: spec.action,
                            title: palette.title,
                            keywords: palette.keywords,
                        })
                    })
                    .collect()
            }

            pub fn menu_roots() -> &'static [MenuRoot] {
                &MENU_ROOTS
            }

            pub fn menu_entries_for_root(root: MenuRoot) -> Vec<CommandMenuEntry> {
                let mut entries = COMMAND_SPECS
                    .iter()
                    .filter_map(|spec| {
                        let menu = spec.menu?;
                        if menu.root != root || !menu.visibility.is_visible() {
                            return None;
                        }

                        Some(CommandMenuEntry {
                            action: spec.action,
                            root: menu.root,
                            section: menu.section,
                            title: menu.title,
                            role: menu.role,
                        })
                    })
                    .collect::<Vec<_>>();

                // App info is intentionally surfaced from both the app menu and Help.
                if root == MenuRoot::Help {
                    entries.push(CommandMenuEntry {
                        action: CommandAction::AppInfo,
                        root: MenuRoot::Help,
                        section: 0,
                        title: "App Info",
                        role: MenuActionRole::Normal,
                    });
                }

                // Keep menu section ordering deterministic even when command specs are grouped
                // by action families instead of menu layout.
                entries.sort_by_key(|entry| entry.section);

                entries
            }

            pub fn availability(self, caps: CommandCapabilities) -> CommandAvailability {
                self.to_command_id().availability(caps)
            }

            pub fn to_menu_item(self, title: &'static str, role: MenuActionRole) -> MenuItem {
                let os_action = match role {
                    MenuActionRole::Normal => None,
                    MenuActionRole::Copy => Some(OsAction::Copy),
                    MenuActionRole::Paste => Some(OsAction::Paste),
                    MenuActionRole::SelectAll => Some(OsAction::SelectAll),
                };

                match self {
                    $(
                        Self::$variant => match os_action {
                            Some(os_action) => MenuItem::os_action(title, $variant, os_action),
                            None => MenuItem::action(title, $variant),
                        },
                    )+
                }
            }

            pub fn to_key_binding(self, trigger: &str) -> KeyBinding {
                match self {
                    $(Self::$variant => KeyBinding::new(trigger, $variant, $context),)+
                }
            }

            pub fn keybinding_label(
                self,
                window: &Window,
                focus_handle: &FocusHandle,
            ) -> Option<String> {
                let binding = match self {
                    $(Self::$variant => window.bindings_for_action_in(&$variant, focus_handle).into_iter().next(),)+
                };

                binding.map(|binding| {
                    binding
                        .keystrokes()
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
            }
        }
    };
}

macro_rules! impl_command_action_id_mapping {
    ($(($variant:ident, $_config_name:literal),)+) => {
        impl CommandAction {
            pub fn from_command_id(id: CommandId) -> Self {
                match id {
                    $(CommandId::$variant => Self::$variant,)+
                }
            }

            #[allow(dead_code)]
            pub fn to_command_id(self) -> CommandId {
                match self {
                    $(Self::$variant => CommandId::$variant,)+
                }
            }
        }
    };
}

define_commands!(
    (
        NewTab,
        TERMINAL_CONTEXT,
        Some(palette(
            "New Tab",
            "create tab",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::File,
            0,
            "New Tab",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (CloseTab, TERMINAL_CONTEXT, None, None),
    (
        ClosePaneOrTab,
        TERMINAL_CONTEXT,
        Some(palette(
            "Close Pane or Tab",
            "close pane tab remove",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::File,
            1,
            "Close Pane or Tab",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        MoveTabLeft,
        TERMINAL_CONTEXT,
        Some(palette(
            "Move Tab Left",
            "reorder tab left",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::Window,
            1,
            "Move Tab Left",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        MoveTabRight,
        TERMINAL_CONTEXT,
        Some(palette(
            "Move Tab Right",
            "reorder tab right",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::Window,
            1,
            "Move Tab Right",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        SwitchTabLeft,
        TERMINAL_CONTEXT,
        Some(palette(
            "Switch Tab Left",
            "change active tab left",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::Window,
            1,
            "Switch Tab Left",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        SwitchTabRight,
        TERMINAL_CONTEXT,
        Some(palette(
            "Switch Tab Right",
            "change active tab right",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::Window,
            1,
            "Switch Tab Right",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        CycleTabs,
        TERMINAL_CONTEXT,
        Some(palette(
            "Cycle Tabs",
            "cycle next tab wrap",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::Window,
            1,
            "Cycle Tabs",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (SwitchToTab1, TERMINAL_CONTEXT, None, None),
    (SwitchToTab2, TERMINAL_CONTEXT, None, None),
    (SwitchToTab3, TERMINAL_CONTEXT, None, None),
    (SwitchToTab4, TERMINAL_CONTEXT, None, None),
    (SwitchToTab5, TERMINAL_CONTEXT, None, None),
    (SwitchToTab6, TERMINAL_CONTEXT, None, None),
    (SwitchToTab7, TERMINAL_CONTEXT, None, None),
    (SwitchToTab8, TERMINAL_CONTEXT, None, None),
    (SwitchToTab9, TERMINAL_CONTEXT, None, None),
    (
        ManageTmuxSessions,
        TERMINAL_CONTEXT,
        Some(palette(
            "Tmux Sessions",
            "tmux sessions attach switch create manage",
            CommandPaletteVisibility::NotWindows
        )),
        Some(menu(
            MenuRoot::File,
            1,
            "Tmux Sessions",
            MenuVisibility::NotWindows,
            MenuActionRole::Normal
        ))
    ),
    (
        ManageSavedLayouts,
        TERMINAL_CONTEXT,
        Some(palette(
            "Saved Layouts",
            "saved layouts split panes tabs restore snapshot",
            CommandPaletteVisibility::Always
        )),
        None
    ),
    (
        RunTask,
        TERMINAL_CONTEXT,
        Some(palette(
            "Run Task",
            "task run command layout session",
            CommandPaletteVisibility::Always
        )),
        None
    ),
    (
        SplitPaneVertical,
        TERMINAL_CONTEXT,
        Some(palette(
            "Split Pane Vertical",
            "split pane vertical right",
            CommandPaletteVisibility::NotWindows
        )),
        Some(menu(
            MenuRoot::File,
            1,
            "Split Pane Vertical",
            MenuVisibility::NotWindows,
            MenuActionRole::Normal
        ))
    ),
    (
        SplitPaneHorizontal,
        TERMINAL_CONTEXT,
        Some(palette(
            "Split Pane Horizontal",
            "split pane horizontal down",
            CommandPaletteVisibility::NotWindows
        )),
        Some(menu(
            MenuRoot::File,
            1,
            "Split Pane Horizontal",
            MenuVisibility::NotWindows,
            MenuActionRole::Normal
        ))
    ),
    (
        ClosePane,
        TERMINAL_CONTEXT,
        Some(palette(
            "Close Pane",
            "kill close pane",
            CommandPaletteVisibility::NotWindows
        )),
        None
    ),
    (
        FocusPaneLeft,
        TERMINAL_CONTEXT,
        Some(palette(
            "Focus Pane Left",
            "focus pane left",
            CommandPaletteVisibility::NotWindows
        )),
        None
    ),
    (
        FocusPaneRight,
        TERMINAL_CONTEXT,
        Some(palette(
            "Focus Pane Right",
            "focus pane right",
            CommandPaletteVisibility::NotWindows
        )),
        None
    ),
    (
        FocusPaneUp,
        TERMINAL_CONTEXT,
        Some(palette(
            "Focus Pane Up",
            "focus pane up",
            CommandPaletteVisibility::NotWindows
        )),
        None
    ),
    (
        FocusPaneDown,
        TERMINAL_CONTEXT,
        Some(palette(
            "Focus Pane Down",
            "focus pane down",
            CommandPaletteVisibility::NotWindows
        )),
        None
    ),
    (
        FocusPaneNext,
        TERMINAL_CONTEXT,
        Some(palette(
            "Focus Next Pane",
            "focus pane next cycle",
            CommandPaletteVisibility::NotWindows
        )),
        Some(menu(
            MenuRoot::File,
            1,
            "Focus Next Pane",
            MenuVisibility::NotWindows,
            MenuActionRole::Normal
        ))
    ),
    (
        FocusPanePrevious,
        TERMINAL_CONTEXT,
        Some(palette(
            "Focus Previous Pane",
            "focus pane previous cycle",
            CommandPaletteVisibility::NotWindows
        )),
        None
    ),
    (
        ResizePaneLeft,
        TERMINAL_CONTEXT,
        Some(palette(
            "Resize Pane Left",
            "resize pane left",
            CommandPaletteVisibility::NotWindows
        )),
        None
    ),
    (
        ResizePaneRight,
        TERMINAL_CONTEXT,
        Some(palette(
            "Resize Pane Right",
            "resize pane right",
            CommandPaletteVisibility::NotWindows
        )),
        None
    ),
    (
        ResizePaneUp,
        TERMINAL_CONTEXT,
        Some(palette(
            "Resize Pane Up",
            "resize pane up",
            CommandPaletteVisibility::NotWindows
        )),
        None
    ),
    (
        ResizePaneDown,
        TERMINAL_CONTEXT,
        Some(palette(
            "Resize Pane Down",
            "resize pane down",
            CommandPaletteVisibility::NotWindows
        )),
        None
    ),
    (
        TogglePaneZoom,
        TERMINAL_CONTEXT,
        Some(palette(
            "Toggle Pane Zoom",
            "zoom pane maximize",
            CommandPaletteVisibility::NotWindows
        )),
        Some(menu(
            MenuRoot::View,
            1,
            "Toggle Pane Zoom",
            MenuVisibility::NotWindows,
            MenuActionRole::Normal
        ))
    ),
    (
        MinimizeWindow,
        TERMINAL_CONTEXT,
        None,
        Some(menu(
            MenuRoot::Window,
            0,
            "Minimize",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        RenameTab,
        TERMINAL_CONTEXT,
        Some(palette(
            "Rename Tab",
            "title name",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::File,
            0,
            "Rename Tab",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        AppInfo,
        TERMINAL_CONTEXT,
        Some(palette(
            "App Info",
            "information version about build",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::App,
            0,
            "App Info",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        RestartApp,
        TERMINAL_CONTEXT,
        Some(palette(
            "Restart App",
            "relaunch reopen restart",
            CommandPaletteVisibility::Always
        )),
        None
    ),
    (
        OpenSettings,
        GLOBAL_CONTEXT,
        Some(palette(
            "Settings",
            "settings preferences options",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::App,
            1,
            "Settings...",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        OpenConfig,
        GLOBAL_CONTEXT,
        Some(palette(
            "Open Settings File",
            "settings file config edit",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::App,
            1,
            "Open Config File...",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        PrettifyConfig,
        GLOBAL_CONTEXT,
        Some(palette(
            "Prettify Settings File",
            "prettify format config settings file tidy sort",
            CommandPaletteVisibility::Always
        )),
        None
    ),
    (
        ImportColors,
        TERMINAL_CONTEXT,
        Some(palette(
            "Import Colors",
            "theme palette json",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::View,
            2,
            "Import Colors",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        SwitchTheme,
        TERMINAL_CONTEXT,
        Some(palette(
            "Switch Theme",
            "theme palette colors appearance",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::View,
            2,
            "Switch Theme",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        ZoomIn,
        TERMINAL_CONTEXT,
        Some(palette(
            "Zoom In",
            "font increase",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::View,
            1,
            "Zoom In",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        ZoomOut,
        TERMINAL_CONTEXT,
        Some(palette(
            "Zoom Out",
            "font decrease",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::View,
            1,
            "Zoom Out",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        ZoomReset,
        TERMINAL_CONTEXT,
        Some(palette(
            "Reset Zoom",
            "font default",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::View,
            1,
            "Reset Zoom",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        OpenSearch,
        TERMINAL_CONTEXT,
        Some(palette(
            "Find",
            "search lookup text",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::Edit,
            1,
            "Find",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        CheckForUpdates,
        TERMINAL_CONTEXT,
        Some(palette(
            "Check for Updates",
            "release version updater",
            CommandPaletteVisibility::MacOsOnly
        )),
        Some(menu(
            MenuRoot::App,
            0,
            "Check for Updates",
            MenuVisibility::MacOsOnly,
            MenuActionRole::Normal
        ))
    ),
    (
        Quit,
        GLOBAL_CONTEXT,
        Some(palette(
            "Quit Termy",
            "quit exit close",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::App,
            2,
            "Quit Termy",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        ToggleCommandPalette,
        TERMINAL_CONTEXT,
        None,
        Some(menu(
            MenuRoot::View,
            0,
            "Command Palette",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        Copy,
        TERMINAL_CONTEXT,
        None,
        Some(menu(
            MenuRoot::Edit,
            0,
            "Copy",
            MenuVisibility::Always,
            MenuActionRole::Copy
        ))
    ),
    (
        Paste,
        TERMINAL_CONTEXT,
        None,
        Some(menu(
            MenuRoot::Edit,
            0,
            "Paste",
            MenuVisibility::Always,
            MenuActionRole::Paste
        ))
    ),
    (
        SelectAll,
        TERMINAL_CONTEXT,
        None,
        Some(menu(
            MenuRoot::Edit,
            0,
            "Select All",
            MenuVisibility::Always,
            MenuActionRole::SelectAll
        ))
    ),
    (
        ClearScreen,
        TERMINAL_CONTEXT,
        Some(palette(
            "Clear Screen",
            "clear terminal viewport",
            CommandPaletteVisibility::Always
        )),
        None
    ),
    (CloseSearch, TERMINAL_CONTEXT, None, None),
    (
        SearchNext,
        TERMINAL_CONTEXT,
        None,
        Some(menu(
            MenuRoot::Edit,
            1,
            "Find Next",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        SearchPrevious,
        TERMINAL_CONTEXT,
        None,
        Some(menu(
            MenuRoot::Edit,
            1,
            "Find Previous",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (ToggleSearchCaseSensitive, TERMINAL_CONTEXT, None, None),
    (ToggleSearchRegex, TERMINAL_CONTEXT, None, None),
    (
        InstallCli,
        TERMINAL_CONTEXT,
        Some(palette(
            "Install CLI",
            "install command line interface terminal shell path",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::Help,
            0,
            "Install CLI",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        ToggleTabBarVisibility,
        TERMINAL_CONTEXT,
        Some(palette(
            "Toggle Tab Bar Visibility",
            "tab bar tabs strip show hide visibility",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::View,
            0,
            "Tab Bar",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        ToggleWorkspaceSidebar,
        TERMINAL_CONTEXT,
        Some(palette(
            "Toggle Workspace Sidebar",
            "workspace sidebar show hide fade edge peek",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::View,
            0,
            "Workspace Sidebar",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
    (
        ToggleInspector,
        TERMINAL_CONTEXT,
        Some(palette(
            "Toggle Inspector",
            "open show close hide inspector developer debug devtools terminal state keyboard render",
            CommandPaletteVisibility::Always
        )),
        Some(menu(
            MenuRoot::View,
            0,
            "Inspector",
            MenuVisibility::Always,
            MenuActionRole::Normal
        ))
    ),
);

termy_command_core::termy_command_catalog!(impl_command_action_id_mapping);

actions!(
    termy_inline_input,
    [
        InlineBackspace,
        InlineDelete,
        InlineMoveLeft,
        InlineMoveRight,
        InlineSelectLeft,
        InlineSelectRight,
        InlineSelectAll,
        InlineMoveToStart,
        InlineMoveToEnd,
        InlineDeleteWordBackward,
        InlineDeleteWordForward,
        InlineDeleteToStart,
        InlineDeleteToEnd,
    ]
);

pub fn inline_input_keybindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("secondary-c", Copy, INLINE_INPUT_CONTEXT),
        KeyBinding::new("secondary-v", Paste, INLINE_INPUT_CONTEXT),
        KeyBinding::new("backspace", InlineBackspace, INLINE_INPUT_CONTEXT),
        KeyBinding::new("delete", InlineDelete, INLINE_INPUT_CONTEXT),
        KeyBinding::new("left", InlineMoveLeft, INLINE_INPUT_CONTEXT),
        KeyBinding::new("right", InlineMoveRight, INLINE_INPUT_CONTEXT),
        KeyBinding::new("shift-left", InlineSelectLeft, INLINE_INPUT_CONTEXT),
        KeyBinding::new("shift-right", InlineSelectRight, INLINE_INPUT_CONTEXT),
        KeyBinding::new("secondary-a", InlineSelectAll, INLINE_INPUT_CONTEXT),
        KeyBinding::new("home", InlineMoveToStart, INLINE_INPUT_CONTEXT),
        KeyBinding::new("end", InlineMoveToEnd, INLINE_INPUT_CONTEXT),
        KeyBinding::new("secondary-left", InlineMoveToStart, INLINE_INPUT_CONTEXT),
        KeyBinding::new("secondary-right", InlineMoveToEnd, INLINE_INPUT_CONTEXT),
        KeyBinding::new(
            "alt-backspace",
            InlineDeleteWordBackward,
            INLINE_INPUT_CONTEXT,
        ),
        KeyBinding::new("alt-delete", InlineDeleteWordForward, INLINE_INPUT_CONTEXT),
        KeyBinding::new(
            "secondary-backspace",
            InlineDeleteToStart,
            INLINE_INPUT_CONTEXT,
        ),
        KeyBinding::new("secondary-delete", InlineDeleteToEnd, INLINE_INPUT_CONTEXT),
        KeyBinding::new("ctrl-backspace", InlineDeleteToStart, INLINE_INPUT_CONTEXT),
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        CommandAction, CommandPaletteVisibility, MenuActionRole, MenuRoot, MenuVisibility,
        inline_input_keybindings,
    };
    use std::collections::HashSet;
    use termy_command_core::{CommandCapabilities, CommandId};

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
    fn windows_hides_tmux_commands_from_palette_entries() {
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
}
