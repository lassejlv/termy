use super::{
    Cell, Combining, DamageSnapshot, FrameUpdate, Palette, RenderDamageSnapshot, Snapshot,
    Terminal, ViewportMetadata,
};

impl Terminal {
    /// Take legacy damage and viewport state under one engine lock.
    pub fn take_damage_snapshot_with_viewport_state(&self) -> (DamageSnapshot, ViewportMetadata) {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let damage = engine.grid.take_damage();
        let viewport = ViewportMetadata {
            cols: engine.size.cols,
            rows: engine.size.rows,
            cursor: engine.grid.cursor_state(),
            display_offset: engine.grid.display_offset(),
            history_size: engine.grid.history_size(),
        };
        (damage, viewport)
    }

    /// Take renderer-aware damage and the live palette revision under one engine lock.
    pub fn take_render_damage_snapshot_with_palette_revision(&self) -> (RenderDamageSnapshot, u64) {
        let (damage, palette_revision, _) = self.take_render_damage_snapshot_with_render_state();
        (damage, palette_revision)
    }

    /// Take renderer-aware damage, palette revision, and viewport state under
    /// one engine lock.
    pub fn take_render_damage_snapshot_with_render_state(
        &self,
    ) -> (RenderDamageSnapshot, u64, ViewportMetadata) {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let generation = engine.render_generation;
        let (damage, scrolls) = engine.grid.take_render_damage();
        let viewport = ViewportMetadata {
            cols: engine.size.cols,
            rows: engine.size.rows,
            cursor: engine.grid.cursor_state(),
            display_offset: engine.grid.display_offset(),
            history_size: engine.grid.history_size(),
        };
        (
            RenderDamageSnapshot {
                damage,
                scrolls,
                generation,
            },
            engine.grid.palette().revision(),
            viewport,
        )
    }

    /// Visit a coherent full viewport while preserving the pending damage shape.
    ///
    /// Unlike [`Self::visit_frame_update_with_options`], this always visits every
    /// visible cell. Damage is still reported as partial unless `force_full` is set.
    /// The callback runs under the engine lock and must not call back into this terminal.
    pub fn visit_render_read(
        &self,
        force_full: bool,
        mut visitor: impl FnMut(usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> FrameUpdate {
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (damage, scrolls) = if force_full {
            (DamageSnapshot::Full, Vec::new())
        } else {
            engine.grid.render_damage_snapshot()
        };
        let viewport = engine.grid.visit_viewport_cells(&mut visitor);
        engine.grid.clear_damage();
        FrameUpdate {
            render: RenderDamageSnapshot {
                damage,
                scrolls,
                generation: engine.render_generation,
            },
            size: engine.size,
            viewport,
            palette: engine.grid.palette(),
            graphics_revision: engine.parser.graphics_revision(),
            graphics_placements: engine.parser.graphics_placements(&engine.grid),
        }
    }

    /// Capture the visible snapshot and palette under one engine lock.
    pub fn snapshot_with_palette(&self) -> (Snapshot, Palette) {
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (engine.grid.snapshot(), engine.grid.palette())
    }

    /// Visit the viewport and return its render generation and palette revision
    /// from the same engine lock.
    #[inline]
    pub fn visit_viewport_cells_with_render_state(
        &self,
        visitor: impl FnMut(usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> (ViewportMetadata, u64, u64) {
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let viewport = engine.grid.visit_viewport_cells(visitor);
        (
            viewport,
            engine.render_generation,
            engine.grid.palette().revision(),
        )
    }
}
