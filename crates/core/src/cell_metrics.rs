use std::sync::OnceLock;

use crate::config_core::{AppConfig, DEFAULT_LINE_HEIGHT, MAX_LINE_HEIGHT, MIN_LINE_HEIGHT};

const METRIC_GLYPHS: [char; 3] = ['M', '0', ' '];
const FALLBACK_REFERENCE_FONT_SIZE: f32 = 14.0;
const FALLBACK_CELL_WIDTH: f32 = 9.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalCellMetrics {
    pub cell_width: f32,
    pub cell_height: f32,
}

impl TerminalCellMetrics {
    pub fn fallback(font_size: f32, line_height: f32) -> Self {
        let font_size = normalized_font_size(font_size);
        let line_height = normalized_line_height(line_height);
        Self {
            cell_width: fallback_cell_width(font_size),
            cell_height: cell_height(font_size, line_height),
        }
    }
}

/// Measure the terminal grid cell for a font family, font size, and line-height
/// multiplier.
///
/// Width is based on the selected font's horizontal advance for `M`, falling
/// back to `0` and then space for fonts without that glyph. Height follows the
/// same row metric Termy's renderer uses: `font_size * line_height`.
///
/// If the requested font cannot be resolved from system fonts, the result falls
/// back to Termy's default monospace ratio instead of returning an error.
pub fn measure_cell(
    font_family: impl AsRef<str>,
    font_size: f32,
    line_height: f32,
) -> TerminalCellMetrics {
    let font_size = normalized_font_size(font_size);
    let line_height = normalized_line_height(line_height);
    let cell_height = cell_height(font_size, line_height);
    let cell_width = measure_cell_width(font_family.as_ref(), font_size)
        .unwrap_or_else(|| fallback_cell_width(font_size));

    TerminalCellMetrics {
        cell_width,
        cell_height,
    }
}

pub fn measure_cell_from_config(config: &AppConfig) -> TerminalCellMetrics {
    measure_cell(&config.font_family, config.font_size, config.line_height)
}

fn normalized_font_size(font_size: f32) -> f32 {
    if font_size.is_finite() && font_size > 0.0 {
        font_size
    } else {
        AppConfig::default().font_size
    }
}

fn normalized_line_height(line_height: f32) -> f32 {
    if line_height.is_finite() && (MIN_LINE_HEIGHT..=MAX_LINE_HEIGHT).contains(&line_height) {
        line_height
    } else {
        DEFAULT_LINE_HEIGHT
    }
}

fn cell_height(font_size: f32, line_height: f32) -> f32 {
    (font_size * line_height).max(1.0)
}

fn fallback_cell_width(font_size: f32) -> f32 {
    (font_size * FALLBACK_CELL_WIDTH / FALLBACK_REFERENCE_FONT_SIZE).max(1.0)
}

fn measure_cell_width(font_family: &str, font_size: f32) -> Option<f32> {
    let database = system_font_database();
    let font_id = query_font(database, font_family)?;
    database.with_face_data(font_id, |data, face_index| {
        measure_cell_width_from_font_data(data, face_index, font_size)
    })?
}

fn query_font(database: &fontdb::Database, font_family: &str) -> Option<fontdb::ID> {
    let font_family = font_family.trim();
    let id = if font_family.is_empty() {
        database.query(&fontdb::Query {
            families: &[fontdb::Family::Monospace],
            weight: fontdb::Weight::NORMAL,
            ..fontdb::Query::default()
        })
    } else {
        database.query(&fontdb::Query {
            families: &[fontdb::Family::Name(font_family), fontdb::Family::Monospace],
            weight: fontdb::Weight::NORMAL,
            ..fontdb::Query::default()
        })
    };

    if let Some(id) = id {
        Some(id)
    } else {
        database
            .faces()
            .find(|face| face.monospaced)
            .map(|face| face.id)
    }
}

fn measure_cell_width_from_font_data(data: &[u8], face_index: u32, font_size: f32) -> Option<f32> {
    let face = ttf_parser::Face::parse(data, face_index).ok()?;
    let units_per_em = f32::from(face.units_per_em());
    if units_per_em <= 0.0 {
        return None;
    }

    let glyph_id = METRIC_GLYPHS
        .into_iter()
        .find_map(|glyph| face.glyph_index(glyph))?;
    let advance = f32::from(face.glyph_hor_advance(glyph_id)?);
    let cell_width = advance * font_size / units_per_em;
    (cell_width.is_finite() && cell_width > 0.0).then_some(cell_width)
}

fn system_font_database() -> &'static fontdb::Database {
    static SYSTEM_FONT_DATABASE: OnceLock<fontdb::Database> = OnceLock::new();
    SYSTEM_FONT_DATABASE.get_or_init(|| {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        database
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_scales_width_and_uses_line_height() {
        let metrics = TerminalCellMetrics::fallback(28.0, 1.25);
        assert_eq!(metrics.cell_width, 18.0);
        assert_eq!(metrics.cell_height, 35.0);
    }

    #[test]
    fn measure_cell_handles_missing_font_with_stable_fallback() {
        let metrics = measure_cell("Definitely Missing Termy Test Font", 18.0, 1.25);
        assert!(metrics.cell_width >= 1.0);
        assert_eq!(metrics.cell_height, 22.5);
    }

    #[test]
    fn measure_cell_normalizes_invalid_inputs() {
        let metrics = measure_cell("", f32::NAN, f32::INFINITY);
        assert_eq!(
            metrics.cell_height,
            AppConfig::default().font_size * DEFAULT_LINE_HEIGHT
        );
    }
}
