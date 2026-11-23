//! Kitty graphics placements in a flat, renderer-neutral form.

use std::sync::Arc;
use termy_core::terminal_engine::Engine;
use termy_core::terminal_engine::media::GraphicsImage;
use termy_core::{KittyGraphicsRenderPlacement, graphics_display_layout};
use web_time::Instant;

/// `f64` slots per placement, in this order:
/// `imageId, placementId, imageGeneration, imageWidth, imageHeight,
///  viewportRow, col, colOffset, sourceX, sourceY, sourceWidth, sourceHeight,
///  displayCols, displayRows, occupiedCols, occupiedRows, clipTopRows,
///  clipBottomRows, xOffset, yOffset, zIndex, placementSerial,
///  clipLeft, clipTop, clipWidth, clipHeight, drawLeft, drawTop, drawWidth, drawHeight`.
/// Missing display sizes are `-1`. The last eight slots are pixel geometry for
/// the cell size passed to `capture`, matching the desktop renderer: clip to
/// the `clip*` box, then draw the source rectangle into the `draw*` box.
pub const PLACEMENT_STRIDE: usize = 30;

#[derive(Default)]
pub struct GraphicsSnapshot {
    images: Vec<Arc<GraphicsImage>>,
    next_deadline: Option<Instant>,
}

impl GraphicsSnapshot {
    pub fn capture(&mut self, engine: &mut Engine, cell: (f32, f32)) -> Vec<f64> {
        self.images.clear();
        self.next_deadline = None;
        if !crate::is_positive_size(cell.0) || !crate::is_positive_size(cell.1) {
            return Vec::new();
        }
        let (_, placements) = engine.graphics_snapshot();
        let mut out = Vec::with_capacity(placements.len() * PLACEMENT_STRIDE);
        for placement in placements {
            self.next_deadline = match (self.next_deadline, placement.animation_deadline) {
                (Some(current), Some(next)) => Some(current.min(next)),
                (current, next) => current.or(next),
            };
            encode(&placement, &mut out);
            geometry(&placement, cell, &mut out);
            self.images.push(placement.image);
        }
        out
    }

    /// Placement image dimensions plus pixels: RGBA when decoded, otherwise the
    /// original PNG stream (decode it with `createImageBitmap`).
    pub fn image(&self, index: usize) -> Option<(bool, Vec<u8>)> {
        let image = self.images.get(index)?;
        Some(match image.rgba() {
            Some(rgba) => (false, rgba.to_vec()),
            None => (true, image.png().as_ref().clone()),
        })
    }

    pub fn deadline_ms(&self) -> f64 {
        self.next_deadline.map_or(-1.0, |deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .as_secs_f64()
                * 1000.0
        })
    }
}

fn encode(placement: &KittyGraphicsRenderPlacement, out: &mut Vec<f64>) {
    let optional = |value: Option<u32>| value.map_or(-1.0, f64::from);
    out.extend([
        f64::from(placement.image_id),
        f64::from(placement.placement_id),
        placement.image_generation as f64,
        f64::from(placement.image_width),
        f64::from(placement.image_height),
        f64::from(placement.viewport_row),
        placement.col as f64,
        f64::from(placement.col_offset),
        f64::from(placement.source_x),
        f64::from(placement.source_y),
        f64::from(placement.source_width),
        f64::from(placement.source_height),
        optional(placement.display_cols),
        optional(placement.display_rows),
        f64::from(placement.occupied_cols),
        f64::from(placement.occupied_rows),
        f64::from(placement.clip_top_rows),
        f64::from(placement.clip_bottom_rows),
        f64::from(placement.x_offset),
        f64::from(placement.y_offset),
        f64::from(placement.z_index),
        placement.placement_serial as f64,
    ]);
}

/// Mirrors `kitty_graphics_placement_bounds` and the image layer layout in
/// `crates/desktop_app/src/terminal_view`.
fn geometry(placement: &KittyGraphicsRenderPlacement, cell: (f32, f32), out: &mut Vec<f64>) {
    let (cell_width, cell_height) = cell;
    let virtual_placement = placement.virtual_cell.is_some();
    let layout = graphics_display_layout(
        placement.source_width,
        placement.source_height,
        placement.display_cols,
        placement.display_rows,
        cell,
        (placement.x_offset, placement.y_offset),
        virtual_placement,
    );
    let (width, height) = if virtual_placement {
        (cell_width, cell_height)
    } else {
        layout.placement_size
    };
    let clip_rows = (placement.clip_top_rows + placement.clip_bottom_rows) as f32;
    let left = (placement.col as f32 + placement.col_offset as f32) * cell_width
        + placement.x_offset as f32;
    let top = (placement.viewport_row as f32 + placement.clip_top_rows as f32) * cell_height
        + placement.y_offset as f32;
    let (tile_x, tile_y) = placement.virtual_cell.unwrap_or((0, 0));
    let draw_left = left + layout.image_offset.0 - tile_x as f32 * cell_width;
    let draw_top = top + layout.image_offset.1
        - (tile_y as f32 + placement.clip_top_rows as f32) * cell_height;
    out.extend(
        [
            left,
            top,
            width,
            (height - clip_rows * cell_height).max(0.0),
            draw_left,
            draw_top,
            layout.image_size.0,
            layout.image_size.1,
        ]
        .map(f64::from),
    );
}
