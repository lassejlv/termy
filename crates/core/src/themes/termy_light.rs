use super::{ThemeColors, rgba};

/// The built-in light counterpart to Termy's default dark palette.
///
/// Keep this in sync with `termy-light` in the official theme registry so
/// system appearance switching still has a usable light palette offline.
pub fn theme() -> ThemeColors {
    ThemeColors {
        ansi: [
            rgba(0x09, 0x09, 0x0B), // Black
            rgba(0xC7, 0x24, 0x2A), // Red
            rgba(0x3F, 0x8B, 0x3A), // Green
            rgba(0xB9, 0x8A, 0x0A), // Yellow
            rgba(0x1F, 0x6F, 0xB8), // Blue
            rgba(0x8A, 0x3F, 0xB8), // Magenta
            rgba(0x1A, 0x8A, 0x8A), // Cyan
            rgba(0xE4, 0xE4, 0xE7), // White
            rgba(0x52, 0x52, 0x5B), // Bright Black
            rgba(0xE0, 0x4A, 0x50), // Bright Red
            rgba(0x5B, 0xAE, 0x54), // Bright Green
            rgba(0xD4, 0xA1, 0x1A), // Bright Yellow
            rgba(0x3A, 0x8F, 0xD8), // Bright Blue
            rgba(0xA8, 0x5F, 0xD8), // Bright Magenta
            rgba(0x2F, 0xAF, 0xA8), // Bright Cyan
            rgba(0xFF, 0xFF, 0xFF), // Bright White
        ],
        foreground: rgba(0x09, 0x09, 0x0B),
        background: rgba(0xFA, 0xFA, 0xF9),
        cursor: rgba(0x4F, 0xA8, 0x4A),
    }
}
