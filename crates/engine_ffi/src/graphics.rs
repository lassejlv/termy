//! Kitty graphics placements for host-owned renderers.
//!
//! Placements come with pixel rectangles already worked out (cropping, `c`/`r`
//! fitting, letterboxing, pixel offsets, scroll clipping and Unicode
//! placeholder tiles), so a host only draws a textured rectangle per entry.

use std::{collections::HashMap, ptr, sync::Arc};

use termy_core::{GraphicsImage, KittyGraphicsRenderPlacement, graphics_display_layout};

use crate::{TermyEngine, TermyEngineBytes, TermyEngineStatus, guard, handle};

pub const IMAGE_FORMAT_RGBA: u32 = 0;
pub const IMAGE_FORMAT_PNG: u32 = 1;

/// One visible placement. Rectangles are in pixels from the top-left of the
/// grid, in the cell size the host passed.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TermyEngineImagePlacement {
    /// With `image_generation`, names the pixels; the same pair always has
    /// the same pixels, so hosts can cache textures by it.
    pub image_id: u32,
    pub z_index: i32,
    pub image_generation: u64,
    pub image_width: u32,
    pub image_height: u32,
    /// The part of the grid the placement shows on.
    pub clip_x: f32,
    pub clip_y: f32,
    pub clip_width: f32,
    pub clip_height: f32,
    /// Where the whole image lands, often past the clip.
    pub image_x: f32,
    pub image_y: f32,
    pub image_draw_width: f32,
    pub image_draw_height: f32,
}

#[derive(Default)]
pub(crate) struct GraphicsBuffers {
    placements: Vec<TermyEngineImagePlacement>,
    /// Images of the last placements call, for `termy_engine_image_data`.
    images: HashMap<(u32, u64), Arc<GraphicsImage>>,
}

/// Pixel rectangles of one placement, None when nothing of it shows.
pub(crate) fn place(
    placement: &KittyGraphicsRenderPlacement,
    cell_width: f32,
    cell_height: f32,
) -> Option<TermyEngineImagePlacement> {
    let virtual_placement = placement.virtual_cell.is_some();
    let layout = graphics_display_layout(
        placement.source_width,
        placement.source_height,
        placement.display_cols,
        placement.display_rows,
        (cell_width, cell_height),
        (placement.x_offset, placement.y_offset),
        virtual_placement,
    );
    // The placement's box on screen: one cell for a placeholder tile,
    // otherwise the requested box less rows scrolled out or past a margin.
    let (box_width, box_height) = if virtual_placement {
        (cell_width, cell_height)
    } else {
        layout.placement_size
    };
    let clip_rows = placement.clip_top_rows as f32 + placement.clip_bottom_rows as f32;
    let left = (placement.col as f32 + placement.col_offset as f32) * cell_width
        + placement.x_offset as f32;
    let top = (placement.viewport_row as f32 + placement.clip_top_rows as f32) * cell_height
        + placement.y_offset as f32;
    let height = (box_height - clip_rows * cell_height).max(0.0);

    // The fitted source rectangle, relative to the box. A tile shows the
    // part of the whole placement at its virtual cell.
    let (tile_x, tile_y) = placement.virtual_cell.unwrap_or((0, 0));
    let fitted_x = left + layout.image_offset.0 - tile_x as f32 * cell_width;
    let fitted_y = top + layout.image_offset.1
        - (tile_y as f32 + placement.clip_top_rows as f32) * cell_height;
    let (fitted_width, fitted_height) = layout.image_size;

    // Pixels outside the source rectangle are cropped, then the box clips.
    let clip_x = left.max(fitted_x);
    let clip_y = top.max(fitted_y);
    let clip_right = (left + box_width).min(fitted_x + fitted_width);
    let clip_bottom = (top + height).min(fitted_y + fitted_height);
    if clip_right <= clip_x || clip_bottom <= clip_y {
        return None;
    }

    let scale_x = fitted_width / placement.source_width.max(1) as f32;
    let scale_y = fitted_height / placement.source_height.max(1) as f32;
    Some(TermyEngineImagePlacement {
        image_id: placement.image_id,
        z_index: placement.z_index,
        image_generation: placement.image_generation,
        image_width: placement.image_width,
        image_height: placement.image_height,
        clip_x,
        clip_y,
        clip_width: clip_right - clip_x,
        clip_height: clip_bottom - clip_y,
        image_x: fitted_x - placement.source_x as f32 * scale_x,
        image_y: fitted_y - placement.source_y as f32 * scale_y,
        image_draw_width: placement.image_width as f32 * scale_x,
        image_draw_height: placement.image_height as f32 * scale_y,
    })
}

