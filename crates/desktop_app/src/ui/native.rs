//! Native macOS vocabulary for Termy's chrome.
//!
//! Chrome borrows everything it can from AppKit so Termy reads as if it
//! shipped with the OS: semantic label and fill colors, the system blue
//! accent, `NSSwitch`, and one shared set of motion curves. The terminal
//! grid keeps the user's theme; only the chrome around it is native.
//!
//! The neutral ladder is AppKit's: white at fixed alphas over a dark
//! ground, black over a light one. That way every surface stays legible on
//! whatever theme background sits underneath.

use gpui_kit::{
    App, BoxShadow, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, Rgba, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    base::{Transition, transition},
    div, point,
    prelude::FluentBuilder as _,
    px,
};
use std::rc::Rc;
use std::time::Duration;

// MARK: Motion

/// A cubic Bézier timing curve, as in `CAMediaTimingFunction`. `y` values
/// above 1 overshoot the target and settle back, which reads as a spring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Curve {
    pub(crate) x1: f32,
    pub(crate) y1: f32,
    pub(crate) x2: f32,
    pub(crate) y2: f32,
}

/// A duration paired with its curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Motion {
    pub(crate) duration: Duration,
    pub(crate) curve: Curve,
}

const fn motion(millis: u64, x1: f32, y1: f32, x2: f32, y2: f32) -> Motion {
    Motion {
        duration: Duration::from_millis(millis),
        curve: Curve { x1, y1, x2, y2 },
    }
}

/// A popover-like surface (command palette) arriving: grows from
/// [`POPOVER_SCALE`] with a hint of overshoot, the way `NSPopover` does.
pub(crate) const CARD_IN: Motion = motion(220, 0.25, 1.04, 0.3, 1.0);
/// The scale a popover grows from as it appears.
#[allow(dead_code)]
pub(crate) const POPOVER_SCALE: f32 = 0.96;
/// A selection gliding between neighbors (active tab capsule, palette row):
/// a plain ease-out, so quick sweeps never wobble.
#[allow(dead_code)]
pub(crate) const SELECTION_MOVE: Motion = motion(200, 0.25, 0.8, 0.25, 1.0);
/// A switch's thumb sliding across, settling with a hint of spring.
pub(crate) const SWITCH: Motion = motion(260, 0.3, 1.12, 0.4, 1.0);
/// A page or window fading in.
#[allow(dead_code)]
pub(crate) const PAGE_IN: Motion = motion(180, 0.25, 0.1, 0.25, 1.0);

/// Samples `curve` at `t` (0–1).
pub(crate) fn sample(curve: Curve, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    // Solve x(s) = t for s by Newton's method, then return y(s).
    let bezier = |a: f32, b: f32, s: f32| {
        let inv = 1.0 - s;
        3.0 * inv * inv * s * a + 3.0 * inv * s * s * b + s * s * s
    };
    let slope = |a: f32, b: f32, s: f32| {
        let inv = 1.0 - s;
        3.0 * inv * inv * a + 6.0 * inv * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    let mut s = t;
    for _ in 0..8 {
        let error = bezier(curve.x1, curve.x2, s) - t;
        let d = slope(curve.x1, curve.x2, s);
        if error.abs() < 1e-5 || d.abs() < 1e-6 {
            break;
        }
        s = (s - error / d).clamp(0.0, 1.0);
    }
    bezier(curve.y1, curve.y2, s)
}

/// A [`Transition`] policy driven by `motion`.
pub(crate) fn transition_policy(motion: Motion) -> Transition {
    Transition::new(motion.duration).ease(move |t| sample(motion.curve, t))
}

// MARK: Color

/// AppKit semantic colors for one appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NativePalette {
    pub(crate) dark: bool,
    /// `labelColor`
    pub(crate) label: Rgba,
    /// `secondaryLabelColor`
    pub(crate) secondary: Rgba,
    /// `tertiaryLabelColor`
    pub(crate) tertiary: Rgba,
    /// Hairline around floating panels, buttons and keycaps.
    pub(crate) stroke: Rgba,
    /// `separatorColor`
    pub(crate) separator: Rgba,
    /// Track behind gauges and sliders; recessed tab track.
    pub(crate) track: Rgba,
    /// Hover highlight, placeholder tiles, icon backgrounds.
    pub(crate) fill: Rgba,
    /// Background of a grouped form section.
    pub(crate) group: Rgba,
    /// Face of push buttons and keycaps.
    pub(crate) keycap: Rgba,
    /// Selected segment of a segmented control; the active tab capsule.
    pub(crate) segment: Rgba,
    /// A switch's track when off; on is `blue`.
    pub(crate) switch_off: Rgba,
    /// Text and glyphs on an accent fill.
    pub(crate) on_accent: Rgba,
    pub(crate) blue: Rgba,
    pub(crate) green: Rgba,
    pub(crate) orange: Rgba,
    pub(crate) red: Rgba,
    pub(crate) purple: Rgba,
    pub(crate) gray: Rgba,
}

