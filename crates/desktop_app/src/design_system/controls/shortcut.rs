use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::design_system::controls::ClickHandler;
use crate::design_system::metrics::{
    CAPTION_SIZE, CONTROL_HEIGHT, CONTROL_RADIUS, CONTROL_TEXT_PADDING, CONTROL_WIDTH,
    KEY_CHIP_GAP, KEY_CHIP_HEIGHT, KEY_CHIP_PADDING_X, KEY_CHIP_RADIUS, LABEL_SIZE,
};
use crate::design_system::theme::{Tone, tokens};

/// Which modifier names a trigger's `secondary` and `alt` resolve to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Platform {
    #[default]
    Mac,
    Other,
}

impl Platform {
    /// The platform this binary is running on.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Other
        }
    }
}

/// Turn one canonical trigger chord — `secondary-alt-i` — into display keys.
///
/// Mirrors `SettingsWindow::display_trigger_for_os` in the desktop app. Chorded
/// triggers are whitespace separated upstream; call this once per chord.
pub fn format_binding(trigger: &str, platform: Platform) -> Vec<SharedString> {
    if trigger.is_empty() {
        return Vec::new();
    }

    // `secondary--` binds the minus key: a trailing separator is the key, not a
    // delimiter, so peel it off before splitting.
    let (body, trailing_key) = match trigger.strip_suffix('-') {
        Some(body) => (body, Some("-")),
        None => (trigger, None),
    };

    let mut parts = body
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    let key = match trailing_key {
        Some(key) => key,
        None => match parts.pop() {
            Some(key) => key,
            None => return Vec::new(),
        },
    };

    parts
        .into_iter()
        .map(|modifier| modifier_label(modifier, platform))
        .chain(std::iter::once(key_label(key)))
        .map(SharedString::from)
        .collect()
}

