pub(crate) const DEFAULT_TAB_TITLE_FALLBACK: &str = "Terminal";
pub(crate) const DEFAULT_TAB_TITLE_EXPLICIT_PREFIX: &str = "termy:tab:";
pub(crate) const DEFAULT_TAB_TITLE_PROMPT_FORMAT: &str = "{cwd}";
pub(crate) const DEFAULT_TAB_TITLE_COMMAND_FORMAT: &str = "{command}";
/// Default terminal/UI font family: a monospace font guaranteed to ship with
/// the platform, so fresh installs render without requiring a font download.
#[cfg(target_os = "macos")]
pub const DEFAULT_FONT_FAMILY: &str = "Menlo";
#[cfg(target_os = "windows")]
pub const DEFAULT_FONT_FAMILY: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const DEFAULT_FONT_FAMILY: &str = "monospace";
pub(crate) const DEFAULT_TERM: &str = "xterm-256color";
pub(crate) const DEFAULT_COLORTERM: &str = "truecolor";
pub(crate) const DEFAULT_TMUX_ENABLED: bool = false;
pub(crate) const DEFAULT_TMUX_BINARY: &str = "tmux";
pub(crate) const DEFAULT_TMUX_PERSISTENCE: bool = true;
pub(crate) const DEFAULT_TMUX_EXCLUSIVE: bool = false;
pub(crate) const DEFAULT_TMUX_SHOW_ACTIVE_PANE_BORDER: bool = false;
pub(crate) const DEFAULT_MOUSE_SCROLL_MULTIPLIER: f32 = 3.0;
/// Unitless multiplier applied to the font's natural cell height to produce the
/// terminal row height. `1.0` means no extra vertical space; `2.0` doubles it.
pub const DEFAULT_LINE_HEIGHT: f32 = 1.4;
/// Lowest accepted line-height multiplier. Values below this cause rows to
/// visually overlap.
pub const MIN_LINE_HEIGHT: f32 = 0.8;
/// Highest accepted line-height multiplier. Values above this make the grid
/// unusably sparse.
pub const MAX_LINE_HEIGHT: f32 = 2.5;
pub(crate) const DEFAULT_SCROLLBACK_HISTORY: usize = 1000;
/// Upper clamp for scrollback lines. Each pane eagerly grows toward
/// `lines × cols × ~24 bytes` of grid memory, so at 200 columns this cap
/// bounds a pane to roughly 100 MB; the previous 100k cap allowed ~480 MB
/// from a single config line.
pub(crate) const MAX_SCROLLBACK_HISTORY: usize = 20_000;
pub(crate) const DEFAULT_INACTIVE_TAB_SCROLLBACK: Option<usize> = Some(250);
pub(crate) const DEFAULT_PANE_FOCUS_STRENGTH: f32 = 0.6;
pub(crate) const DEFAULT_TAB_SWITCH_MODIFIER_HINTS: bool = true;
pub const DEFAULT_SIDEBAR_WIDTH: f32 = 200.0;
pub const MIN_SIDEBAR_WIDTH: f32 = 120.0;
pub const MAX_SIDEBAR_WIDTH: f32 = 420.0;
pub(crate) const MAX_PANE_FOCUS_STRENGTH: f32 = 2.0;
pub(crate) const MIN_MOUSE_SCROLL_MULTIPLIER: f32 = 0.1;
pub(crate) const MAX_MOUSE_SCROLL_MULTIPLIER: f32 = 1_000.0;
pub(crate) const DEFAULT_CURSOR_BLINK: bool = true;
pub(crate) const DEFAULT_WARN_ON_QUIT: bool = false;
pub(crate) const DEFAULT_WARN_ON_QUIT_WITH_RUNNING_PROCESS: bool = true;

pub const VALID_SECTIONS: &[&str] = &["colors", "tab_title"];

pub const SHELL_DECIDE_THEME_ID: &str = "shell-decide";