/// Placements on the viewport, lowest z first; equal z keeps creation order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_image_placements(
    engine: *mut TermyEngine,
    cell_width: f32,
    cell_height: f32,
    out_placements: *mut *const TermyEngineImagePlacement,
    out_len: *mut usize,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(handle) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        if out_placements.is_null() || out_len.is_null() {
            return TermyEngineStatus::Null;
        }
        if !(cell_width.is_finite()
            && cell_height.is_finite()
            && cell_width > 0.0
            && cell_height > 0.0)
        {
            return TermyEngineStatus::InvalidArgument;
        }
        let mut placements = handle.engine.graphics_placements();
        placements.sort_by_key(|placement| (placement.z_index, placement.placement_serial));
        let buffers = &mut handle.graphics;
        buffers.placements.clear();
        buffers.images.clear();
        for placement in &placements {
            if let Some(placed) = place(placement, cell_width, cell_height) {
                buffers.images.insert(
                    (placed.image_id, placed.image_generation),
                    placement.image.clone(),
                );
                buffers.placements.push(placed);
            }
        }
        // SAFETY: both checked non-null; the host passes writable slots.
        unsafe {
            *out_placements = if buffers.placements.is_empty() {
                ptr::null()
            } else {
                buffers.placements.as_ptr()
            };
            *out_len = buffers.placements.len();
        }
        TermyEngineStatus::Ok
    })
}