fn modifier_label(modifier: &str, platform: Platform) -> String {
    match modifier {
        "secondary" => match platform {
            Platform::Mac => "CMD".to_string(),
            Platform::Other => "CTRL".to_string(),
        },
        "ctrl" => "CTRL".to_string(),
        "alt" => match platform {
            Platform::Mac => "OPT".to_string(),
            Platform::Other => "ALT".to_string(),
        },
        "shift" => "SHIFT".to_string(),
        "cmd" => "CMD".to_string(),
        "fn" => "FN".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

fn key_label(key: &str) -> String {
    match key {
        "escape" => "ESC".to_string(),
        "pageup" => "PAGE UP".to_string(),
        "pagedown" => "PAGE DOWN".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

/// A single keycap.
#[derive(IntoElement)]
pub struct KeyChip {
    label: SharedString,
}

impl KeyChip {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl RenderOnce for KeyChip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);

        div()
            .flex()
            .flex_none()
            .items_center()
            .h(KEY_CHIP_HEIGHT)
            .px(KEY_CHIP_PADDING_X)
            .rounded(KEY_CHIP_RADIUS)
            .bg(theme.bg_hover)
            .text_size(LABEL_SIZE)
            .text_color(theme.text_primary)
            .child(self.label)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ShortcutState {
    #[default]
    Bound,
    /// Listening for the next key combo.
    Capturing,
    /// The command has no binding on this platform.
    Unbound,
    /// The pressed combo already belongs to another command.
    Conflict,
}

/// The shortcut lane of a keybinding row, in all four capture states.
#[derive(IntoElement)]
pub struct ShortcutBox {
    id: ElementId,
    keys: Vec<SharedString>,
    state: ShortcutState,
    width: Pixels,
    on_click: Option<ClickHandler>,
}

impl ShortcutBox {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            keys: Vec::new(),
            state: ShortcutState::default(),
            width: CONTROL_WIDTH,
            on_click: None,
        }
    }

    /// Display keys, usually from [`format_binding`].
    pub fn keys(mut self, keys: impl IntoIterator<Item = SharedString>) -> Self {
        self.keys = keys.into_iter().collect();
        self
    }

    pub fn state(mut self, state: ShortcutState) -> Self {
        self.state = state;
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
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

impl RenderOnce for ShortcutBox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = tokens(cx);

        let mut container = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(KEY_CHIP_GAP)
            .w(self.width)
            .h(CONTROL_HEIGHT)
            .px(crate::design_system::metrics::CONTROL_INNER_PADDING)
            .rounded(CONTROL_RADIUS)
            .cursor_pointer();

        container = match self.state {
            ShortcutState::Bound => container
                .justify_end()
                .bg(theme.bg_input)
                .border_1()
                .border_color(theme.border),
            ShortcutState::Capturing => container
                .justify_center()
                .gap(px(8.0))
                .bg(theme.bg_input)
                .border_1()
                .border_color(theme.accent),
            ShortcutState::Unbound => container
                .px(CONTROL_TEXT_PADDING)
                .gap(px(8.0))
                .bg(theme.bg_input)
                .border_1()
                .border_dashed()
                .border_color(theme.border),
            ShortcutState::Conflict => container
                .justify_end()
                .bg(theme.status_surface(Tone::Danger))
                .border_1()
                .border_color(theme.danger),
        };

        if let Some(handler) = self.on_click {
            container = container.on_click(move |event, window, cx| handler(event, window, cx));
        }

        match self.state {
            ShortcutState::Bound | ShortcutState::Conflict => {
                let last = self.keys.len().saturating_sub(1);
                container.children(self.keys.into_iter().enumerate().flat_map(|(index, key)| {
                    let separator = (index < last).then(|| {
                        div()
                            .flex_none()
                            .text_size(LABEL_SIZE)
                            .text_color(theme.text_muted)
                            .child("+")
                            .into_any_element()
                    });
                    std::iter::once(KeyChip::new(key).into_any_element()).chain(separator)
                }))
            }
            ShortcutState::Capturing => container
                .child(
                    div()
                        .size(px(6.0))
                        .flex_none()
                        .rounded_full()
                        .bg(theme.accent),
                )
                .child(
                    div()
                        .text_size(CAPTION_SIZE)
                        .text_color(theme.accent)
                        .child("Press shortcut…"),
                ),
            ShortcutState::Unbound => container
                .child(
                    div()
                        .flex_grow()
                        .min_w(px(0.0))
                        .text_size(CAPTION_SIZE)
                        .text_color(theme.text_muted)
                        .child("Unbound"),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(LABEL_SIZE)
                        .text_color(theme.accent)
                        .child("Set"),
                ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(trigger: &str, platform: Platform) -> Vec<String> {
        format_binding(trigger, platform)
            .into_iter()
            .map(|key| key.to_string())
            .collect()
    }

    #[test]
    fn secondary_and_alt_follow_the_platform() {
        assert_eq!(
            labels("secondary-alt-i", Platform::Mac),
            ["CMD", "OPT", "I"]
        );
        assert_eq!(
            labels("secondary-alt-i", Platform::Other),
            ["CTRL", "ALT", "I"]
        );
    }

    #[test]
    fn named_keys_get_their_display_spelling() {
        assert_eq!(labels("secondary-escape", Platform::Mac), ["CMD", "ESC"]);
        assert_eq!(labels("ctrl-pageup", Platform::Mac), ["CTRL", "PAGE UP"]);
        assert_eq!(
            labels("shift-pagedown", Platform::Mac),
            ["SHIFT", "PAGE DOWN"]
        );
        assert_eq!(labels("secondary-enter", Platform::Mac), ["CMD", "ENTER"]);
    }

    #[test]
    fn punctuation_and_bare_keys_survive() {
        assert_eq!(labels("secondary-,", Platform::Mac), ["CMD", ","]);
        assert_eq!(labels("secondary-=", Platform::Mac), ["CMD", "="]);
        assert_eq!(labels("f5", Platform::Mac), ["F5"]);
    }

    #[test]
    fn a_trailing_separator_is_the_minus_key() {
        // `secondary--` is Termy's zoom-out default.
        assert_eq!(labels("secondary--", Platform::Mac), ["CMD", "-"]);
        assert_eq!(labels("-", Platform::Mac), ["-"]);
    }

    #[test]
    fn an_empty_trigger_yields_no_keys() {
        assert!(format_binding("", Platform::Mac).is_empty());
    }
}
