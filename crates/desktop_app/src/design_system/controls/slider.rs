use gpui::{
    ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce, SharedString,
    Styled, Window, div, px, relative,
};

use crate::design_system::metrics::{
    BODY_SIZE, CONTROL_HEIGHT, CONTROL_RADIUS, CONTROL_WIDTH, SLIDER_GAP, SLIDER_KNOB_SIZE,
    SLIDER_TRACK_HEIGHT, SLIDER_VALUE_WIDTH,
};
use crate::design_system::theme::tokens;

/// Track plus committed value field, as used by Background Opacity and Scroll
/// Multiplier. Dragging is the host's job — this renders a position, so a live
/// preview and a committed value can be shown with the same component.
#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    ratio: f32,
    value: SharedString,
    width: Pixels,
}

impl Slider {
    pub fn new(id: impl Into<ElementId>, ratio: f32, value: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            ratio: ratio.clamp(0.0, 1.0),
            value: value.into(),
            width: CONTROL_WIDTH,
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }
}

impl RenderOnce for Slider {
    fn render(self, _window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        let theme = tokens(cx);
        // Centers the 4px track and the 16px knob on the 30px control lane.
        let track_top = (CONTROL_HEIGHT - SLIDER_TRACK_HEIGHT) / 2.0;
        let knob_top = (CONTROL_HEIGHT - SLIDER_KNOB_SIZE) / 2.0;

        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(SLIDER_GAP)
            .w(self.width)
            .h(CONTROL_HEIGHT)
            .child(
                div()
                    .relative()
                    .flex_grow()
                    .min_w(px(0.0))
                    .h(CONTROL_HEIGHT)
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(track_top)
                            .h(SLIDER_TRACK_HEIGHT)
                            .rounded_full()
                            .bg(theme.bg_input),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(track_top)
                            .h(SLIDER_TRACK_HEIGHT)
                            .w(relative(self.ratio))
                            .rounded_full()
                            .bg(theme.accent)
                            // The knob rides the fill's trailing edge, pulled
                            // half its width so it centers on the value.
                            .child(
                                div()
                                    .absolute()
                                    .right(-SLIDER_KNOB_SIZE / 2.0)
                                    .top(knob_top - track_top)
                                    .size(SLIDER_KNOB_SIZE)
                                    .rounded_full()
                                    .bg(theme.text_primary)
                                    .border_1()
                                    .border_color(theme.border),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(SLIDER_VALUE_WIDTH)
                    .h(CONTROL_HEIGHT)
                    .rounded(CONTROL_RADIUS)
                    .bg(theme.bg_input)
                    .border_1()
                    .border_color(theme.border)
                    .text_size(BODY_SIZE)
                    .text_color(theme.text_primary)
                    .child(self.value),
            )
    }
}
