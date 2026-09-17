//! Renderer-neutral support for the Kitty terminal graphics protocol.
//!
//! The core owns parsing, upload assembly, storage, placement, deletion and
//! protocol replies. Hosts upload shared RGBA pixels; PNG is encoded only on export.

mod placement;
mod protocol;
pub use protocol::{KittyGraphicsCommand, KittyGraphicsInterceptor, KittyGraphicsItem};
mod animation;
mod deletion;
mod image;
use image::normalize_image;
#[cfg(test)]
mod conformance;

use alacritty_terminal::{
    grid::{Dimensions, Grid},
    index::{Column, Line},
    term::cell::Cell,
    vte::ansi::{Color as AnsiColor, NamedColor},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use flate2::read::ZlibDecoder;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::TerminalSize;

const MAX_IMAGE_BYTES: usize = 128 * 1024 * 1024;
const MAX_CONTROL_BYTES: usize = 4096;
const MAX_CONTROL_FIELDS: usize = 64;
const MAX_COMMAND_BYTES: usize = MAX_IMAGE_BYTES * 2;
const MAX_DIMENSION: u32 = 32_768;
const MAX_PIXELS: u64 = (MAX_IMAGE_BYTES / 4) as u64;
const MAX_PLACEMENTS: usize = 4_096;
const MAX_RELATIVE_DEPTH: usize = 8;

#[derive(Clone, Debug)]
struct StoredImage {
    image: Arc<crate::tmon::GraphicsImage>,
    width: u32,
    height: u32,
    generation: u64,
    number: Option<u32>,
    animation: Option<crate::tmon::GraphicsAnimation>,
}

impl StoredImage {
    fn byte_len(&self) -> usize {
        self.animation
            .as_ref()
            .map_or_else(|| self.image.byte_len(), |animation| animation.byte_len())
    }
}

#[derive(Clone, Debug)]
struct Placement {
    placement_serial: u64,
    screen: KittyGraphicsScreen,
    image_id: u32,
    placement_id: u32,
    location: PlacementLocation,
    source_x: u32,
    source_y: u32,
    source_width: u32,
    source_height: u32,
    display_cols: Option<u32>,
    display_rows: Option<u32>,
    occupied_cols: u32,
    occupied_rows: u32,
    clip_top_rows: u32,
    clip_bottom_rows: u32,
    x_offset: u32,
    y_offset: u32,
    z_index: i32,
}

#[derive(Clone, Debug)]
enum PlacementLocation {
    Direct {
        anchor_line: i64,
        col: usize,
    },
    Virtual,
    Relative {
        parent_image_id: u32,
        parent_placement_id: u32,
        horizontal_offset: i32,
        vertical_offset: i32,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct KittyGraphicsPlaceholder {
    viewport_row: i64,
    col: usize,
    image_id_low: u32,
    image_id_high: u8,
    image_id: u32,
    placement_id: u32,
    image_row: u32,
    image_col: u32,
}

#[derive(Clone, Copy, Debug)]
enum ResolvedOrigin {
    Buffer { anchor_line: i64, col: i64 },
    Viewport { row: i64, col: i64 },
}

#[derive(Clone, Debug)]
struct PendingUpload {
    command: KittyGraphicsCommand,
    decoded: Vec<u8>,
    screen: KittyGraphicsScreen,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KittyGraphicsScreen {
    #[default]
    Primary,
    Alternate,
}

impl KittyGraphicsScreen {
    pub fn from_alternate_screen(alternate_screen: bool) -> Self {
        if alternate_screen {
            Self::Alternate
        } else {
            Self::Primary
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct KittyGraphicsRenderPlacement {
    pub placement_serial: u64,
    pub image_id: u32,
    pub placement_id: u32,
    #[serde(with = "crate::remote::serde_image")]
    pub image: Arc<crate::tmon::GraphicsImage>,
    pub image_width: u32,
    pub image_height: u32,
    pub image_generation: u64,
    #[serde(with = "crate::remote::serde_deadline")]
    pub animation_deadline: Option<std::time::Instant>,
    pub viewport_row: i32,
    pub col: usize,
    pub col_offset: i32,
    pub virtual_cell: Option<(u32, u32)>,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub display_cols: Option<u32>,
    pub display_rows: Option<u32>,
    pub occupied_cols: u32,
    pub occupied_rows: u32,
    pub clip_top_rows: u32,
    pub clip_bottom_rows: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub z_index: i32,
}

#[derive(Default)]
pub struct KittyGraphicsApplyResult {
    pub response: Option<Vec<u8>>,
    pub cursor_advance: Option<(u32, u32)>,
    pub cursor_advance_screen: Option<KittyGraphicsScreen>,
    pub changed: bool,
}

#[derive(Default)]
pub struct KittyGraphicsState {
    images: HashMap<u32, StoredImage>,
    placements: Vec<Placement>,
    insertion_order: VecDeque<u32>,
    pending: Option<PendingUpload>,
    next_anonymous_id: u32,
    stored_bytes: usize,
    next_generation: u64,
    next_placement_serial: u64,
    viewport_rows: u16,
}

pub fn kitty_graphics_placeholders_from_alacritty_grid(
    grid: &Grid<Cell>,
) -> Vec<KittyGraphicsPlaceholder> {
    let display_offset = grid.display_offset();
    let rows = grid.screen_lines();
    let cols = grid.columns();
    let mut placeholders = Vec::new();
    let mut previous = None;
    for viewport_row in 0..rows {
        let line = i32::try_from(viewport_row)
            .unwrap_or(i32::MAX)
            .saturating_sub(i32::try_from(display_offset).unwrap_or(i32::MAX));
        for col in 0..cols {
            let cell = &grid[Line(line)][Column(col)];
            if cell.c != crate::tmon::kitty_graphics_unicode::PLACEHOLDER {
                previous = None;
                continue;
            }
            let image_id_low = ansi_color_to_placeholder_id(cell.fg);
            let placement_id = cell
                .underline_color()
                .map_or(0, ansi_color_to_placeholder_id);
            let [row, image_col, high] = placeholder_diacritics(cell.zerowidth());
            let continuation = previous.filter(|previous: &KittyGraphicsPlaceholder| {
                previous.viewport_row == viewport_row as i64
                    && previous.col.saturating_add(1) == col
                    && previous.image_id_low == image_id_low
                    && previous.placement_id == placement_id
                    && row.is_none_or(|row| row == previous.image_row)
                    && image_col
                        .is_none_or(|image_col| image_col == previous.image_col.saturating_add(1))
                    && high.is_none_or(|high| high == u32::from(previous.image_id_high))
            });
            let image_row = row
                .or_else(|| continuation.map(|value| value.image_row))
                .unwrap_or(0);
            let image_col = image_col
                .or_else(|| continuation.map(|value| value.image_col.saturating_add(1)))
                .unwrap_or(0);
            let image_id_high = high
                .or_else(|| continuation.map(|value| u32::from(value.image_id_high)))
                .and_then(|value| u8::try_from(value).ok())
                .unwrap_or(0);
            let placeholder = KittyGraphicsPlaceholder {
                viewport_row: viewport_row as i64,
                col,
                image_id_low,
                image_id_high,
                image_id: image_id_low | (u32::from(image_id_high) << 24),
                placement_id,
                image_row,
                image_col,
            };
            placeholders.push(placeholder);
            previous = Some(placeholder);
        }
    }
    placeholders
}

fn ansi_color_to_placeholder_id(color: AnsiColor) -> u32 {
    match color {
        AnsiColor::Spec(rgb) => {
            (u32::from(rgb.r) << 16) | (u32::from(rgb.g) << 8) | u32::from(rgb.b)
        }
        AnsiColor::Indexed(index) => u32::from(index),
        AnsiColor::Named(name) if (name as usize) < 16 => name as u32,
        AnsiColor::Named(NamedColor::Foreground) => 0,
        AnsiColor::Named(_) => 0,
    }
}

fn placeholder_diacritics(combining: Option<&[char]>) -> [Option<u32>; 3] {
    let mut decoded = [None; 3];
    for (slot, character) in decoded
        .iter_mut()
        .zip(combining.unwrap_or_default().iter().copied())
    {
        *slot = crate::tmon::kitty_graphics_unicode::diacritic_index(character);
    }
    decoded
}

impl KittyGraphicsState {
    pub fn resize(&mut self, size: TerminalSize) {
        self.viewport_rows = size.rows;
        for placement in &mut self.placements {
            let (width, height) = crate::tmon::graphics_display_size(
                placement.source_width,
                placement.source_height,
                placement.display_cols,
                placement.display_rows,
                (size.cell_width.max(1.0), size.cell_height.max(1.0)),
                (placement.x_offset, placement.y_offset),
                matches!(placement.location, PlacementLocation::Virtual),
            );
            placement.occupied_cols = ((width + placement.x_offset as f32)
                / size.cell_width.max(1.0))
            .ceil()
            .max(1.0) as u32;
            placement.occupied_rows = ((height + placement.y_offset as f32)
                / size.cell_height.max(1.0))
            .ceil()
            .max(1.0) as u32;
        }
    }

    pub fn apply(
        &mut self,
        command: KittyGraphicsCommand,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        size: TerminalSize,
    ) -> KittyGraphicsApplyResult {
        self.apply_on_screen(
            command,
            cursor_col,
            cursor_row,
            history_size,
            size,
            KittyGraphicsScreen::Primary,
        )
    }

    pub fn apply_on_screen(
        &mut self,
        command: KittyGraphicsCommand,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        size: TerminalSize,
        screen: KittyGraphicsScreen,
    ) -> KittyGraphicsApplyResult {
        self.apply_on_screen_with_placeholders(
            command,
            (cursor_col, cursor_row),
            history_size,
            size,
            screen,
            &[],
        )
    }

    pub fn apply_on_screen_with_placeholders(
        &mut self,
        mut command: KittyGraphicsCommand,
        cursor: (usize, usize),
        history_size: usize,
        size: TerminalSize,
        screen: KittyGraphicsScreen,
        placeholders: &[KittyGraphicsPlaceholder],
    ) -> KittyGraphicsApplyResult {
        let (cursor_col, cursor_row) = cursor;
        self.viewport_rows = size.rows;
        log::debug!(
            "kitty graphics command: a={:?} i={:?} p={:?} q={:?} m={:?} f={:?} t={:?} s={:?} v={:?} c={:?} r={:?} payload={}B cursor=({cursor_col},{cursor_row}) screen={screen:?}",
            command.char_value('a'),
            command.u32_value('i'),
            command.u32_value('p'),
            command.u32_value('q'),
            command.u32_value('m'),
            command.u32_value('f'),
            command.char_value('t'),
            command.u32_value('s'),
            command.u32_value('v'),
            command.u32_value('c'),
            command.u32_value('r'),
            command.payload.len(),
        );

        if command.oversized {
            let response_command = self
                .pending
                .take()
                .map_or_else(|| command.clone(), |pending| pending.command);
            return self.failure(
                &response_command,
                "EFBIG:image command exceeds storage limit",
            );
        }

        if let Err(error) = command.validate() {
            let response_command = self
                .pending
                .take()
                .map_or(command, |pending| pending.command);
            return self.failure(&response_command, error);
        }
        if command.value('i').is_some() && command.value('I').is_some() {
            self.pending = None;
            return self.failure(
                &command,
                "EINVAL:image id and image number are mutually exclusive",
            );
        }
        let action = command.char_value('a').unwrap_or('t');
        if matches!(action, 'a' | 'c')
            || (action == 'd' && matches!(command.char_value('d'), Some('f' | 'F')))
        {
            if action == 'd' {
                self.pending = None;
            }
            return match self.edit_animation(&command, None) {
                Ok(id) => {
                    if command.u32_value('I').is_some() {
                        command.control.push(('i', id.to_string()));
                    }
                    self.success(&command, true, None, screen)
                }
                Err(error) => self.failure(&command, &error),
            };
        }

        if action == 'd' {
            return self.delete(
                &command,
                cursor_col,
                cursor_row,
                history_size,
                screen,
                placeholders,
            );
        }
        if action == 'p' {
            return self.put(command, cursor_col, cursor_row, history_size, size, screen);
        }
        if !matches!(action, 't' | 'T' | 'q' | 'f') {
            return self.failure(&command, "EINVAL:unsupported graphics action");
        }

        let encoded = std::mem::take(&mut command.payload);
        let decoded = match BASE64.decode(encoded) {
            Ok(decoded) if decoded.len() <= MAX_IMAGE_BYTES => decoded,
            Ok(_) => {
                let response_command = self
                    .pending
                    .take()
                    .map_or_else(|| command.clone(), |pending| pending.command);
                return self.failure(
                    &response_command,
                    "EFBIG:image payload exceeds storage limit",
                );
            }
            Err(_) => {
                let response_command = self
                    .pending
                    .take()
                    .map_or_else(|| command.clone(), |pending| pending.command);
                return self.failure(&response_command, "EINVAL:invalid base64 payload");
            }
        };

        let more = command.u32_value('m').unwrap_or(0) == 1;
        if let Some(mut pending) = self.pending.take() {
            if command.control.iter().any(|(key, _)| {
                !(matches!(key, 'm' | 'q')
                    || *key == 'a'
                        && command.char_value('a') == Some('f')
                        && pending.command.char_value('a') == Some('f'))
            }) {
                return self.failure(
                    &pending.command,
                    "EINVAL:continuation contains unsupported control data",
                );
            }
            if pending.decoded.len().saturating_add(decoded.len()) > MAX_IMAGE_BYTES {
                return self.failure(
                    &pending.command,
                    "EFBIG:image payload exceeds storage limit",
                );
            }
            pending.decoded.extend_from_slice(&decoded);
            if more {
                self.pending = Some(pending);
                return KittyGraphicsApplyResult::default();
            }
            let mut first = pending.command;
            for (key, value) in command.control {
                if key == 'q' {
                    first.control.push((key, value));
                }
            }
            return self.finish_upload(
                first,
                pending.decoded,
                cursor_col,
                cursor_row,
                history_size,
                size,
                screen,
            );
        }

        if more {
            self.pending = Some(PendingUpload {
                command,
                decoded,
                screen,
            });
            return KittyGraphicsApplyResult::default();
        }
        self.finish_upload(
            command,
            decoded,
            cursor_col,
            cursor_row,
            history_size,
            size,
            screen,
        )
    }

    pub fn render_placements(
        &self,
        history_size: usize,
        display_offset: usize,
        rows: usize,
        cols: usize,
    ) -> Vec<KittyGraphicsRenderPlacement> {
        self.render_placements_on_screen(
            history_size,
            display_offset,
            rows,
            cols,
            KittyGraphicsScreen::Primary,
        )
    }

    pub fn render_placements_on_screen(
        &self,
        history_size: usize,
        display_offset: usize,
        rows: usize,
        cols: usize,
        screen: KittyGraphicsScreen,
    ) -> Vec<KittyGraphicsRenderPlacement> {
        self.render_placements_on_screen_with_placeholders(
            history_size,
            display_offset,
            rows,
            cols,
            screen,
            &[],
        )
    }

    pub fn render_placements_on_screen_with_placeholders(
        &self,
        history_size: usize,
        display_offset: usize,
        rows: usize,
        cols: usize,
        screen: KittyGraphicsScreen,
        placeholders: &[KittyGraphicsPlaceholder],
    ) -> Vec<KittyGraphicsRenderPlacement> {
        let history_size = i64::try_from(history_size).unwrap_or(i64::MAX);
        let display_offset = i64::try_from(display_offset).unwrap_or(i64::MAX);
        let rows_i64 = i64::try_from(rows).unwrap_or(i64::MAX);
        let mut result = Vec::new();
        for placement in self
            .placements
            .iter()
            .filter(|placement| placement.screen == screen)
        {
            let Some(image) = self.images.get(&placement.image_id) else {
                continue;
            };
            let mut emit = |viewport_row: i64, col: i64, virtual_cell: Option<(u32, u32)>| {
                let (occupied_cols, occupied_rows) = if virtual_cell.is_some() {
                    (1, 1)
                } else {
                    (placement.occupied_cols, placement.occupied_rows)
                };
                if viewport_row.saturating_add(occupied_rows as i64) <= 0
                    || viewport_row >= rows_i64
                    || col.saturating_add(occupied_cols as i64) <= 0
                    || col >= cols as i64
                {
                    return;
                }
                result.push(KittyGraphicsRenderPlacement {
                    placement_serial: placement.placement_serial,
                    image_id: placement.image_id,
                    placement_id: placement.placement_id,
                    image: image.image.clone(),
                    image_width: image.width,
                    image_height: image.height,
                    image_generation: image.generation,
                    animation_deadline: image
                        .animation
                        .as_ref()
                        .and_then(|animation| animation.next_deadline()),
                    viewport_row: viewport_row.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
                    col: col.max(0) as usize,
                    col_offset: col.min(0).max(i32::MIN as i64) as i32,
                    virtual_cell,
                    source_x: placement.source_x,
                    source_y: placement.source_y,
                    source_width: placement.source_width,
                    source_height: placement.source_height,
                    display_cols: placement.display_cols,
                    display_rows: placement.display_rows,
                    occupied_cols,
                    occupied_rows,
                    clip_top_rows: placement.clip_top_rows,
                    clip_bottom_rows: placement.clip_bottom_rows,
                    x_offset: if virtual_cell.is_some() {
                        0
                    } else {
                        placement.x_offset
                    },
                    y_offset: if virtual_cell.is_some() {
                        0
                    } else {
                        placement.y_offset
                    },
                    z_index: placement.z_index,
                });
            };
            if matches!(placement.location, PlacementLocation::Virtual) {
                for cell in placeholders {
                    if cell.image_id != placement.image_id
                        || (cell.placement_id != 0 && cell.placement_id != placement.placement_id)
                        || cell.image_col
                            >= placement.display_cols.unwrap_or(placement.occupied_cols)
                        || cell.image_row
                            >= placement.display_rows.unwrap_or(placement.occupied_rows)
                    {
                        continue;
                    }
                    // A zero underline color selects one prototype, never every placement.
                    if cell.placement_id == 0
                        && self
                            .placements
                            .iter()
                            .rev()
                            .find(|p| {
                                p.screen == screen
                                    && p.image_id == cell.image_id
                                    && matches!(p.location, PlacementLocation::Virtual)
                            })
                            .is_some_and(|p| p.placement_serial != placement.placement_serial)
                    {
                        continue;
                    }
                    emit(
                        cell.viewport_row,
                        cell.col as i64,
                        Some((cell.image_col, cell.image_row)),
                    );
                }
            } else if let Some(origin) = self.resolve_render_origin(placement, placeholders) {
                let (row, col) = match origin {
                    ResolvedOrigin::Buffer { anchor_line, col } => (
                        anchor_line
                            .saturating_sub(history_size)
                            .saturating_add(display_offset),
                        col,
                    ),
                    ResolvedOrigin::Viewport { row, col } => (row, col),
                };
                emit(row, col, None);
            }
        }
        result.sort_by_key(|placement| {
            (
                placement.z_index,
                placement.image_id,
                placement.placement_serial,
            )
        });
        result
    }

    pub fn clear_visible(&mut self) -> bool {
        self.clear_visible_on_screen(KittyGraphicsScreen::Primary)
    }

    pub fn has_placements(&self) -> bool {
        !self.placements.is_empty()
    }

    pub fn has_virtual_placements(&self) -> bool {
        self.placements
            .iter()
            .any(|placement| matches!(placement.location, PlacementLocation::Virtual))
    }

    pub fn reset(&mut self) -> bool {
        let changed = !self.placements.is_empty() || self.pending.is_some();
        self.placements.clear();
        self.pending = None;
        changed
    }

    pub fn clear_visible_on_screen(&mut self, screen: KittyGraphicsScreen) -> bool {
        let before = self.placements.len();
        self.placements.retain(|placement| {
            placement.screen != screen || matches!(placement.location, PlacementLocation::Virtual)
        });
        self.remove_orphaned_relative_placements();
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.screen == screen)
        {
            self.pending = None;
        }
        before != self.placements.len()
    }

    pub fn clear_viewport_on_screen(
        &mut self,
        screen: KittyGraphicsScreen,
        history_size: usize,
        rows: usize,
        cols: usize,
    ) -> bool {
        if rows == 0 || cols == 0 {
            return false;
        }

        let viewport_start = i64::try_from(history_size).unwrap_or(i64::MAX);
        let viewport_end = viewport_start.saturating_add(i64::try_from(rows).unwrap_or(i64::MAX));
        let before = self.placements.len();
        self.placements.retain(|placement| {
            if placement.screen != screen {
                return true;
            }

            let PlacementLocation::Direct { anchor_line, col } = placement.location else {
                return true;
            };

            let placement_end = anchor_line.saturating_add(i64::from(placement.occupied_rows));
            let vertically_visible = anchor_line < viewport_end && placement_end > viewport_start;
            let horizontally_visible = col < cols && placement.occupied_cols > 0;
            !(vertically_visible && horizontally_visible)
        });
        self.remove_orphaned_relative_placements();
        before != self.placements.len()
    }

    /// Move placements with a screen scroll that was not recorded in history.
    ///
    /// Normal primary-screen scrolling is represented by a growing history
    /// size and is handled by [`Self::render_placements`]. Alternate-screen,
    /// zero-history, and full-history grids rotate rows without growing that
    /// value, so their placements need the equivalent anchor adjustment here.
    pub fn scroll_up_without_history(&mut self, lines: usize) -> bool {
        self.scroll_up_without_history_on_screen(lines, KittyGraphicsScreen::Primary)
    }

    pub fn scroll_up_without_history_on_screen(
        &mut self,
        lines: usize,
        screen: KittyGraphicsScreen,
    ) -> bool {
        if lines == 0
            || !self.placements.iter().any(|placement| {
                placement.screen == screen
                    && matches!(placement.location, PlacementLocation::Direct { .. })
            })
        {
            return false;
        }

        let lines = i64::try_from(lines).unwrap_or(i64::MAX);
        for placement in self.placements.iter_mut().filter(|placement| {
            placement.screen == screen
                && matches!(placement.location, PlacementLocation::Direct { .. })
        }) {
            if let PlacementLocation::Direct { anchor_line, .. } = &mut placement.location {
                *anchor_line = anchor_line.saturating_sub(lines);
            }
        }
        self.placements.retain(|placement| {
            placement.screen != screen
                || !matches!(
                    placement.location,
                    PlacementLocation::Direct { anchor_line, .. }
                        if anchor_line.saturating_add(i64::from(placement.occupied_rows)) <= 0
                )
        });
        self.remove_orphaned_relative_placements();
        true
    }

    pub fn scroll_region_on_screen(
        &mut self,
        screen: KittyGraphicsScreen,
        top: usize,
        bottom: usize,
        lines: i64,
        history_size: usize,
    ) -> bool {
        let mut changed = false;
        self.placements.retain_mut(|placement| {
            if placement.screen != screen {
                return true;
            }
            let PlacementLocation::Direct { anchor_line, .. } = &mut placement.location else {
                return true;
            };
            let mut span = crate::tmon::GraphicsRowSpan {
                anchor: *anchor_line - history_size as i64,
                rows: placement.occupied_rows,
                clip_top: placement.clip_top_rows,
                clip_bottom: placement.clip_bottom_rows,
            };
            if span.scroll(top as i64, bottom as i64, lines) {
                *anchor_line = span.anchor + history_size as i64;
                placement.clip_top_rows = span.clip_top;
                placement.clip_bottom_rows = span.clip_bottom;
                changed = true;
            }
            span.visible()
        });
        if changed {
            self.remove_orphaned_relative_placements();
        }
        changed
    }

    pub fn preserve_primary_placements_across_partial_history_growth(
        &mut self,
        lines: usize,
    ) -> bool {
        if lines == 0
            || !self.placements.iter().any(|placement| {
                placement.screen == KittyGraphicsScreen::Primary
                    && matches!(placement.location, PlacementLocation::Direct { .. })
            })
        {
            return false;
        }

        let lines = i64::try_from(lines).unwrap_or(i64::MAX);
        for placement in self.placements.iter_mut().filter(|placement| {
            placement.screen == KittyGraphicsScreen::Primary
                && matches!(placement.location, PlacementLocation::Direct { .. })
        }) {
            // Alacritty grows history when a partial DECSTBM region starts at
            // the top. Kitty placements are not region-aware, so cancel that
            // global history offset rather than moving fixed footer images.
            if let PlacementLocation::Direct { anchor_line, .. } = &mut placement.location {
                *anchor_line = anchor_line.saturating_add(lines);
            }
        }
        true
    }

    fn finish_upload(
        &mut self,
        mut command: KittyGraphicsCommand,
        decoded: Vec<u8>,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        size: TerminalSize,
        screen: KittyGraphicsScreen,
    ) -> KittyGraphicsApplyResult {
        let data = match self.resolve_transmission_data(&command, decoded) {
            Ok(data) => data,
            Err(error) => return self.failure(&command, &error),
        };
        let data = match command.char_value('o') {
            None => data,
            Some('z') => match decompress_zlib(&data) {
                Ok(data) => data,
                Err(error) => return self.failure(&command, &error),
            },
            Some(_) => return self.failure(&command, "EINVAL:unsupported compression"),
        };
        let (image, width, height) = match normalize_image(&command, data) {
            Ok(image) => image,
            Err(error) => return self.failure(&command, &error),
        };

        if command.char_value('a') == Some('f') {
            return match self.edit_animation(&command, Some(image)) {
                Ok(id) => {
                    if command.u32_value('I').is_some() {
                        command.control.push(('i', id.to_string()));
                    }
                    self.success(&command, true, None, screen)
                }
                Err(error) => self.failure(&command, &error),
            };
        }
        if command.char_value('a').unwrap_or('t') == 'q' {
            return self.success(&command, false, None, screen);
        }

        let requested_id = command.u32_value('i').unwrap_or(0);
        let image_id = if requested_id == 0 {
            self.allocate_anonymous_id()
        } else {
            requested_id
        };
        let number = command.u32_value('I').filter(|number| *number > 0);
        if number.is_some() {
            command.control.push(('i', image_id.to_string()));
        }
        let next_generation = self.next_generation.wrapping_add(1).max(1);
        let image = StoredImage {
            image: Arc::new(image),
            width,
            height,
            generation: next_generation,
            number,
            animation: None,
        };
        let byte_len = image.byte_len();
        let Some(evictions) = self.quota_evictions_for_replacement(image_id, byte_len) else {
            return self.failure(&command, "ENOSPC:image storage quota exceeded");
        };
        let staged = if command.char_value('a') == Some('T') {
            match self.placement_for_image(
                &image,
                image_id,
                &command,
                (cursor_col, cursor_row),
                history_size,
                size,
                screen,
            ) {
                Ok(placement) => Some(placement),
                Err(error) => return self.failure(&command, &error),
            }
        } else {
            None
        };
        for id in evictions {
            self.remove_image(id);
        }
        self.remove_image(image_id);
        self.next_generation = next_generation;
        self.images.insert(image_id, image);
        self.insertion_order.push_back(image_id);
        self.stored_bytes = self.stored_bytes.saturating_add(byte_len);
        let cursor_advance = staged.and_then(|(placement, advance)| {
            self.commit_placement(placement);
            advance
        });
        self.success(&command, true, cursor_advance, screen)
    }

    fn put(
        &mut self,
        mut command: KittyGraphicsCommand,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        size: TerminalSize,
        screen: KittyGraphicsScreen,
    ) -> KittyGraphicsApplyResult {
        let image_id = self.resolve_image_id(&command).unwrap_or(0);
        if command.u32_value('I').is_some() && image_id != 0 {
            command.control.push(('i', image_id.to_string()));
        }
        if image_id == 0 || !self.images.contains_key(&image_id) {
            return self.failure(&command, "ENOENT:image id not found");
        }
        match self.add_placement(
            image_id,
            &command,
            cursor_col,
            cursor_row,
            history_size,
            size,
            screen,
        ) {
            Ok(advance) => self.success(&command, true, advance, screen),
            Err(error) => self.failure(&command, &error),
        }
    }

    fn add_placement(
        &mut self,
        image_id: u32,
        command: &KittyGraphicsCommand,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        size: TerminalSize,
        screen: KittyGraphicsScreen,
    ) -> Result<Option<(u32, u32)>, String> {
        let image = self
            .images
            .get(&image_id)
            .ok_or("ENOENT:image id not found")?;
        let (placement, advance) = self.placement_for_image(
            image,
            image_id,
            command,
            (cursor_col, cursor_row),
            history_size,
            size,
            screen,
        )?;
        self.commit_placement(placement);
        Ok(advance)
    }

    fn placement_for_image(
        &self,
        image: &StoredImage,
        image_id: u32,
        command: &KittyGraphicsCommand,
        cursor: (usize, usize),
        history_size: usize,
        size: TerminalSize,
        screen: KittyGraphicsScreen,
    ) -> Result<(Placement, Option<(u32, u32)>), String> {
        let (cursor_col, cursor_row) = cursor;
        let source_x = command.u32_value('x').unwrap_or(0).min(image.width);
        let source_y = command.u32_value('y').unwrap_or(0).min(image.height);
        let source_width = command
            .u32_value('w')
            .filter(|value| *value > 0)
            .unwrap_or(image.width.saturating_sub(source_x))
            .min(image.width.saturating_sub(source_x));
        let source_height = command
            .u32_value('h')
            .filter(|value| *value > 0)
            .unwrap_or(image.height.saturating_sub(source_y))
            .min(image.height.saturating_sub(source_y));
        if source_width == 0 || source_height == 0 {
            return Err("EINVAL:empty source rectangle".into());
        }

        let cell_width = size.cell_width.max(1.0);
        let cell_height = size.cell_height.max(1.0);
        let display_cols = command.u32_value('c').filter(|value| *value > 0);
        let display_rows = command.u32_value('r').filter(|value| *value > 0);
        let virtual_placement = command.u32_value('U').unwrap_or(0) == 1;
        let x_offset = (if virtual_placement {
            0
        } else {
            command.u32_value('X').unwrap_or(0)
        })
        .min((cell_width.ceil() as u32).saturating_sub(1));
        let y_offset = (if virtual_placement {
            0
        } else {
            command.u32_value('Y').unwrap_or(0)
        })
        .min((cell_height.ceil() as u32).saturating_sub(1));
        let mut placed_source_width = source_width;
        if display_cols.is_none()
            && display_rows.is_none()
            && !virtual_placement
            && command.value('P').is_none()
        {
            let available = (usize::from(size.cols).saturating_sub(cursor_col) as f32 * cell_width
                - x_offset as f32)
                .max(1.0);
            placed_source_width = source_width.min(available.floor() as u32);
        }
        let (width, height) = crate::tmon::graphics_display_size(
            placed_source_width,
            source_height,
            display_cols,
            display_rows,
            (cell_width, cell_height),
            (x_offset, y_offset),
            virtual_placement,
        );
        let occupied_cols = ((width + x_offset as f32) / cell_width).ceil().max(1.0) as u32;
        let occupied_rows = ((height + y_offset as f32) / cell_height).ceil().max(1.0) as u32;
        let placement_id = command.u32_value('p').unwrap_or(0);
        let virtual_placement = command.u32_value('U').unwrap_or(0) == 1;
        let relative_parent = command.u32_value('P').filter(|id| *id > 0);
        if virtual_placement && relative_parent.is_some() {
            return Err("EINVAL:a virtual placement cannot be relative".into());
        }
        let location = if virtual_placement {
            PlacementLocation::Virtual
        } else if let Some(parent_image_id) = relative_parent {
            let parent_placement_id = command.u32_value('Q').unwrap_or(0);
            self.validate_relative_parent(
                screen,
                image_id,
                placement_id,
                parent_image_id,
                parent_placement_id,
            )?;
            PlacementLocation::Relative {
                parent_image_id,
                parent_placement_id,
                horizontal_offset: command.i32_value('H').unwrap_or(0),
                vertical_offset: command.i32_value('V').unwrap_or(0),
            }
        } else {
            PlacementLocation::Direct {
                anchor_line: i64::try_from(history_size)
                    .unwrap_or(i64::MAX)
                    .saturating_add(i64::try_from(cursor_row).unwrap_or(i64::MAX)),
                col: cursor_col,
            }
        };
        let placement = Placement {
            placement_serial: self.next_placement_serial.wrapping_add(1).max(1),
            screen,
            image_id,
            placement_id,
            location,
            source_x,
            source_y,
            source_width: placed_source_width,
            source_height,
            display_cols,
            display_rows,
            occupied_cols,
            occupied_rows,
            clip_top_rows: 0,
            clip_bottom_rows: 0,
            x_offset,
            y_offset,
            z_index: command.i32_value('z').unwrap_or(0),
        };
        let advances_cursor = !virtual_placement && relative_parent.is_none();
        let advance = (advances_cursor && command.u32_value('C').unwrap_or(0) == 0)
            .then_some((occupied_cols, occupied_rows));
        Ok((placement, advance))
    }

    fn commit_placement(&mut self, placement: Placement) {
        if placement.placement_id != 0 {
            self.placements.retain(|existing| {
                existing.screen != placement.screen
                    || existing.image_id != placement.image_id
                    || existing.placement_id != placement.placement_id
            });
        }
        self.next_placement_serial = placement.placement_serial;
        self.placements.push(placement);
        if self.placements.len() > MAX_PLACEMENTS {
            let overflow = self.placements.len() - MAX_PLACEMENTS;
            self.placements.drain(..overflow);
            self.remove_orphaned_relative_placements();
        }
    }

    fn resolve_transmission_data(
        &self,
        command: &KittyGraphicsCommand,
        decoded: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        match command.char_value('t').unwrap_or('d') {
            'd' => Ok(decoded),
            'f' | 't' => {
                let temporary = command.char_value('t') == Some('t');
                let path = PathBuf::from(
                    std::str::from_utf8(&decoded).map_err(|_| "EINVAL:file path is not UTF-8")?,
                );
                let result = read_regular_file(
                    &path,
                    command.u32_value('O').unwrap_or(0) as u64,
                    command
                        .u32_value('S')
                        .filter(|size| *size > 0)
                        .map(u64::from),
                );
                if temporary && result.is_ok() && temporary_path_can_be_removed(&path) {
                    let _ = std::fs::remove_file(&path);
                }
                result
            }
            's' => crate::tmon::read_graphics_shared_memory(
                &decoded,
                u64::from(command.u32_value('O').unwrap_or(0)),
                command
                    .u32_value('S')
                    .filter(|size| *size > 0)
                    .map(u64::from),
                MAX_IMAGE_BYTES,
            ),
            _ => Err("EINVAL:unsupported transmission medium".into()),
        }
    }

    fn resolve_image_id(&self, command: &KittyGraphicsCommand) -> Option<u32> {
        command.u32_value('i').or_else(|| {
            let number = command.u32_value('I')?;
            self.insertion_order.iter().rev().copied().find(|id| {
                self.images
                    .get(id)
                    .is_some_and(|image| image.number == Some(number))
            })
        })
    }

    fn allocate_anonymous_id(&mut self) -> u32 {
        if self.next_anonymous_id == 0 {
            self.next_anonymous_id = u32::MAX;
        }
        while self.images.contains_key(&self.next_anonymous_id) {
            self.next_anonymous_id = if self.next_anonymous_id <= 1 {
                u32::MAX
            } else {
                self.next_anonymous_id - 1
            };
        }
        let id = self.next_anonymous_id;
        self.next_anonymous_id = if self.next_anonymous_id <= 1 {
            u32::MAX
        } else {
            self.next_anonymous_id - 1
        };
        id
    }

    fn quota_evictions_for_replacement(&self, image_id: u32, new_bytes: usize) -> Option<Vec<u32>> {
        let old_bytes = self
            .images
            .get(&image_id)
            .map_or(0, |image| image.byte_len());
        let mut stored_bytes = self
            .stored_bytes
            .saturating_sub(old_bytes)
            .saturating_add(new_bytes);
        let mut image_count = self.images.len() + usize::from(!self.images.contains_key(&image_id));
        if stored_bytes <= MAX_IMAGE_BYTES && image_count <= 4096 {
            return Some(Vec::new());
        }

        let placed = self
            .placements
            .iter()
            .map(|placement| placement.image_id)
            .collect::<HashSet<_>>();
        let mut evictions = Vec::new();
        for candidate in self
            .insertion_order
            .iter()
            .copied()
            .filter(|candidate| *candidate != image_id && !placed.contains(candidate))
        {
            if stored_bytes <= MAX_IMAGE_BYTES && image_count <= 4096 {
                break;
            }
            let Some(image) = self.images.get(&candidate) else {
                continue;
            };
            stored_bytes = stored_bytes.saturating_sub(image.byte_len());
            image_count = image_count.saturating_sub(1);
            evictions.push(candidate);
        }
        (stored_bytes <= MAX_IMAGE_BYTES && image_count <= 4096).then_some(evictions)
    }

    fn remove_image(&mut self, image_id: u32) {
        if let Some(image) = self.images.remove(&image_id) {
            self.stored_bytes = self.stored_bytes.saturating_sub(image.byte_len());
        }
        self.placements
            .retain(|placement| placement.image_id != image_id);
        self.remove_orphaned_relative_placements();
        self.insertion_order.retain(|id| *id != image_id);
    }

    fn success(
        &self,
        command: &KittyGraphicsCommand,
        changed: bool,
        cursor_advance: Option<(u32, u32)>,
        screen: KittyGraphicsScreen,
    ) -> KittyGraphicsApplyResult {
        log::debug!(
            "kitty graphics ok: a={:?} i={:?} p={:?} changed={changed} cursor_advance={cursor_advance:?}",
            command.char_value('a'),
            command.u32_value('i'),
            command.u32_value('p'),
        );
        KittyGraphicsApplyResult {
            response: response(command, true, "OK"),
            cursor_advance,
            cursor_advance_screen: cursor_advance.map(|_| screen),
            changed,
        }
    }

    fn failure(&self, command: &KittyGraphicsCommand, message: &str) -> KittyGraphicsApplyResult {
        // Logged even when q=2 suppresses the wire reply, so silent client
        // failures remain visible in host logs.
        log::warn!(
            "kitty graphics error: a={:?} i={:?} p={:?} q={:?} {message}",
            command.char_value('a'),
            command.u32_value('i'),
            command.u32_value('p'),
            command.u32_value('q'),
        );
        KittyGraphicsApplyResult {
            response: response(command, false, message),
            ..KittyGraphicsApplyResult::default()
        }
    }
}

fn response(command: &KittyGraphicsCommand, success: bool, message: &str) -> Option<Vec<u8>> {
    let quiet = command.u32_value('q').unwrap_or(0);
    if (success && quiet >= 1) || (!success && quiet >= 2) {
        return None;
    }
    let image_id = command.u32_value('i').or_else(|| command.u32_value('I'))?;
    let mut control = format!("i={image_id}");
    if let Some(number) = command.u32_value('I') {
        control.push_str(&format!(",I={number}"));
    }
    if let Some(placement_id) = command.u32_value('p') {
        control.push_str(&format!(",p={placement_id}"));
    }
    Some(format!("\x1b_G{control};{message}\x1b\\").into_bytes())
}

fn validate_dimensions(width: u32, height: u32, channels: usize) -> Result<usize, String> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err("EINVAL:invalid image dimensions".into());
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PIXELS {
        return Err("EFBIG:image dimensions exceed storage limit".into());
    }
    usize::try_from(pixels)
        .ok()
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| "EFBIG:image dimensions overflow".into())
}

fn decompress_zlib(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    ZlibDecoder::new(data)
        .take(MAX_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut output)
        .map_err(|_| "EINVAL:invalid zlib payload".to_string())?;
    if output.len() > MAX_IMAGE_BYTES {
        return Err("EFBIG:decompressed image exceeds storage limit".into());
    }
    Ok(output)
}

fn read_regular_file(path: &Path, offset: u64, size: Option<u64>) -> Result<Vec<u8>, String> {
    // Reject FIFOs and devices before opening: a child controls this path and a
    // blocking FIFO open would otherwise stall the parser thread indefinitely.
    let initial_metadata =
        std::fs::metadata(path).map_err(|_| "ENOENT:unable to open image file".to_string())?;
    if !initial_metadata.is_file() {
        return Err("EINVAL:invalid image file".into());
    }
    let mut file = open_image_file(path)?;
    let metadata = file
        .metadata()
        .map_err(|_| "EIO:unable to inspect image file".to_string())?;
    if !metadata.is_file() || offset > metadata.len() {
        return Err("EINVAL:invalid image file".into());
    }
    let length = size
        .unwrap_or(metadata.len() - offset)
        .min(metadata.len() - offset);
    if length > MAX_IMAGE_BYTES as u64 {
        return Err("EFBIG:image file exceeds storage limit".into());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| "EIO:unable to seek image file".to_string())?;
    let mut output = Vec::with_capacity(length as usize);
    file.take(length)
        .read_to_end(&mut output)
        .map_err(|_| "EIO:unable to read image file".to_string())?;
    Ok(output)
}

fn open_image_file(path: &Path) -> Result<File, String> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const NONBLOCK: i32 = 0o4000;
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    const NONBLOCK: i32 = 0x0004;

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};

        OpenOptions::new()
            .read(true)
            .custom_flags(NONBLOCK)
            .open(path)
            .map_err(|_| "ENOENT:unable to open image file".to_string())
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )))]
    File::open(path).map_err(|_| "ENOENT:unable to open image file".to_string())
}

fn temporary_path_can_be_removed(path: &Path) -> bool {
    let path_text = path.to_string_lossy();
    if !path_text.contains("tty-graphics-protocol") {
        return false;
    }
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    let mut roots = vec![PathBuf::from("/tmp"), PathBuf::from("/private/tmp")];
    roots.push(std::env::temp_dir());
    roots.into_iter().any(|root| {
        root.canonicalize()
            .is_ok_and(|root| canonical.starts_with(root))
    })
}

#[cfg(test)]
mod tests;
