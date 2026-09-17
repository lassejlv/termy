use super::*;
use crate::tmon::{
    grid::{Color, Combining},
    kitty_graphics_unicode::{PLACEHOLDER, diacritic_index},
};

impl GraphicsState {
    pub(super) fn placement_by_key(
        &self,
        alternate: bool,
        image_id: u32,
        placement_id: u32,
    ) -> Option<&Placement> {
        self.placements.iter().find(|placement| {
            placement.alternate == alternate
                && placement.image_id == image_id
                && (placement_id == 0 || placement.placement_id == placement_id)
        })
    }

    pub(super) fn validate_relative_parent(
        &self,
        alternate: bool,
        image_id: u32,
        placement_id: u32,
        parent_image_id: u32,
        parent_placement_id: u32,
    ) -> Result<(), String> {
        if image_id == parent_image_id && placement_id == parent_placement_id {
            return Err("ECYCLE:a placement cannot be relative to itself".into());
        }
        let mut parent = self
            .placement_by_key(alternate, parent_image_id, parent_placement_id)
            .ok_or_else(|| "ENOPARENT:relative placement parent not found".to_string())?;
        for depth in 1..=MAX_RELATIVE_DEPTH {
            let PlacementLocation::Relative {
                parent_image_id,
                parent_placement_id,
                ..
            } = parent.location
            else {
                return Ok(());
            };
            if parent_image_id == image_id && parent_placement_id == placement_id {
                return Err("ECYCLE:relative placement cycle".into());
            }
            parent = self
                .placement_by_key(alternate, parent_image_id, parent_placement_id)
                .ok_or_else(|| "ENOPARENT:relative placement parent not found".to_string())?;
            if depth == MAX_RELATIVE_DEPTH {
                return Err("ETOODEEP:relative placement chain is too deep".into());
            }
        }
        unreachable!()
    }

    pub(super) fn resolve_render_origin(
        &self,
        placement: &Placement,
        placeholders: &[UnicodePlaceholder],
    ) -> Option<ResolvedOrigin> {
        let mut current = placement;
        let mut horizontal_offset = 0i64;
        let mut vertical_offset = 0i64;
        let relative = matches!(placement.location, PlacementLocation::Relative { .. });
        for _ in 0..=MAX_RELATIVE_DEPTH {
            match current.location {
                PlacementLocation::Direct { anchor_line, col } => {
                    let col = i64::try_from(col)
                        .unwrap_or(i64::MAX)
                        .saturating_add(horizontal_offset);
                    return Some(ResolvedOrigin::Buffer {
                        anchor_line: anchor_line.saturating_add(vertical_offset),
                        col,
                    });
                }
                PlacementLocation::Virtual => {
                    let matching = placeholders.iter().filter(|placeholder| {
                        placeholder.image_id == current.image_id
                            && (placeholder.placement_id == current.placement_id
                                || (placeholder.placement_id == 0
                                    && self
                                        .placements
                                        .iter()
                                        .rev()
                                        .find(|candidate| {
                                            candidate.alternate == current.alternate
                                                && candidate.image_id == current.image_id
                                                && matches!(
                                                    candidate.location,
                                                    PlacementLocation::Virtual
                                                )
                                        })
                                        .is_some_and(|candidate| {
                                            candidate.serial == current.serial
                                        })))
                    });
                    let (row, col) = if relative {
                        matching.fold(None, |origin, placeholder| {
                            let candidate = (placeholder.viewport_row, placeholder.col);
                            Some(origin.map_or(candidate, |(row, col): (i64, usize)| {
                                (row.min(candidate.0), col.min(candidate.1))
                            }))
                        })?
                    } else {
                        matching.fold(None, |origin, placeholder| {
                            let row = placeholder
                                .viewport_row
                                .saturating_sub(i64::from(placeholder.image_row));
                            let col = placeholder
                                .col
                                .saturating_sub(placeholder.image_col as usize);
                            Some(
                                origin.map_or((row, col), |(old_row, old_col): (i64, usize)| {
                                    (old_row.min(row), old_col.min(col))
                                }),
                            )
                        })?
                    };
                    let col = i64::try_from(col)
                        .unwrap_or(i64::MAX)
                        .saturating_add(horizontal_offset);
                    return Some(ResolvedOrigin::Viewport {
                        row: row.saturating_add(vertical_offset),
                        col,
                    });
                }
                PlacementLocation::Relative {
                    parent_image_id,
                    parent_placement_id,
                    horizontal_offset: horizontal,
                    vertical_offset: vertical,
                } => {
                    horizontal_offset = horizontal_offset.saturating_add(i64::from(horizontal));
                    vertical_offset = vertical_offset.saturating_add(i64::from(vertical));
                    current = self.placement_by_key(
                        placement.alternate,
                        parent_image_id,
                        parent_placement_id,
                    )?;
                }
            }
        }
        None
    }

