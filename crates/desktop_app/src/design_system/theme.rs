//! Color tokens, derived from the active terminal theme.
//!
//! The ladder mirrors `crates/desktop_app/src/settings_view/style.rs`: nothing
//! here is a fixed brand color, every value falls out of the terminal palette
//! so switching themes repaints the whole kit.
//!
//! Two representations coexist on purpose:
//!
//! - **Surfaces** (`bg_*`) are composited to opaque colors. Embedders paint the
//!   kit on a solid window ground, so a surface has to carry its own value.
//! - **Overlays** (borders, separators, hover, tints) keep their alpha, exactly
//!   as the app does, so they read correctly over whichever surface is beneath.

use gpui::{App, Global, Rgba, rgb};

/// Terminal colors a theme must provide for the kit to derive its tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub background: Rgba,
    pub foreground: Rgba,
    /// Drives the accent: selection fills, focus rings, primary buttons.
    pub cursor: Rgba,
    pub green: Rgba,
    pub yellow: Rgba,
    pub red: Rgba,
}

impl Palette {
    /// `tokyo-night-storm`, Termy's shipped default.
    pub fn tokyo_night_storm() -> Self {
        Self {
            background: rgb(0x1a1b26),
            foreground: rgb(0xc0caf5),
            cursor: rgb(0x7aa2f7),
            green: rgb(0x9ece6a),
            yellow: rgb(0xe0af68),
            red: rgb(0xf7768e),
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::tokyo_night_storm()
    }
}

/// Surface elevation, expressed as foreground bleed over the theme background.
/// The app reaches the same values by compositing translucent panels over the
/// desktop; opaque embedders need the composite baked in.
mod elevation {
    pub(super) const PANEL: f32 = 0.040;
    pub(super) const CARD: f32 = 0.078;
    pub(super) const INPUT: f32 = 0.023;
    pub(super) const HOVER: f32 = 0.110;
    /// Menus and popovers, which float above cards.
    pub(super) const OVERLAY: f32 = 0.033;
}

/// Alphas for tokens that stay translucent. These match `style.rs` directly.
mod alpha {
    pub(super) const TEXT_SECONDARY: f32 = 0.82;
    pub(super) const TEXT_MUTED: f32 = 0.68;
    pub(super) const BORDER: f32 = 0.24;
    pub(super) const CARD_BORDER: f32 = 0.14;
    pub(super) const ROW_SEPARATOR: f32 = 0.08;
    pub(super) const ACCENT_SOFT: f32 = 0.16;
    pub(super) const STATUS_SURFACE: f32 = 0.12;
    pub(super) const STATUS_BORDER: f32 = 0.38;
}

/// Every color the kit paints with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub bg_window: Rgba,
    pub bg_panel: Rgba,
    pub bg_card: Rgba,
    pub bg_input: Rgba,
    pub bg_hover: Rgba,
    pub bg_overlay: Rgba,

    pub border: Rgba,
    pub card_border: Rgba,
    pub row_separator: Rgba,

    pub text_primary: Rgba,
    pub text_secondary: Rgba,
    pub text_muted: Rgba,
    /// Text that sits on top of an accent fill.
    pub text_on_accent: Rgba,

    pub accent: Rgba,
    pub accent_soft: Rgba,

    pub success: Rgba,
    pub warning: Rgba,
    pub danger: Rgba,
}

impl Tokens {
    pub fn from_palette(palette: Palette) -> Self {
        let bg = palette.background;
        let fg = palette.foreground;
        let elevate = |amount: f32| composite(with_alpha(fg, amount), bg);

        Self {
            bg_window: opaque(bg),
            bg_panel: elevate(elevation::PANEL),
            bg_card: elevate(elevation::CARD),
            bg_input: elevate(elevation::INPUT),
            bg_hover: elevate(elevation::HOVER),
            bg_overlay: elevate(elevation::OVERLAY),

            border: with_alpha(fg, alpha::BORDER),
            card_border: with_alpha(fg, alpha::CARD_BORDER),
            row_separator: with_alpha(fg, alpha::ROW_SEPARATOR),

            text_primary: opaque(fg),
            text_secondary: with_alpha(fg, alpha::TEXT_SECONDARY),
            text_muted: with_alpha(fg, alpha::TEXT_MUTED),
            text_on_accent: opaque(bg),

            accent: opaque(palette.cursor),
            accent_soft: with_alpha(palette.cursor, alpha::ACCENT_SOFT),

            success: opaque(palette.green),
            warning: opaque(palette.yellow),
            danger: opaque(palette.red),
        }
    }

    /// Tokens for the shipped default theme.
    pub fn dark() -> Self {
        Self::from_palette(Palette::tokyo_night_storm())
    }

    /// Tinted fill behind a status row or banner.
    pub fn status_surface(self, tone: Tone) -> Rgba {
        with_alpha(self.tone_color(tone), alpha::STATUS_SURFACE)
    }

    /// Outline for a status surface.
    pub fn status_border(self, tone: Tone) -> Rgba {
        with_alpha(self.tone_color(tone), alpha::STATUS_BORDER)
    }

    pub fn tone_color(self, tone: Tone) -> Rgba {
        match tone {
            Tone::Accent => self.accent,
            Tone::Success => self.success,
            Tone::Warning => self.warning,
            Tone::Danger => self.danger,
            Tone::Neutral => self.text_muted,
        }
    }
}

impl Default for Tokens {
    fn default() -> Self {
        Self::dark()
    }
}

impl Global for Tokens {}

