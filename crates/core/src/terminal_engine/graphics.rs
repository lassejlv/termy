//! Graphics follow the same parser and ordered grid mutations as text. This
//! keeps image commands inside synchronized-output commits without a second
//! parser, screen snapshot or duplicated input buffer.

pub(crate) mod unicode;

use web_time::Instant;

use super::{Color, dispatch::State, types::GridEffect};
use crate::{
    KittyGraphicsCommand, KittyGraphicsPlaceholder, KittyGraphicsRenderPlacement,
    KittyGraphicsScreen, KittyGraphicsState, TerminalSize,
};

#[derive(Default)]
pub(super) struct Graphics {
    state: KittyGraphicsState,
    pub(super) revision: u64,
    effects: Vec<GridEffect>,
    placeholders: Vec<KittyGraphicsPlaceholder>,
    last_view: Option<(bool, usize, usize, usize, usize)>,
    size: Option<TerminalSize>,
    #[cfg(test)]
    placeholder_scans: usize,
}

impl Graphics {
    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

impl State {
    fn graphics_size(&self) -> TerminalSize {
        let size = self.grid.size();
        TerminalSize {
            cols: size.cols as u16,
            rows: size.rows as u16,
            cell_width: f32::from(self.cell_pixels.0),
            cell_height: f32::from(self.cell_pixels.1),
        }
    }

    pub(super) fn resize_graphics(&mut self) {
        let size = self.graphics_size();
        if self.graphics.size != Some(size) {
            self.graphics.state.resize(size);
            self.graphics.size = Some(size);
            if self.graphics.state.has_placements() {
                self.graphics.changed();
            }
        }
    }

    fn collect_placeholders(&mut self) {
        self.graphics.placeholders.clear();
        if !self.graphics.state.has_virtual_placements_on_screen(
            KittyGraphicsScreen::from_alternate_screen(self.alternate_screen),
        ) {
            return;
        }
        #[cfg(test)]
        {
            self.graphics.placeholder_scans += 1;
        }
        let mut scratch = Vec::new();
        for row in 0..self.grid.size().rows {
            let Some(line) = self.grid.visible_row(row) else {
                continue;
            };
            let placeholders = &mut self.graphics.placeholders;
            line.with_cells(&mut scratch, |cells| {
                let mut previous = None;
                for (col, cell) in cells.iter().enumerate() {
                    if cell.character != unicode::PLACEHOLDER {
                        previous = None;
                        continue;
                    }
                    let mut diacritics = [None; 3];
                    for (slot, character) in diacritics.iter_mut().zip(cell.combining().chars()) {
                        *slot = unicode::diacritic_index(character);
                    }
                    let placeholder = KittyGraphicsPlaceholder::from_cell(
                        row as i64,
                        col,
                        placeholder_id(cell.style.foreground),
                        placeholder_id(cell.style.underline_color),
                        diacritics,
                        previous,
                    );
                    placeholders.push(placeholder);
                    previous = Some(placeholder);
                }
            });
        }
    }

    pub(super) fn flush_graphics_effects(&mut self) {
        self.grid.drain_effects(&mut self.graphics.effects);
        let mut changed = false;
        for effect in self.graphics.effects.drain(..) {
            match effect {
                GridEffect::Scroll {
                    alternate,
                    top,
                    bottom,
                    lines,
                    history_before,
                    history_after,
                    retains_history,
                } => {
                    let screen = KittyGraphicsScreen::from_alternate_screen(alternate);
                    if retains_history && bottom < self.grid.size().rows {
                        changed |= self.graphics.state.scroll_partial_history_region(
                            bottom,
                            lines.max(0) as usize,
                            history_before,
                            history_after,
                        );
                    } else if retains_history {
                        let evicted = (lines.max(0) as usize)
                            .saturating_sub(history_after.saturating_sub(history_before));
                        changed |= self
                            .graphics
                            .state
                            .scroll_up_without_history_on_screen(evicted, screen);
                        changed |= self.graphics.state.has_placements();
                    } else if top == 0 && bottom == self.grid.size().rows && lines > 0 {
                        // A freshly placed image can extend below the screen
                        // until its cursor advance scrolls it into view. A full
                        // page scroll moves that complete image with the text.
                        changed |= self
                            .graphics
                            .state
                            .scroll_up_without_history_on_screen(lines as usize, screen);
                    } else {
                        changed |= self.graphics.state.scroll_region_on_screen(
                            screen,
                            top,
                            bottom,
                            lines,
                            history_before,
                        );
                    }
                }
                GridEffect::Clear {
                    alternate,
                    history_size,
                } => {
                    let size = self.grid.size();
                    changed |= self.graphics.state.clear_viewport_on_screen(
                        KittyGraphicsScreen::from_alternate_screen(alternate),
                        history_size,
                        size.rows,
                        size.cols,
                    );
                }
                GridEffect::ClearHistory { removed } => {
                    changed |= self
                        .graphics
                        .state
                        .scroll_up_without_history_on_screen(removed, KittyGraphicsScreen::Primary);
                }
                GridEffect::Reset => changed |= self.graphics.state.reset(),
            }
        }
        let size = self.grid.size();
        let view = (
            self.alternate_screen,
            self.grid.history_size(),
            self.grid.display_offset(),
            size.rows,
            size.cols,
        );
        changed |= self.graphics.last_view != Some(view) && self.graphics.state.has_placements();
        self.graphics.last_view = Some(view);
        // A text edit may remove/recolor a Unicode placeholder without changing
        // any image commands or scroll geometry. Direct images avoid this cost.
        let visual_dirty = self.grid.take_visual_dirty();
        changed |= visual_dirty
            && self.graphics.state.has_virtual_placements_on_screen(
                KittyGraphicsScreen::from_alternate_screen(self.alternate_screen),
            );
        if changed {
            self.graphics.changed();
        }
        self.grid
            .set_effect_tracking(self.graphics.state.needs_grid_effects());
    }

