use gpui::{
    AnyElement, App, IntoElement, ParentElement, Pixels, RenderOnce, Rgba, SharedString, Styled,
    Window, div, px,
};

use crate::design_system::metrics::{
    BADGE_SIZE, BODY_SIZE, CONTROL_HEIGHT, CONTROL_INNER_PADDING, CONTROL_RADIUS,
    CONTROL_TEXT_PADDING, CONTROL_WIDTH, SWATCH_RADIUS, SWATCH_SIZE,
};
use crate::design_system::theme::tokens;

/// A single-line field. The kit renders the field, not the editor: hosts drive
/// text through their own input state and pass the display value in, which is
/// how the settings window already works.
#[derive(IntoElement)]
pub struct TextField {
    value: Option<SharedString>,
    placeholder: Option<SharedString>,
    /// Draws the accent border and a caret, matching a focused GPUI input.
    focused: bool,
    invalid: bool,
    swatch: Option<Rgba>,
    leading: Option<AnyElement>,
    /// Right-aligned annotation such as `OVERRIDE` or `KEYCHAIN`.
    trailing_label: Option<SharedString>,
    trailing: Option<AnyElement>,
    width: Pixels,
    masked: bool,
}

impl TextField {
    pub fn new() -> Self {
        Self {
            value: None,
            placeholder: None,
            focused: false,
            invalid: false,
            swatch: None,
            leading: None,
            trailing_label: None,
            trailing: None,
            width: CONTROL_WIDTH,
            masked: false,
        }
    }

    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Color chip in the leading slot, used by the Colors section.
    pub fn swatch(mut self, color: Rgba) -> Self {
        self.swatch = Some(color);
        self
    }

    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
        self
    }

    pub fn trailing_label(mut self, label: impl Into<SharedString>) -> Self {
        self.trailing_label = Some(label.into());
        self
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// Renders bullets instead of the value, for secrets held in the keychain.
    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }
}

impl Default for TextField {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for TextField {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let border = if self.invalid {
            theme.danger
        } else if self.focused {
            theme.accent
        } else {
            theme.border
        };
        let background = if self.invalid {
            theme.status_surface(crate::design_system::theme::Tone::Danger)
        } else {
            theme.bg_input
        };

        let (text, text_color) = match (&self.value, &self.placeholder) {
            (Some(value), _) if self.masked => (
                SharedString::from("•".repeat(value.chars().count().clamp(6, 12))),
                theme.text_muted,
            ),
            (Some(value), _) => (value.clone(), theme.text_primary),
            (None, Some(placeholder)) => (placeholder.clone(), theme.text_muted),
            (None, None) => (SharedString::default(), theme.text_muted),
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(9.0))
            .w(self.width)
            .h(CONTROL_HEIGHT)
            .pl(if self.swatch.is_some() {
                px(7.0)
            } else {
                CONTROL_TEXT_PADDING
            })
            .pr(CONTROL_INNER_PADDING)
            .rounded(CONTROL_RADIUS)
            .bg(background)
            .border_1()
            .border_color(border)
            .children(self.leading)
            .children(self.swatch.map(|color| {
                div()
                    .size(SWATCH_SIZE)
                    .flex_none()
                    .rounded(SWATCH_RADIUS)
                    .bg(color)
            }))
            .child(
                div()
                    .flex_grow()
                    .min_w(px(0.0))
                    .text_size(BODY_SIZE)
                    .text_color(text_color)
                    .truncate()
                    .child(text),
            )
            // Caret sits after the value the way a real cursor would, so a
            // focused field reads as focused in a static mock too.
            .children(
                self.focused
                    .then(|| div().flex_none().w(px(1.5)).h(px(15.0)).bg(theme.accent)),
            )
            .children(self.trailing_label.map(|label| {
                div()
                    .flex_none()
                    .text_size(BADGE_SIZE)
                    .text_color(theme.text_muted)
                    .child(label)
            }))
            .children(self.trailing)
    }
}
