//! The settings sidebar: search, group labels, and section items.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::controls::ClickHandler;
use crate::icon::{Icon, IconName};
use crate::metrics::{
    CAPTION_SIZE, CONTROL_HEIGHT, CONTROL_RADIUS, GROUP_TITLE_SIZE, LABEL_SIZE, SIDEBAR_GROUP_GAP,
    SIDEBAR_GROUP_LABEL_PADDING_BOTTOM, SIDEBAR_GROUP_LABEL_PADDING_X, SIDEBAR_ICON_GAP,
    SIDEBAR_ICON_SIZE, SIDEBAR_ITEM_GAP, SIDEBAR_ITEM_HEIGHT, SIDEBAR_ITEM_PADDING_X,
    SIDEBAR_ITEM_RADIUS, SIDEBAR_PADDING_X, SIDEBAR_PADDING_Y, SIDEBAR_SELECTED_ACCENT_INSET_Y,
    SIDEBAR_SELECTED_ACCENT_WIDTH, SIDEBAR_WIDTH,
};
use crate::theme::tokens;

/// Fixed 208px navigation rail. It never scrolls with the content column.
#[derive(IntoElement)]
pub struct Sidebar {
    children: Vec<AnyElement>,
    footer: Option<AnyElement>,
}

impl Sidebar {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            footer: None,
        }
    }

    /// Pinned to the bottom of the rail — the app puts the version there.
    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }
}

impl Default for Sidebar {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for Sidebar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Sidebar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(SIDEBAR_WIDTH)
            .h_full()
            .px(SIDEBAR_PADDING_X)
            .py(SIDEBAR_PADDING_Y)
            .gap(SIDEBAR_ITEM_GAP)
            .bg(theme.bg_panel)
            .border_r_1()
            .border_color(theme.row_separator)
            .children(self.children)
            .child(div().flex_grow())
            .children(self.footer.map(|footer| {
                div()
                    .px(SIDEBAR_ITEM_PADDING_X)
                    .pt(px(8.0))
                    .text_size(LABEL_SIZE)
                    .text_color(theme.text_muted)
                    .child(footer)
            }))
    }
}

/// Filter field at the top of the rail.
#[derive(IntoElement)]
pub struct SidebarSearch {
    query: Option<SharedString>,
    placeholder: SharedString,
    active: bool,
    trailing_label: Option<SharedString>,
}

impl SidebarSearch {
    pub fn new() -> Self {
        Self {
            query: None,
            placeholder: SharedString::from("Search settings"),
            active: false,
            trailing_label: None,
        }
    }

    pub fn query(mut self, query: impl Into<SharedString>) -> Self {
        self.query = Some(query.into());
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Result count, shown while filtering.
    pub fn trailing_label(mut self, label: impl Into<SharedString>) -> Self {
        self.trailing_label = Some(label.into());
        self
    }
}

impl Default for SidebarSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for SidebarSearch {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let (border, icon_color) = if self.active {
            (theme.accent, theme.accent)
        } else {
            (theme.card_border, theme.text_muted)
        };
        let (text, text_color) = match &self.query {
            Some(query) => (query.clone(), theme.text_primary),
            None => (self.placeholder.clone(), theme.text_muted),
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .h(CONTROL_HEIGHT)
            .px(px(9.0))
            .rounded(CONTROL_RADIUS)
            .bg(theme.bg_input)
            .border_1()
            .border_color(border)
            .child(Icon::new(IconName::Search).size(px(13.0)).color(icon_color))
            .child(
                div()
                    .flex_grow()
                    .min_w(px(0.0))
                    .text_size(CAPTION_SIZE)
                    .text_color(text_color)
                    .truncate()
                    .child(text),
            )
            .children(
                self.active
                    .then(|| div().flex_none().w(px(1.5)).h(px(14.0)).bg(theme.accent)),
            )
            .children(self.trailing_label.map(|label| {
                div()
                    .flex_none()
                    .text_size(LABEL_SIZE)
                    .text_color(theme.text_muted)
                    .child(label)
            }))
    }
}

/// Uppercase divider label — `INTERFACE`, `SESSION`, `SYSTEM`.
#[derive(IntoElement)]
pub struct SidebarGroupLabel {
    label: SharedString,
    first: bool,
}

impl SidebarGroupLabel {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            first: false,
        }
    }

    /// Drops the leading gap for the first group under the search field.
    pub fn first(mut self, first: bool) -> Self {
        self.first = first;
        self
    }
}

impl RenderOnce for SidebarGroupLabel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let top_gap = if self.first {
            px(12.0)
        } else {
            SIDEBAR_GROUP_GAP
        };

        div()
            .flex_none()
            .px(SIDEBAR_GROUP_LABEL_PADDING_X)
            .pt(top_gap)
            .pb(SIDEBAR_GROUP_LABEL_PADDING_BOTTOM)
            .text_size(GROUP_TITLE_SIZE)
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.text_muted)
            .child(self.label)
    }
}

/// One navigable section. Selected items take an accent wash plus a 2px accent
/// bar inset 8px top and bottom.
#[derive(IntoElement)]
pub struct SidebarItem {
    id: ElementId,
    label: SharedString,
    icon: Option<IconName>,
    selected: bool,
    on_click: Option<ClickHandler>,
}

impl SidebarItem {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            selected: false,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
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

impl RenderOnce for SidebarItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let (icon_color, label_color, weight) = if self.selected {
            (theme.accent, theme.text_primary, gpui::FontWeight::MEDIUM)
        } else {
            (
                theme.text_secondary,
                theme.text_secondary,
                gpui::FontWeight::NORMAL,
            )
        };

        let mut item = div()
            .id(self.id)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(SIDEBAR_ICON_GAP)
            .h(SIDEBAR_ITEM_HEIGHT)
            .px(SIDEBAR_ITEM_PADDING_X)
            .rounded(SIDEBAR_ITEM_RADIUS)
            .cursor_pointer();

        if self.selected {
            item = item.bg(theme.accent_soft);
        } else {
            item = item.hover(move |style| style.bg(theme.bg_hover));
        }

        if let Some(handler) = self.on_click {
            item = item.on_click(move |event, window, cx| handler(event, window, cx));
        }

        item.children(self.selected.then(|| {
            div()
                .absolute()
                .left_0()
                .top(SIDEBAR_SELECTED_ACCENT_INSET_Y)
                .w(SIDEBAR_SELECTED_ACCENT_WIDTH)
                .h(SIDEBAR_ITEM_HEIGHT - SIDEBAR_SELECTED_ACCENT_INSET_Y * 2.0)
                .rounded(px(1.0))
                .bg(theme.accent)
        }))
        .children(
            self.icon
                .map(|icon| Icon::new(icon).size(SIDEBAR_ICON_SIZE).color(icon_color)),
        )
        .child(
            div()
                .flex_grow()
                .min_w(px(0.0))
                .text_size(crate::metrics::BODY_SIZE)
                .font_weight(weight)
                .text_color(label_color)
                .truncate()
                .child(self.label),
        )
    }
}
