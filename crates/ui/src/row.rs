//! The setting row: three lanes, always in the same places.
//!
//! Label grows, control is a fixed 300px, reset is a fixed 20px — reserved even
//! when a row has nothing to reset, so glyphs and controls line up vertically
//! down a whole card.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, px,
};

use crate::feedback::Badge;
use crate::metrics::{
    BODY_SIZE, CARD_ROW_PADDING_X, CARD_ROW_PADDING_Y, CONTROL_WIDTH, LABEL_LINE_HEIGHT,
    LABEL_SIZE, RESET_SLOT_SIZE, ROW_LANE_GAP,
};
use crate::theme::{Tone, tokens};

/// Optional wash behind a row, for rows that are being edited or are invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RowTone {
    #[default]
    Default,
    Accent,
    Danger,
}

#[derive(IntoElement)]
pub struct SettingRow {
    label: SharedString,
    description: Option<SharedString>,
    badge: Option<Badge>,
    control: Option<AnyElement>,
    reset: Option<AnyElement>,
    error: Option<SharedString>,
    tone: RowTone,
}

impl SettingRow {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            description: None,
            badge: None,
            control: None,
            reset: None,
            error: None,
            tone: RowTone::default(),
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Inline state chip beside the label, such as `SAVED`.
    pub fn badge(mut self, badge: Badge) -> Self {
        self.badge = Some(badge);
        self
    }

    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.control = Some(control.into_any_element());
        self
    }

    /// Reset affordance. Rows without one still reserve the lane.
    pub fn reset(mut self, reset: impl IntoElement) -> Self {
        self.reset = Some(reset.into_any_element());
        self
    }

    /// Validation message under the control. Implies [`RowTone::Danger`].
    pub fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.error = Some(error.into());
        self.tone = RowTone::Danger;
        self
    }

    pub fn tone(mut self, tone: RowTone) -> Self {
        self.tone = tone;
        self
    }
}

impl RenderOnce for SettingRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let has_error = self.error.is_some();

        let mut row = div()
            .flex()
            .w_full()
            .gap(ROW_LANE_GAP)
            .px(CARD_ROW_PADDING_X)
            .py(CARD_ROW_PADDING_Y);

        // An error grows the control lane downward, so the lanes align to the
        // top and the label gets nudged onto the control's baseline.
        row = if has_error {
            row.items_start()
        } else {
            row.items_center()
        };

        row = match self.tone {
            RowTone::Default => row,
            RowTone::Accent => row.bg(theme.status_surface(Tone::Accent)),
            RowTone::Danger => row.bg(theme.status_surface(Tone::Danger)),
        };

        let mut label_lane = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .flex_grow()
            .min_w(px(0.0));

        if has_error {
            label_lane = label_lane.pt(px(6.0));
        }

        let label = div()
            .text_size(BODY_SIZE)
            .text_color(theme.text_primary)
            .child(self.label);

        label_lane = match self.badge {
            Some(badge) => label_lane.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(label)
                    .child(badge),
            ),
            None => label_lane.child(label),
        };

        label_lane = label_lane.children(self.description.map(|description| {
            div()
                .text_size(LABEL_SIZE)
                .line_height(LABEL_LINE_HEIGHT)
                .text_color(theme.text_muted)
                .child(description)
        }));

        let control_lane = match (self.control, self.error) {
            (Some(control), Some(error)) => Some(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(6.0))
                    .w(CONTROL_WIDTH)
                    .child(control)
                    .child(
                        div()
                            .text_size(LABEL_SIZE)
                            .line_height(LABEL_LINE_HEIGHT)
                            .text_color(theme.danger)
                            .child(error),
                    )
                    .into_any_element(),
            ),
            (Some(control), None) => Some(control),
            (None, _) => None,
        };

        row.child(label_lane).children(control_lane).child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(RESET_SLOT_SIZE)
                .h(RESET_SLOT_SIZE)
                .children(self.reset),
        )
    }
}
