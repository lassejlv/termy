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
        let mut parent = self
            .placement_by_key(screen, parent_image_id, parent_placement_id)
            .ok_or_else(|| "ENOPARENT:relative placement parent not found".to_string())?;
        for depth in 1..=MAX_RELATIVE_DEPTH {
            // Q=0 selects an existing placement. Compare its resolved identity,
            // not the wildcard, before accepting a replacement of that placement.
            if placement_id != 0
                && parent.image_id == image_id
                && parent.placement_id == placement_id
            {
                return Err("ECYCLE:relative placement cycle".into());
            }
            let PlacementLocation::Relative {
                parent_image_id,
                parent_placement_id,
                ..
            } = parent.location
            else {
                return Ok(());
            };
            parent = self
                .placement_by_key(screen, parent_image_id, parent_placement_id)
                .ok_or_else(|| "ENOPARENT:relative placement parent not found".to_string())?;
            if depth == MAX_RELATIVE_DEPTH {
                return Err("ETOODEEP:relative placement chain is too deep".into());
            }
        }
        unreachable!()
    }

    pub(super) fn placement_for_image(
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
        let (width, height) = crate::terminal_engine::media::graphics_display_layout(
            placed_source_width,
            source_height,
            display_cols,
            display_rows,
            (cell_width, cell_height),
            (x_offset, y_offset),
            virtual_placement,
        )
        .placement_size;
        let occupied_cols = occupied_cells(display_cols, width + x_offset as f32, cell_width);
        let occupied_rows = occupied_cells(display_rows, height + y_offset as f32, cell_height);
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

    pub(super) fn retain_placements(&mut self, mut keep: impl FnMut(&Placement) -> bool) {
        let (virtual_placements, relative_placements) =
            (&mut self.virtual_placements, &mut self.relative_placements);
        self.placements.retain(|placement| {
            if keep(placement) {
                return true;
            }
            count_placement(virtual_placements, relative_placements, placement, false);
            false
        });
    }

    /// Append without the cap; snapshot decoding checks the count itself.
    pub(super) fn push_placement(&mut self, placement: Placement) {
        count_placement(
            &mut self.virtual_placements,
            &mut self.relative_placements,
            &placement,
            true,
        );
        self.placements.push_back(placement);
    }

    pub(super) fn commit_placement(&mut self, placement: Placement) {
        if placement.placement_id != 0 {
            self.retain_placements(|existing| {
                existing.screen != placement.screen
                    || existing.image_id != placement.image_id
                    || existing.placement_id != placement.placement_id
            });
        }
        self.next_placement_serial = placement.placement_serial;
        self.push_placement(placement);
        if self.placements.len() > MAX_PLACEMENTS {
            self.evict_overflowing_placements();
        }
    }

    /// Drop the oldest placements over [`MAX_PLACEMENTS`]. A program that
    /// keeps placing images reaches the cap and stays there, so this runs
    /// for every later placement and must not shift or rebuild anything
    /// the size of the list: the front is popped, and orphans are looked
    /// for only when the evicted placement was some relative placement's
    /// parent.
    ///
    /// The placement just added, and the parents it is relative to, are
    /// kept: it was validated against them, and must not be removed as an
    /// orphan after its OK reply.
    fn evict_overflowing_placements(&mut self) {
        let newest = self.placements.back().expect("a placement was just added");
        let mut kept = vec![newest.placement_serial];
        let mut current = newest;
        while let PlacementLocation::Relative {
            parent_image_id,
            parent_placement_id,
            ..
        } = current.location
            && kept.len() <= MAX_RELATIVE_DEPTH
            && let Some(parent) =
                self.placement_by_key(current.screen, parent_image_id, parent_placement_id)
        {
            kept.push(parent.placement_serial);
            current = parent;
        }
        let mut orphans = false;
        while self.placements.len() > MAX_PLACEMENTS {
            // At most `kept.len()` placements are skipped, so the search and
            // the removal near the front take constant time.
            let index = self
                .placements
                .iter()
                .position(|placement| !kept.contains(&placement.placement_serial))
                .expect("the cap is far above the deepest relative chain");
            let removed = self
                .placements
                .remove(index)
                .expect("the index was just found");
            count_placement(
                &mut self.virtual_placements,
                &mut self.relative_placements,
                &removed,
                false,
            );
            orphans = orphans || self.orphans_children_of(&removed);
            if !orphans {
                self.release_if_unplaced_anonymous(removed.image_id);
            }
        }
        if orphans {
            // This also releases id-less images left without placements.
            self.remove_orphaned_relative_placements();
        }
    }

    /// Whether removing `removed` left a relative placement without its
    /// parent, by the lookup of [`Self::placement_by_key`]: `Q=0` names any
    /// placement of the image, any other `Q` the placement with that id.
    fn orphans_children_of(&self, removed: &Placement) -> bool {
        if self.relative_placements == 0 {
            return false;
        }
        let (mut image_left, mut key_left) = (false, false);
        for placement in &self.placements {
            if placement.screen == removed.screen && placement.image_id == removed.image_id {
                image_left = true;
                key_left |= placement.placement_id == removed.placement_id;
            }
        }
        if image_left && (key_left || removed.placement_id == 0) {
            return false;
        }
        self.placements.iter().any(|placement| {
            let PlacementLocation::Relative {
                parent_image_id,
                parent_placement_id,
                ..
            } = placement.location
            else {
                return false;
            };
            placement.screen == removed.screen
                && parent_image_id == removed.image_id
                && if parent_placement_id == 0 {
                    !image_left
                } else {
                    parent_placement_id == removed.placement_id && !key_left
                }
        })
    }

    /// [`Self::release_unplaced_anonymous_images`] for one image, without
    /// collecting every placed image.
    fn release_if_unplaced_anonymous(&mut self, image_id: u32) {
        if self
            .images
            .get(&image_id)
            .is_some_and(|image| image.anonymous)
            && !self
                .placements
                .iter()
                .any(|placement| placement.image_id == image_id)
        {
            self.forget_image(image_id);
        }
    }

    /// Remove relative placements whose parent is gone. Their images stay:
    /// named ones can be placed again, and id-less ones are released below.
    pub(super) fn remove_orphaned_relative_placements(&mut self) {
        #[cfg(test)]
        ORPHAN_SWEEPS.with(|sweeps| sweeps.set(sweeps.get() + 1));
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
            self.retain_placements(|placement| {
                let PlacementLocation::Relative {
                    parent_image_id,
                    parent_placement_id,
                    ..
                } = placement.location
                else {
                    return true;
                };
                existing.contains(&(placement.screen, parent_image_id, parent_placement_id))
            });
            if before == self.placements.len() {
                break;
            }
        }
        self.release_unplaced_anonymous_images();
    }
}

