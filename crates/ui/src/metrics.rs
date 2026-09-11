//! Layout constants for Termy's settings chrome.
//!
//! These mirror the shipped values in `crates/desktop_app/src/settings_view/`
//! so a kit-built surface lines up pixel for pixel with the app's own panels.
//! Change a number here only when the design changes — not to make one call
//! site look better.

use gpui::{Pixels, px};

// ── Sidebar ──────────────────────────────────────────────────────
pub const SIDEBAR_WIDTH: Pixels = px(208.0);
pub const SIDEBAR_PADDING_X: Pixels = px(10.0);
pub const SIDEBAR_PADDING_Y: Pixels = px(12.0);
pub const SIDEBAR_ITEM_HEIGHT: Pixels = px(32.0);
pub const SIDEBAR_ITEM_RADIUS: Pixels = px(7.0);
pub const SIDEBAR_ITEM_PADDING_X: Pixels = px(10.0);
pub const SIDEBAR_ITEM_GAP: Pixels = px(4.0);
pub const SIDEBAR_ICON_SIZE: Pixels = px(18.0);
pub const SIDEBAR_ICON_GAP: Pixels = px(10.0);
pub const SIDEBAR_GROUP_GAP: Pixels = px(16.0);
pub const SIDEBAR_GROUP_LABEL_PADDING_X: Pixels = px(8.0);
pub const SIDEBAR_GROUP_LABEL_PADDING_BOTTOM: Pixels = px(6.0);
/// Accent bar on the selected item — 2px wide, inset 8px top and bottom.
pub const SIDEBAR_SELECTED_ACCENT_WIDTH: Pixels = px(2.0);
pub const SIDEBAR_SELECTED_ACCENT_INSET_Y: Pixels = px(8.0);

// ── Content column ───────────────────────────────────────────────
/// Content never widens past this; extra window width becomes right slack.
pub const CONTENT_MAX_WIDTH: Pixels = px(800.0);
pub const CONTENT_GUTTER_X: Pixels = px(32.0);
pub const CONTENT_GUTTER_Y: Pixels = px(24.0);
pub const CARD_GAP: Pixels = px(22.0);
pub const CARD_RADIUS: Pixels = px(10.0);
pub const CARD_ROW_PADDING_X: Pixels = px(16.0);
pub const CARD_ROW_PADDING_Y: Pixels = px(11.0);
pub const GROUP_LABEL_GAP: Pixels = px(8.0);
/// Gap between a row's label lane, control lane, and reset lane.
pub const ROW_LANE_GAP: Pixels = px(16.0);
/// Reserved even when a row has nothing to reset, so glyphs share one lane.
pub const RESET_SLOT_SIZE: Pixels = px(20.0);
pub const RESET_ICON_SIZE: Pixels = px(14.0);

// ── Controls ─────────────────────────────────────────────────────
pub const CONTROL_WIDTH: Pixels = px(300.0);
pub const CONTROL_HEIGHT: Pixels = px(30.0);
pub const CONTROL_RADIUS: Pixels = px(6.0);
pub const CONTROL_INNER_PADDING: Pixels = px(8.0);
pub const CONTROL_TEXT_PADDING: Pixels = px(10.0);
pub const BUTTON_RADIUS: Pixels = px(6.0);
pub const BUTTON_HEIGHT: Pixels = px(30.0);
pub const BUTTON_HEIGHT_SMALL: Pixels = px(26.0);

pub const SWITCH_WIDTH: Pixels = px(38.0);
pub const SWITCH_HEIGHT: Pixels = px(22.0);
pub const SWITCH_KNOB_SIZE: Pixels = px(18.0);
pub const SWITCH_RADIUS: Pixels = px(11.0);
pub const SWITCH_PADDING: Pixels = px(2.0);

pub const STEPPER_BUTTON_SIZE: Pixels = px(22.0);
pub const STEPPER_BUTTON_RADIUS: Pixels = px(4.0);
pub const STEPPER_BUTTON_GAP: Pixels = px(4.0);

pub const SLIDER_TRACK_HEIGHT: Pixels = px(4.0);
pub const SLIDER_KNOB_SIZE: Pixels = px(16.0);
pub const SLIDER_VALUE_WIDTH: Pixels = px(60.0);
pub const SLIDER_GAP: Pixels = px(6.0);

pub const SWATCH_SIZE: Pixels = px(18.0);
pub const SWATCH_RADIUS: Pixels = px(4.0);

pub const KEY_CHIP_HEIGHT: Pixels = px(20.0);
pub const KEY_CHIP_RADIUS: Pixels = px(4.0);
pub const KEY_CHIP_PADDING_X: Pixels = px(7.0);
pub const KEY_CHIP_GAP: Pixels = px(5.0);

pub const BADGE_HEIGHT: Pixels = px(16.0);
pub const BADGE_RADIUS: Pixels = px(8.0);
pub const BADGE_PADDING_X: Pixels = px(6.0);

pub const SCROLLBAR_WIDTH: Pixels = px(8.0);

// ── Type scale ───────────────────────────────────────────────────
pub const SECTION_TITLE_SIZE: Pixels = px(22.0);
pub const SECTION_SUBTITLE_SIZE: Pixels = px(13.0);
/// Uppercase group labels above each card.
pub const GROUP_TITLE_SIZE: Pixels = px(11.0);
/// Setting labels and control values.
pub const BODY_SIZE: Pixels = px(13.0);
/// Descriptions under a label, captions, chips.
pub const LABEL_SIZE: Pixels = px(11.0);
pub const LABEL_LINE_HEIGHT: Pixels = px(16.0);
pub const CAPTION_SIZE: Pixels = px(12.0);
pub const BADGE_SIZE: Pixels = px(10.0);
