use super::SelectionPos;
use crate::terminal_ui::{TerminalGridPaintCacheHandle, TerminalGridRows};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::terminal_view) struct KittyGraphicsRenderCacheKey {
    pub(in crate::terminal_view) graphics_revision: u64,
    pub(in crate::terminal_view) terminal_generation: Option<u64>,
    pub(in crate::terminal_view) cols: usize,
    pub(in crate::terminal_view) rows: usize,
    pub(in crate::terminal_view) display_offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::terminal_view) struct TerminalPaneCellColorTransformKey {
    pub(in crate::terminal_view) fg_blend_bits: u32,
    pub(in crate::terminal_view) bg_blend_bits: u32,
    pub(in crate::terminal_view) desaturate_bits: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::terminal_view) struct TerminalPaneRenderCacheKey {
    pub(in crate::terminal_view) is_active_pane: bool,
    pub(in crate::terminal_view) alternate_screen_mode: bool,
    pub(in crate::terminal_view) selection_range: Option<(SelectionPos, SelectionPos)>,
    pub(in crate::terminal_view) search_results_revision: Option<u64>,
    pub(in crate::terminal_view) search_position: Option<(usize, usize)>,
    pub(in crate::terminal_view) palette_revision: Option<u64>,
    pub(in crate::terminal_view) effective_background_opacity_bits: u32,
    pub(in crate::terminal_view) background_opacity_cells: bool,
    pub(in crate::terminal_view) color_transform: TerminalPaneCellColorTransformKey,
}

#[derive(Clone, Default)]
pub(in crate::terminal_view) struct TerminalPaneRenderCache {
    pub(in crate::terminal_view) cells: TerminalGridRows,
    pub(in crate::terminal_view) cols: usize,
    pub(in crate::terminal_view) rows: usize,
    pub(in crate::terminal_view) display_offset: usize,
    pub(in crate::terminal_view) key: Option<TerminalPaneRenderCacheKey>,
    pub(in crate::terminal_view) paint_cache: TerminalGridPaintCacheHandle,
    pub(in crate::terminal_view) kitty_images: super::kitty_images::KittyImageCache,
    pub(in crate::terminal_view) kitty_placements: Vec<termy_core::KittyGraphicsRenderPlacement>,
    pub(in crate::terminal_view) kitty_placements_key: Option<KittyGraphicsRenderCacheKey>,
}

impl TerminalPaneRenderCache {
    pub(in crate::terminal_view) fn clear(&mut self) {
        self.cells = std::sync::Arc::new(Vec::new());
        self.cols = 0;
        self.rows = 0;
        self.display_offset = 0;
        self.key = None;
        self.paint_cache.clear();
        self.kitty_images.clear();
        self.kitty_placements.clear();
        self.kitty_placements_key = None;
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[test]
    fn terminal_pane_render_cache_clear_resets_paint_cache_state() {
        let mut cache = TerminalPaneRenderCache {
            cells: std::sync::Arc::new(vec![std::sync::Arc::new(vec![])]),
            cols: 120,
            rows: 40,
            display_offset: 4,
            key: Some(TerminalPaneRenderCacheKey {
                is_active_pane: true,
                alternate_screen_mode: false,
                selection_range: Some((
                    SelectionPos { line: 1, col: 1 },
                    SelectionPos { line: 1, col: 2 },
                )),
                search_results_revision: Some(7),
                search_position: Some((1, 1)),
                palette_revision: Some(9),
                effective_background_opacity_bits: 0.92f32.to_bits(),
                background_opacity_cells: false,
                color_transform: TerminalPaneCellColorTransformKey {
                    fg_blend_bits: 0.1f32.to_bits(),
                    bg_blend_bits: 0.2f32.to_bits(),
                    desaturate_bits: 0.3f32.to_bits(),
                },
            }),
            paint_cache: TerminalGridPaintCacheHandle::default(),
            kitty_images: Default::default(),
            kitty_placements: Vec::new(),
            kitty_placements_key: None,
        };
        cache.paint_cache.debug_seed_rows_for_tests(3);
        assert_eq!(cache.paint_cache.debug_row_cache_len_for_tests(), 3);

        cache.clear();

        assert!(cache.cells.is_empty());
        assert_eq!(cache.cols, 0);
        assert_eq!(cache.rows, 0);
        assert_eq!(cache.display_offset, 0);
        assert!(cache.key.is_none());
        assert_eq!(cache.paint_cache.debug_row_cache_len_for_tests(), 0);
    }
}
