use super::*;
use std::collections::HashSet;

impl KittyGraphicsState {
    pub(super) fn delete(
        &mut self,
        command: &KittyGraphicsCommand,
        cursor_col: usize,
        cursor_row: usize,
        history_size: usize,
        screen: KittyGraphicsScreen,
        placeholders: &[KittyGraphicsPlaceholder],
    ) -> KittyGraphicsApplyResult {
        self.pending = None;
        let selector = command.char_value('d').unwrap_or('a');
        let free = selector.is_ascii_uppercase();
        let selector = selector.to_ascii_lowercase();
        if !matches!(
            selector,
            'a' | 'i' | 'n' | 'c' | 'p' | 'q' | 'r' | 'x' | 'y' | 'z'
        ) {
            return self.failure(command, "EINVAL:unsupported delete selector");
        }
        let id = self.resolve_image_id(command);
        let placement_id = command.u32_value('p').unwrap_or(0);
        let low = command.u32_value('x').unwrap_or(0);
        let high = command.u32_value('y').unwrap_or(u32::MAX);
        if selector == 'r' && low > high {
            return self.failure(command, "EINVAL:invalid image id range");
        }
        let row = if selector == 'c' {
            cursor_row as i64
        } else {
            command.u32_value('y').unwrap_or(1).saturating_sub(1) as i64
        };
        let col = if selector == 'c' {
            cursor_col as i64
        } else {
            command.u32_value('x').unwrap_or(1).saturating_sub(1) as i64
        };
        let z = command.i32_value('z').unwrap_or(0);
        let mut ids = HashSet::new();
        if matches!(selector, 'i' | 'n') && placement_id == 0 {
            ids.extend(id);
        }
        if selector == 'r' {
            ids.extend(
                self.images
                    .keys()
                    .copied()
                    .filter(|id| *id >= low && *id <= high),
            );
        }
        let removed: HashSet<_> = self
            .placements
            .iter()
            .filter(|placement| {
                if placement.screen != screen && !matches!(selector, 'i' | 'n' | 'r') {
                    return false;
                }
                let selected = match selector {
                    'i' | 'n' => {
                        Some(placement.image_id) == id
                            && (placement_id == 0 || placement.placement_id == placement_id)
                    }
                    'r' => placement.image_id >= low && placement.image_id <= high,
                    _ if matches!(placement.location, PlacementLocation::Virtual) => false,
                    _ => {
                        let Some(origin) = self.resolve_render_origin(placement, placeholders)
                        else {
                            return false;
                        };
                        let (top, origin_col) = match origin {
                            ResolvedOrigin::Buffer { anchor_line, col } => {
                                (anchor_line.saturating_sub(history_size as i64), col)
                            }
                            ResolvedOrigin::Viewport { row, col } => (row, col),
                        };
                        let top = top.saturating_add(placement.clip_top_rows as i64);
                        let rows = placement
                            .occupied_rows
                            .saturating_sub(placement.clip_top_rows)
                            .saturating_sub(placement.clip_bottom_rows);
                        let left = origin_col;
                        let hits_row = row >= top && row < top.saturating_add(rows as i64);
                        let hits_col = col >= left
                            && col < left.saturating_add(placement.occupied_cols as i64);
                        match selector {
                            'a' => {
                                top.saturating_add(rows as i64) > 0
                                    && top < i64::from(self.viewport_rows)
                            }
                            'c' | 'p' => hits_row && hits_col,
                            'q' => hits_row && hits_col && placement.z_index == z,
                            'x' => hits_col,
                            'y' => hits_row,
                            'z' => placement.z_index == z,
                            _ => false,
                        }
                    }
                };
                if selected {
                    ids.insert(placement.image_id);
                }
                selected
            })
            .map(|placement| placement.placement_serial)
            .collect();
        let before = self.placements.len();
        self.placements
            .retain(|placement| !removed.contains(&placement.placement_serial));
        self.remove_orphaned_relative_placements();
        if free {
            for id in ids {
                if !self
                    .placements
                    .iter()
                    .any(|placement| placement.image_id == id)
                {
                    self.remove_image(id);
                }
            }
        }
        self.success(command, self.placements.len() != before, None, screen)
    }
}
