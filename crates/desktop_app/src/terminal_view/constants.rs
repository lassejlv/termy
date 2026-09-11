use std::time::Duration;

pub(super) const MIN_FONT_SIZE: f32 = 8.0;
pub(super) const MAX_FONT_SIZE: f32 = 40.0;
pub(super) const ZOOM_STEP: f32 = 1.0;
#[cfg(target_os = "windows")]
pub(super) const TITLEBAR_HEIGHT: f32 = 32.0;
#[cfg(not(target_os = "windows"))]
pub(super) const TITLEBAR_HEIGHT: f32 = 34.0;
pub(super) const MAX_TAB_TITLE_CHARS: usize = 96;
pub(super) const DEFAULT_TAB_TITLE: &str = "Terminal";
pub(super) const COMMAND_TITLE_DELAY_MS: u64 = 250;
pub(super) const CURSOR_BLINK_INTERVAL_MS: u64 = 530;
pub(super) const TMUX_TITLE_REFRESH_DEBOUNCE_MS: u64 = 120;
const CHILD_WORKING_DIR_CACHE_TTL_MS: u64 = 1500;
pub(super) const SELECTION_BG_ALPHA: f32 = 0.35;
pub(super) const DIM_TEXT_FACTOR: f32 = 0.66;
pub(super) const COMMAND_PALETTE_WIDTH: f32 = 560.0;
pub(super) const COMMAND_PALETTE_MIN_WIDTH: f32 = 320.0;
pub(super) const COMMAND_PALETTE_MAX_ITEMS: usize = 8;
/// Floor for the row count in short windows; below this the list stops being
/// worth scrolling.
pub(super) const COMMAND_PALETTE_MIN_ITEMS: usize = 3;
/// Breathing room kept between the panel and the window edges.
pub(super) const COMMAND_PALETTE_VIEWPORT_MARGIN_X: f32 = 24.0;
pub(super) const COMMAND_PALETTE_VIEWPORT_MARGIN_Y: f32 = 24.0;
pub(super) const COMMAND_PALETTE_MIN_TOP_OFFSET: f32 = 12.0;
pub(super) const COMMAND_PALETTE_TOP_OFFSET_RATIO: f32 = 0.08;
pub(super) const COMMAND_PALETTE_LIST_PADDING_Y: f32 = 6.0;
pub(super) const COMMAND_PALETTE_ROW_HEIGHT: f32 = 34.0;
pub(super) const COMMAND_PALETTE_SCROLLBAR_WIDTH: f32 = 8.0;
pub(super) const COMMAND_PALETTE_SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 18.0;
pub(super) const COMMAND_PALETTE_INPUT_HEAD_HEIGHT: f32 = 44.0;
pub(super) const COMMAND_PALETTE_INPUT_TEXT_SIZE: f32 = 14.0;
pub(super) const COMMAND_PALETTE_ROW_ICON_SIZE: f32 = 14.0;
pub(super) const COMMAND_PALETTE_ROW_PADDING_X: f32 = 12.0;
pub(super) const COMMAND_PALETTE_ROW_CATEGORY_MAX_WIDTH: f32 = 96.0;
pub(super) const COMMAND_PALETTE_BREADCRUMB_MAX_WIDTH: f32 = 220.0;
/// Gap between the keycaps of one keystroke, and between keystrokes of a
/// multi-stroke binding.
pub(super) const COMMAND_PALETTE_KEYCAP_GAP: f32 = 2.0;
pub(super) const COMMAND_PALETTE_KEYSTROKE_GAP: f32 = 6.0;
/// Shown on rows that are unavailable but carry no more specific reason.
pub(super) const COMMAND_PALETTE_UNAVAILABLE_HINT: &str = "unavailable";
pub(super) const COMMAND_PALETTE_FOOTER_HEIGHT: f32 = 32.0;
pub(super) const COMMAND_PALETTE_SELECTED_ACCENT_WIDTH: f32 = 2.0;
pub(super) const COMMAND_PALETTE_SELECTED_ACCENT_INSET_Y: f32 = 8.0;
pub(super) const COMMAND_PALETTE_SCRIM_ALPHA: f32 = 0.12;
pub(super) const COMMAND_PALETTE_DIVIDER_ALPHA: f32 = 0.10;
pub(super) const COMMAND_PALETTE_TOP_OFFSET: f32 = 60.0;
pub(super) const TERMINAL_SCROLLBAR_GUTTER_WIDTH: f32 = 12.0;
pub(super) const TERMINAL_SCROLLBAR_TRACK_WIDTH: f32 = 12.0;
pub(super) const TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 40.0;
pub(super) const TERMINAL_SCROLLBAR_TRACK_HOLD_REPEAT_MS: u64 = 65;
pub(super) const TERMINAL_SCROLLBAR_HOLD_DURATION: Duration = Duration::from_millis(900);
pub(super) const TERMINAL_SCROLLBAR_FADE_DURATION: Duration = Duration::from_millis(140);
pub(super) const TERMINAL_SCROLLBAR_GUTTER_ALPHA: f32 = 0.0;
pub(super) const TERMINAL_SCROLLBAR_TRACK_ALPHA: f32 = 0.06;
pub(super) const TERMINAL_SCROLLBAR_THUMB_ALPHA: f32 = 0.38;
pub(super) const TERMINAL_SCROLLBAR_THUMB_ACTIVE_ALPHA: f32 = 0.72;
pub(super) const TERMINAL_SCROLLBAR_MATCH_MARKER_ALPHA: f32 = 0.48;
pub(super) const TERMINAL_SCROLLBAR_CURRENT_MARKER_ALPHA: f32 = 0.92;
pub(super) const TERMINAL_SCROLLBAR_MARKER_HEIGHT: f32 = 2.0;
pub(super) const TERMINAL_SCROLLBAR_TRACK_RADIUS: f32 = 999.0;
pub(super) const TERMINAL_SCROLLBAR_THUMB_RADIUS: f32 = 999.0;
pub(super) const TERMINAL_SCROLLBAR_THUMB_INSET: f32 = 3.0;
pub(super) const TERMINAL_SCROLLBAR_MUTED_THEME_BLEND: f32 = 0.38;
pub(super) const SEARCH_BAR_WIDTH: f32 = 392.0;
pub(super) const SEARCH_BAR_HEIGHT: f32 = 38.0;
pub(super) const SEARCH_DEBOUNCE_MS: u64 = 50;
pub(super) const TMUX_RESIZE_ERROR_TOAST_DEBOUNCE_MS: u64 = 2000;
pub(super) const DEBUG_OVERLAY_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);
#[cfg(target_os = "windows")]
pub(super) const TMUX_UNSUPPORTED_WINDOWS_TOAST: &str =
    "tmux integration is unsupported on Windows; using native runtime instead.";
