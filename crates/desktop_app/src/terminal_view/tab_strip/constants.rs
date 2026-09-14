pub(crate) const TABBAR_HEIGHT: f32 = 32.0;
pub(crate) const TOP_STRIP_SIDE_PADDING: f32 = 8.0;
#[cfg(macos_sdk_26)]
pub(crate) const TOP_STRIP_MACOS_TRAFFIC_LIGHT_PADDING: f32 = 78.0;
#[cfg(not(macos_sdk_26))]
pub(crate) const TOP_STRIP_MACOS_TRAFFIC_LIGHT_PADDING: f32 = 71.0;
pub(crate) const TOP_STRIP_CONTENT_OFFSET_Y: f32 = 0.0;
pub(crate) const TOP_STRIP_TERMY_BRANDING_TEXT: &str = "termy";
pub(crate) const TOP_STRIP_TERMY_BRANDING_FONT_SIZE: f32 = 12.0;
pub(crate) const TOP_STRIP_TERMY_BRANDING_SIDE_PADDING: f32 = 10.0;
pub(crate) const TOP_STRIP_TERMY_BRANDING_TAB_GAP: f32 = 8.0;
pub(crate) const TAB_HORIZONTAL_PADDING: f32 = 6.0;
pub(crate) const TAB_ITEM_HEIGHT: f32 = 26.0;
pub(crate) const TAB_ITEM_GAP: f32 = 4.0;
pub(crate) const TAB_ITEM_RADIUS: f32 = 5.0;
pub(crate) const TAB_TEXT_PADDING_X: f32 = 10.0;
// Reserved leading area inside each tab. Stays empty unless the tab reports a
// progress state, in which case the progress dot renders here; reserving it
// unconditionally keeps the title from shifting when progress appears.
pub(crate) const TAB_LEADING_SLOT_WIDTH: f32 = 14.0;
// Accent bar marking the active tab. Inset vertically past the chip corner
// radius so the rounded corners never clip it.
pub(crate) const TAB_ACTIVE_INDICATOR_WIDTH: f32 = 2.0;
pub(crate) const TAB_ACTIVE_INDICATOR_INSET_Y: f32 = 6.0;
pub(crate) const TAB_HORIZONTAL_TITLE_FONT_SIZE: f32 = 12.0;
pub(crate) const TAB_TITLE_FONT_SIZE: f32 = 12.0;
// Adds a small cushion to avoid early clipping from glyph/metrics variance.
pub(crate) const TAB_TITLE_LAYOUT_SLACK_PX: f32 = 18.0;
pub(crate) const TAB_MIN_WIDTH: f32 = 96.0;
pub(crate) const TAB_MAX_WIDTH: f32 = 260.0;
// Width a tab expands to while its title is being renamed, to give the inline
// editor room to type. Exceeds TAB_MAX_WIDTH on purpose; clamped to the
// available viewport width so it never overflows the strip.
pub(crate) const TAB_RENAME_MIN_WIDTH: f32 = 260.0;
// While renaming, the tab grows with the typed text up to this width so long
// names stay fully visible. Bounded by the viewport so it never eats the whole
// strip; falls back to TAB_RENAME_MIN_WIDTH growth on narrow windows.
pub(crate) const TAB_RENAME_MAX_WIDTH: f32 = 420.0;
pub(crate) const TAB_CLOSE_SLOT_WIDTH: f32 = 22.0;
pub(crate) const TAB_CLOSE_HITBOX: f32 = TAB_CLOSE_SLOT_WIDTH;
pub(crate) const TAB_CLOSE_CHIP_WIDTH: f32 = 16.0;
pub(crate) const TAB_CLOSE_CHIP_HEIGHT: f32 = 16.0;
pub(crate) const TAB_CLOSE_CHIP_RADIUS: f32 = 8.0;
pub(crate) const TAB_SWITCH_HINT_TEXT_SIZE: f32 = 10.0;
pub(crate) const TAB_STROKE_FOREGROUND_MIX: f32 = 0.16;
pub(crate) const TAB_STROKE_THICKNESS: f32 = 1.0;
pub(crate) const TAB_DROP_MARKER_WIDTH: f32 = 3.0;
pub(crate) const TAB_DROP_MARKER_INSET_Y: f32 = 3.0;
pub(crate) const TAB_DRAG_AUTOSCROLL_EDGE_WIDTH: f32 = 32.0;
pub(crate) const TAB_DRAG_AUTOSCROLL_MAX_STEP: f32 = 24.0;
pub(crate) const TABBAR_ACTION_RAIL_WIDTH: f32 = 28.0;
pub(crate) const TABBAR_NEW_TAB_BUTTON_SIZE: f32 = TAB_ITEM_HEIGHT;
// Dropdown under the "+" button for platform-specific tab and shell choices.
pub(crate) const NEW_TAB_MENU_WIDTH: f32 = 230.0;

// Vertical tab sidebar (tab_bar_position = right).
pub(crate) const SIDEBAR_WIDTH: f32 = 200.0;
pub(crate) const SIDEBAR_COLLAPSED_WIDTH: f32 = 28.0;
pub(crate) const SIDEBAR_HEADER_HEIGHT: f32 = 32.0;
pub(crate) const SIDEBAR_TAB_ROW_HEIGHT: f32 = 32.0;
pub(crate) const SIDEBAR_TAB_ROW_GAP: f32 = 2.0;
// Horizontal margin around sidebar tab rows and vertical padding above the
// first row, so the rounded rows read as chips inside the sidebar column.
pub(crate) const SIDEBAR_TAB_MARGIN_X: f32 = 6.0;
pub(crate) const SIDEBAR_TAB_PADDING_Y: f32 = 6.0;
// Left workspace sidebar (sidebar_enabled = true).
pub(crate) const WORKSPACE_SIDEBAR_HEADER_HEIGHT: f32 = 36.0;
pub(crate) const WORKSPACE_SIDEBAR_ROW_HEIGHT: f32 = 30.0;
pub(crate) const WORKSPACE_SIDEBAR_ROW_GAP: f32 = 2.0;
pub(crate) const WORKSPACE_SIDEBAR_PADDING_X: f32 = 8.0;
pub(crate) const WORKSPACE_SIDEBAR_PADDING_Y: f32 = 8.0;
pub(crate) const WORKSPACE_SIDEBAR_RESIZE_HANDLE_WIDTH: f32 = 6.0;

pub(crate) const TAB_STRIP_SCROLL_EPSILON: f32 = 0.5;
pub(crate) const TAB_STRIP_WHEEL_DELTA_LINE_REFERENCE_PX: f32 = 16.0;
pub(crate) const TAB_PROGRESS_BADGE_SIZE: f32 = 7.0;
pub(crate) const TAB_STRIP_BRANDING_TEXT_ALPHA_FLOOR: f32 = 0.82;
