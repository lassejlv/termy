use crate::constants::{
    DEFAULT_COLORTERM, DEFAULT_CURSOR_BLINK, DEFAULT_INACTIVE_TAB_SCROLLBACK,
    DEFAULT_MOUSE_SCROLL_MULTIPLIER, DEFAULT_PANE_FOCUS_STRENGTH, DEFAULT_SCROLLBACK_HISTORY,
    DEFAULT_SIDEBAR_WIDTH, DEFAULT_TAB_SWITCH_MODIFIER_HINTS, DEFAULT_TAB_TITLE_COMMAND_FORMAT,
    DEFAULT_TAB_TITLE_EXPLICIT_PREFIX, DEFAULT_TAB_TITLE_FALLBACK, DEFAULT_TAB_TITLE_PROMPT_FORMAT,
    DEFAULT_TERM, DEFAULT_TMUX_BINARY, DEFAULT_TMUX_ENABLED, DEFAULT_TMUX_EXCLUSIVE,
    DEFAULT_TMUX_PERSISTENCE, DEFAULT_TMUX_SHOW_ACTIVE_PANE_BORDER, DEFAULT_WARN_ON_QUIT,
    DEFAULT_WARN_ON_QUIT_WITH_RUNNING_PROCESS,
};

pub use termy_theme_core::Rgb8;

pub type ThemeId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabTitleSource {
    Manual,
    Explicit,
    Shell,
    Fallback,
}