pub(super) const INPUT_SCROLL_SUPPRESS_MS: u64 = 160;
pub(super) const TOAST_COPY_FEEDBACK_MS: u64 = 1200;
pub(super) const WINDOW_RESIZE_INDICATOR_MS: u64 = 850;
pub(super) const RESIZE_THROTTLE_MS: u64 = 16;
pub(super) const INACTIVE_RESIZE_DEBOUNCE_MS: u64 = 120;
pub(super) const CHILD_WORKING_DIR_CACHE_TTL: Duration =
    Duration::from_millis(CHILD_WORKING_DIR_CACHE_TTL_MS);
pub(super) const BENCHMARK_EXIT_GRACE_DURATION: Duration = Duration::from_millis(250);
pub(super) const OVERLAY_PANEL_ALPHA_FLOOR_RATIO: f32 = 0.72;
pub(super) const OVERLAY_PRIMARY_TEXT_ALPHA: f32 = 0.95;
pub(super) const OVERLAY_MUTED_TEXT_ALPHA: f32 = 0.62;
pub(super) const COMMAND_PALETTE_PANEL_SOLID_ALPHA: f32 = 0.90;
pub(super) const COMMAND_PALETTE_ROW_SELECTED_BG_ALPHA: f32 = 0.20;
pub(super) const COMMAND_PALETTE_SELECTED_ACCENT_ALPHA: f32 = 0.95;
pub(super) const COMMAND_PALETTE_MATCH_TEXT_ALPHA: f32 = 1.0;
/// Row icons hold one tone regardless of selection, so selection reads from the
/// row background and accent bar alone.
pub(super) const COMMAND_PALETTE_ICON_TEXT_ALPHA: f32 = 0.72;
pub(super) const COMMAND_PALETTE_SHORTCUT_BG_ALPHA: f32 = 0.10;
pub(super) const COMMAND_PALETTE_SHORTCUT_TEXT_ALPHA: f32 = 0.80;
pub(super) const COMMAND_PALETTE_PANEL_BG_ALPHA: f32 = 0.98;
pub(super) const COMMAND_PALETTE_INPUT_SELECTION_ALPHA: f32 = 0.28;
pub(super) const COMMAND_PALETTE_SCROLLBAR_TRACK_ALPHA: f32 = 0.10;
pub(super) const COMMAND_PALETTE_SCROLLBAR_THUMB_ALPHA: f32 = 0.42;
pub(super) const SEARCH_BAR_BG_ALPHA: f32 = 0.92;
pub(super) const SEARCH_INPUT_BG_ALPHA: f32 = 0.20;
pub(super) const SEARCH_COUNTER_TEXT_ALPHA: f32 = 0.72;
pub(super) const SEARCH_BUTTON_TEXT_ALPHA: f32 = 0.70;
pub(super) const SEARCH_BUTTON_HOVER_BG_ALPHA: f32 = 0.20;
pub(super) const SEARCH_INPUT_SELECTION_ALPHA: f32 = 0.30;
pub(super) const TAB_SWITCH_HINT_ANIMATION_FRAME_MS: u64 = 16;
pub(super) const NEW_TAB_ANIMATION_DURATION: Duration = Duration::from_millis(180);
pub(super) const NEW_TAB_ANIMATION_FRAME_MS: u64 = 16;
pub(super) const TAB_INTERACTION_ANIMATION_FRAME_MS: u64 = 16;
pub(super) const MAX_PANE_FOCUS_STRENGTH: f32 = 2.0;
pub(super) const NATIVE_PANE_MIN_COLS: u16 = 24;
pub(super) const NATIVE_PANE_MIN_ROWS: u16 = 8;
// Grab handle pill at each pane's top center for drag-to-move.
pub(super) const PANE_DRAG_HANDLE_WIDTH: f32 = 44.0;
pub(super) const PANE_DRAG_HANDLE_HEIGHT: f32 = 12.0;
pub(super) const PANE_DRAG_HANDLE_INSET_Y: f32 = 3.0;
#[cfg(debug_assertions)]
pub(super) const RENDER_METRICS_LOG_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TerminalOverlayGeometry {
    pub(super) panel_radius: f32,
    pub(super) input_radius: f32,
    pub(super) control_radius: f32,
}

// Floating terminal chrome stays square to match the app's shared overlay language.
pub(super) const TERMINAL_OVERLAY_GEOMETRY: TerminalOverlayGeometry = TerminalOverlayGeometry {
    panel_radius: 0.0,
    input_radius: 0.0,
    control_radius: 0.0,
};

// Search bar uses rounded corners for a native macOS feel.
pub(super) const SEARCH_OVERLAY_GEOMETRY: TerminalOverlayGeometry = TerminalOverlayGeometry {
    panel_radius: 10.0,
    input_radius: 6.0,
    control_radius: 6.0,
};

// Toast feedback uses rounded corners for a softer, modern look.
pub(super) const TOAST_GEOMETRY: TerminalOverlayGeometry = TerminalOverlayGeometry {
    panel_radius: 10.0,
    input_radius: 6.0,
    control_radius: 6.0,
};
