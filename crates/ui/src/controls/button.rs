use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    Rgba, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::controls::ClickHandler;
use crate::icon::{Icon, IconName};
use crate::metrics::{
    BUTTON_HEIGHT, BUTTON_HEIGHT_SMALL, BUTTON_RADIUS, CAPTION_SIZE, LABEL_SIZE, RESET_ICON_SIZE,
    RESET_SLOT_SIZE,
};
use crate::theme::{Tokens, tokens, with_alpha};

/// How loud a button is. The settings surfaces use exactly these four:
/// one accent action per view, outlined secondaries, danger for destructive
/// work, ghost for the tertiary opt-out next to a retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    Primary,
    #[default]
    Secondary,
    Danger,
    Ghost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// 26px — inline row actions such as Edit, Disable, Uninstall.
    Small,
    /// 30px — toolbar and form actions.
    #[default]
    Medium,
}

impl ButtonSize {
    fn height(self) -> Pixels {
        match self {
            Self::Small => BUTTON_HEIGHT_SMALL,
            Self::Medium => BUTTON_HEIGHT,
        }
    }

    fn padding_x(self) -> Pixels {
        match self {
            Self::Small => px(10.0),
            Self::Medium => px(12.0),
        }
    }

    fn text_size(self) -> Pixels {
        match self {
            Self::Small => LABEL_SIZE,
            Self::Medium => CAPTION_SIZE,
        }
    }
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: ButtonVariant,
    size: ButtonSize,
    icon: Option<IconName>,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            variant: ButtonVariant::default(),
            size: ButtonSize::default(),
            icon: None,
            disabled: false,
            on_click: None,
        }
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A disabled button keeps its footprint and drops to 45% so the row does
    /// not reflow when the form becomes submittable.
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

    fn colors(&self, theme: &Tokens) -> ButtonColors {
        match self.variant {
            ButtonVariant::Primary => ButtonColors {
                background: Some(theme.accent),
                border: None,
                label: theme.text_on_accent,
                hover_background: Some(with_alpha(theme.accent, 0.88)),
            },
            ButtonVariant::Secondary => ButtonColors {
                background: None,
                border: Some(theme.border),
                label: theme.text_secondary,
                hover_background: Some(theme.bg_hover),
            },
            ButtonVariant::Danger => ButtonColors {
                background: None,
                border: Some(with_alpha(theme.danger, 0.4)),
                label: theme.danger,
                hover_background: Some(theme.status_surface(crate::theme::Tone::Danger)),
            },
            ButtonVariant::Ghost => ButtonColors {
                background: None,
                border: None,
                label: theme.text_muted,
                hover_background: Some(theme.bg_hover),
            },
        }
    }
}

struct ButtonColors {
    background: Option<Rgba>,
    border: Option<Rgba>,
    label: Rgba,
    hover_background: Option<Rgba>,
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let colors = self.colors(&theme);
        let weight = match self.variant {
            ButtonVariant::Primary => gpui::FontWeight::MEDIUM,
            _ => gpui::FontWeight::NORMAL,
        };

        let mut button = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(7.0))
            .h(self.size.height())
            .px(self.size.padding_x())
            .rounded(BUTTON_RADIUS)
            .text_size(self.size.text_size())
            .text_color(colors.label)
            .font_weight(weight);

        if let Some(background) = colors.background {
            button = button.bg(background);
        }
        if let Some(border) = colors.border {
            button = button.border_1().border_color(border);
        }

        if self.disabled {
            button = button.opacity(0.45);
        } else {
            button = button.cursor_pointer();
            if let Some(hover_background) = colors.hover_background {
                button = button.hover(move |style| style.bg(hover_background));
            }
            if let Some(handler) = self.on_click {
                button = button.on_click(move |event, window, cx| handler(event, window, cx));
            }
        }

        button
            .children(
                self.icon
                    .map(|icon| Icon::new(icon).size(px(13.0)).color(colors.label)),
            )
            .child(self.label)
    }
}

/// The bare glyph button used for a row's reset affordance. It occupies the
/// row's reserved 20px lane so rows without one still line up.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    color: Option<Rgba>,
    size: Pixels,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: IconName) -> Self {
        Self {
            id: id.into(),
            icon,
            color: None,
            size: RESET_ICON_SIZE,
            on_click: None,
        }
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
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

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);
        let color = self.color.unwrap_or(theme.accent);

        let mut button = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(RESET_SLOT_SIZE)
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(move |style| style.bg(theme.bg_hover));

        if let Some(handler) = self.on_click {
            button = button.on_click(move |event, window, cx| handler(event, window, cx));
        }

        button.child(Icon::new(self.icon).size(self.size).color(color))
    }
}
