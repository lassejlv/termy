use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    StatefulInteractiveElement, Styled, Window, div,
};

use crate::controls::ClickHandler;
use crate::metrics::{
    SWITCH_HEIGHT, SWITCH_KNOB_SIZE, SWITCH_PADDING, SWITCH_RADIUS, SWITCH_WIDTH,
};
use crate::theme::tokens;

/// 38×22 track, 18px knob. On is an accent fill with the knob punched out in
/// the window background, so the switch reads as lit rather than merely tinted.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    checked: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Switch {
    pub fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        Self {
            id: id.into(),
            checked,
            disabled: false,
            on_click: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);

        let mut track = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .w(SWITCH_WIDTH)
            .h(SWITCH_HEIGHT)
            .p(SWITCH_PADDING)
            .rounded(SWITCH_RADIUS);

        let knob_color = if self.checked {
            track = track.justify_end().bg(theme.accent);
            theme.text_on_accent
        } else {
            track = track
                .bg(theme.bg_input)
                .border_1()
                .border_color(theme.border);
            theme.text_muted
        };

        if self.disabled {
            track = track.opacity(0.45);
        } else {
            track = track.cursor_pointer();
            if let Some(handler) = self.on_click {
                track = track.on_click(move |event, window, cx| handler(event, window, cx));
            }
        }

        track.child(
            div()
                .size(SWITCH_KNOB_SIZE)
                .rounded_full()
                .flex_none()
                .bg(knob_color),
        )
    }
}
