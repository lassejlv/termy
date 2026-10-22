/// Fitted image and placement box in the same pixel coordinate system as cell metrics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphicsDisplayLayout {
    pub image_size: (f32, f32),
    pub placement_size: (f32, f32),
    /// Letterbox/pillarbox offset within the placement, after the cell offsets.
    pub image_offset: (f32, f32),
}

/// Kitty image dimensions. Use `graphics_display_layout` for the placement
/// box and centering offsets.
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

/// The placement box for cursor movement and clipping, and the image inside it.
///
/// As in kitty and ghostty, a direct placement given both `c` and `r` stretches
/// the image over that box, and one given only one of them derives the other
/// from the aspect ratio. A virtual (Unicode placeholder) placement fills whole
/// cells: the image keeps its aspect ratio and is centered in its cell box.
/// Virtual placements ignore pixel offsets, as required by the protocol.
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
    if !virtual_placement {
        return GraphicsDisplayLayout {
            image_size: placement_size,
            placement_size,
            image_offset: (0.0, 0.0),
        };
    }
    let cells = |count: Option<u32>, extent: f32, cell: f32| {
        count.map_or_else(
            || (extent / cell - 1e-3).ceil().max(1.0),
            |count| count as f32,
        ) * cell
    };
    let placement_size = (
        cells(cols, placement_size.0, cell_width),
        cells(rows, placement_size.1, cell_height),
    );
    let scale =
        (placement_size.0 / natural_width.max(1.0)).min(placement_size.1 / natural_height.max(1.0));
    let image_size = (natural_width * scale, natural_height * scale);
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
    fn virtual_boxes_center_images_without_distorting_or_shrinking_the_box() {
        let wide = graphics_display_layout(40, 10, Some(2), Some(2), (10.0, 20.0), (0, 0), true);
        assert_eq!(wide.image_size, (20.0, 5.0));
        assert_eq!(wide.placement_size, (20.0, 40.0));
        assert_eq!(wide.image_offset, (0.0, 17.5));
        let tall = graphics_display_layout(10, 40, Some(2), Some(2), (10.0, 20.0), (0, 0), true);
        assert_eq!(tall.image_size, (10.0, 40.0));
        assert_eq!(tall.image_offset, (5.0, 0.0));
        // Pixel offsets never apply to placeholders.
        let offset = graphics_display_layout(40, 10, Some(2), Some(2), (10.0, 20.0), (3, 4), true);
        assert_eq!(offset, wide);
    }

    #[test]
    fn virtual_boxes_round_up_to_whole_cells_and_center_the_image() {
        // Only `c`: rows follow the aspect ratio, rounded up to whole cells.
        let layout = graphics_display_layout(40, 10, Some(3), None, (10.0, 20.0), (0, 0), true);
        assert_eq!(layout.placement_size, (30.0, 20.0));
        assert_eq!(layout.image_size, (30.0, 7.5));
        assert_eq!(layout.image_offset, (0.0, 6.25));
        // Neither: the natural size, scaled to fit its cell box.
        let layout = graphics_display_layout(16, 10, None, None, (10.0, 20.0), (0, 0), true);
        assert_eq!(layout.placement_size, (20.0, 20.0));
        assert_eq!(layout.image_size, (20.0, 12.5));
        assert_eq!(layout.image_offset, (0.0, 3.75));
        // A box that is a whole number of fractional cells keeps its size.
        let layout = graphics_display_layout(96, 10, None, None, (9.6, 20.0), (0, 0), true);
        assert_eq!(layout.placement_size.0, 96.0);
    }

    #[test]
    fn direct_placements_stretch_over_an_explicit_box_like_kitty() {
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
    fn one_requested_dimension_keeps_the_aspect_ratio() {
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
            graphics_display_size(40, 10, None, Some(1), (10.0, 20.0), (0, 0), false),
            (80.0, 20.0)
        );
        assert_eq!(
            graphics_display_size(40, 10, Some(3), Some(2), (10.0, 20.0), (0, 0), true),
            (30.0, 7.5)
        );
    }
}
