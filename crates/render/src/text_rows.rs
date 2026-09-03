use engine::{RowMove, RowMoveDirection};
use glyphon::{Cache, TextAtlas, TextRenderer, Viewport};
use wgpu::{Device, MultisampleState};

pub(crate) struct TextRowRenderer {
    pub renderer: TextRenderer,
    pub viewport: Viewport,
}

pub(crate) struct TextRows {
    rows: Vec<TextRowRenderer>,
}

impl TextRows {
    pub(crate) const fn new() -> Self {
        Self { rows: Vec::new() }
    }

    pub(crate) fn resize(
        &mut self,
        device: &Device,
        cache: &Cache,
        atlas: &mut TextAtlas,
        rows: usize,
    ) {
        self.rows.truncate(rows);
        while self.rows.len() < rows {
            self.rows.push(TextRowRenderer {
                renderer: TextRenderer::new(atlas, device, MultisampleState::default(), None),
                viewport: Viewport::new(device, cache),
            });
        }
    }

    pub(crate) fn move_rows(&mut self, movement: RowMove) -> bool {
        let height = movement.end_row.saturating_sub(movement.start_row);
        if movement.start_row >= movement.end_row
            || movement.end_row > self.rows.len()
            || movement.count == 0
            || movement.count >= height
        {
            return false;
        }
        let rows = &mut self.rows[movement.start_row..movement.end_row];
        match movement.direction {
            RowMoveDirection::Up => rows.rotate_left(movement.count),
            RowMoveDirection::Down => rows.rotate_right(movement.count),
        }
        true
    }

    pub(crate) fn get_mut(&mut self, row: usize) -> Option<&mut TextRowRenderer> {
        self.rows.get_mut(row)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &TextRowRenderer> {
        self.rows.iter()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut TextRowRenderer> {
        self.rows.iter_mut()
    }
}
