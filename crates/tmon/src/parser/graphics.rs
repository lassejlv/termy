use super::Parser;
use crate::{graphics::GraphicsRenderPlacement, grid::Grid};

impl Parser {
    pub(crate) fn advance_graphics(&mut self) {
        if self.graphics.advance_animations(std::time::Instant::now()) {
            self.graphics.bump_revision();
        }
    }

    pub(crate) fn graphics_revision(&self) -> u64 {
        self.graphics.revision()
    }

    pub(crate) fn graphics_placements(&self, grid: &Grid) -> Vec<GraphicsRenderPlacement> {
        self.graphics.render_placements(grid)
    }

    pub(crate) fn has_graphics_placements(&self) -> bool {
        self.graphics.has_placements()
    }

    pub(crate) fn has_primary_graphics_placements(&self) -> bool {
        self.graphics.has_primary_placements()
    }

    pub(crate) fn bump_graphics_revision(&mut self) {
        self.graphics.bump_revision();
    }
}
