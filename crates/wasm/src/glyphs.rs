//! Termy's canonical special-glyph geometry (block elements, box drawing,
//! sextants, Braille runs, rounded corners and diagonals) in a flat form.

use termy_core::{
    MAX_TERMINAL_GLYPH_STROKE_POINTS, TerminalGlyphMetrics, TerminalGlyphNeighbors,
    TerminalGlyphRectSnap, TerminalGlyphRenderKind, TerminalGlyphStrokeKind, terminal_glyph_plan,
};

pub const RECT_STRIDE: usize = 6;
pub const STROKE_STRIDE: usize = 3 + MAX_TERMINAL_GLYPH_STROKE_POINTS * 2;

/// `[kind, rectCount, strokeCount, rects..., strokes...]`, cell-normalized.
/// A rect is `left, top, right, bottom, alpha, snap` (snap 0 nearest, 1 outward).
/// A stroke is `kind, width, pointCount, x0, y0, ...` padded to
/// `MAX_TERMINAL_GLYPH_STROKE_POINTS` points (kind 0 line, 1 rounded corner).
pub fn glyph_plan(
    character: char,
    neighbors: [Option<char>; 4],
    metrics: [f32; 3],
) -> Option<Vec<f32>> {
    if !metrics.iter().copied().all(crate::is_positive_size) {
        return None;
    }
    let plan = terminal_glyph_plan(
        character,
        TerminalGlyphMetrics {
            cell_width: metrics[0],
            cell_height: metrics[1],
            font_size: metrics[2],
        },
        TerminalGlyphNeighbors {
            two_before: neighbors[0],
            before: neighbors[1],
            after: neighbors[2],
            two_after: neighbors[3],
        },
    )?;
    let rects = plan.rects();
    let strokes = plan.strokes();
    let kind = match plan.kind() {
        TerminalGlyphRenderKind::BlockElement => 0.0,
        TerminalGlyphRenderKind::BoxDrawing => 1.0,
        TerminalGlyphRenderKind::Sextant => 2.0,
        TerminalGlyphRenderKind::Braille => 3.0,
        TerminalGlyphRenderKind::RoundedCorner => 4.0,
        TerminalGlyphRenderKind::Diagonal => 5.0,
    };
    let mut out = Vec::with_capacity(3 + rects.len() * RECT_STRIDE + strokes.len() * STROKE_STRIDE);
    out.extend([kind, rects.len() as f32, strokes.len() as f32]);
    for rect in rects {
        let snap = match rect.snap {
            TerminalGlyphRectSnap::Nearest => 0.0,
            TerminalGlyphRectSnap::Outward => 1.0,
        };
        out.extend([
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            rect.alpha,
            snap,
        ]);
    }
    for stroke in strokes {
        let kind = match stroke.kind {
            TerminalGlyphStrokeKind::Line => 0.0,
            TerminalGlyphStrokeKind::RoundedCorner => 1.0,
        };
        out.extend([kind, stroke.width, f32::from(stroke.point_count)]);
        for point in stroke.points {
            out.extend([point.x, point.y]);
        }
    }
    Some(out)
}