const fn color(value: u32) -> Rgba {
    Rgba {
        r: ((value >> 24) & 0xff) as f32 / 255.0,
        g: ((value >> 16) & 0xff) as f32 / 255.0,
        b: ((value >> 8) & 0xff) as f32 / 255.0,
        a: (value & 0xff) as f32 / 255.0,
    }
}

impl NativePalette {
    pub(crate) const DARK: Self = Self {
        dark: true,
        label: color(0xffffffd9),
        secondary: color(0xffffff8c),
        tertiary: color(0xffffff40),
        stroke: color(0xffffff26),
        separator: color(0xffffff1a),
        track: color(0xffffff1a),
        fill: color(0xffffff14),
        group: color(0xffffff0d),
        keycap: color(0xffffff1f),
        segment: color(0xffffff2e),
        switch_off: color(0xffffff19),
        on_accent: color(0xffffffff),
        blue: color(0x0a84ffff),
        green: color(0x32d74bff),
        orange: color(0xff9f0aff),
        red: color(0xff453aff),
        purple: color(0x8e7cffff),
        gray: color(0x8e8e93ff),
    };

    pub(crate) const LIGHT: Self = Self {
        dark: false,
        label: color(0x000000d9),
        secondary: color(0x00000080),
        tertiary: color(0x00000042),
        stroke: color(0x0000001f),
        separator: color(0x0000001a),
        track: color(0x0000000f),
        fill: color(0x0000000d),
        group: color(0xffffffb3),
        keycap: color(0xffffffff),
        segment: color(0xffffffff),
        switch_off: color(0x00000019),
        on_accent: color(0xffffffff),
        blue: color(0x007affff),
        green: color(0x28cd41ff),
        orange: color(0xff9500ff),
        red: color(0xff3b30ff),
        purple: color(0x8e7cffff),
        gray: color(0x8e8e93ff),
    };

    /// The palette that reads on `background`: dark when the ground is dark.
    pub(crate) fn for_background(background: Rgba) -> Self {
        // Rec. 709 luma is enough to pick between the two appearances.
        let luma = 0.2126 * background.r + 0.7152 * background.g + 0.0722 * background.b;
        if luma < 0.5 { Self::DARK } else { Self::LIGHT }
    }

    /// Strengthens secondary text and edges, as Increase Contrast does.
    pub(crate) fn with_increased_contrast(mut self, enabled: bool) -> Self {
        if enabled {
            self.secondary = with_alpha(self.label, 0.8);
            self.tertiary = with_alpha(self.label, 0.6);
            self.stroke = with_alpha(self.label, 0.45);
            self.separator = with_alpha(self.label, 0.3);
        }
        self
    }

    /// The selected-row fill behind `on_accent` text.
    pub(crate) fn selection(&self) -> Rgba {
        self.blue
    }

    /// The highlight behind a keyboard-focused row that is not selected.
    #[allow(dead_code)]
    pub(crate) fn hover(&self) -> Rgba {
        self.fill
    }

    /// The soft drop shadow under a floating surface.
    pub(crate) fn popover_shadow(&self) -> Vec<BoxShadow> {
        let alpha = if self.dark { 1.0 } else { 0.45 };
        vec![
            shadow(0.5 * alpha, 24.0, 64.0),
            shadow(0.28 * alpha, 6.0, 18.0),
        ]
    }

    /// The small shadow that lifts a capsule (thumb, active tab) off its track.
    pub(crate) fn lift_shadow(&self) -> Vec<BoxShadow> {
        vec![shadow(0.18, 0.5, 2.0), shadow(0.06, 0.5, 2.5)]
    }
}

pub(crate) fn with_alpha(color: Rgba, alpha: f32) -> Rgba {
    Rgba {
        a: alpha.clamp(0.0, 1.0),
        ..color
    }
}

fn shadow(alpha: f32, y: f32, blur: f32) -> BoxShadow {
    BoxShadow {
        color: gpui_kit::black().opacity(alpha),
        offset: point(px(0.0), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.0),
        inset: false,
    }
}

/// System colors for icon tiles, the way System Settings tints its rows.
pub(crate) fn tile_gradient(color: Rgba) -> gpui_kit::Background {
    let top = Rgba {
        r: (color.r + (1.0 - color.r) * 0.18).min(1.0),
        g: (color.g + (1.0 - color.g) * 0.18).min(1.0),
        b: (color.b + (1.0 - color.b) * 0.18).min(1.0),
        a: color.a,
    };
    let bottom = Rgba {
        r: color.r * 0.86,
        g: color.g * 0.86,
        b: color.b * 0.86,
        a: color.a,
    };
    gpui_kit::linear_gradient(
        180.0,
        gpui_kit::linear_color_stop(top, 0.0),
        gpui_kit::linear_color_stop(bottom, 1.0),
    )
}

