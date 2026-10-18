use super::constants::{MAX_PANE_FOCUS_STRENGTH, OVERLAY_PANEL_ALPHA_FLOOR_RATIO};
use super::tab_strip::constants::TAB_STROKE_FOREGROUND_MIX;
use crate::chrome_style::ChromeContrastProfile;
use crate::colors::TerminalColors;
#[cfg(test)]
use crate::config;
use crate::config::{AppConfig, PaneFocusEffect};
use gpui_kit::WindowBackgroundAppearance;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PaneFocusPreset {
    pub(super) inactive_fg_blend: f32,
    pub(super) inactive_bg_blend: f32,
    pub(super) inactive_desaturate: f32,
    pub(super) active_border_alpha: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum BackgroundPlatform {
    MacOs,
    Windows,
    /// Windows 11 22H2 or later, where DWM provides system backdrops.
    WindowsSystemBackdrop,
    Linux,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BackgroundSupportContext {
    platform: BackgroundPlatform,
    linux_wayland_session: bool,
}

impl BackgroundSupportContext {
    pub(super) fn current() -> Self {
        #[cfg(target_os = "macos")]
        let platform = BackgroundPlatform::MacOs;
        #[cfg(target_os = "windows")]
        let platform =
            if crate::native_sdk::win32_window::windows_build_number().is_some_and(|build| {
                build >= crate::native_sdk::win32_window::WINDOWS_11_SYSTEM_BACKDROP_BUILD
            }) {
                BackgroundPlatform::WindowsSystemBackdrop
            } else {
                BackgroundPlatform::Windows
            };
        #[cfg(target_os = "linux")]
        let platform = BackgroundPlatform::Linux;
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        let platform = BackgroundPlatform::Other;

        #[cfg(target_os = "linux")]
        let linux_wayland_session = std::env::var("XDG_SESSION_TYPE")
            .ok()
            .is_some_and(|session_type| session_type.eq_ignore_ascii_case("wayland"))
            || std::env::var_os("WAYLAND_DISPLAY").is_some();
        #[cfg(not(target_os = "linux"))]
        let linux_wayland_session = false;

        Self {
            platform,
            linux_wayland_session,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BlurFallbackReason {
    None,
    KnownUnsupported,
    UnknownSupport,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ResolvedBackgroundAppearance {
    pub(super) appearance: WindowBackgroundAppearance,
    pub(super) blur_fallback: BlurFallbackReason,
}

pub(super) fn resolve_background_appearance(
    background_opacity: f32,
    background_blur: bool,
    context: BackgroundSupportContext,
) -> ResolvedBackgroundAppearance {
    let opacity = background_opacity.clamp(0.0, 1.0);
    if opacity >= 1.0 {
        return ResolvedBackgroundAppearance {
            appearance: WindowBackgroundAppearance::Opaque,
            blur_fallback: BlurFallbackReason::None,
        };
    }

    if !background_blur {
        return ResolvedBackgroundAppearance {
            appearance: WindowBackgroundAppearance::Transparent,
            blur_fallback: BlurFallbackReason::None,
        };
    }

    match context.platform {
        BackgroundPlatform::MacOs | BackgroundPlatform::Windows => ResolvedBackgroundAppearance {
            appearance: WindowBackgroundAppearance::Blurred,
            blur_fallback: BlurFallbackReason::None,
        },
        // The DWM backdrop is smoother than the legacy acrylic accent (which
        // lags while the window is dragged). Mica Alt is the variant meant
        // for windows with tabs in the titlebar.
        BackgroundPlatform::WindowsSystemBackdrop => ResolvedBackgroundAppearance {
            appearance: WindowBackgroundAppearance::MicaAltBackdrop,
            blur_fallback: BlurFallbackReason::None,
        },
        BackgroundPlatform::Linux => {
            if context.linux_wayland_session {
                ResolvedBackgroundAppearance {
                    appearance: WindowBackgroundAppearance::Blurred,
                    blur_fallback: BlurFallbackReason::UnknownSupport,
                }
            } else {
                ResolvedBackgroundAppearance {
                    appearance: WindowBackgroundAppearance::Transparent,
                    blur_fallback: BlurFallbackReason::KnownUnsupported,
                }
            }
        }
        BackgroundPlatform::Other => ResolvedBackgroundAppearance {
            appearance: WindowBackgroundAppearance::Blurred,
            blur_fallback: BlurFallbackReason::UnknownSupport,
        },
    }
}

pub(super) fn background_opacity_factor(background_opacity: f32) -> f32 {
    background_opacity.clamp(0.0, 1.0)
}

pub(super) fn scaled_background_alpha_for_opacity(base_alpha: f32, background_opacity: f32) -> f32 {
    (base_alpha * background_opacity_factor(background_opacity)).clamp(0.0, 1.0)
}

pub(super) fn scaled_chrome_alpha_for_opacity(base_alpha: f32, background_opacity: f32) -> f32 {
    scaled_background_alpha_for_opacity(base_alpha, background_opacity)
}

fn adaptive_overlay_panel_alpha_for_opacity(base_alpha: f32, background_opacity: f32) -> f32 {
    let floor = base_alpha * OVERLAY_PANEL_ALPHA_FLOOR_RATIO;
    scaled_background_alpha_for_opacity(base_alpha, background_opacity)
        .max(floor)
        .clamp(0.0, 1.0)
}

fn adaptive_overlay_panel_alpha_with_floor_for_opacity(
    base_alpha: f32,
    background_opacity: f32,
    translucent_floor_alpha: f32,
) -> f32 {
    let alpha = adaptive_overlay_panel_alpha_for_opacity(base_alpha, background_opacity);
    if background_opacity_factor(background_opacity) < 1.0 {
        alpha.max(translucent_floor_alpha).clamp(0.0, 1.0)
    } else {
        alpha
    }
}

pub(super) fn blend_rgba(
    base: gpui_kit::Rgba,
    tint: gpui_kit::Rgba,
    tint_factor: f32,
) -> gpui_kit::Rgba {
    let tint_factor = tint_factor.clamp(0.0, 1.0);
    let base_factor = 1.0 - tint_factor;
    gpui_kit::Rgba {
        r: (base.r * base_factor) + (tint.r * tint_factor),
        g: (base.g * base_factor) + (tint.g * tint_factor),
        b: (base.b * base_factor) + (tint.b * tint_factor),
        a: (base.a * base_factor) + (tint.a * tint_factor),
    }
}

pub(super) fn resolve_chrome_stroke_color(
    chrome_background: gpui_kit::Rgba,
    foreground: gpui_kit::Rgba,
    foreground_mix: f32,
) -> gpui_kit::Rgba {
    let mix = foreground_mix.clamp(0.0, 1.0);
    let inv_mix = 1.0 - mix;

    gpui_kit::Rgba {
        r: (chrome_background.r * inv_mix) + (foreground.r * mix),
        g: (chrome_background.g * inv_mix) + (foreground.g * mix),
        b: (chrome_background.b * inv_mix) + (foreground.b * mix),
        a: 1.0,
    }
}

pub(super) fn pane_divider_color(
    chrome_background: gpui_kit::Rgba,
    foreground: gpui_kit::Rgba,
) -> gpui_kit::Rgba {
    resolve_chrome_stroke_color(chrome_background, foreground, TAB_STROKE_FOREGROUND_MIX)
}

pub(super) fn pane_focus_strength_factor(pane_focus_strength: f32) -> f32 {
    pane_focus_strength.clamp(0.0, MAX_PANE_FOCUS_STRENGTH)
}

pub(super) fn pane_focus_preset(effect: PaneFocusEffect) -> Option<PaneFocusPreset> {
    match effect {
        PaneFocusEffect::Off => None,
        PaneFocusEffect::SoftSpotlight => Some(PaneFocusPreset {
            inactive_fg_blend: 0.36,
            inactive_bg_blend: 0.12,
            inactive_desaturate: 0.0,
            active_border_alpha: 0.38,
        }),
        PaneFocusEffect::Cinematic => Some(PaneFocusPreset {
            inactive_fg_blend: 0.52,
            inactive_bg_blend: 0.18,
            inactive_desaturate: 0.34,
            active_border_alpha: 0.46,
        }),
        PaneFocusEffect::Minimal => Some(PaneFocusPreset {
            inactive_fg_blend: 0.22,
            inactive_bg_blend: 0.08,
            inactive_desaturate: 0.0,
            active_border_alpha: 0.28,
        }),
    }
}

#[derive(Clone, Copy)]
pub(super) struct OverlayStyleBuilder<'a> {
    colors: &'a TerminalColors,
    background_opacity: f32,
    contrast_profile: ChromeContrastProfile,
}

impl<'a> OverlayStyleBuilder<'a> {
    pub(super) fn new(
        colors: &'a TerminalColors,
        background_opacity: f32,
        contrast_profile: ChromeContrastProfile,
    ) -> Self {
        Self {
            colors,
            background_opacity,
            contrast_profile,
        }
    }

    pub(super) fn panel_background(self, base_alpha: f32) -> gpui_kit::Rgba {
        let alpha = adaptive_overlay_panel_alpha_for_opacity(base_alpha, self.background_opacity);
        self.with_alpha(self.colors.background, alpha)
    }

    pub(super) fn panel_cursor(self, base_alpha: f32) -> gpui_kit::Rgba {
        let alpha = adaptive_overlay_panel_alpha_for_opacity(base_alpha, self.background_opacity);
        self.with_alpha(self.colors.cursor, alpha)
    }

    pub(super) fn panel_foreground(self, base_alpha: f32) -> gpui_kit::Rgba {
        let alpha = adaptive_overlay_panel_alpha_for_opacity(base_alpha, self.background_opacity);
        self.with_alpha(self.colors.foreground, alpha)
    }

    pub(super) fn chrome_panel_background(self, base_alpha: f32) -> gpui_kit::Rgba {
        let alpha = adaptive_overlay_panel_alpha_for_opacity(
            self.contrast_profile.panel_surface_alpha(base_alpha),
            self.background_opacity,
        );
        self.with_alpha(self.colors.background, alpha)
    }

    pub(super) fn chrome_panel_background_with_floor(
        self,
        base_alpha: f32,
        translucent_floor_alpha: f32,
    ) -> gpui_kit::Rgba {
        let alpha = adaptive_overlay_panel_alpha_with_floor_for_opacity(
            self.contrast_profile.panel_surface_alpha(base_alpha),
            self.background_opacity,
            self.contrast_profile
                .panel_surface_alpha(translucent_floor_alpha),
        );
        self.with_alpha(self.colors.background, alpha)
    }

    pub(super) fn chrome_panel_cursor(self, base_alpha: f32) -> gpui_kit::Rgba {
        let alpha = adaptive_overlay_panel_alpha_for_opacity(
            self.contrast_profile.panel_accent_alpha(base_alpha),
            self.background_opacity,
        );
        self.with_alpha(self.colors.cursor, alpha)
    }

    pub(super) fn chrome_panel_neutral(self, base_alpha: f32) -> gpui_kit::Rgba {
        let alpha = adaptive_overlay_panel_alpha_for_opacity(
            self.contrast_profile.panel_neutral_alpha(base_alpha),
            self.background_opacity,
        );
        self.with_alpha(self.colors.foreground, alpha)
    }

    pub(super) fn transparent_background(self) -> gpui_kit::Rgba {
        self.with_alpha(self.colors.background, 0.0)
    }

    fn with_alpha(self, mut color: gpui_kit::Rgba, alpha: f32) -> gpui_kit::Rgba {
        color.a = alpha.clamp(0.0, 1.0);
        color
    }
}

pub(crate) fn initial_window_background_appearance(
    config: &AppConfig,
) -> WindowBackgroundAppearance {
    resolve_background_appearance(
        config.background_opacity,
        config.background_blur,
        BackgroundSupportContext::current(),
    )
    .appearance
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_background_appearance_is_opaque_when_opacity_is_full() {
        let resolved = resolve_background_appearance(
            1.0,
            true,
            BackgroundSupportContext {
                platform: BackgroundPlatform::MacOs,
                linux_wayland_session: false,
            },
        );
        assert_eq!(resolved.appearance, WindowBackgroundAppearance::Opaque);
        assert_eq!(resolved.blur_fallback, BlurFallbackReason::None);
    }

    #[test]
    fn resolve_background_appearance_is_transparent_without_blur() {
        let resolved = resolve_background_appearance(
            0.85,
            false,
            BackgroundSupportContext {
                platform: BackgroundPlatform::Windows,
                linux_wayland_session: false,
            },
        );
        assert_eq!(resolved.appearance, WindowBackgroundAppearance::Transparent);
        assert_eq!(resolved.blur_fallback, BlurFallbackReason::None);
    }

    #[test]
    fn resolve_background_appearance_blur_is_known_unsupported_on_linux_non_wayland() {
        let resolved = resolve_background_appearance(
            0.9,
            true,
            BackgroundSupportContext {
                platform: BackgroundPlatform::Linux,
                linux_wayland_session: false,
            },
        );
        assert_eq!(resolved.appearance, WindowBackgroundAppearance::Transparent);
        assert_eq!(resolved.blur_fallback, BlurFallbackReason::KnownUnsupported);
    }

    #[test]
    fn resolve_background_appearance_blur_is_unknown_on_linux_wayland() {
        let resolved = resolve_background_appearance(
            0.9,
            true,
            BackgroundSupportContext {
                platform: BackgroundPlatform::Linux,
                linux_wayland_session: true,
            },
        );
        assert_eq!(resolved.appearance, WindowBackgroundAppearance::Blurred);
        assert_eq!(resolved.blur_fallback, BlurFallbackReason::UnknownSupport);
    }

    #[test]
    fn resolve_background_appearance_blur_uses_mica_alt_on_windows_11() {
        let resolved = resolve_background_appearance(
            0.9,
            true,
            BackgroundSupportContext {
                platform: BackgroundPlatform::WindowsSystemBackdrop,
                linux_wayland_session: false,
            },
        );
        assert_eq!(
            resolved.appearance,
            WindowBackgroundAppearance::MicaAltBackdrop
        );
        assert_eq!(resolved.blur_fallback, BlurFallbackReason::None);
    }

    #[test]
    fn resolve_background_appearance_blur_keeps_acrylic_on_older_windows() {
        let resolved = resolve_background_appearance(
            0.9,
            true,
            BackgroundSupportContext {
                platform: BackgroundPlatform::Windows,
                linux_wayland_session: false,
            },
        );
        assert_eq!(resolved.appearance, WindowBackgroundAppearance::Blurred);
    }

    #[test]
    fn resolve_background_appearance_without_blur_skips_mica() {
        let resolved = resolve_background_appearance(
            0.9,
            false,
            BackgroundSupportContext {
                platform: BackgroundPlatform::WindowsSystemBackdrop,
                linux_wayland_session: false,
            },
        );
        assert_eq!(resolved.appearance, WindowBackgroundAppearance::Transparent);
    }

    #[test]
    fn resolve_background_appearance_blur_is_enabled_on_macos() {
        let resolved = resolve_background_appearance(
            0.9,
            true,
            BackgroundSupportContext {
                platform: BackgroundPlatform::MacOs,
                linux_wayland_session: false,
            },
        );
        assert_eq!(resolved.appearance, WindowBackgroundAppearance::Blurred);
        assert_eq!(resolved.blur_fallback, BlurFallbackReason::None);
    }

    #[test]
    fn chrome_alpha_scales_without_floor() {
        let base = 0.92;
        let alpha = scaled_chrome_alpha_for_opacity(base, 0.1);
        assert_eq!(alpha, base * 0.1);
    }

    #[test]
    fn overlay_panel_floor_applies_only_when_background_is_translucent() {
        let base = 0.64;
        let floor = 0.76;
        let translucent = adaptive_overlay_panel_alpha_with_floor_for_opacity(base, 0.2, floor);
        let opaque = adaptive_overlay_panel_alpha_with_floor_for_opacity(base, 1.0, floor);
        assert!(translucent >= floor);
        assert!(opaque < floor);
    }

    #[test]
    fn pane_divider_color_matches_shared_chrome_stroke_resolution() {
        let chrome_surface_bg = gpui_kit::Rgba {
            r: 0.04,
            g: 0.08,
            b: 0.13,
            a: 0.94,
        };
        let foreground = gpui_kit::Rgba {
            r: 0.82,
            g: 0.88,
            b: 0.93,
            a: 1.0,
        };

        assert_eq!(
            pane_divider_color(chrome_surface_bg, foreground),
            resolve_chrome_stroke_color(chrome_surface_bg, foreground, TAB_STROKE_FOREGROUND_MIX)
        );
    }

    #[test]
    fn pane_focus_preset_is_disabled_for_off() {
        assert!(pane_focus_preset(PaneFocusEffect::Off).is_none());
    }

    #[test]
    fn pane_focus_preset_strength_scales_monotonically() {
        let preset = pane_focus_preset(PaneFocusEffect::SoftSpotlight)
            .expect("soft spotlight preset should exist");
        let low_strength = pane_focus_strength_factor(0.2);
        let high_strength = pane_focus_strength_factor(0.8);

        assert!(
            (preset.inactive_fg_blend * high_strength) > (preset.inactive_fg_blend * low_strength)
        );
        assert!(
            (preset.inactive_bg_blend * high_strength) > (preset.inactive_bg_blend * low_strength)
        );
        assert!(
            (preset.active_border_alpha * high_strength)
                > (preset.active_border_alpha * low_strength)
        );
    }

    #[test]
    fn pane_focus_strength_factor_clamps_to_extended_upper_bound() {
        assert_eq!(pane_focus_strength_factor(2.5), MAX_PANE_FOCUS_STRENGTH);
    }

    #[test]
    fn resolve_background_appearance_uses_preview_opacity_during_drag() {
        let effective_opacity = config::effective_background_opacity(
            1.0,
            Some(config::BackgroundOpacityPreview {
                owner_id: 1,
                opacity: 0.4,
            }),
        );
        let resolved = resolve_background_appearance(
            effective_opacity,
            false,
            BackgroundSupportContext {
                platform: BackgroundPlatform::MacOs,
                linux_wayland_session: false,
            },
        );

        assert_eq!(resolved.appearance, WindowBackgroundAppearance::Transparent);
    }
}