    pub(super) fn apply_graphics(&mut self, bytes: &[u8]) {
        let Some(body) = bytes.strip_prefix(b"G") else {
            return;
        };
        self.flush_graphics_effects();
        self.resize_graphics();
        let command = KittyGraphicsCommand::parse(body.to_vec(), false);
        // Only placement deletion resolves positions from placeholder cells.
        // In particular, upload continuations must not scan the whole viewport.
        if command.needs_placeholder_positions() {
            self.collect_placeholders();
        } else {
            self.graphics.placeholders.clear();
        }
        let screen = KittyGraphicsScreen::from_alternate_screen(self.alternate_screen);
        let result = self.graphics.state.apply_on_screen_with_placeholders(
            command,
            (self.grid.cursor.col, self.grid.cursor.row),
            self.grid.history_size(),
            self.graphics_size(),
            screen,
            &self.graphics.placeholders,
        );
        if let Some(reply) = result.response {
            self.reply(&reply);
        }
        if result.changed {
            self.graphics.changed();
        }
        self.grid
            .set_effect_tracking(self.graphics.state.needs_grid_effects());
        if let Some((cols, rows)) = result.cursor_advance
            && result
                .cursor_advance_screen
                .is_none_or(|target| target == screen)
        {
            let size = self.grid.size();
            self.grid
                .move_cursor(0, (cols as usize).min(size.cols) as isize);
            let (top, bottom) = self.grid.scroll_region();
            let rows = (rows as usize).min(size.rows);
            if top == 0 && bottom == size.rows {
                let newline = self.grid.newline_mode;
                self.grid.newline_mode = false;
                for _ in 0..rows {
                    self.grid.linefeed();
                }
                self.grid.newline_mode = newline;
            } else {
                self.grid.move_cursor(rows as isize, 0);
            }
            self.flush_graphics_effects();
        }
    }

    pub(super) fn poll_graphics_revision(&mut self) -> u64 {
        if self.graphics.state.advance_animations(Instant::now()) {
            self.graphics.changed();
        }
        self.graphics.revision
    }