impl TabTitleSource {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(Self::Manual),
            "explicit" => Some(Self::Explicit),
            "shell" | "app" | "terminal" => Some(Self::Shell),
            "fallback" | "default" => Some(Self::Fallback),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabTitleMode {
    Smart,
    Shell,
    Explicit,
    Static,
}

impl TabTitleMode {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "smart" => Some(Self::Smart),
            "shell" => Some(Self::Shell),
            "explicit" => Some(Self::Explicit),
            "static" => Some(Self::Static),
            _ => None,
        }
    }

    pub(crate) fn default_priority(self) -> Vec<TabTitleSource> {
        match self {
            Self::Smart => vec![
                TabTitleSource::Manual,
                TabTitleSource::Explicit,
                TabTitleSource::Shell,
                TabTitleSource::Fallback,
            ],
            Self::Shell => vec![
                TabTitleSource::Manual,
                TabTitleSource::Shell,
                TabTitleSource::Fallback,
            ],
            Self::Explicit => vec![
                TabTitleSource::Manual,
                TabTitleSource::Explicit,
                TabTitleSource::Fallback,
            ],
            Self::Static => vec![TabTitleSource::Manual, TabTitleSource::Fallback],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppearanceMode {
    #[default]
    Manual,
    System,
}

impl AppearanceMode {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "manual" | "off" | "fixed" => Some(Self::Manual),
            "system" | "auto" | "sync" => Some(Self::System),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppIcon {
    #[default]
    TermyDefault,
    TermyOld,
}

impl AppIcon {
    // Not `FromStr`: config parsing wants Option-style lookups, matching the
    // other config enums in this module.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "default" | "termy_default" | "termy-default" | "termy default" => {
                Some(Self::TermyDefault)
            }
            "old" | "termy_old" | "termy-old" | "termy old" => Some(Self::TermyOld),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAppearance {
    Light,
    Dark,
}

pub fn resolve_active_theme(config: &AppConfig, system_appearance: SystemAppearance) -> &str {
    match config.theme_mode {
        AppearanceMode::Manual => &config.theme,
        AppearanceMode::System => match system_appearance {
            SystemAppearance::Light => &config.theme_light,
            SystemAppearance::Dark => &config.theme_dark,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabCloseVisibility {
    ActiveHover,
    #[default]
    Hover,
    Always,
}

impl TabCloseVisibility {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "active_hover" | "activehover" | "active+hover" => Some(Self::ActiveHover),
            "hover" => Some(Self::Hover),
            "always" => Some(Self::Always),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabWidthMode {
    Stable,
    ActiveGrow,
    ActiveGrowSticky,
    #[default]
    Uniform,
}

impl TabWidthMode {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stable" => Some(Self::Stable),
            "active_grow" | "activegrow" | "active-grow" => Some(Self::ActiveGrow),
            "uniform" | "fixed" | "equal" => Some(Self::Uniform),
            "active_grow_sticky" | "activegrowsticky" | "active-grow-sticky" => {
                Some(Self::ActiveGrowSticky)
            }
            _ => None,
        }
    }
}

/// Where the tab bar is rendered: the top strip (horizontal, default) or a
/// vertical sidebar on the right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabBarPosition {
    #[default]
    Top,
    Right,
}

impl TabBarPosition {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "top" => Some(Self::Top),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

/// Where the native macOS host presents tabs: AppKit's tab bar or Termy's
/// sidebar list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NativeTabPlacement {
    #[default]
    NativeTabbar,
    Sidebar,
}

impl NativeTabPlacement {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "native_tabbar" | "native-tabbar" | "native" | "tabbar" | "tab_bar" => {
                Some(Self::NativeTabbar)
            }
            "sidebar" | "side_bar" | "left_sidebar" | "left-sidebar" => Some(Self::Sidebar),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabTitleConfig {
    pub mode: TabTitleMode,
    pub priority: Vec<TabTitleSource>,
    pub fallback: String,
    pub explicit_prefix: String,
    pub shell_integration: bool,
    pub prompt_format: String,
    pub command_format: String,
}

impl Default for TabTitleConfig {
    fn default() -> Self {
        Self {
            mode: TabTitleMode::Smart,
            priority: TabTitleMode::Smart.default_priority(),
            fallback: DEFAULT_TAB_TITLE_FALLBACK.to_string(),
            explicit_prefix: DEFAULT_TAB_TITLE_EXPLICIT_PREFIX.to_string(),
            shell_integration: true,
            prompt_format: DEFAULT_TAB_TITLE_PROMPT_FORMAT.to_string(),
            command_format: DEFAULT_TAB_TITLE_COMMAND_FORMAT.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorStyle {
    Line,
    #[default]
    Block,
}

impl CursorStyle {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "line" | "bar" | "beam" | "ibeam" => Some(Self::Line),
            "block" | "box" => Some(Self::Block),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalScrollbarVisibility {
    Off,
    Always,
    #[default]
    OnScroll,
}

impl TerminalScrollbarVisibility {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "always" => Some(Self::Always),
            "on_scroll" | "onscroll" => Some(Self::OnScroll),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalScrollbarStyle {
    #[default]
    Neutral,
    MutedTheme,
    Theme,
}

impl TerminalScrollbarStyle {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "neutral" => Some(Self::Neutral),
            "muted_theme" | "mutedtheme" => Some(Self::MutedTheme),
            "theme" => Some(Self::Theme),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowsShell {
    #[default]
    Cmd,
    PowerShell,
    PowerShellCore,
    GitBash,
}

impl WindowsShell {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command_prompt" | "commandprompt" => Some(Self::Cmd),
            "powershell" | "power_shell" | "windows_powershell" | "ps" => Some(Self::PowerShell),
            "pwsh" | "powershell_core" | "powershellcore" | "powershell_7" | "powershell-7"
            | "powershell7" | "power_shell_7" | "power_shell_core" => Some(Self::PowerShellCore),
            "git_bash" | "gitbash" | "git-bash" | "bash" => Some(Self::GitBash),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocusEffect {
    Off,
    #[default]
    SoftSpotlight,
    Cinematic,
    Minimal,
}

impl PaneFocusEffect {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "soft_spotlight" | "softspotlight" | "soft-spotlight" => Some(Self::SoftSpotlight),
            "cinematic" => Some(Self::Cinematic),
            "minimal" => Some(Self::Minimal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CustomColors {
    pub foreground: Option<Rgb8>,
    pub background: Option<Rgb8>,
    pub cursor: Option<Rgb8>,
    pub ansi: [Option<Rgb8>; 16],
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
    pub theme: ThemeId,
    pub theme_mode: AppearanceMode,
    pub theme_light: ThemeId,
    pub theme_dark: ThemeId,
    pub app_icon: AppIcon,
    pub chrome_contrast: bool,
    pub auto_update: bool,
    pub tmux_enabled: bool,
    pub tmux_persistence: bool,
    pub tmux_exclusive: bool,
    pub native_tab_persistence: bool,
    pub native_layout_autosave: bool,
    pub native_buffer_persistence: bool,
    pub show_debug_overlay: bool,
    pub tmux_binary: String,
    pub tmux_command_prefix: Option<String>,
    pub tmux_show_active_pane_border: bool,
    pub working_dir: Option<String>,
    pub working_dir_fallback: WorkingDirFallback,
    pub warn_on_quit: bool,
    pub warn_on_quit_with_running_process: bool,
    pub tab_title: TabTitleConfig,
    pub tab_close_visibility: TabCloseVisibility,
    pub tab_width_mode: TabWidthMode,
    pub tab_bar_position: TabBarPosition,
    pub native_tab_placement: NativeTabPlacement,
    pub tab_switch_modifier_hints: bool,
    pub auto_hide_tabbar: bool,
    pub sidebar_enabled: bool,
    pub sidebar_width: f32,
    pub show_termy_in_titlebar: bool,
    pub windows_shell: WindowsShell,
    pub shell: Option<String>,
    pub term: String,
    pub colorterm: Option<String>,
    pub macos_option_as_alt: bool,
    pub window_width: f32,
    pub window_height: f32,
    pub inspector_height: f32,
    pub font_family: String,
    pub ui_font_family: String,
    pub font_size: f32,
    /// Unitless multiplier on the font cell height that controls vertical row
    /// spacing. Clamped to [`MIN_LINE_HEIGHT`]..=[`MAX_LINE_HEIGHT`] at the
    /// use-site in `TerminalView`.
    pub line_height: f32,
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    pub background_opacity: f32,
    pub background_opacity_cells: bool,
    pub background_blur: bool,
    pub padding_x: f32,
    pub padding_y: f32,
    pub mouse_scroll_multiplier: f32,
    pub terminal_scrollbar_visibility: TerminalScrollbarVisibility,
    pub terminal_scrollbar_style: TerminalScrollbarStyle,
    pub scrollback_history: usize,
    pub inactive_tab_scrollback: Option<usize>,
    pub pane_focus_effect: PaneFocusEffect,
    pub pane_focus_strength: f32,
    pub copy_on_select: bool,
    pub copy_on_select_toast: bool,
    pub command_palette_show_keybinds: bool,
    pub simple_mode: bool,
    pub onboarding_complete: bool,
    pub shell_integration_enabled: bool,
    pub progress_indicator_enabled: bool,
    pub keybind_lines: Vec<KeybindConfigLine>,
    pub tasks: Vec<TaskConfig>,
    pub colors: CustomColors,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindConfigLine {
    pub line_number: usize,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskConfig {
    pub name: String,
    pub command: String,
    pub layout: Option<String>,
    pub working_dir: Option<String>,
    pub keybind: Option<KeybindConfigLine>,
}

impl AppConfig {
    /// The configured tmux command prefix split into argv items
    /// (`wsl.exe -e` becomes `["wsl.exe", "-e"]`). Empty when unset.
    pub fn tmux_command_prefix_argv(&self) -> Vec<String> {
        self.tmux_command_prefix
            .as_deref()
            .map(|prefix| prefix.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default()
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: "termy".to_string(),
            theme_mode: AppearanceMode::default(),
            theme_light: "termy-light".to_string(),
            theme_dark: "termy".to_string(),
            app_icon: AppIcon::default(),
            chrome_contrast: false,
            auto_update: true,
            tmux_enabled: DEFAULT_TMUX_ENABLED,
            tmux_persistence: DEFAULT_TMUX_PERSISTENCE,
            tmux_exclusive: DEFAULT_TMUX_EXCLUSIVE,
            native_tab_persistence: false,
            native_layout_autosave: false,
            native_buffer_persistence: false,
            show_debug_overlay: false,
            tmux_binary: DEFAULT_TMUX_BINARY.to_string(),
            tmux_command_prefix: None,
            tmux_show_active_pane_border: DEFAULT_TMUX_SHOW_ACTIVE_PANE_BORDER,
            working_dir: None,
            working_dir_fallback: WorkingDirFallback::default(),
            warn_on_quit: DEFAULT_WARN_ON_QUIT,
            warn_on_quit_with_running_process: DEFAULT_WARN_ON_QUIT_WITH_RUNNING_PROCESS,
            tab_title: TabTitleConfig::default(),
            tab_close_visibility: TabCloseVisibility::default(),
            tab_width_mode: TabWidthMode::default(),
            tab_bar_position: TabBarPosition::default(),
            native_tab_placement: NativeTabPlacement::default(),
            tab_switch_modifier_hints: DEFAULT_TAB_SWITCH_MODIFIER_HINTS,
            auto_hide_tabbar: true,
            sidebar_enabled: false,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            show_termy_in_titlebar: true,
            windows_shell: WindowsShell::default(),
            shell: None,
            term: DEFAULT_TERM.to_string(),
            colorterm: Some(DEFAULT_COLORTERM.to_string()),
            macos_option_as_alt: false,
            window_width: 1280.0,
            window_height: 820.0,
            inspector_height: 280.0,
            font_family: crate::constants::DEFAULT_FONT_FAMILY.to_string(),
            ui_font_family: crate::constants::DEFAULT_FONT_FAMILY.to_string(),
            font_size: 14.0,
            line_height: crate::constants::DEFAULT_LINE_HEIGHT,
            cursor_style: CursorStyle::default(),
            cursor_blink: DEFAULT_CURSOR_BLINK,
            background_opacity: 1.0,
            background_opacity_cells: false,
            background_blur: false,
            padding_x: 12.0,
            padding_y: 8.0,
            mouse_scroll_multiplier: DEFAULT_MOUSE_SCROLL_MULTIPLIER,
            terminal_scrollbar_visibility: TerminalScrollbarVisibility::default(),
            terminal_scrollbar_style: TerminalScrollbarStyle::default(),
            scrollback_history: DEFAULT_SCROLLBACK_HISTORY,
            inactive_tab_scrollback: DEFAULT_INACTIVE_TAB_SCROLLBACK,
            pane_focus_effect: PaneFocusEffect::default(),
            pane_focus_strength: DEFAULT_PANE_FOCUS_STRENGTH,
            copy_on_select: false,
            copy_on_select_toast: true,
            command_palette_show_keybinds: true,
            simple_mode: false,
            onboarding_complete: true,
            shell_integration_enabled: true,
            progress_indicator_enabled: true,
            keybind_lines: Vec::new(),
            tasks: Vec::new(),
            colors: CustomColors::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingDirFallback {
    Home,
    Process,
}

impl WorkingDirFallback {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "home" | "user" => Some(Self::Home),
            "process" | "cwd" => Some(Self::Process),
            _ => None,
        }
    }
}

#[allow(clippy::derivable_impls)]
impl Default for WorkingDirFallback {
    fn default() -> Self {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            Self::Home
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Process
        }
    }
}
