use super::*;

impl KittyGraphicsState {
    pub(super) fn placement_by_key(
        &self,
        screen: KittyGraphicsScreen,
        image_id: u32,
        placement_id: u32,
    ) -> Option<&Placement> {
        self.placements.iter().find(|placement| {
            placement.screen == screen
                && placement.image_id == image_id
                && (placement_id == 0 || placement.placement_id == placement_id)
        })
    }

    pub(super) fn validate_relative_parent(
        &self,
        screen: KittyGraphicsScreen,
        image_id: u32,
        placement_id: u32,
        parent_image_id: u32,
        parent_placement_id: u32,
    ) -> Result<(), String> {
        if image_id == parent_image_id && placement_id == parent_placement_id {
            return Err("ECYCLE:a placement cannot be relative to itself".into());
        }
        let mut parent = self
            .placement_by_key(screen, parent_image_id, parent_placement_id)
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
                .placement_by_key(screen, parent_image_id, parent_placement_id)
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
        placeholders: &[KittyGraphicsPlaceholder],
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
                                            candidate.screen == current.screen
                                                && candidate.image_id == current.image_id
                                                && matches!(
                                                    candidate.location,
                                                    PlacementLocation::Virtual
                                                )
                                        })
                                        .is_some_and(|candidate| {
                                            candidate.placement_serial == current.placement_serial
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
                        placement.screen,
                        parent_image_id,
                        parent_placement_id,
                    )?;
                }
            }
        }
        None
    }

    pub(super) fn remove_orphaned_relative_placements(&mut self) {
        let mut removed_images = std::collections::HashSet::new();
        loop {
            let existing: std::collections::HashSet<_> = self
                .placements
                .iter()
                .flat_map(|p| {
                    [
                        (p.screen, p.image_id, p.placement_id),
                        (p.screen, p.image_id, 0),
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
                    existing.contains(&(placement.screen, parent_image_id, parent_placement_id));
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
