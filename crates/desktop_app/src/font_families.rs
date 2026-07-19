use std::sync::Mutex;

use gpui::{Font, FontWeight, SharedString, TextSystem, px};
use termy_config_core::DEFAULT_FONT_FAMILY;

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
/// The platform default is the one family that must stay selectable even when
/// enumeration misses it (e.g. the generic `monospace` alias on Linux): it is
/// the fallback target, so listing it can never send a user to a worse font.
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
/// A missing family otherwise falls through GPUI's application font stack to
/// Segoe UI on Windows. Termy's grid then forces every glyph to that
/// proportional font's `M` advance, which looks like extra letter spacing.
pub(crate) fn effective_terminal_font_family(
    requested: &str,
    text_system: &TextSystem,
) -> SharedString {
    let available = available_font_families(text_system.all_font_names());
    let Some(candidate) = canonical_available_font_family(requested, &available) else {
        notify_fallback(requested.trim(), "is not installed");
        return DEFAULT_FONT_FAMILY.into();
    };

    // The platform default is exempt: it is the fallback target, so rejecting
    // it would only produce a self-referential warning.
    if !candidate.eq_ignore_ascii_case(DEFAULT_FONT_FAMILY)
        && !font_has_fixed_ascii_advances(text_system, &candidate)
    {
        notify_fallback(&candidate, "resolved to a proportional font");
        return DEFAULT_FONT_FAMILY.into();
    }

    clear_fallback_notification();
    candidate.into()
}

fn notify_fallback(requested: &str, reason: &str) {
    let mut last = LAST_FALLBACK_NOTIFICATION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !mark_fallback(&mut last, requested) {
        return;
    }
    log::warn!("Configured terminal font '{requested}' {reason}; using '{DEFAULT_FONT_FAMILY}'");
    termy_toast::warning(format!(
        "Font \"{requested}\" {reason}; using {DEFAULT_FONT_FAMILY}"
    ));
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
        ..Font::default()
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
    fn fallback_notification_fires_once_per_family() {
        let mut last = None;
        assert!(mark_fallback(&mut last, "JetBrains Mono"));
        assert!(!mark_fallback(&mut last, "JetBrains Mono"));
        assert!(mark_fallback(&mut last, "Fira Code"));
        assert!(mark_fallback(&mut last, "JetBrains Mono"));
    }
}