/// Semantic color roles shared by badges, banners, toasts, and buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Accent,
    Success,
    Warning,
    Danger,
    Neutral,
}

/// Install the tokens every component reads from.
pub fn set_tokens(tokens: Tokens, cx: &mut App) {
    cx.set_global(tokens);
}

/// Tokens for the current app, falling back to the default dark theme so a
/// component never panics in a host that forgot to install them.
pub fn tokens(cx: &App) -> Tokens {
    cx.try_global::<Tokens>().copied().unwrap_or_default()
}

/// Same color at a different alpha.
pub fn with_alpha(color: Rgba, alpha: f32) -> Rgba {
    Rgba {
        a: alpha.clamp(0.0, 1.0),
        ..color
    }
}

fn opaque(color: Rgba) -> Rgba {
    with_alpha(color, 1.0)
}

/// Flatten `fg` over `bg` into an opaque color.
fn composite(fg: Rgba, bg: Rgba) -> Rgba {
    let a = fg.a.clamp(0.0, 1.0);
    Rgba {
        r: bg.r + (fg.r - bg.r) * a,
        g: bg.g + (fg.g - bg.g) * a,
        b: bg.b + (fg.b - bg.b) * a,
        a: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The design boards specify opaque hexes for every surface. The ladder is
    /// a linear blend, so a couple of channels land a step or two off the hand
    /// picked value — anything inside this tolerance is invisible at 1x.
    const CHANNEL_TOLERANCE: i32 = 6;

    fn channels(color: Rgba) -> [i32; 3] {
        [
            (color.r * 255.0).round() as i32,
            (color.g * 255.0).round() as i32,
            (color.b * 255.0).round() as i32,
        ]
    }

    #[track_caller]
    fn assert_matches_design(actual: Rgba, expected_hex: u32) {
        let actual = channels(actual);
        let expected = channels(rgb(expected_hex));
        for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
            assert!(
                (actual - expected).abs() <= CHANNEL_TOLERANCE,
                "channel {index}: {actual} is more than {CHANNEL_TOLERANCE} off {expected} \
                 (expected #{expected_hex:06x})"
            );
        }
    }

    #[test]
    fn surfaces_match_the_design_boards() {
        let tokens = Tokens::dark();

        assert_matches_design(tokens.bg_window, 0x1a1b26);
        assert_matches_design(tokens.bg_panel, 0x20222f);
        assert_matches_design(tokens.bg_card, 0x24283b);
        assert_matches_design(tokens.bg_input, 0x1d1f2c);
        assert_matches_design(tokens.bg_hover, 0x2a2e40);
        assert_matches_design(tokens.bg_overlay, 0x1e2030);
    }

    #[test]
    fn overlays_keep_the_app_alpha_ladder() {
        let tokens = Tokens::dark();

        assert_eq!(tokens.text_secondary.a, 0.82);
        assert_eq!(tokens.text_muted.a, 0.68);
        assert_eq!(tokens.border.a, 0.24);
        assert_eq!(tokens.card_border.a, 0.14);
        assert_eq!(tokens.row_separator.a, 0.08);
        assert_eq!(tokens.accent_soft.a, 0.16);
    }

    #[test]
    fn surfaces_are_opaque_and_climb_in_order() {
        let tokens = Tokens::dark();
        let luminance = |color: Rgba| color.r + color.g + color.b;

        for surface in [
            tokens.bg_window,
            tokens.bg_panel,
            tokens.bg_card,
            tokens.bg_input,
            tokens.bg_hover,
            tokens.bg_overlay,
        ] {
            assert_eq!(surface.a, 1.0, "surfaces must be paintable on their own");
        }

        assert!(luminance(tokens.bg_window) < luminance(tokens.bg_input));
        assert!(luminance(tokens.bg_input) < luminance(tokens.bg_panel));
        assert!(luminance(tokens.bg_panel) < luminance(tokens.bg_card));
        assert!(luminance(tokens.bg_card) < luminance(tokens.bg_hover));
    }

    #[test]
    fn accent_follows_the_cursor_color() {
        let palette = Palette {
            cursor: rgb(0xff0000),
            ..Palette::tokyo_night_storm()
        };
        let tokens = Tokens::from_palette(palette);

        assert_eq!(channels(tokens.accent), channels(rgb(0xff0000)));
        assert_eq!(channels(tokens.accent_soft), channels(rgb(0xff0000)));
        assert_eq!(tokens.accent_soft.a, 0.16);
    }

    #[test]
    fn a_light_palette_inverts_the_elevation_direction() {
        let palette = Palette {
            background: rgb(0xffffff),
            foreground: rgb(0x24262b),
            ..Palette::tokyo_night_storm()
        };
        let tokens = Tokens::from_palette(palette);
        let luminance = |color: Rgba| color.r + color.g + color.b;

        // Elevation is a bleed toward the foreground, so on a light theme the
        // raised surfaces get darker rather than lighter.
        assert!(luminance(tokens.bg_card) < luminance(tokens.bg_window));
    }

    #[test]
    fn status_tints_stay_translucent() {
        let tokens = Tokens::dark();

        assert_eq!(tokens.status_surface(Tone::Danger).a, 0.12);
        assert_eq!(tokens.status_border(Tone::Danger).a, 0.38);
        assert_eq!(
            channels(tokens.status_surface(Tone::Danger)),
            channels(tokens.danger)
        );
        assert_eq!(
            channels(tokens.tone_color(Tone::Success)),
            channels(tokens.success)
        );
    }
}
