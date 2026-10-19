/// Fitted image and placement box in the same pixel coordinate system as cell metrics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphicsDisplayLayout {
    pub image_size: (f32, f32),
    pub placement_size: (f32, f32),
    /// Letterbox/pillarbox offset of a virtual placement's fitted image within
    /// its box; zero for direct placements, which fill theirs.
    pub image_offset: (f32, f32),
}

/// Kitty image dimensions, as drawn inside the placement box.
/// Use `graphics_display_layout` for the placement box and centering offsets.
pub fn graphics_display_size(
    source_width: u32,
    source_height: u32,
    cols: Option<u32>,
    rows: Option<u32>,
    cell_size: (f32, f32),
    offsets: (u32, u32),
    virtual_placement: bool,
) -> (f32, f32) {
    graphics_display_layout(
        source_width,
        source_height,
        cols,
        rows,
        cell_size,
        offsets,
        virtual_placement,
    )
    .image_size
}

/// The requested box for cursor movement and clipping, and the image drawn in it.
///
/// As in kitty and ghostty, a direct placement with both `c` and `r` stretches
/// the image over that box. A virtual placement (Unicode placeholders) instead
/// fits the image inside its box with the aspect ratio kept, centered, and
/// ignores pixel offsets, as required by the protocol.
pub fn graphics_display_layout(
    source_width: u32,
    source_height: u32,
    cols: Option<u32>,
    rows: Option<u32>,
    cell_size: (f32, f32),
    offsets: (u32, u32),
    virtual_placement: bool,
) -> GraphicsDisplayLayout {
    let (cell_width, cell_height) = cell_size;
    let (x_offset, y_offset) = if virtual_placement {
        (0.0, 0.0)
    } else {
        (offsets.0 as f32, offsets.1 as f32)
    };
    let natural_width = source_width as f32;
    let natural_height = source_height as f32;
    let placement_size = match (cols, rows) {
        // As in kitty, a virtual box spans whole cells, and an omitted
        // dimension comes from the image's natural size.
        _ if virtual_placement => {
            let cells = |count: Option<u32>, natural: f32, cell: f32| {
                count.map_or_else(|| (natural / cell).ceil().max(1.0), |count| count as f32)
            };
            (
                cells(cols, natural_width, cell_width) * cell_width,
                cells(rows, natural_height, cell_height) * cell_height,
            )
        }
        (Some(cols), Some(rows)) => (
            (cols as f32 * cell_width - x_offset).max(0.0),
            (rows as f32 * cell_height - y_offset).max(0.0),
        ),
        (Some(cols), None) => {
            let width = (cols as f32 * cell_width - x_offset).max(0.0);
            (width, width * natural_height / natural_width.max(1.0))
        }
        (None, Some(rows)) => {
            let height = (rows as f32 * cell_height - y_offset).max(0.0);
            (height * natural_width / natural_height.max(1.0), height)
        }
        (None, None) => (natural_width, natural_height),
    };
    let image_size = if virtual_placement {
        let scale = (placement_size.0 / natural_width.max(1.0))
            .min(placement_size.1 / natural_height.max(1.0));
        (natural_width * scale, natural_height * scale)
    } else {
        placement_size
    };
    GraphicsDisplayLayout {
        image_size,
        placement_size,
        image_offset: (
            (placement_size.0 - image_size.0) / 2.0,
            (placement_size.1 - image_size.1) / 2.0,
        ),
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
    fn direct_boxes_stretch_the_image_like_kitty() {
        let wide = graphics_display_layout(40, 10, Some(2), Some(2), (10.0, 20.0), (0, 0), false);
        assert_eq!(wide.image_size, (20.0, 40.0));
        assert_eq!(wide.placement_size, (20.0, 40.0));
        assert_eq!(wide.image_offset, (0.0, 0.0));
        let offset = graphics_display_layout(40, 10, Some(2), Some(2), (10.0, 20.0), (3, 4), false);
        assert_eq!(offset.placement_size, (17.0, 36.0));
        assert_eq!(offset.image_size, (17.0, 36.0));
        assert_eq!(offset.image_offset, (0.0, 0.0));
    }

    #[test]
    fn virtual_boxes_fit_and_center_the_image_in_whole_cells() {
        let wide = graphics_display_layout(40, 10, Some(2), Some(2), (10.0, 20.0), (3, 4), true);
        assert_eq!(wide.image_size, (20.0, 5.0));
        assert_eq!(wide.placement_size, (20.0, 40.0));
        assert_eq!(wide.image_offset, (0.0, 17.5));
        let tall = graphics_display_layout(10, 40, Some(2), Some(2), (10.0, 20.0), (0, 0), true);
        assert_eq!(tall.image_size, (10.0, 40.0));
        assert_eq!(tall.image_offset, (5.0, 0.0));
        // An omitted dimension spans the image's natural size in whole cells.
        let rows = graphics_display_layout(40, 10, Some(3), None, (10.0, 20.0), (0, 0), true);
        assert_eq!(rows.placement_size, (30.0, 20.0));
        assert_eq!(rows.image_size, (30.0, 7.5));
        assert_eq!(rows.image_offset, (0.0, 6.25));
        let natural = graphics_display_layout(20, 10, None, None, (10.0, 20.0), (0, 0), true);
        assert_eq!(natural.placement_size, (20.0, 20.0));
        assert_eq!(natural.image_size, (20.0, 10.0));
        assert_eq!(natural.image_offset, (0.0, 5.0));
    }

    #[test]
    fn a_single_dimension_keeps_the_aspect_ratio() {
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
