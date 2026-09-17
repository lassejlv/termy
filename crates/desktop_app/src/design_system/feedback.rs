//! Status surfaces: badges, banners, toasts, and empty states.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, px,
};

use crate::design_system::icon::{Icon, IconName};
use crate::design_system::metrics::{
    BADGE_HEIGHT, BADGE_PADDING_X, BADGE_RADIUS, BADGE_SIZE, BODY_SIZE, CARD_RADIUS,
    LABEL_LINE_HEIGHT, LABEL_SIZE,
};
use crate::design_system::theme::{Tone, tokens};

/// Small state label: `SAVED`, `ENABLED`, `INVALID`, or a live status pill.
#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    tone: Tone,
    dot: bool,
}

impl Badge {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            tone: Tone::Accent,
            dot: false,
        }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Grows the badge into a status pill with a leading dot — used for live
    /// runtime state such as the plugin runtime's `Ready`.
    pub fn dot(mut self) -> Self {
        self.dot = true;
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let color = theme.tone_color(self.tone);
        let background = match self.tone {
            Tone::Accent => theme.accent_soft,
            Tone::Neutral => theme.bg_input,
            tone => theme.status_surface(tone),
        };
        let label_color = if self.tone == Tone::Neutral {
            theme.text_muted
        } else {
            color
        };

        let mut badge = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .bg(background)
            .text_color(label_color);

        if self.dot {
            badge = badge
                .h(px(22.0))
                .px(px(9.0))
                .rounded(px(11.0))
                .text_size(LABEL_SIZE)
                .child(div().size(px(6.0)).flex_none().rounded_full().bg(color));
        } else {
            badge = badge
                .h(BADGE_HEIGHT)
                .px(BADGE_PADDING_X)
                .rounded(BADGE_RADIUS)
                .text_size(BADGE_SIZE);
        }

        badge.child(self.label)
    }
}

/// A wide status block above the content — runtime health, a failed registry
/// sync, a cached-data warning.
#[derive(IntoElement)]
pub struct Banner {
    title: SharedString,
    body: Option<SharedString>,
    tone: Tone,
    icon: Option<IconName>,
    actions: Vec<AnyElement>,
    trailing: Vec<AnyElement>,
}

impl Banner {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            body: None,
            tone: Tone::Accent,
            icon: None,
            actions: Vec::new(),
            trailing: Vec::new(),
        }
    }

    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Buttons under the copy.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    /// Elements pinned to the right of the title row, such as a status pill.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }
}

impl RenderOnce for Banner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let neutral = matches!(self.tone, Tone::Accent | Tone::Neutral);
        let (background, border) = if neutral {
            (theme.bg_card, theme.card_border)
        } else {
            (
                theme.status_surface(self.tone),
                theme.status_border(self.tone),
            )
        };
        let icon_color = theme.tone_color(self.tone);

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(20.0))
            .rounded(CARD_RADIUS)
            .bg(background)
            .border_1()
            .border_color(border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .flex_grow()
                            .min_w(px(0.0))
                            .children(
                                self.icon
                                    .map(|icon| Icon::new(icon).size(px(16.0)).color(icon_color)),
                            )
                            .child(
                                div()
                                    .text_size(BODY_SIZE)
                                    .text_color(theme.text_primary)
                                    .child(self.title),
                            ),
                    )
                    .children(self.trailing),
            )
            .children(self.body.map(|body| {
                div()
                    .text_size(LABEL_SIZE)
                    .line_height(px(17.0))
                    .text_color(theme.text_muted)
                    .child(body)
            }))
            .children((!self.actions.is_empty()).then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .children(self.actions)
            }))
    }
}

/// Transient confirmation, mirroring the app's toast copy.
#[derive(IntoElement)]
pub struct Toast {
    message: SharedString,
    tone: Tone,
    icon: Option<IconName>,
}

impl Toast {
    pub fn new(message: impl Into<SharedString>) -> Self {
        Self {
            message: message.into(),
            tone: Tone::Neutral,
            icon: None,
        }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }
}

impl RenderOnce for Toast {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let destructive = self.tone == Tone::Danger;
        let (background, border) = if destructive {
            (
                theme.status_surface(Tone::Danger),
                theme.status_border(Tone::Danger),
            )
        } else {
            (theme.bg_overlay, theme.border)
        };

        div()
            .flex()
            .items_center()
            .gap(px(9.0))
            .px(px(11.0))
            .py(px(9.0))
            .rounded(px(8.0))
            .bg(background)
            .border_1()
            .border_color(border)
            .children(self.icon.map(|icon| {
                Icon::new(icon)
                    .size(px(14.0))
                    .color(theme.tone_color(self.tone))
            }))
            .child(
                div()
                    .text_size(LABEL_SIZE)
                    .line_height(LABEL_LINE_HEIGHT)
                    .text_color(theme.text_secondary)
                    .child(self.message),
            )
    }
}

/// Placeholder for a list with nothing in it yet, or a search with no matches.
#[derive(IntoElement)]
pub struct EmptyState {
    title: SharedString,
    body: Option<SharedString>,
    icon: Option<IconName>,
    action: Option<AnyElement>,
}

impl EmptyState {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            body: None,
            icon: None,
            action: None,
        }
    }

    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.0))
            .px(px(20.0))
            .py(px(28.0))
            .rounded(CARD_RADIUS)
            .bg(theme.bg_card)
            .border_1()
            .border_dashed()
            .border_color(theme.card_border)
            .children(
                self.icon
                    .map(|icon| Icon::new(icon).size(px(26.0)).color(theme.text_muted)),
            )
            .child(
                div()
                    .text_size(BODY_SIZE)
                    .text_color(theme.text_primary)
                    .child(self.title),
            )
            .children(self.body.map(|body| {
                div()
                    .text_size(LABEL_SIZE)
                    .line_height(px(17.0))
                    .text_color(theme.text_muted)
                    .child(body)
            }))
            .children(self.action)
    }
}
