use gpui::{
    App, ClickEvent, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::design_system::controls::ClickHandler;
use crate::design_system::metrics::{CAPTION_SIZE, CONTROL_HEIGHT, CONTROL_RADIUS, CONTROL_WIDTH};
use crate::design_system::theme::tokens;

/// Two-to-three mutually exclusive options sharing one control lane — the SSH
/// form's Password / SSH key switch. More options than that belong in a select.
#[derive(IntoElement)]
pub struct SegmentedControl {
    id: SharedString,
    options: Vec<Segment>,
    width: Pixels,
}

struct Segment {
    label: SharedString,
    selected: bool,
    on_click: Option<ClickHandler>,
}

impl SegmentedControl {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            options: Vec::new(),
            width: CONTROL_WIDTH,
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn option(mut self, label: impl Into<SharedString>, selected: bool) -> Self {
        self.options.push(Segment {
            label: label.into(),
            selected,
            on_click: None,
        });
        self
    }

    /// Attaches a handler to the most recently added option.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        if let Some(option) = self.options.last_mut() {
            option.on_click = Some(Box::new(handler));
        }
        self
    }
}

impl RenderOnce for SegmentedControl {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let id = self.id;

        div()
            .flex()
            .flex_none()
            .items_center()
            .w(self.width)
            .h(CONTROL_HEIGHT)
            .p(px(2.0))
            .rounded(CONTROL_RADIUS)
            .bg(theme.bg_input)
            .border_1()
            .border_color(theme.border)
            .children(self.options.into_iter().enumerate().map(|(index, option)| {
                let mut segment = div()
                    .id(SharedString::from(format!("{id}-{index}")))
                    .flex()
                    .flex_grow()
                    .min_w(px(0.0))
                    .items_center()
                    .justify_center()
                    .h(px(24.0))
                    .rounded(px(4.0))
                    .text_size(CAPTION_SIZE)
                    .cursor_pointer();

                if option.selected {
                    segment = segment.bg(theme.accent_soft).text_color(theme.accent);
                } else {
                    segment = segment
                        .text_color(theme.text_muted)
                        .hover(move |style| style.bg(theme.bg_hover));
                }

                if let Some(handler) = option.on_click {
                    segment = segment.on_click(move |event, window, cx| handler(event, window, cx));
                }

                segment.child(option.label)
            }))
    }
}