/// The pixels of an image from the last placements call: RGBA rows of
/// `image_width * 4` bytes, or a PNG of that size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_image_data(
    engine: *mut TermyEngine,
    image_id: u32,
    image_generation: u64,
    out_format: *mut u32,
    out_bytes: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(handle) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        if out_format.is_null() || out_bytes.is_null() {
            return TermyEngineStatus::Null;
        }
        let Some(image) = handle.graphics.images.get(&(image_id, image_generation)) else {
            return TermyEngineStatus::InvalidArgument;
        };
        let (format, bytes) = match image.rgba() {
            Some(rgba) => (IMAGE_FORMAT_RGBA, rgba.to_vec()),
            None => (IMAGE_FORMAT_PNG, image.png().as_ref().clone()),
        };
        // SAFETY: both checked non-null; the host passes writable slots.
        unsafe {
            *out_format = format;
            *out_bytes = TermyEngineBytes::from_vec(bytes);
        }
        TermyEngineStatus::Ok
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        termy_engine_feed, termy_engine_free, termy_engine_new, termy_engine_options_default,
        termy_engine_set_cell_pixels,
    };

    fn engine() -> *mut TermyEngine {
        let mut options = termy_engine_options_default();
        options.cols = 20;
        options.rows = 10;
        let mut engine = ptr::null_mut();
        // SAFETY: valid options and out pointer.
        assert_eq!(
            unsafe { termy_engine_new(&options, &mut engine) },
            TermyEngineStatus::Ok
        );
        // SAFETY: live handle.
        unsafe { termy_engine_set_cell_pixels(engine, 10, 20) };
        engine
    }

    fn feed(engine: *mut TermyEngine, bytes: &[u8]) {
        // SAFETY: live handle, valid input, replies dropped.
        assert_eq!(
            unsafe { termy_engine_feed(engine, bytes.as_ptr(), bytes.len(), ptr::null_mut()) },
            TermyEngineStatus::Ok
        );
    }

    fn placements(engine: *mut TermyEngine, cell: (f32, f32)) -> Vec<TermyEngineImagePlacement> {
        let mut out = ptr::null();
        let mut len = 0;
        // SAFETY: live handle, writable out slots.
        let status =
            unsafe { termy_engine_image_placements(engine, cell.0, cell.1, &mut out, &mut len) };
        assert_eq!(status, TermyEngineStatus::Ok);
        if len == 0 {
            return Vec::new();
        }
        // SAFETY: valid until the next call on this handle.
        unsafe { std::slice::from_raw_parts(out, len) }.to_vec()
    }

    /// 2x2 RGBA: red, green / blue, white.
    const PIXELS: &str = "/wAA/wD/AP8AAP///////w==";

    #[test]
    fn a_placement_scales_to_its_cells_and_follows_the_cell_size() {
        let engine = engine();
        // Cursor at row 1, col 2; the image fills 4 columns by 2 rows.
        feed(engine, b"\r\n  ");
        feed(
            engine,
            format!("\x1b_Ga=T,f=32,s=2,v=2,i=5,c=4,r=2,q=2;{PIXELS}\x1b\\").as_bytes(),
        );

        let shown = placements(engine, (10.0, 20.0));
        assert_eq!(shown.len(), 1);
        let p = shown[0];
        assert_eq!((p.image_id, p.image_width, p.image_height), (5, 2, 2));
        // A 40x40 box for a square image: centered, 40 wide.
        assert_eq!(
            (p.clip_x, p.clip_y, p.clip_width, p.clip_height),
            (20.0, 20.0, 40.0, 40.0)
        );
        assert_eq!(
            (
                p.image_x,
                p.image_y,
                p.image_draw_width,
                p.image_draw_height
            ),
            (20.0, 20.0, 40.0, 40.0)
        );

        // A bigger font redraws the same cells larger.
        let larger = placements(engine, (20.0, 40.0))[0];
        assert_eq!(
            (
                larger.clip_x,
                larger.clip_y,
                larger.clip_width,
                larger.clip_height
            ),
            (40.0, 40.0, 80.0, 80.0)
        );

        let mut format = u32::MAX;
        let mut bytes = TermyEngineBytes::EMPTY;
        // SAFETY: live handle, writable out slots.
        let status = unsafe {
            termy_engine_image_data(engine, 5, p.image_generation, &mut format, &mut bytes)
        };
        assert_eq!(status, TermyEngineStatus::Ok);
        assert_eq!(format, IMAGE_FORMAT_RGBA);
        // SAFETY: bytes from this library, freed once.
        unsafe {
            assert_eq!(
                std::slice::from_raw_parts(bytes.ptr, bytes.len)[..4],
                [255, 0, 0, 255]
            );
            crate::termy_engine_bytes_free(&mut bytes);
            termy_engine_free(engine);
        }
    }

    #[test]
    fn a_cropped_placement_shows_only_its_source_rectangle() {
        let engine = engine();
        // The right column of the 2x2 image, at natural size.
        feed(
            engine,
            format!("\x1b_Ga=T,f=32,s=2,v=2,i=1,x=1,w=1,q=2;{PIXELS}\x1b\\").as_bytes(),
        );
        let p = placements(engine, (10.0, 20.0))[0];
        assert_eq!(
            (p.clip_x, p.clip_y, p.clip_width, p.clip_height),
            (0.0, 0.0, 1.0, 2.0)
        );
        // The whole image is shifted left so only its right column shows.
        assert_eq!((p.image_x, p.image_draw_width), (-1.0, 2.0));
        // SAFETY: live handle, freed once.
        unsafe { termy_engine_free(engine) };
    }

    #[test]
    fn deleted_images_leave_the_viewport_and_lookups_fail() {
        let engine = engine();
        feed(
            engine,
            format!("\x1b_Ga=T,f=32,s=2,v=2,i=9,q=2;{PIXELS}\x1b\\").as_bytes(),
        );
        let generation = placements(engine, (10.0, 20.0))[0].image_generation;
        feed(engine, b"\x1b_Ga=d,d=I,i=9,q=2\x1b\\");
        assert!(placements(engine, (10.0, 20.0)).is_empty());

        let mut format = 0;
        let mut bytes = TermyEngineBytes::EMPTY;
        // SAFETY: live handle, writable out slots.
        let status =
            unsafe { termy_engine_image_data(engine, 9, generation, &mut format, &mut bytes) };
        assert_eq!(status, TermyEngineStatus::InvalidArgument);
        // SAFETY: live handle, freed once.
        unsafe { termy_engine_free(engine) };
    }

    #[test]
    fn bad_cell_sizes_are_rejected() {
        let engine = engine();
        let mut out = ptr::null();
        let mut len = 0;
        // SAFETY: live handle, writable out slots.
        let status =
            unsafe { termy_engine_image_placements(engine, 0.0, f32::NAN, &mut out, &mut len) };
        assert_eq!(status, TermyEngineStatus::InvalidArgument);
        // SAFETY: live handle, freed once.
        unsafe { termy_engine_free(engine) };
    }
}
