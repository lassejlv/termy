//! Kitty graphics state for engine snapshots: stored images (with their
//! animations), placements, an upload still arriving in chunks, and the
//! counters that pick later ids, generations and quota evictions.
//!
//! Images are written once each, however many frames or entries share them,
//! as PNG-compressed pixels (or their PNG source when they have no decoded
//! pixels). Animation deadlines are written relative to the encode time.
//! Not included: an upload handed to the host undecoded by a deferring feed.

use std::{collections::HashMap, sync::Arc, time::Duration};

use web_time::Instant;

use super::{
    KittyGraphicsCommand, KittyGraphicsScreen, KittyGraphicsState, MAX_CONTROL_FIELDS,
    MAX_IMAGE_BYTES, MAX_PLACEMENTS, PendingUpload, Placement, PlacementLocation, StoredImage,
    validate_dimensions,
};
use crate::terminal_engine::{
    media::{GraphicsAnimation, GraphicsFrame, GraphicsImage, decode_rgba_png, encode_png},
    snapshot::{Reader, Result, SnapshotError::Corrupt, Writer},
};

const MAX_IMAGES: usize = 4096;
const MAX_FRAMES: usize = 4096;
const RGBA: u8 = 0;
const PNG: u8 = 1;

fn screen_code(screen: KittyGraphicsScreen) -> u8 {
    match screen {
        KittyGraphicsScreen::Primary => 0,
        KittyGraphicsScreen::Alternate => 1,
    }
}

