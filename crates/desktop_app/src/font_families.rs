use std::sync::Mutex;

use gpui::{Font, FontWeight, SharedString, TextSystem, px};
use termy_core::config_core::DEFAULT_FONT_FAMILY;

const TERMINAL_METRIC_GLYPHS: [char; 5] = ['M', 'i', 'W', '0', ' '];
const TERMINAL_METRIC_FONT_SIZE: f32 = 14.0;
const FIXED_ADVANCE_TOLERANCE: f32 = 0.01;

/// Requested family from the most recent fallback notification.
///
/// `effective_terminal_font_family` runs once per pane and again on every
/// config apply; without this, one bad config line raises a warning toast per
/// pane.
static LAST_FALLBACK_NOTIFICATION: Mutex<Option<String>> = Mutex::new(None);

/// Clean GPUI's font list before exposing it in settings.
///
/// GPUI includes `.ZedMono` and `.ZedSans` even when the concrete fonts they
/// map to are not installed. Presenting those as installed lets users save a
/// family that silently resolves to a proportional fallback.
///
/// Keep the platform default selectable. Generic aliases such as Linux's
/// `monospace` are resolved to a concrete family before measuring or shaping.
pub(crate) fn available_font_families(fonts: Vec<String>) -> Vec<String> {
    let mut fonts = fonts
        .into_iter()
        .map(|font| font.trim().to_string())
        .filter(|font| !font.is_empty() && !matches!(font.as_str(), ".ZedMono" | ".ZedSans"))
        .collect::<Vec<_>>();

    if !fonts
        .iter()
        .any(|font| font.eq_ignore_ascii_case(DEFAULT_FONT_FAMILY))
    {
        fonts.push(DEFAULT_FONT_FAMILY.to_string());
    }

    fonts.sort_unstable_by_key(|font| font.to_ascii_lowercase());
    fonts.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    fonts
}

/// Return the installed spelling of a configured font family.
///
/// GPUI's legacy Zed aliases are only accepted when their concrete family is
/// present. Returning the concrete name keeps later measurement and shaping on
/// exactly the same face.
pub(crate) fn canonical_available_font_family(
    requested: &str,
    available: &[String],
) -> Option<String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return None;
    }

    let concrete = match requested {
        ".ZedMono" | "Zed Plex Mono" => "Lilex",
        ".ZedSans" | "Zed Plex Sans" => "IBM Plex Sans",
        family => family,
    };

    available
        .iter()
        .find(|font| font.eq_ignore_ascii_case(concrete))
        .cloned()
}

/// Resolve the configured terminal font to a real fixed-pitch family.
///
/// GPUI 0.2.2 treats Linux's `monospace` alias as a literal family name. A
/// missing family falls through to a UI font (e.g. Noto Sans on KDE). Forcing
/// every glyph to that proportional font's `M` advance adds letter spacing.
pub(crate) fn effective_terminal_font_family(
    requested: &str,
    text_system: &TextSystem,
) -> SharedString {
    // Most launches need one installed family. Enumerating every system font
    // creates thousands of descriptors before the first window can paint.
    #[cfg(target_os = "macos")]
    if let Some(candidate) = directly_available_font_family(requested)
        && font_has_fixed_ascii_advances(text_system, &candidate)
    {
        clear_fallback_notification();
        return candidate.into();
    }
    let available = available_font_families(text_system.all_font_names());
    crate::launch_probe::record_stage("font_catalog_loaded");
    let fallback = || -> SharedString {
        let preferred = system_monospace_family();
        select_fixed_pitch_family(Some(&preferred), &available, |family| {
            font_has_fixed_ascii_advances(text_system, family)
        })
        .unwrap_or_else(|| {
            log::error!(
                "No usable monospace font is installed; install a fixed-pitch terminal font"
            );
            DEFAULT_FONT_FAMILY
        })
        .to_string()
        .into()
    };

    if requested.trim().eq_ignore_ascii_case("monospace") {
        clear_fallback_notification();
        return fallback();
    }

    let Some(candidate) = canonical_available_font_family(requested, &available) else {
        let fallback = fallback();
        notify_fallback(requested.trim(), "is not installed", &fallback);
        return fallback;
    };

    if !font_has_fixed_ascii_advances(text_system, &candidate) {
        let fallback = fallback();
        notify_fallback(&candidate, "resolved to a proportional font", &fallback);
        return fallback;
    }

    clear_fallback_notification();
    candidate.into()
}

