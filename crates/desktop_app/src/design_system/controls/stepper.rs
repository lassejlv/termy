use gpui::{
    App, ClickEvent, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::design_system::controls::ClickHandler;
use crate::design_system::icon::{Icon, IconName};
use crate::design_system::metrics::{
    BODY_SIZE, CONTROL_HEIGHT, CONTROL_RADIUS, CONTROL_TEXT_PADDING, CONTROL_WIDTH,
    STEPPER_BUTTON_GAP, STEPPER_BUTTON_RADIUS, STEPPER_BUTTON_SIZE,
};
use crate::design_system::theme::tokens;

/// Numeric field with the increment pair parked inside its right edge, so a
/// stepper and a select still present the same 300×30 silhouette in the lane.
#[derive(IntoElement)]
pub struct Stepper {
    /// Kept as a string so the two step buttons can derive stable child ids.
    id: SharedString,
    value: SharedString,
    width: Pixels,
    on_increment: Option<ClickHandler>,
    on_decrement: Option<ClickHandler>,
}

impl Stepper {
    pub fn new(id: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            width: CONTROL_WIDTH,
            on_increment: None,
            on_decrement: None,
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn on_increment(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_increment = Some(Box::new(handler));
        self
    }

    pub fn on_decrement(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_decrement = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Stepper {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let id = self.id;

        let step_button = |suffix: &str, icon: IconName, handler: Option<ClickHandler>| {
            let mut button = div()
                .id(SharedString::from(format!("{id}-{suffix}")))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(STEPPER_BUTTON_SIZE)
                .rounded(STEPPER_BUTTON_RADIUS)
                .bg(theme.bg_hover)
                .cursor_pointer()
                .hover(move |style| style.bg(theme.border));

            if let Some(handler) = handler {
                button = button.on_click(move |event, window, cx| handler(event, window, cx));
            }

            button.child(Icon::new(icon).size(px(10.0)).color(theme.text_secondary))
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .w(self.width)
            .h(CONTROL_HEIGHT)
            .pr(STEPPER_BUTTON_GAP)
            .gap(STEPPER_BUTTON_GAP)
            .rounded(CONTROL_RADIUS)
            .bg(theme.bg_input)
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .child(
                div()
                    .flex_grow()
                    .min_w(px(0.0))
                    .px(CONTROL_TEXT_PADDING)
                    .text_size(BODY_SIZE)
                    .text_color(theme.text_primary)
                    .truncate()
                    .child(self.value),
            )
            .child(step_button("increment", IconName::Plus, self.on_increment))
            .child(step_button("decrement", IconName::Minus, self.on_decrement))
    }
}
