/// Kitty display dimensions in the same pixel coordinate system as cell metrics.
/// Cell occupancy rounds up, but the actual image must retain fractional dimensions.
pub fn graphics_display_size(
    source_width: u32,
    source_height: u32,
    cols: Option<u32>,
    rows: Option<u32>,
    cell_size: (f32, f32),
    offsets: (u32, u32),
    preserve_aspect: bool,
) -> (f32, f32) {
    let (cell_width, cell_height) = cell_size;
    let (x_offset, y_offset) = if preserve_aspect {
        (0.0, 0.0)
    } else {
        (offsets.0 as f32, offsets.1 as f32)
    };
    let natural_width = source_width as f32;
    let natural_height = source_height as f32;
    match (cols, rows) {
        (Some(cols), Some(rows)) => {
            let (width, height) = (
                (cols as f32 * cell_width - x_offset).max(0.0),
                (rows as f32 * cell_height - y_offset).max(0.0),
            );
            if preserve_aspect {
                let scale = (width / natural_width.max(1.0)).min(height / natural_height.max(1.0));
                (natural_width * scale, natural_height * scale)
            } else {
                (width, height)
            }
        }
        (Some(cols), None) => {
            let width = (cols as f32 * cell_width - x_offset).max(0.0);
            (width, width * natural_height / natural_width.max(1.0))
        }
        (None, Some(rows)) => {
            let height = (rows as f32 * cell_height - y_offset).max(0.0);
            (height * natural_width / natural_height.max(1.0), height)
        }
        (None, None) => (natural_width, natural_height),
    }
}

/// Visible vertical span of a placement after scrolling within page margins.
#[derive(Clone, Copy, Debug)]
pub struct GraphicsRowSpan {
    pub anchor: i64,
    pub rows: u32,
    pub clip_top: u32,
    pub clip_bottom: u32,
}

impl GraphicsRowSpan {
    pub fn visible(&self) -> bool {
        self.clip_top.saturating_add(self.clip_bottom) < self.rows
    }

    /// Positive lines scroll up. Pixels clipped by a margin never reappear.
    pub fn scroll(&mut self, top: i64, bottom: i64, lines: i64) -> bool {
        let visible_top = self.anchor.saturating_add(self.clip_top as i64);
        let visible_bottom = self
            .anchor
            .saturating_add(self.rows as i64)
            .saturating_sub(self.clip_bottom as i64);
        if lines == 0 || visible_top < top || visible_bottom > bottom || !self.visible() {
            return false;
        }
        self.anchor = self.anchor.saturating_sub(lines);
        self.clip_top = self
            .clip_top
            .max(top.saturating_sub(self.anchor).max(0).min(self.rows as i64) as u32);
        self.clip_bottom = self.clip_bottom.max(
            self.anchor
                .saturating_add(self.rows as i64)
                .saturating_sub(bottom)
                .max(0)
                .min(self.rows as i64) as u32,
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_cells_do_not_stretch_images() {
        assert_eq!(
            graphics_display_size(40, 10, Some(2), Some(2), (10.0, 20.0), (3, 4), false),
            (17.0, 36.0)
        );
        assert_eq!(
            graphics_display_size(40, 10, Some(2), None, (10.0, 20.0), (3, 4), false),
            (17.0, 4.25)
        );
        assert_eq!(
            graphics_display_size(13, 7, None, None, (10.0, 20.0), (0, 0), false),
            (13.0, 7.0)
        );
        assert_eq!(
            graphics_display_size(40, 10, Some(3), None, (10.0, 20.0), (0, 0), false),
            (30.0, 7.5)
        );
        assert_eq!(
            graphics_display_size(40, 10, Some(3), Some(2), (10.0, 20.0), (0, 0), true),
            (30.0, 7.5)
        );
    }
}