/// Update the counts kept beside the placement list for `placement` being
/// added or removed.
fn count_placement(
    virtual_placements: &mut [usize; 2],
    relative_placements: &mut usize,
    placement: &Placement,
    added: bool,
) {
    let count = match placement.location {
        PlacementLocation::Virtual => &mut virtual_placements[placement.screen.index()],
        PlacementLocation::Relative { .. } => relative_placements,
        PlacementLocation::Direct { .. } => return,
    };
    if added {
        *count += 1;
    } else {
        *count -= 1;
    }
}

/// Resolves placement origins for one command or snapshot. Parents are found
/// through an index built on the first relative hop, and each virtual
/// parent's placeholder scan runs once, instead of once per placement.
pub(super) struct OriginResolver<'a> {
    placements: &'a VecDeque<Placement>,
    placeholders: &'a [KittyGraphicsPlaceholder],
    by_key: Option<HashMap<(KittyGraphicsScreen, u32, u32), usize>>,
    virtual_origins: HashMap<(u64, bool), Option<(i64, usize)>>,
}

impl<'a> OriginResolver<'a> {
    pub(super) fn new(
        placements: &'a VecDeque<Placement>,
        placeholders: &'a [KittyGraphicsPlaceholder],
    ) -> Self {
        Self {
            placements,
            placeholders,
            by_key: None,
            virtual_origins: HashMap::new(),
        }
    }

    /// Same lookup as [`KittyGraphicsState::placement_by_key`]: `Q=0` selects
    /// the image's first placement.
    fn parent(
        &mut self,
        screen: KittyGraphicsScreen,
        image_id: u32,
        placement_id: u32,
    ) -> Option<&'a Placement> {
        let placements = self.placements;
        let by_key = self.by_key.get_or_insert_with(|| {
            let mut by_key = HashMap::with_capacity(placements.len() * 2);
            for (index, placement) in placements.iter().enumerate() {
                let (screen, image_id) = (placement.screen, placement.image_id);
                by_key
                    .entry((screen, image_id, placement.placement_id))
                    .or_insert(index);
                by_key.entry((screen, image_id, 0)).or_insert(index);
            }
            by_key
        });
        by_key
            .get(&(screen, image_id, placement_id))
            .map(|index| &placements[*index])
    }

    fn virtual_origin(&mut self, current: &Placement, relative: bool) -> Option<(i64, usize)> {
        if let Some(origin) = self
            .virtual_origins
            .get(&(current.placement_serial, relative))
        {
            return *origin;
        }
        #[cfg(test)]
        VIRTUAL_ORIGIN_SCANS.with(|scans| scans.set(scans.get() + 1));
        // Whether `current` is the image's prototype is the same for every
        // placeholder, so resolve it once.
        let prototype = self
            .placements
            .iter()
            .rev()
            .find(|candidate| {
                candidate.screen == current.screen
                    && candidate.image_id == current.image_id
                    && matches!(candidate.location, PlacementLocation::Virtual)
            })
            .is_some_and(|candidate| candidate.placement_serial == current.placement_serial);
        let matching = self.placeholders.iter().filter(|placeholder| {
            placeholder.image_id == current.image_id
                && (placeholder.placement_id == current.placement_id
                    || (placeholder.placement_id == 0 && prototype))
        });
        let origin = if relative {
            matching.fold(None, |origin, placeholder| {
                let candidate = (placeholder.viewport_row, placeholder.col);
                Some(origin.map_or(candidate, |(row, col): (i64, usize)| {
                    (row.min(candidate.0), col.min(candidate.1))
                }))
            })
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
            })
        };
        self.virtual_origins
            .insert((current.placement_serial, relative), origin);
        origin
    }

    pub(super) fn resolve(&mut self, placement: &'a Placement) -> Option<ResolvedOrigin> {
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
                    let (row, col) = self.virtual_origin(current, relative)?;
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
                    current =
                        self.parent(placement.screen, parent_image_id, parent_placement_id)?;
                }
            }
        }
        None
    }
}

#[cfg(test)]
thread_local! {
    pub(super) static VIRTUAL_ORIGIN_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static ORPHAN_SWEEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests;
