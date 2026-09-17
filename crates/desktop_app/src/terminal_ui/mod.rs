mod alacritty_bridge;
mod grid;
mod keyboard;
mod pane_terminal;
mod tmux;

// Intentionally re-exported for the app renderer adapter boundary. These types are the
// cross-crate contract for row-level paint-cache invalidation between `termy` and this crate.
pub use grid::{
    CellRenderInfo, TerminalGrid, TerminalGridPaintCacheHandle, TerminalGridPaintDamage,
    TerminalGridPaintPhase, TerminalGridRow, TerminalGridRows, TerminalUnderline,
    TerminalUnderlineStyle,
};
pub use keyboard::keystroke_to_input;
pub use pane_terminal::PaneTerminal;
pub use tmux::{
    TmuxClient, TmuxLaunchTarget, TmuxNotification, TmuxPaneMouseMode, TmuxPaneState,
    TmuxRuntimeConfig, TmuxSessionSummary, TmuxShutdownMode, TmuxSnapshot, TmuxSocketTarget,
    TmuxWindowState,
};
