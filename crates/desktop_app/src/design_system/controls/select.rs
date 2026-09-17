use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    Rgba, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::design_system::controls::ClickHandler;
use crate::design_system::icon::{Icon, IconName};
use crate::design_system::metrics::{
    BODY_SIZE, CAPTION_SIZE, CONTROL_HEIGHT, CONTROL_INNER_PADDING, CONTROL_RADIUS,
    CONTROL_TEXT_PADDING, CONTROL_WIDTH,
};
use crate::design_system::theme::tokens;

/// The closed select: value on the left, chevron on the right, optional color
/// swatch for theme and palette rows. Open, it borrows the accent border and
/// flips the chevron; the popup itself is [`SelectMenu`].
#[derive(IntoElement)]
pub struct Select {
    id: ElementId,
    value: SharedString,
    swatch: Option<Rgba>,
    width: Pixels,
    open: bool,
    on_click: Option<ClickHandler>,
}

impl Select {
    pub fn new(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            swatch: None,
            width: CONTROL_WIDTH,
            open: false,
            on_click: None,
        }
    }

    pub fn swatch(mut self, color: Rgba) -> Self {
        self.swatch = Some(color);
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
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

impl RenderOnce for Select {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let (border, chevron_color, chevron) = if self.open {
            (theme.accent, theme.accent, IconName::ChevronUp)
        } else {
            (theme.border, theme.text_muted, IconName::ChevronDown)
        };

        let mut select = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .w(self.width)
            .h(CONTROL_HEIGHT)
            .pl(CONTROL_TEXT_PADDING)
            .pr(CONTROL_INNER_PADDING)
            .rounded(CONTROL_RADIUS)
            .bg(theme.bg_input)
            .border_1()
            .border_color(border)
            .cursor_pointer();

        if let Some(handler) = self.on_click {
            select = select.on_click(move |event, window, cx| handler(event, window, cx));
        }

        select
            .children(
                self.swatch
                    .map(|color| div().size(px(12.0)).flex_none().rounded(px(3.0)).bg(color)),
            )
            .child(
                div()
                    .flex_grow()
                    .min_w(px(0.0))
                    .text_size(BODY_SIZE)
                    .text_color(theme.text_primary)
                    .truncate()
                    .child(self.value),
            )
            .child(Icon::new(chevron).size(px(12.0)).color(chevron_color))
    }
}

/// One row of an open [`SelectMenu`].
pub struct SelectItem {
    label: SharedString,
    selected: bool,
    hovered: bool,
    leading: Option<gpui::AnyElement>,
}

impl SelectItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            selected: false,
            hovered: false,
            leading: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Pre-rendered hover, for previewing the state in a design surface.
    pub fn hovered(mut self, hovered: bool) -> Self {
        self.hovered = hovered;
        self
    }

    /// Small preview element in the fixed 14px leading lane — a cursor shape,
    /// a color chip, whatever the option is choosing between.
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
        self
    }
}

/// The popup list. Hosts position it themselves (usually directly under the
/// select) because anchoring belongs to the view that owns the overlay.
#[derive(IntoElement)]
pub struct SelectMenu {
    items: Vec<SelectItem>,
    width: Pixels,
}

impl SelectMenu {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            width: CONTROL_WIDTH,
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn item(mut self, item: SelectItem) -> Self {
        self.items.push(item);
        self
    }
}

impl Default for SelectMenu {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for SelectMenu {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);

        div()
            .flex()
            .flex_col()
            .w(self.width)
            .p(px(4.0))
            .rounded(px(8.0))
            .bg(theme.bg_overlay)
            .border_1()
            .border_color(theme.border)
            .children(self.items.into_iter().map(|item| {
                let mut row = div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(28.0))
                    .px(px(8.0))
                    .rounded(px(5.0));

                if item.selected {
                    row = row.bg(theme.accent_soft);
                } else if item.hovered {
                    row = row.bg(theme.bg_hover);
                }

                let label_color = if item.selected || item.hovered {
                    theme.text_primary
                } else {
                    theme.text_secondary
                };

                row.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .w(px(14.0))
                        .children(item.leading),
                )
                .child(
                    div()
                        .flex_grow()
                        .min_w(px(0.0))
                        .text_size(CAPTION_SIZE)
                        .text_color(label_color)
                        .truncate()
                        .child(item.label),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .w(px(12.0))
                        .children(item.selected.then(|| {
                            Icon::new(IconName::Check)
                                .size(px(12.0))
                                .color(theme.accent)
                        })),
                )
            }))
    }
}
