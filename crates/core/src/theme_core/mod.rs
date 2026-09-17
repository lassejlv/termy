#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb8 {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parses a 6-digit RGB hex color, with optional leading `#`.
    ///
    /// Accepted examples: `"#112233"`, `"112233"`.
    /// Rejected examples: `"#fff"` (3-digit shorthand), `"#11223344"` (RGBA).
    pub fn from_hex(value: &str) -> Option<Self> {
        let hex = value.trim().trim_start_matches('#');
        if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }

        Some(Self {
            r: u8::from_str_radix(&hex[0..2], 16).ok()?,
            g: u8::from_str_radix(&hex[2..4], 16).ok()?,
            b: u8::from_str_radix(&hex[4..6], 16).ok()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeColors {
    pub ansi: [Rgb8; 16],
    pub foreground: Rgb8,
    pub background: Rgb8,
    pub cursor: Rgb8,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ThemeColorsJson {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    schema: Option<String>,
    foreground: String,
    background: String,
    cursor: String,
    black: String,
    red: String,
    green: String,
    yellow: String,
    blue: String,
    magenta: String,
    cyan: String,
    white: String,
    bright_black: String,
    bright_red: String,
    bright_green: String,
    bright_yellow: String,
    bright_blue: String,
    bright_magenta: String,
    bright_cyan: String,
    bright_white: String,
}

impl ThemeColorsJson {
    fn into_colors(self) -> Result<ThemeColors, String> {
        let ansi = [
            parse_required_color("black", &self.black)?,
            parse_required_color("red", &self.red)?,
            parse_required_color("green", &self.green)?,
            parse_required_color("yellow", &self.yellow)?,
            parse_required_color("blue", &self.blue)?,
            parse_required_color("magenta", &self.magenta)?,
            parse_required_color("cyan", &self.cyan)?,
            parse_required_color("white", &self.white)?,
            parse_required_color("bright_black", &self.bright_black)?,
            parse_required_color("bright_red", &self.bright_red)?,
            parse_required_color("bright_green", &self.bright_green)?,
            parse_required_color("bright_yellow", &self.bright_yellow)?,
            parse_required_color("bright_blue", &self.bright_blue)?,
            parse_required_color("bright_magenta", &self.bright_magenta)?,
            parse_required_color("bright_cyan", &self.bright_cyan)?,
            parse_required_color("bright_white", &self.bright_white)?,
        ];

        Ok(ThemeColors {
            ansi,
            foreground: parse_required_color("foreground", &self.foreground)?,
            background: parse_required_color("background", &self.background)?,
            cursor: parse_required_color("cursor", &self.cursor)?,
        })
    }
}

impl From<(&ThemeColors, Option<&str>)> for ThemeColorsJson {
    fn from((colors, schema): (&ThemeColors, Option<&str>)) -> Self {
        Self {
            schema: schema.map(ToString::to_string),
            foreground: format_hex(colors.foreground),
            background: format_hex(colors.background),
            cursor: format_hex(colors.cursor),
            black: format_hex(colors.ansi[0]),
            red: format_hex(colors.ansi[1]),
            green: format_hex(colors.ansi[2]),
            yellow: format_hex(colors.ansi[3]),
            blue: format_hex(colors.ansi[4]),
            magenta: format_hex(colors.ansi[5]),
            cyan: format_hex(colors.ansi[6]),
            white: format_hex(colors.ansi[7]),
            bright_black: format_hex(colors.ansi[8]),
            bright_red: format_hex(colors.ansi[9]),
            bright_green: format_hex(colors.ansi[10]),
            bright_yellow: format_hex(colors.ansi[11]),
            bright_blue: format_hex(colors.ansi[12]),
            bright_magenta: format_hex(colors.ansi[13]),
            bright_cyan: format_hex(colors.ansi[14]),
            bright_white: format_hex(colors.ansi[15]),
        }
    }
}

pub const BUILTIN_THEME_IDS: &[&str] = &[];

pub const ANSI_COLOR_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright_black",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "bright_white",
];

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ThemeRegistryIndex {
    #[serde(default = "default_registry_version")]
    pub version: u32,
    #[serde(default)]
    pub themes: Vec<ThemeRegistryEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeRegistryEntry {
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub description: String,
    pub latest_version: String,
    pub file: String,
    #[serde(default)]
    pub checksum_sha256: Option<String>,
}

/// Current on-disk format version for `theme_registry.cache`.
pub const THEME_REGISTRY_CACHE_VERSION: u32 = 1;

/// One theme listing normalized from the public registry or its legacy payload.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ThemeStoreTheme {
    pub name: String,
    pub slug: String,
    pub description: String,
    pub latest_version: Option<String>,
    pub file_url: Option<String>,
}

/// Shared persisted representation of `theme_registry.cache`.
///
/// Field order is part of the bincode format. Add a new version instead of
/// reordering or changing fields in place.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ThemeRegistryCache {
    pub version: u32,
    pub fetched_at: u64,
    pub registry_url: String,
    pub etag: Option<String>,
    pub themes: Vec<ThemeStoreTheme>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeMetadata {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub description: String,
    pub latest_version: String,
    #[serde(default)]
    pub versions: Vec<ThemeMetadataVersion>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeMetadataVersion {
    pub version: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum_sha256: Option<String>,
}

fn default_registry_version() -> u32 {
    1
}

pub fn normalize_theme_id(theme_id: &str) -> String {
    let mut normalized = String::new();
    let mut last_dash = false;

    for ch in theme_id.trim().chars() {
        let ch = ch.to_ascii_lowercase();
        match ch {
            'a'..='z' | '0'..='9' => {
                normalized.push(ch);
                last_dash = false;
            }
            '-' | '_' | ' ' if !normalized.is_empty() && !last_dash => {
                normalized.push('-');
                last_dash = true;
            }
            _ => {}
        }
    }

    while normalized.ends_with('-') {
        normalized.pop();
    }

    normalized
}

pub fn canonical_builtin_theme_id(theme_id: &str) -> Option<&'static str> {
    let _ = theme_id;
    None
}

pub fn format_hex(color: Rgb8) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

pub fn parse_theme_colors_json(contents: &str) -> Result<ThemeColors, String> {
    let json: ThemeColorsJson =
        serde_json::from_str(contents).map_err(|error| format!("Invalid theme colors: {error}"))?;
    json.into_colors()
}

pub fn theme_colors_json_pretty(
    colors: &ThemeColors,
    schema: Option<&str>,
) -> Result<String, String> {
    serde_json::to_string_pretty(&ThemeColorsJson::from((colors, schema)))
        .map_err(|error| format!("Failed to serialize theme colors: {error}"))
}

pub fn registry_file_url(index_url: &str, file: &str) -> String {
    if file.starts_with("http://") || file.starts_with("https://") {
        return file.to_string();
    }

    let base = index_url
        .rsplit_once('/')
        .map_or_else(|| index_url.trim_end_matches('/'), |(base, _)| base);
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        file.trim_start_matches('/')
    )
}

fn parse_required_color(key: &str, value: &str) -> Result<Rgb8, String> {
    parse_hex_color(value).ok_or_else(|| format!("Theme color '{key}' must be a #RRGGBB hex"))
}

fn parse_hex_color(value: &str) -> Option<Rgb8> {
    if !value.starts_with('#') {
        return None;
    }

    Rgb8::from_hex(value)
}

#[cfg(test)]
mod tests {
    use super::{
        Rgb8, canonical_builtin_theme_id, format_hex, normalize_theme_id, parse_theme_colors_json,
        registry_file_url, theme_colors_json_pretty,
    };

    #[test]
    fn formats_hex_in_lowercase() {
        assert_eq!(format_hex(Rgb8::new(0xAB, 0xCD, 0xEF)), "#abcdef");
    }

    #[test]
    fn parses_hex_with_optional_hash() {
        assert_eq!(Rgb8::from_hex("#12AB34"), Some(Rgb8::new(0x12, 0xab, 0x34)));
        assert_eq!(Rgb8::from_hex("12ab34"), Some(Rgb8::new(0x12, 0xab, 0x34)));
        assert_eq!(Rgb8::from_hex("#fff"), None);
        assert_eq!(Rgb8::from_hex("#11223344"), None);
        assert_eq!(Rgb8::from_hex("#zzzzzz"), None);
    }

    #[test]
    fn normalize_theme_id_is_stable() {
        assert_eq!(normalize_theme_id("  Tokyo_Night  "), "tokyo-night");
        assert_eq!(normalize_theme_id("gruvbox---dark"), "gruvbox-dark");
    }

    #[test]
    fn builtin_aliases_are_disabled() {
        assert_eq!(canonical_builtin_theme_id("gruvbox"), None);
        assert_eq!(canonical_builtin_theme_id("tokyonight"), None);
        assert_eq!(canonical_builtin_theme_id("default"), None);
    }

    #[test]
    fn parses_theme_color_json() {
        let json = r##"{
            "foreground": "#e5e5e5",
            "background": "#111111",
            "cursor": "#ffffff",
            "black": "#000000",
            "red": "#111111",
            "green": "#222222",
            "yellow": "#333333",
            "blue": "#444444",
            "magenta": "#555555",
            "cyan": "#666666",
            "white": "#777777",
            "bright_black": "#888888",
            "bright_red": "#999999",
            "bright_green": "#aaaaaa",
            "bright_yellow": "#bbbbbb",
            "bright_blue": "#cccccc",
            "bright_magenta": "#dddddd",
            "bright_cyan": "#eeeeee",
            "bright_white": "#ffffff"
        }"##;
        let colors = parse_theme_colors_json(json).expect("valid colors");
        assert_eq!(colors.foreground, Rgb8::new(0xe5, 0xe5, 0xe5));
        assert_eq!(colors.ansi[3], Rgb8::new(0x33, 0x33, 0x33));
    }

    #[test]
    fn rejects_malformed_unicode_hex_without_panicking() {
        let json = r##"{
            "foreground": "#€€",
            "background": "#111111",
            "cursor": "#ffffff",
            "black": "#000000",
            "red": "#111111",
            "green": "#222222",
            "yellow": "#333333",
            "blue": "#444444",
            "magenta": "#555555",
            "cyan": "#666666",
            "white": "#777777",
            "bright_black": "#888888",
            "bright_red": "#999999",
            "bright_green": "#aaaaaa",
            "bright_yellow": "#bbbbbb",
            "bright_blue": "#cccccc",
            "bright_magenta": "#dddddd",
            "bright_cyan": "#eeeeee",
            "bright_white": "#ffffff"
        }"##;
        assert!(parse_theme_colors_json(json).is_err());
    }

    #[test]
    fn serializes_theme_color_json() {
        let colors = parse_theme_colors_json(
            r##"{
                "foreground": "#e5e5e5",
                "background": "#111111",
                "cursor": "#ffffff",
                "black": "#000000",
                "red": "#111111",
                "green": "#222222",
                "yellow": "#333333",
                "blue": "#444444",
                "magenta": "#555555",
                "cyan": "#666666",
                "white": "#777777",
                "bright_black": "#888888",
                "bright_red": "#999999",
                "bright_green": "#aaaaaa",
                "bright_yellow": "#bbbbbb",
                "bright_blue": "#cccccc",
                "bright_magenta": "#dddddd",
                "bright_cyan": "#eeeeee",
                "bright_white": "#ffffff"
            }"##,
        )
        .expect("valid colors");
        let serialized = theme_colors_json_pretty(&colors, Some("./theme.schema.json"))
            .expect("serialized colors");
        assert!(serialized.contains("\"$schema\": \"./theme.schema.json\""));
        assert!(serialized.contains("\"foreground\": \"#e5e5e5\""));
    }

    #[test]
    fn resolves_registry_relative_file_urls() {
        assert_eq!(
            registry_file_url(
                "https://raw.githubusercontent.com/termy-org/themes/main/index.json",
                "themes/tokyonight/files/1.0.0.json"
            ),
            "https://raw.githubusercontent.com/termy-org/themes/main/themes/tokyonight/files/1.0.0.json"
        );
    }
}