    pub(super) fn graphics_snapshot(&mut self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        self.poll_graphics_revision();
        self.collect_placeholders();
        let size = self.grid.size();
        let placements = self
            .graphics
            .state
            .render_placements_on_screen_with_placeholders(
                self.grid.history_size(),
                self.grid.display_offset(),
                size.rows,
                size.cols,
                KittyGraphicsScreen::from_alternate_screen(self.alternate_screen),
                &self.graphics.placeholders,
            );
        (self.graphics.revision, placements)
    }
}

fn placeholder_id(color: Color) -> u32 {
    if let Some((r, g, b)) = color.as_rgb() {
        (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
    } else {
        color.as_indexed().map_or(0, u32::from)
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal_engine::{Engine, Options, Size};

    #[test]
    fn chunked_uploads_do_not_scan_virtual_placeholder_cells() {
        let mut engine = Engine::new(
            Size {
                cols: 120,
                rows: 40,
            },
            Options::default(),
        );
        engine.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=1,c=1,r=1,U=1,q=2;AQID/w==\x1b\\");
        for _ in 0..128 {
            engine.feed(b"\x1b_Ga=t,f=32,s=1,v=1,i=2,m=1,q=2;AQID\x1b\\");
            engine.feed(b"\x1b_Gm=0;/w==\x1b\\");
        }
        assert_eq!(engine.state.graphics.placeholder_scans, 0);
        assert!(engine.graphics_placements().is_empty());
        assert_eq!(engine.state.graphics.placeholder_scans, 1);

        engine.feed(b"\x1b[?1049h");
        assert!(engine.graphics_placements().is_empty());
        assert_eq!(engine.state.graphics.placeholder_scans, 1);
    }

    #[test]
    fn virtual_graphics_revision_tracks_cell_edits_without_query_invalidations() {
        let mut engine = Engine::new(Size { cols: 10, rows: 4 }, Options::default());
        engine.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=1,c=1,r=1,U=1,q=2;AQID/w==\x1b\\");
        let before_query = engine.graphics_revision();
        engine.feed(b"\x1b[6n\x1b[38;5;1m");
        assert_eq!(engine.graphics_revision(), before_query);

        engine.feed("\u{10eeee}\u{0305}\u{0305}".as_bytes());
        assert_ne!(engine.graphics_revision(), before_query);
        assert_eq!(engine.graphics_placements().len(), 1);
        let before_erase = engine.graphics_revision();
        // Full text damage is still pending, so graphics cannot rely on a
        // transition from clean to dirty to observe this destructive edit.
        engine.feed(b"\r ");
        assert_ne!(engine.graphics_revision(), before_erase);
        assert!(engine.graphics_placements().is_empty());

        engine.feed(b"\x1b[?1049h");
        let alternate_revision = engine.graphics_revision();
        engine.feed(b"\x1b[6ntext");
        assert_eq!(engine.graphics_revision(), alternate_revision);
    }

    #[test]
    fn cursor_only_damage_preserves_virtual_graphics_revision() {
        for consume_damage in [false, true] {
            let mut engine = Engine::new(Size { cols: 10, rows: 4 }, Options::default());
            engine.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=1,c=1,r=1,U=1,q=2;AQID/w==\x1b\\");
            engine.feed("\x1b[38;5;1m\u{10eeee}\u{0305}\u{0305}".as_bytes());
            let (before, placements) = engine.graphics_snapshot();
            assert_eq!(placements.len(), 1);
            if consume_damage {
                engine.take_damage();
            }
            for sequence in [
                b"\x1b[2;3H".as_slice(),
                b"\r",
                b"\t",
                b"\x08",
                b"\n",
                b"\x1bM",
                b"\x1b[?25l",
                b"\x1b[?25h",
                b"\x1b[5 q",
                b"\x1b[?12l",
            ] {
                engine.feed(sequence);
                assert_eq!(engine.graphics_revision(), before, "{sequence:?}");
                if consume_damage {
                    assert!(
                        matches!(engine.take_damage(), super::super::Damage::Partial(spans) if !spans.is_empty()),
                        "{sequence:?}"
                    );
                }
            }
            assert_eq!(engine.graphics_placements().len(), 1);
            engine.feed("\x1b[H界".as_bytes());
            assert_ne!(engine.graphics_revision(), before);
            assert!(engine.graphics_placements().is_empty());
        }
    }

    #[test]
    fn hidden_cursor_unicode_overwrite_invalidates_virtual_graphics_with_full_damage() {
        let mut engine = Engine::new(Size { cols: 10, rows: 4 }, Options::default());
        engine.feed(b"\x1b[?25l\x1b_Ga=T,f=32,s=1,v=1,i=1,c=1,r=1,U=1,q=2;AQID/w==\x1b\\");
        engine.feed("\x1b[38;5;1m\u{10eeee}\u{0305}\u{0305}".as_bytes());
        let (before, placements) = engine.graphics_snapshot();
        assert_eq!(placements.len(), 1);

        // A scalar write takes a separate full-damage fast path. With the
        // cursor hidden, neither carriage return nor cursor damage covers it.
        engine.feed("\r界".as_bytes());
        assert_ne!(engine.graphics_revision(), before);
        assert!(engine.graphics_placements().is_empty());
    }

    #[test]
    fn placement_deletion_still_resolves_virtual_parent_cells() {
        let mut engine = Engine::new(Size { cols: 10, rows: 4 }, Options::default());
        engine.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=1,p=1,c=1,r=1,U=1,q=2;AQID/w==\x1b\\");
        engine.feed("\x1b[38;5;1m\u{10eeee}\u{0305}\u{0305}".as_bytes());
        engine.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=2,c=1,r=1,P=1,Q=1,q=2;AQID/w==\x1b\\");
        assert_eq!(engine.graphics_placements().len(), 2);

        engine.feed(b"\x1b_Ga=d,d=p,x=1,y=1,q=2;\x1b\\");
        let placements = engine.graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 1);
    }

    #[test]
    fn image_commands_observe_preceding_text_and_follow_scrolls() {
        let mut engine = Engine::new(
            Size { cols: 10, rows: 3 },
            Options {
                scrollback_history: 2,
            },
        );
        engine.feed(b"\r\n\x1b_Ga=T,f=32,s=1,v=1,i=7,c=1,r=1,C=1;AQID/w==\x1b\\");
        let image = engine.graphics_placements();
        assert_eq!(image.len(), 1);
        assert_eq!(image[0].viewport_row, 1);
        engine.feed(b"\r\n\r\n");
        assert_eq!(engine.graphics_placements()[0].viewport_row, 0);
        engine.feed(b"\r\n");
        assert!(engine.graphics_placements().is_empty());
        engine.scroll_display(2);
        assert!(!engine.graphics_placements().is_empty());
    }

    #[test]
    fn height_resize_keeps_images_with_text_when_history_is_evicted() {
        for history in [0, 2, 20] {
            for alternate in [false, true] {
                let mut engine = Engine::new(
                    Size { cols: 10, rows: 4 },
                    Options {
                        scrollback_history: history,
                    },
                );
                engine.feed(b"0\r\n1\r\n2\r\n3\r\n4\r\n5");
                engine.feed(
                    b"\x1b[3;1H\x1b_Ga=T,f=32,s=1,v=1,i=7,c=1,r=1,C=1;AQID/w==\x1b\\\x1b[4;1H",
                );
                assert_eq!(engine.viewport_row(2).unwrap()[0].character, '4');
                assert_eq!(engine.graphics_placements()[0].viewport_row, 2);
                if alternate {
                    engine.feed(b"\x1b[?1049h");
                }
                engine.resize(Size { cols: 10, rows: 3 });
                if alternate {
                    engine.feed(b"\x1b[?1049l");
                }
                assert_eq!(engine.viewport_row(1).unwrap()[0].character, '4');
                assert_eq!(
                    engine.graphics_placements()[0].viewport_row,
                    1,
                    "history={history}, alternate={alternate}"
                );
                engine.resize(Size { cols: 10, rows: 4 });
                let row = engine.graphics_placements()[0].viewport_row as usize;
                assert_eq!(engine.viewport_row(row).unwrap()[0].character, '4');
            }
        }
    }

    #[test]
    fn visible_clear_removes_placements_but_retains_images_for_reuse() {
        let mut engine = Engine::new(Size { cols: 10, rows: 3 }, Options::default());
        engine.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,c=1,r=1,C=1;AQID/w==\x1b\\");
        assert_eq!(engine.graphics_placements().len(), 1);
        engine.feed(b"\x1b[2J");
        assert!(engine.graphics_placements().is_empty());
        engine.feed(b"\x1b_Ga=p,i=7,c=1,r=1,C=1;\x1b\\");
        assert_eq!(engine.graphics_placements().len(), 1);
    }

    #[test]
    fn partial_scroll_retains_history_and_keeps_footer_fixed_at_capacity() {
        let mut engine = Engine::new(
            Size { cols: 10, rows: 4 },
            Options {
                scrollback_history: 2,
            },
        );
        engine.feed(b"\x1b[3;1H\x1b_Ga=T,f=32,s=1,v=1,i=1,c=1,r=1,C=1;AQID/w==\x1b\\");
        engine.feed(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=2,c=1,r=1,C=1;AQID/w==\x1b\\");
        engine.feed(b"\x1b[1;3r\x1b[3;1H\n\n\n");
        assert_eq!(engine.history_size(), 2);
        let placements = engine.graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!((placements[0].image_id, placements[0].viewport_row), (2, 3));
        engine.scroll_display(2);
        let placements = engine.graphics_placements();
        assert_eq!((placements[0].image_id, placements[0].viewport_row), (1, 1));
        engine.scroll_display(-2);
        engine.feed(b"\n\n");
        let placements = engine.graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!((placements[0].image_id, placements[0].viewport_row), (2, 3));
    }

    #[test]
    fn reset_discards_incomplete_image_upload() {
        let mut engine = Engine::new(Size { cols: 10, rows: 4 }, Options::default());
        engine.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=1,m=1;AQID\x1b\\");
        engine.feed(b"\x1bc\x1b_Gm=0;/w==\x1b\\");
        assert!(engine.graphics_placements().is_empty());
    }

    #[test]
    fn large_direct_feeds_bound_ordered_graphics_effect_storage() {
        let mut engine = Engine::new(
            Size { cols: 10, rows: 4 },
            Options {
                scrollback_history: 0,
            },
        );
        engine.feed(b"\x1b[3;1H\x1b_Ga=T,f=32,s=1,v=1,i=1,c=1,r=1,C=1;AQID/w==\x1b\\");
        let input = b"\x1b[S\x1b[T".repeat(128 * 1024);
        engine.feed(&input);
        assert_eq!(engine.graphics_placements()[0].viewport_row, 2);
        assert!(
            engine.state.graphics.effects.capacity() * size_of::<super::GridEffect>()
                < 4 * 1024 * 1024
        );
    }
}
