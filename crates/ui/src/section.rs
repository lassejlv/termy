//! Content column, section headers, and the grouped cards inside them.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, px,
};

use crate::metrics::{
    CARD_GAP, CARD_RADIUS, CONTENT_GUTTER_X, CONTENT_GUTTER_Y, CONTENT_MAX_WIDTH, GROUP_LABEL_GAP,
    GROUP_TITLE_SIZE, LABEL_LINE_HEIGHT, SECTION_SUBTITLE_SIZE, SECTION_TITLE_SIZE,
};
use crate::theme::tokens;

/// The scrolling column to the right of the sidebar. Children are capped at
/// 800px; surplus window width becomes right slack rather than wider controls.
#[derive(IntoElement)]
pub struct SettingsContent {
    children: Vec<AnyElement>,
}

impl SettingsContent {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }
}

impl Default for SettingsContent {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for SettingsContent {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for SettingsContent {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_grow()
            .min_w(px(0.0))
            .gap(CARD_GAP)
            .px(CONTENT_GUTTER_X)
            .py(CONTENT_GUTTER_Y)
            .children(self.children.into_iter().map(|child| {
                div()
                    .w_full()
                    .max_w(CONTENT_MAX_WIDTH)
                    .min_w(px(0.0))
                    .child(child)
            }))
    }
}

/// Title, subtitle, and an optional trailing action (usually Reset section).
#[derive(IntoElement)]
pub struct SectionHeader {
    title: SharedString,
    subtitle: Option<SharedString>,
    action: Option<AnyElement>,
}

impl SectionHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            action: None,
        }
    }

    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);

        div()
            .flex()
            .items_end()
            .justify_between()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .flex_grow()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .text_size(SECTION_TITLE_SIZE)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(self.title),
                    )
                    .children(self.subtitle.map(|subtitle| {
                        div()
                            .text_size(SECTION_SUBTITLE_SIZE)
                            .line_height(px(20.0))
                            .text_color(theme.text_muted)
                            .child(subtitle)
                    })),
            )
            .children(self.action)
    }
}

/// A bare card. Use [`SettingsGroup`] for the usual label-plus-rows shape.
#[derive(IntoElement)]
pub struct Card {
    children: Vec<AnyElement>,
}

impl Card {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }
}

impl Default for Card {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for Card {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Card {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);

        div()
            .flex()
            .flex_col()
            .w_full()
            .rounded(CARD_RADIUS)
            .bg(theme.bg_card)
            .border_1()
            .border_color(theme.card_border)
            .overflow_hidden()
            .children(self.children)
    }
}

/// An uppercase group label above a card of rows. The group owns the hairlines
/// between rows so callers never have to remember which row is first.
#[derive(IntoElement)]
pub struct SettingsGroup {
    label: SharedString,
    trailing_label: Option<SharedString>,
    rows: Vec<AnyElement>,
}

impl SettingsGroup {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            trailing_label: None,
            rows: Vec::new(),
        }
    }

    /// Right-aligned note on the label line, e.g. `color0–color15`.
    pub fn trailing_label(mut self, label: impl Into<SharedString>) -> Self {
        self.trailing_label = Some(label.into());
        self
    }
}

impl ParentElement for SettingsGroup {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.rows.extend(elements);
    }
}

impl RenderOnce for SettingsGroup {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);

        let label_line = div()
            .flex()
            .items_baseline()
            .gap(px(10.0))
            .child(
                div()
                    .flex_grow()
                    .min_w(px(0.0))
                    .text_size(GROUP_TITLE_SIZE)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(self.label),
            )
            .children(self.trailing_label.map(|label| {
                div()
                    .flex_none()
                    .text_size(GROUP_TITLE_SIZE)
                    .line_height(LABEL_LINE_HEIGHT)
                    .text_color(theme.text_muted)
                    .child(label)
            }));

        let card = Card::new().children(self.rows.into_iter().enumerate().map(|(index, row)| {
            let mut wrapper = div().w_full();
            if index > 0 {
                wrapper = wrapper.border_t_1().border_color(theme.row_separator);
            }
            wrapper.child(row)
        }));

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(GROUP_LABEL_GAP)
            .child(label_line)
            .child(card)
    }
}