fn read_screen(reader: &mut Reader<'_>) -> Result<KittyGraphicsScreen> {
    match reader.u8()? {
        0 => Ok(KittyGraphicsScreen::Primary),
        1 => Ok(KittyGraphicsScreen::Alternate),
        _ => Err(Corrupt("graphics screen")),
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Every distinct image, in first-use order.
#[derive(Default)]
struct Images<'a> {
    list: Vec<&'a Arc<GraphicsImage>>,
    index: HashMap<*const GraphicsImage, u32>,
}

impl<'a> Images<'a> {
    fn add(&mut self, image: &'a Arc<GraphicsImage>) {
        let next = self.list.len() as u32;
        self.index.entry(Arc::as_ptr(image)).or_insert_with(|| {
            self.list.push(image);
            next
        });
    }

    fn of(&self, image: &Arc<GraphicsImage>) -> u32 {
        self.index[&Arc::as_ptr(image)]
    }
}

fn write_image(writer: &mut Writer, image: &GraphicsImage) {
    writer.u32_value(image.width);
    writer.u32_value(image.height);
    match image.snapshot_parts() {
        (Some(rgba), _) => {
            writer.u8(RGBA);
            writer.bytes(&encode_png(image.width, image.height, 4, rgba));
        }
        (None, Some((png, charged))) => {
            writer.u8(PNG);
            writer.bytes(png);
            writer.usize(charged);
        }
        // Every constructor sets one of them.
        (None, None) => {
            writer.u8(PNG);
            writer.bytes(&[]);
            writer.usize(0);
        }
    }
}

/// Reads one image, charging what it holds to `charged_total` before any
/// pixels are decoded. Distinct images never hold more than the storage quota
/// in a live state, so a snapshot past it is rejected instead of decoded.
fn read_image(reader: &mut Reader<'_>, charged_total: &mut usize) -> Result<GraphicsImage> {
    let width = reader.u32_value()?;
    let height = reader.u32_value()?;
    validate_dimensions(width, height, 4).map_err(|_| Corrupt("image dimensions"))?;
    let pixel_bytes = width as usize * height as usize * 4;
    match reader.u8()? {
        RGBA => {
            charge_image(charged_total, pixel_bytes)?;
            let pixels =
                decode_rgba_png(reader.bytes()?, width, height).ok_or(Corrupt("image pixels"))?;
            Ok(GraphicsImage::from_rgba(width, height, pixels))
        }
        PNG => {
            let png = reader.bytes()?.to_vec();
            // The source is held as well, whatever the snapshot claims.
            let charged = reader.usize()?.max(png.len());
            if charged > MAX_IMAGE_BYTES * 2 {
                return Err(Corrupt("image source"));
            }
            charge_image(charged_total, pixel_bytes.saturating_add(charged))?;
            Ok(GraphicsImage::from_png_charged(width, height, png, charged))
        }
        _ => Err(Corrupt("image kind")),
    }
}

fn charge_image(charged_total: &mut usize, bytes: usize) -> Result<()> {
    *charged_total = charged_total.saturating_add(bytes);
    if *charged_total > MAX_IMAGE_BYTES {
        return Err(Corrupt("graphics images"));
    }
    Ok(())
}

fn write_placement(writer: &mut Writer, placement: &Placement) {
    writer.varint(placement.placement_serial);
    writer.u8(screen_code(placement.screen));
    writer.u32_value(placement.image_id);
    writer.u32_value(placement.placement_id);
    match placement.location {
        PlacementLocation::Direct { anchor_line, col } => {
            writer.u8(0);
            writer.i64(anchor_line);
            writer.usize(col);
        }
        PlacementLocation::Virtual => writer.u8(1),
        PlacementLocation::Relative {
            parent_image_id,
            parent_placement_id,
            horizontal_offset,
            vertical_offset,
        } => {
            writer.u8(2);
            writer.u32_value(parent_image_id);
            writer.u32_value(parent_placement_id);
            writer.i64(horizontal_offset.into());
            writer.i64(vertical_offset.into());
        }
    }
    for value in [
        placement.source_x,
        placement.source_y,
        placement.source_width,
        placement.source_height,
    ] {
        writer.u32_value(value);
    }
    writer.option_u32(placement.display_cols);
    writer.option_u32(placement.display_rows);
    for value in [
        placement.occupied_cols,
        placement.occupied_rows,
        placement.clip_top_rows,
        placement.clip_bottom_rows,
        placement.x_offset,
        placement.y_offset,
    ] {
        writer.u32_value(value);
    }
    writer.i64(placement.z_index.into());
}

fn read_placement(reader: &mut Reader<'_>) -> Result<Placement> {
    let placement_serial = reader.varint()?;
    let screen = read_screen(reader)?;
    let image_id = reader.u32_value()?;
    let placement_id = reader.u32_value()?;
    let location = match reader.u8()? {
        0 => PlacementLocation::Direct {
            anchor_line: reader.i64()?,
            col: reader.usize()?,
        },
        1 => PlacementLocation::Virtual,
        2 => PlacementLocation::Relative {
            parent_image_id: reader.u32_value()?,
            parent_placement_id: reader.u32_value()?,
            horizontal_offset: reader.i32()?,
            vertical_offset: reader.i32()?,
        },
        _ => return Err(Corrupt("placement location")),
    };
    let mut source = [0; 4];
    for value in &mut source {
        *value = reader.u32_value()?;
    }
    let display_cols = reader.option_u32()?;
    let display_rows = reader.option_u32()?;
    let mut layout = [0; 6];
    for value in &mut layout {
        *value = reader.u32_value()?;
    }
    let [source_x, source_y, source_width, source_height] = source;
    let [
        occupied_cols,
        occupied_rows,
        clip_top_rows,
        clip_bottom_rows,
        x_offset,
        y_offset,
    ] = layout;
    Ok(Placement {
        placement_serial,
        screen,
        image_id,
        placement_id,
        location,
        source_x,
        source_y,
        source_width,
        source_height,
        display_cols,
        display_rows,
        occupied_cols,
        occupied_rows,
        clip_top_rows,
        clip_bottom_rows,
        x_offset,
        y_offset,
        z_index: reader.i32()?,
    })
}

fn write_command(writer: &mut Writer, command: &KittyGraphicsCommand) {
    writer.usize(command.control.len());
    for (key, value) in &command.control {
        writer.u32_value(u32::from(*key));
        writer.str(value);
    }
    writer.bytes(&command.payload);
    writer.bool(command.oversized);
}

fn read_command(reader: &mut Reader<'_>) -> Result<KittyGraphicsCommand> {
    let fields = reader.count(2)?;
    if fields > MAX_CONTROL_FIELDS {
        return Err(Corrupt("graphics command"));
    }
    let mut control = Vec::with_capacity(fields);
    for _ in 0..fields {
        let key = char::from_u32(reader.u32_value()?).ok_or(Corrupt("graphics command"))?;
        control.push((key, reader.string()?));
    }
    Ok(KittyGraphicsCommand {
        control,
        payload: reader.bytes()?.to_vec(),
        oversized: reader.bool()?,
        inline: None,
    })
}

impl KittyGraphicsState {
    pub(crate) fn write_snapshot(&self, writer: &mut Writer) {
        let mut images = Images::default();
        // Stored in insertion order, which also orders quota evictions.
        let stored: Vec<_> = self
            .insertion_order
            .iter()
            .filter_map(|id| Some((*id, self.images.get(id)?)))
            .collect();
        for (_, image) in &stored {
            images.add(&image.image);
            for frame in image
                .animation
                .iter()
                .flat_map(|animation| &animation.frames)
            {
                images.add(&frame.image);
            }
        }
        writer.usize(images.list.len());
        for image in &images.list {
            write_image(writer, image);
        }

        let now = Instant::now();
        writer.usize(stored.len());
        for (id, image) in stored {
            writer.u32_value(id);
            writer.u32_value(images.of(&image.image));
            writer.u32_value(image.width);
            writer.u32_value(image.height);
            writer.varint(image.generation);
            writer.option_u32(image.number);
            writer.bool(image.anonymous);
            writer.bool(image.animation.is_some());
            let Some(animation) = &image.animation else {
                continue;
            };
            writer.usize(animation.frames.len());
            for frame in &animation.frames {
                writer.u32_value(images.of(&frame.image));
                writer.varint(frame.generation);
                writer.u32_value(frame.gap_ms);
            }
            writer.usize(animation.current);
            let (state, loops, completed_loops, deadline) = animation.playback();
            writer.u32_value(state);
            writer.option_u32(loops);
            writer.u32_value(completed_loops);
            writer.bool(deadline.is_some());
            let remaining = deadline.map_or(0, |deadline| {
                if writer.canonical() {
                    0
                } else {
                    millis(deadline.saturating_duration_since(now))
                }
            });
            writer.varint(remaining);
        }

        writer.usize(self.placements.len());
        for placement in &self.placements {
            write_placement(writer, placement);
        }
        writer.bool(self.pending.is_some());
        if let Some(pending) = &self.pending {
            write_command(writer, &pending.command);
            writer.bytes(&pending.decoded);
            writer.u8(screen_code(pending.screen));
        }
        writer.u32_value(self.next_anonymous_id);
        writer.usize(self.stored_bytes);
        writer.varint(self.next_generation);
        writer.varint(self.next_placement_serial);
        writer.u16(self.viewport_rows);
    }

    pub(crate) fn read_snapshot(reader: &mut Reader<'_>) -> Result<Self> {
        let count = reader.count(4)?;
        if count > MAX_IMAGES * (MAX_FRAMES + 1) {
            return Err(Corrupt("graphics images"));
        }
        let mut images = Vec::with_capacity(count.min(MAX_IMAGES));
        let mut charged_total = 0;
        for _ in 0..count {
            images.push(Arc::new(read_image(reader, &mut charged_total)?));
        }
        let image = |reader: &mut Reader<'_>| {
            images
                .get(reader.u32_value()? as usize)
                .cloned()
                .ok_or(Corrupt("graphics image index"))
        };

        let mut state = Self::default();
        let stored = reader.count(4)?;
        if stored > MAX_IMAGES {
            return Err(Corrupt("graphics images"));
        }
        let now = Instant::now();
        for _ in 0..stored {
            let id = reader.u32_value()?;
            let root = image(reader)?;
            let width = reader.u32_value()?;
            let height = reader.u32_value()?;
            if (width, height) != (root.width, root.height) {
                return Err(Corrupt("image dimensions"));
            }
            let generation = reader.varint()?;
            let number = reader.option_u32()?;
            let anonymous = reader.bool()?;
            let animation = if reader.bool()? {
                let count = reader.count(3)?;
                if count == 0 || count > MAX_FRAMES + 1 {
                    return Err(Corrupt("animation frames"));
                }
                let mut frames = Vec::with_capacity(count);
                for _ in 0..count {
                    let frame = GraphicsFrame {
                        image: image(reader)?,
                        generation: reader.varint()?,
                        gap_ms: reader.u32_value()?,
                    };
                    // Frame edits composite at the root's size.
                    if (frame.image.width, frame.image.height) != (width, height) {
                        return Err(Corrupt("animation frame"));
                    }
                    frames.push(frame);
                }
                let current = reader.usize()?;
                if current >= frames.len() {
                    return Err(Corrupt("animation frame"));
                }
                let playback_state = reader.u32_value()?;
                let loops = reader.option_u32()?;
                let completed_loops = reader.u32_value()?;
                let has_deadline = reader.bool()?;
                let remaining = Duration::from_millis(reader.varint()?);
                let deadline = has_deadline.then(|| now.checked_add(remaining).unwrap_or(now));
                Some(GraphicsAnimation::from_playback(
                    frames,
                    current,
                    (playback_state, loops, completed_loops, deadline),
                ))
            } else {
                None
            };
            let entry = StoredImage {
                image: root,
                width,
                height,
                generation,
                number,
                anonymous,
                animation,
            };
            state.anonymous_images += usize::from(anonymous);
            if state.images.insert(id, entry).is_some() {
                return Err(Corrupt("graphics image id"));
            }
            state.insertion_order.push_back(id);
        }

        let placements = reader.count(8)?;
        if placements > MAX_PLACEMENTS {
            return Err(Corrupt("graphics placements"));
        }
        for _ in 0..placements {
            let placement = read_placement(reader)?;
            if !state.images.contains_key(&placement.image_id) {
                return Err(Corrupt("graphics placement"));
            }
            if matches!(placement.location, PlacementLocation::Virtual) {
                state.virtual_placements[placement.screen.index()] += 1;
            }
            state.placements.push(placement);
        }
        if reader.bool()? {
            let command = read_command(reader)?;
            let decoded = reader.bytes()?.to_vec();
            if decoded.len() > MAX_IMAGE_BYTES {
                return Err(Corrupt("graphics upload"));
            }
            state.pending = Some(PendingUpload {
                command,
                decoded,
                screen: read_screen(reader)?,
            });
        }
        state.next_anonymous_id = reader.u32_value()?;
        // Recomputed rather than trusted: quota evictions rely on it.
        let _ = reader.usize()?;
        state.stored_bytes = state.recount_stored_bytes()?;
        state.next_generation = reader.varint()?;
        state.next_placement_serial = reader.varint()?;
        state.viewport_rows = reader.u16()?;
        Ok(state)
    }

    /// What the stored images charge to the quota, as uploads keep it.
    fn recount_stored_bytes(&self) -> Result<usize> {
        let stored_bytes = self.images.values().fold(0_usize, |total, image| {
            total.saturating_add(image.byte_len())
        });
        if stored_bytes > MAX_IMAGE_BYTES {
            return Err(Corrupt("graphics storage"));
        }
        Ok(stored_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(width: u32, height: u32) -> StoredImage {
        StoredImage {
            image: Arc::new(GraphicsImage::from_png(width, height, Vec::new())),
            width,
            height,
            generation: 1,
            number: None,
            anonymous: false,
            animation: None,
        }
    }

    #[test]
    fn stored_bytes_are_recounted_from_the_images() {
        let mut state = KittyGraphicsState::default();
        state.images.insert(1, stored(2, 3));
        state.images.insert(2, stored(4, 1));
        // A snapshot claiming nothing is stored must not hide the images
        // from the quota.
        state.stored_bytes = 0;
        assert_eq!(state.recount_stored_bytes(), Ok(2 * 3 * 4 + 4 * 4));
    }

    #[test]
    fn stored_images_past_the_quota_are_rejected() {
        let mut state = KittyGraphicsState::default();
        // Each is exactly the quota, so one fits and two don't.
        state.images.insert(1, stored(8192, 4096));
        assert_eq!(state.recount_stored_bytes(), Ok(MAX_IMAGE_BYTES));
        state.images.insert(2, stored(1, 1));
        assert!(state.recount_stored_bytes().is_err());
    }

    #[test]
    fn decoded_images_are_charged_against_one_budget() {
        let mut charged = 0;
        assert!(charge_image(&mut charged, MAX_IMAGE_BYTES - 4).is_ok());
        assert!(charge_image(&mut charged, 4).is_ok());
        assert!(charge_image(&mut charged, 1).is_err());
        let mut charged = 0;
        assert!(charge_image(&mut charged, usize::MAX).is_err());
    }
}