// MARK: Switch

/// The track, in points, as `NSSwitch` draws it at its small control size.
const SWITCH_TRACK: (f32, f32) = (44.0, 20.0);
/// The thumb at rest, 2pt in from every edge of the track.
const SWITCH_THUMB: (f32, f32) = (26.0, 16.0);
const SWITCH_INSET: f32 = 2.0;
/// How much wider the thumb gets while pressed, stretching toward the middle.
const SWITCH_STRETCH: f32 = 4.0;

type ChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A switch drawn like AppKit's `NSSwitch`, as System Settings uses in its
/// forms: a capsule track, a white capsule thumb with a soft shadow, and the
/// accent color when on. Pressing stretches the thumb; flipping slides it on
/// the [`SWITCH`] curve.
#[derive(IntoElement)]
pub(crate) struct MacSwitch {
    id: ElementId,
    checked: bool,
    disabled: bool,
    palette: NativePalette,
    label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
}

/// A switch, off and enabled until told otherwise.
pub(crate) fn mac_switch(id: impl Into<ElementId>, palette: &NativePalette) -> MacSwitch {
    MacSwitch {
        id: id.into(),
        checked: false,
        disabled: false,
        palette: *palette,
        label: None,
        on_change: None,
    }
}

impl MacSwitch {
    pub(crate) fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    #[allow(dead_code)]
    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// What VoiceOver reads when the row doesn't name the switch.
    #[allow(dead_code)]
    pub(crate) fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Called with the value the user switched to.
    pub(crate) fn on_change(
        mut self,
        handler: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for MacSwitch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = self.palette;
        let group: SharedString = SharedString::from(format!("mac-switch:{}", self.id));
        let target = if self.checked { 1.0 } else { 0.0 };
        // How far on the switch is, 0–1; it overshoots a little on the way.
        // `transition` snaps when Reduce Motion is on.
        let on: f32 = transition(
            ElementId::from((self.id.clone(), "thumb")),
            target,
            transition_policy(SWITCH),
            window,
            cx,
        );
        let travel = SWITCH_TRACK.0 - SWITCH_THUMB.0 - SWITCH_INSET * 2.0;
        let left = SWITCH_INSET + travel * on;
        let pressed_left = if self.checked {
            left - SWITCH_STRETCH
        } else {
            left
        };

        let thumb = div()
            .id("thumb")
            .absolute()
            .top(px(SWITCH_INSET))
            .left(px(left))
            .w(px(SWITCH_THUMB.0))
            .h(px(SWITCH_THUMB.1))
            .rounded_full()
            .bg(gpui_kit::white())
            .shadow(palette.lift_shadow())
            .when(!self.disabled, |thumb| {
                thumb.group_active(group.clone(), move |style| {
                    style
                        .w(px(SWITCH_THUMB.0 + SWITCH_STRETCH))
                        .left(px(pressed_left))
                })
            });
        let track = div()
            .relative()
            .size_full()
            .rounded_full()
            .bg(palette.switch_off)
            // The on color fades in over the off track as the thumb travels.
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .bg(palette.blue)
                    .opacity(on.clamp(0.0, 1.0)),
            )
            .child(thumb);

        let on_change = self.on_change;
        gpui_kit::base::Switch::new(self.id)
            .checked(self.checked)
            .disabled(self.disabled)
            .when_some(self.label, |switch, label| {
                switch.accessibility_label(label)
            })
            .when_some(on_change, |switch, on_change| {
                switch.on_change(move |checked, _, window, cx| on_change(checked, window, cx))
            })
            .group(group)
            .flex_shrink_0()
            .cursor_pointer()
            .w(px(SWITCH_TRACK.0))
            .h(px(SWITCH_TRACK.1))
            .when(self.disabled, |switch| switch.opacity(0.5))
            .child(track)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_start_at_zero_and_end_at_one() {
        for motion in [CARD_IN, SELECTION_MOVE, SWITCH, PAGE_IN] {
            assert!(sample(motion.curve, 0.0).abs() < 1e-4);
            assert!((sample(motion.curve, 1.0) - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn switch_curve_overshoots_like_a_spring() {
        let peak = (0..=100)
            .map(|step| sample(SWITCH.curve, step as f32 / 100.0))
            .fold(0.0_f32, f32::max);
        assert!(peak > 1.0);
    }

    #[test]
    fn palette_follows_background_luminance() {
        let dark_bg = color(0x1a1b26ff);
        let light_bg = color(0xfafafaff);
        assert!(NativePalette::for_background(dark_bg).dark);
        assert!(!NativePalette::for_background(light_bg).dark);
    }

    #[test]
    fn increased_contrast_strengthens_secondary_text() {
        let normal = NativePalette::DARK;
        let strong = NativePalette::DARK.with_increased_contrast(true);
        assert!(strong.secondary.a > normal.secondary.a);
        assert!(strong.separator.a > normal.separator.a);
    }
}