#[cfg(target_os = "macos")]
fn directly_available_font_family(requested: &str) -> Option<String> {
    let requested = requested.trim();
    let concrete = match requested {
        "" => return None,
        ".ZedMono" | "Zed Plex Mono" => "Lilex",
        ".ZedSans" | "Zed Plex Sans" => "IBM Plex Sans",
        family if family.eq_ignore_ascii_case("monospace") => DEFAULT_FONT_FAMILY,
        family => family,
    };
    let font =
        core_text::font::new_from_name(concrete, f64::from(TERMINAL_METRIC_FONT_SIZE)).ok()?;
    let family = font.family_name();
    // CoreText silently substitutes missing names. Never accept that fallback
    // as evidence that the configured family exists.
    family.eq_ignore_ascii_case(concrete).then_some(family)
}

#[cfg(target_os = "linux")]
fn system_monospace_family() -> String {
    use font_kit::{family_name::FamilyName, properties::Properties, source::SystemSource};

    // Fontconfig applies the user's desktop font preferences and substitutions.
    // GPUI needs the selected font's concrete family, not the generic alias.
    SystemSource::new()
        .select_best_match(&[FamilyName::Monospace], &Properties::new())
        .ok()
        .and_then(|handle| handle.load().ok())
        .map(|font| font.family_name())
        .unwrap_or_else(|| DEFAULT_FONT_FAMILY.to_string())
}

#[cfg(not(target_os = "linux"))]
fn system_monospace_family() -> String {
    DEFAULT_FONT_FAMILY.to_string()
}

fn select_fixed_pitch_family<'a>(
    preferred: Option<&str>,
    available: &'a [String],
    mut is_fixed_pitch: impl FnMut(&str) -> bool,
) -> Option<&'a str> {
    let preferred = preferred.and_then(|preferred| {
        available
            .iter()
            .find(|family| family.eq_ignore_ascii_case(preferred))
    });
    preferred
        .into_iter()
        .chain(available)
        // Never pass the unresolved generic alias back into GPUI's UI fallback.
        .find(|family| !family.eq_ignore_ascii_case("monospace") && is_fixed_pitch(family))
        .map(String::as_str)
}

fn notify_fallback(requested: &str, reason: &str, fallback: &str) {
    let mut last = LAST_FALLBACK_NOTIFICATION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !mark_fallback(&mut last, requested) {
        return;
    }
    log::warn!("Configured terminal font '{requested}' {reason}; using '{fallback}'");
    crate::ui::toast::warning(format!("Font \"{requested}\" {reason}; using {fallback}"));
}

