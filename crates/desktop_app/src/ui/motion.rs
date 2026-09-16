//! Shared overlay enter motion.
//!
//! Occasional surfaces (search, menus, banners, dropdowns) fade and travel a
//! few pixels with the same ease-out used by settings switches. High-frequency
//! keyboard UI (command palette, tab switching) must not use these helpers.

use gpui::{Animation, AnimationExt as _, AnyElement, IntoElement, Styled, ease_out_quint, px};
use std::time::Duration;

/// Small popovers, search, menus, and banners.
pub const OVERLAY_ENTER_MS: u64 = 160;
/// Link previews and other near-hover chrome.
pub const TOOLTIP_ENTER_MS: u64 = 125;
/// Peek / drawer panels that travel further.
pub const DRAWER_ENTER_MS: u64 = 200;
/// Vertical travel for overlay enter. Keep this small so repeated opens stay quiet.
pub const OVERLAY_ENTER_SLIDE_PX: f32 = 8.0;

pub fn enter_from_above(
    element: impl Styled + IntoElement + 'static,
    id: impl Into<gpui::ElementId>,
) -> AnyElement {
    fade_slide_y(element, id, -OVERLAY_ENTER_SLIDE_PX, OVERLAY_ENTER_MS)
}

pub fn fade_in(
    element: impl Styled + IntoElement + 'static,
    id: impl Into<gpui::ElementId>,
) -> AnyElement {
    fade_slide_y(element, id, 0.0, TOOLTIP_ENTER_MS)
}

pub fn enter_from_left(
    element: impl Styled + IntoElement + 'static,
    id: impl Into<gpui::ElementId>,
    width: f32,
) -> AnyElement {
    element
        .with_animation(
            id,
            Animation::new(Duration::from_millis(DRAWER_ENTER_MS)).with_easing(ease_out_quint()),
            move |element, delta| element.opacity(delta).left(px(-width * (1.0 - delta))),
        )
        .into_any_element()
}

fn fade_slide_y(
    element: impl Styled + IntoElement + 'static,
    id: impl Into<gpui::ElementId>,
    from_y: f32,
    duration_ms: u64,
) -> AnyElement {
    element
        .with_animation(
            id,
            Animation::new(Duration::from_millis(duration_ms)).with_easing(ease_out_quint()),
            move |element, delta| element.opacity(delta).mt(px(from_y * (1.0 - delta))),
        )
        .into_any_element()
}

const _: () = {
    assert!(OVERLAY_ENTER_MS <= 180);
    assert!(TOOLTIP_ENTER_MS <= OVERLAY_ENTER_MS);
    assert!(DRAWER_ENTER_MS <= 300);
    assert!(OVERLAY_ENTER_SLIDE_PX > 0.0);
    assert!(OVERLAY_ENTER_SLIDE_PX <= 14.0);
};