    pub(super) fn unicode_placeholders(&self, grid: &Grid) -> Vec<UnicodePlaceholder> {
        if !self
            .placements
            .iter()
            .any(|placement| matches!(placement.location, PlacementLocation::Virtual))
        {
            return Vec::new();
        }
        let mut placeholders = Vec::new();
        let mut previous = None;
        grid.visit_viewport_cells(|display_offset, line, col, cell, combining| {
            let viewport_row =
                i64::from(line).saturating_add(i64::try_from(display_offset).unwrap_or(i64::MAX));
            if cell.character != PLACEHOLDER {
                previous = None;
                return;
            }
            let image_id_low = color_to_placeholder_id(cell.foreground);
            let placement_id = cell.underline_color.map_or(0, color_to_placeholder_id);
            let [row, image_col, high] = placeholder_diacritics(combining);
            let continuation = previous.filter(|previous: &UnicodePlaceholder| {
                previous.viewport_row == viewport_row
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
            let placeholder = UnicodePlaceholder {
                viewport_row,
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
        });
        placeholders
    }

    pub(super) fn remove_orphaned_relative_placements(&mut self) {
        let mut removed_images = std::collections::HashSet::new();
        loop {
            let existing: std::collections::HashSet<_> = self
                .placements
                .iter()
                .flat_map(|p| {
                    [
                        (p.alternate, p.image_id, p.placement_id),
                        (p.alternate, p.image_id, 0),
                    ]
                })
                .collect();
            let before = self.placements.len();
            self.placements.retain(|placement| {
                let PlacementLocation::Relative {
                    parent_image_id,
                    parent_placement_id,
                    ..
                } = placement.location
                else {
                    return true;
                };
                let keep =
                    existing.contains(&(placement.alternate, parent_image_id, parent_placement_id));
                if !keep {
                    removed_images.insert(placement.image_id);
                }
                keep
            });
            if before == self.placements.len() {
                break;
            }
        }
        for id in removed_images {
            if !self.placements.iter().any(|p| p.image_id == id) {
                if let Some(image) = self.images.remove(&id) {
                    self.stored_bytes = self.stored_bytes.saturating_sub(image.byte_len());
                }
                self.insertion_order.retain(|candidate| *candidate != id);
            }
        }
    }
}

fn color_to_placeholder_id(color: Color) -> u32 {
    match color {
        Color::Default => 0,
        Color::Indexed(index) => u32::from(index),
        Color::Rgb { r, g, b } => (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
    }
}

fn placeholder_diacritics(combining: Option<Combining<'_>>) -> [Option<u32>; 3] {
    let mut decoded = [None; 3];
    match combining {
        Some(Combining::Character(character)) => decoded[0] = diacritic_index(character),
        Some(Combining::Text(text)) => {
            for (slot, character) in decoded.iter_mut().zip(text.chars()) {
                *slot = diacritic_index(character);
            }
        }
        None => {}
    }
    decoded
}