fn clear_fallback_notification() {
    *LAST_FALLBACK_NOTIFICATION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Record `requested` as the active fallback; true when the user has not yet
/// been notified about this family.
fn mark_fallback(last: &mut Option<String>, requested: &str) -> bool {
    if last.as_deref() == Some(requested) {
        return false;
    }
    *last = Some(requested.to_string());
    true
}

fn font_has_fixed_ascii_advances(text_system: &TextSystem, family: &str) -> bool {
    let font = Font {
        family: family.to_string().into(),
        weight: FontWeight::NORMAL,
        ..gpui::font("")
    };
    let font_id = text_system.resolve_font(&font);
    let font_size = px(TERMINAL_METRIC_FONT_SIZE);
    let advances = TERMINAL_METRIC_GLYPHS.map(|glyph| {
        text_system
            .advance(font_id, font_size, glyph)
            .ok()
            .map(|advance| f32::from(advance.width))
    });
    let Some(advances) = advances.into_iter().collect::<Option<Vec<_>>>() else {
        return false;
    };
    ascii_advances_are_fixed(&advances)
}

fn ascii_advances_are_fixed(advances: &[f32]) -> bool {
    if advances.is_empty()
        || advances
            .iter()
            .any(|advance| !advance.is_finite() || *advance <= 0.0)
    {
        return false;
    }

    let min = advances.iter().copied().fold(f32::INFINITY, f32::min);
    let max = advances.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    max - min <= FIXED_ADVANCE_TOLERANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn direct_font_lookup_preserves_missing_family_detection_and_generic_alias() {
        assert_eq!(
            directly_available_font_family(" Menlo ").as_deref(),
            Some("Menlo")
        );
        assert_eq!(
            directly_available_font_family("monospace").as_deref(),
            Some(DEFAULT_FONT_FAMILY)
        );
        assert_eq!(
            directly_available_font_family("Termy missing font 8f77d2"),
            None
        );
        assert_eq!(directly_available_font_family(""), None);
    }

    #[test]
    fn available_fonts_hide_unavailable_zed_aliases_and_deduplicate_case() {
        let fonts = available_font_families(vec![
            ".ZedMono".to_string(),
            ".ZedSans".to_string(),
            ".SystemUIFont".to_string(),
            " JetBrains Mono ".to_string(),
            "jetbrains mono".to_string(),
            String::new(),
        ]);

        assert!(!fonts.iter().any(|font| font == ".ZedMono"));
        assert!(!fonts.iter().any(|font| font == ".ZedSans"));
        assert!(fonts.iter().any(|font| font == ".SystemUIFont"));
        assert_eq!(
            fonts
                .iter()
                .filter(|font| font.eq_ignore_ascii_case("JetBrains Mono"))
                .count(),
            1
        );
        assert!(
            fonts
                .iter()
                .any(|font| font.eq_ignore_ascii_case(DEFAULT_FONT_FAMILY))
        );
    }

    #[test]
    fn canonical_font_family_trims_and_preserves_installed_spelling() {
        let available = vec!["JetBrains Mono".to_string(), "Lilex".to_string()];
        assert_eq!(
            canonical_available_font_family("  jetbrains mono ", &available),
            Some("JetBrains Mono".to_string())
        );
        assert_eq!(
            canonical_available_font_family("Missing Mono", &available),
            None
        );
    }

    #[test]
    fn zed_mono_alias_requires_and_returns_lilex() {
        assert_eq!(canonical_available_font_family(".ZedMono", &[]), None);
        assert_eq!(
            canonical_available_font_family(".ZedMono", &["Lilex".to_string()]),
            Some("Lilex".to_string())
        );
    }

    #[test]
    fn fixed_advance_check_rejects_proportional_metrics() {
        assert!(ascii_advances_are_fixed(&[8.4, 8.4, 8.4, 8.4, 8.4]));
        assert!(!ascii_advances_are_fixed(&[12.0, 3.0, 13.0, 8.0, 4.0]));
        assert!(!ascii_advances_are_fixed(&[8.4, f32::NAN]));
    }

    #[test]
    fn fallback_resolves_generic_alias_to_a_concrete_fixed_pitch_family() {
        let available = vec![
            "Noto Sans".to_string(),
            "Noto Sans Mono".to_string(),
            "monospace".to_string(),
        ];
        let selected = select_fixed_pitch_family(Some("monospace"), &available, |family| {
            assert_ne!(
                family, "monospace",
                "an alias must not enter GPUI's fallback stack"
            );
            family == "Noto Sans Mono"
        });
        assert_eq!(selected, Some("Noto Sans Mono"));
    }

    #[test]
    fn fallback_honors_the_desktop_monospace_preference() {
        let available = vec!["DejaVu Sans Mono".to_string(), "JetBrains Mono".to_string()];
        assert_eq!(
            select_fixed_pitch_family(Some("jetbrains mono"), &available, |_| true),
            Some("JetBrains Mono")
        );
    }

    #[test]
    fn fallback_validates_the_default_instead_of_exempting_it() {
        let available = vec!["Noto Sans".to_string(), "DejaVu Sans Mono".to_string()];
        for preferred in [Some("Noto Sans"), Some("Missing Mono"), None] {
            assert_eq!(
                select_fixed_pitch_family(preferred, &available, |family| {
                    family == "DejaVu Sans Mono"
                }),
                Some("DejaVu Sans Mono")
            );
        }
        assert_eq!(select_fixed_pitch_family(None, &available, |_| false), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_font_resolution_uses_real_fixed_pitch_metrics() {
        // GPUI's test context uses NoopTextSystem, which cannot catch a font
        // fallback regression. Headless Application uses the real Linux backend
        // without requiring an X11/Wayland display or GPU.
        let app = gpui::Application::headless();
        let text_system = app.text_system();
        let preferred = system_monospace_family();
        assert_ne!(preferred, "monospace", "install a system monospace font");
        for requested in [
            "monospace",
            " MONOSPACE ",
            "__termy_missing_font__",
            "sans-serif",
            &preferred,
        ] {
            let family = effective_terminal_font_family(requested, &text_system);
            assert_ne!(family.as_ref(), "monospace");
            assert!(
                font_has_fixed_ascii_advances(&text_system, &family),
                "{requested} resolved to proportional {family}"
            );

            let font = gpui::font(family.clone());
            let font_id = text_system.resolve_font(&font);
            for font_size in [10.0, 14.0, 24.0] {
                let font_size = px(font_size);
                let advances = TERMINAL_METRIC_GLYPHS.map(|glyph| {
                    f32::from(
                        text_system
                            .advance(font_id, font_size, glyph)
                            .unwrap()
                            .width,
                    )
                });
                assert!(
                    ascii_advances_are_fixed(&advances),
                    "{family} must keep equal character widths at {font_size:?}: {advances:?}"
                );
            }
        }
        assert_eq!(
            effective_terminal_font_family(&preferred, &text_system).as_ref(),
            preferred
        );
    }

    #[test]
    fn fallback_notification_fires_once_per_family() {
        let mut last = None;
        assert!(mark_fallback(&mut last, "JetBrains Mono"));
        assert!(!mark_fallback(&mut last, "JetBrains Mono"));
        assert!(mark_fallback(&mut last, "Fira Code"));
        assert!(mark_fallback(&mut last, "JetBrains Mono"));
    }
}
