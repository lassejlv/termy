//! Graphics follow the same parser and ordered grid mutations as text. This
//! keeps image commands inside synchronized-output commits without a second
//! parser, screen snapshot or duplicated input buffer.

pub(crate) mod unicode;

use web_time::Instant;

use super::{Color, dispatch::State, types::GridEffect};
use crate::{
    KittyGraphicsApplyResult, KittyGraphicsCommand, KittyGraphicsPlaceholder,
    KittyGraphicsRenderPlacement, KittyGraphicsScreen, KittyGraphicsState, TerminalSize,
    kitty_graphics::{InlineMultipart, KittyPreparedUpload, KittyUpload},
};

/// Animation pixels (`a=c` compositions, `a=f` frames) one deferring feed
/// may write before it pauses, so the runtime can hand the engine lock to
/// other callers between heavy edits. About 16 MiB of RGBA, a few ms.
const ANIMATION_PIXELS_PER_FEED: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Graphics {
    state: KittyGraphicsState,
    pub(super) revision: u64,
    effects: Vec<GridEffect>,
    placeholders: Vec<KittyGraphicsPlaceholder>,
    last_view: Option<(bool, usize, usize, usize, usize)>,
    size: Option<TerminalSize>,
    // While set, a completed upload stops the parser undecoded (see
    // `Engine::feed_deferring_uploads`) and waits in `deferred`. Boxed so
    // the per-feed checks don't move a large, almost always empty value.
    defer_uploads: bool,
    deferred: Option<Box<KittyUpload>>,
    // An OSC 1337 multipart image still receiving parts. Unlike a pending
    // kitty upload, snapshots omit it.
    multipart: Option<Box<InlineMultipart>>,
    #[cfg(test)]
    placeholder_scans: usize,
}

impl Graphics {
    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Snapshot version 2 and later; see `kitty_graphics/snapshot.rs`.
    pub(super) fn write_snapshot(&self, writer: &mut super::snapshot::Writer) {
        writer.u64(self.revision);
        writer.bool(self.last_view.is_some());
        let (alternate, history, offset, rows, cols) = self.last_view.unwrap_or_default();
        writer.bool(alternate);
        for value in [history, offset, rows, cols] {
            writer.usize(value);
        }
        writer.bool(self.size.is_some());
        self.state.write_snapshot(writer);
    }

    /// The decoded graphics, and whether they were sized for the terminal
    /// (the caller restores that size).
    pub(super) fn read_snapshot(
        reader: &mut super::snapshot::Reader<'_>,
    ) -> super::snapshot::Result<(Self, bool)> {
        let revision = reader.u64()?;
        let has_view = reader.bool()?;
        let alternate = reader.bool()?;
        let mut view = [0; 4];
        for value in &mut view {
            *value = reader.usize()?;
        }
        let [history, offset, rows, cols] = view;
        let sized = reader.bool()?;
        let graphics = Self {
            state: KittyGraphicsState::read_snapshot(reader)?,
            revision,
            last_view: has_view.then_some((alternate, history, offset, rows, cols)),
            ..Self::default()
        };
        Ok((graphics, sized))
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

    /// Install graphics decoded from a snapshot.
    pub(super) fn restore_graphics(&mut self, graphics: Graphics, sized: bool) {
        self.graphics = graphics;
        if sized {
            // Placements were laid out for this size already.
            self.graphics.size = Some(self.graphics_size());
        }
        self.grid
            .set_effect_tracking(self.graphics.state.needs_grid_effects());
        // The decoded grid's full damage is not an edit of placeholder cells;
        // the encoded engine had flushed its own.
        self.grid.take_visual_dirty();
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
            line.with_line(&mut scratch, |cells| {
                let mut previous = None;
                for (col, cell) in cells.iter().enumerate() {
                    if cell.character != unicode::PLACEHOLDER {
                        previous = None;
                        continue;
                    }
                    let mut diacritics = [None; 3];
                    for (slot, character) in diacritics.iter_mut().zip(cells.combining(col).chars())
                    {
                        *slot = unicode::diacritic_index(character);
                    }
                    let placeholder = KittyGraphicsPlaceholder::from_cell(
                        row as i64,
                        col,
                        placeholder_id(cell.style.foreground),
                        placeholder_id(cells.underline_color(col)),
                        diacritics,
                        previous,
                    );
                    placeholders.push(placeholder);
                    previous = Some(placeholder);
                }
            });
        }
    }

    #[inline]
    pub(super) fn flush_graphics_effects(&mut self) {
        // Every feed flushes. With nothing placed or pending, the grid records
        // no effects and no text edit or scroll can change graphics; only the
        // view is kept current, as the full path does.
        if !self.grid.tracks_effects() && !self.graphics.state.needs_grid_effects() {
            let size = self.grid.size();
            self.graphics.last_view = Some((
                self.alternate_screen,
                self.grid.history_size(),
                self.grid.display_offset(),
                size.rows,
                size.cols,
            ));
            self.grid.take_visual_dirty();
            return;
        }
        self.flush_tracked_graphics_effects();
    }

    #[inline(never)]
    fn flush_tracked_graphics_effects(&mut self) {
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

    pub(super) fn apply_graphics(&mut self, bytes: &mut Vec<u8>) {
        if bytes.first() != Some(&b'G') {
            return;
        }
        // Take the parser's buffer: payloads reach hundreds of MiB.
        let command = KittyGraphicsCommand::parse_after(std::mem::take(bytes), 1, false);
        self.apply_graphics_command(command);
    }

    /// OSC 1337 file transfers: `File=args:data`, or `MultipartFile=args`
    /// followed by `FilePart=data`s and `FileEnd`. An inline image becomes a
    /// quiet, anonymous kitty upload, decoded, deferred and placed like one.
    pub(super) fn apply_inline_image(&mut self, bytes: &mut Vec<u8>) {
        const FILE: &[u8] = b"1337;File=";
        if bytes.starts_with(FILE) {
            self.graphics.multipart = None;
            let Some(colon) = bytes.iter().position(|byte| *byte == b':') else {
                return;
            };
            // Like an APC, keep the payload in the parser's allocation.
            let mut payload = std::mem::take(bytes);
            let args = payload[FILE.len()..colon].to_vec();
            payload.drain(..=colon);
            if let Some(command) = KittyGraphicsCommand::inline_image(&args, payload) {
                self.apply_graphics_command(command);
            }
        } else if let Some(args) = bytes.strip_prefix(b"1337;MultipartFile=") {
            self.graphics.multipart = Some(Box::new(InlineMultipart::new(args)));
        } else if let Some(part) = bytes.strip_prefix(b"1337;FilePart=") {
            if let Some(upload) = &mut self.graphics.multipart
                && !upload.push(part)
            {
                self.graphics.multipart = None;
            }
        } else if bytes.as_slice() == b"1337;FileEnd"
            && let Some(command) = self
                .graphics
                .multipart
                .take()
                .and_then(|upload| upload.finish())
        {
            self.apply_graphics_command(command);
        }
    }

    fn apply_graphics_command(&mut self, command: KittyGraphicsCommand) {
        self.flush_graphics_effects();
        self.resize_graphics();
        // Only placement deletion resolves positions from placeholder cells.
        // In particular, upload continuations must not scan the whole viewport.
        if command.needs_placeholder_positions() {
            self.collect_placeholders();
        } else {
            self.graphics.placeholders.clear();
        }
        let screen = KittyGraphicsScreen::from_alternate_screen(self.alternate_screen);
        let cursor = (self.grid.cursor.col, self.grid.cursor.row);
        let (history_size, size) = (self.grid.history_size(), self.graphics_size());
        let placeholders = &self.graphics.placeholders;
        let result = if self.graphics.defer_uploads {
            let (result, upload) = self.graphics.state.apply_deferring_upload(
                command,
                cursor,
                history_size,
                size,
                screen,
                placeholders,
            );
            self.graphics.deferred = upload.map(Box::new);
            result
        } else {
            self.graphics.state.apply_on_screen_with_placeholders(
                command,
                cursor,
                history_size,
                size,
                screen,
                placeholders,
            )
        };
        self.finish_graphics_command(result, screen);
    }

    pub(super) fn set_defer_uploads(&mut self, defer: bool) {
        self.graphics.defer_uploads = defer;
    }

    pub(super) fn has_deferred_upload(&self) -> bool {
        self.graphics.deferred.is_some()
    }

    /// Whether this feed's animation edits used up their pixel budget. Only
    /// deferring feeds pause for it; see [`ANIMATION_PIXELS_PER_FEED`].
    pub(super) fn animation_budget_spent(&self) -> bool {
        self.graphics.state.animation_work() >= ANIMATION_PIXELS_PER_FEED
    }

    pub(super) fn pauses_for_animation_work(&self) -> bool {
        self.graphics.defer_uploads && self.animation_budget_spent()
    }

    pub(super) fn reset_animation_work(&mut self) {
        self.graphics.state.reset_animation_work();
    }

    pub(super) fn take_deferred_upload(&mut self) -> Option<KittyUpload> {
        self.graphics.deferred.take().map(|upload| *upload)
    }

    pub(super) fn commit_upload(&mut self, prepared: KittyPreparedUpload) {
        self.flush_graphics_effects();
        self.resize_graphics();
        let screen = KittyGraphicsScreen::from_alternate_screen(self.alternate_screen);
        let result = self.graphics.state.commit_upload(
            prepared,
            (self.grid.cursor.col, self.grid.cursor.row),
            self.grid.history_size(),
            self.graphics_size(),
        );
        self.finish_graphics_command(result, screen);
    }

    fn finish_graphics_command(
        &mut self,
        result: KittyGraphicsApplyResult,
        screen: KittyGraphicsScreen,
    ) {
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
            // As in kitty (and ghostty), the cursor ends on the image's last
            // row, just past its last column; passing the right edge wraps to
            // the next line. Line feeds scroll the region like any output.
            let size = self.grid.size();
            let rows = (rows as usize).min(size.rows);
            let newline = self.grid.newline_mode;
            self.grid.newline_mode = false;
            for _ in 1..rows {
                self.grid.linefeed();
            }
            if self.grid.cursor.col.saturating_add(cols as usize) >= size.cols {
                self.grid.carriage_return();
                self.grid.linefeed();
            } else {
                self.grid.move_cursor(0, cols as isize);
            }
            self.grid.newline_mode = newline;
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
                ..Default::default()
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
                        ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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

    fn row_text(engine: &Engine, row: usize) -> String {
        engine
            .viewport_row(row)
            .unwrap()
            .iter()
            .map(|cell| cell.character)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn drain(engine: &mut Engine) -> Vec<u8> {
        let mut replies = Vec::new();
        engine.drain_replies(&mut replies);
        replies
    }

    #[test]
    fn placing_an_image_leaves_the_cursor_on_its_last_row_like_kitty() {
        // 9x18 px cells: a 2x3-cell image at (1, 0).
        let mut engine = Engine::new(Size { cols: 10, rows: 6 }, Options::default());
        engine.feed(b"\x1b[1;2H\x1b_Ga=T,f=32,s=1,v=1,i=1,c=2,r=3,q=2;AQID/w==\x1b\\after");
        assert_eq!(row_text(&engine, 2), "   after");
        assert_eq!((engine.cursor().row, engine.cursor().col), (2, 8));

        // Reaching the right edge wraps to the start of the next line.
        engine.feed(b"\x1b[5;7H\x1b_Ga=T,f=32,s=1,v=1,i=2,c=4,r=1,q=2;AQID/w==\x1b\\next");
        assert_eq!(row_text(&engine, 5), "next");
        assert_eq!(engine.graphics_placements().len(), 2);
    }

    #[test]
    fn deferred_uploads_pause_the_parser_and_commit_in_stream_order() {
        let upload = b"\x1b_Ga=T,f=32,s=1,v=1,i=7,c=2,r=1;AQID/w==\x1b\\".as_slice();
        let chunked = b"\x1b_Ga=T,f=32,s=1,v=1,i=7,c=2,r=1,m=1;AQID\x1b\\\x1b_Gm=0;/w==\x1b\\";
        for upload in [upload, chunked.as_slice()] {
            let input = [b"before\x1b[6n".as_slice(), upload, b"after\x1b[6n"].concat();
            let size = Size { cols: 20, rows: 4 };
            let mut expected = Engine::new(size, Options::default());
            expected.feed(&input);

            let mut engine = Engine::new(size, Options::default());
            let consumed = engine.feed_deferring_uploads(&input);
            assert_eq!(consumed, b"before\x1b[6n".len() + upload.len());
            assert_eq!(row_text(&engine, 0), "before");
            let deferred = engine.take_deferred_upload().expect("upload is deferred");
            assert!(engine.take_deferred_upload().is_none());
            assert!(engine.graphics_placements().is_empty());
            let mut replies = drain(&mut engine);
            assert_eq!(replies, b"\x1b[1;7R");

            engine.commit_upload(deferred.prepare());
            assert_eq!(engine.graphics_placements().len(), 1);
            assert_eq!(
                engine.feed_deferring_uploads(&input[consumed..]),
                input.len() - consumed
            );
            assert!(engine.take_deferred_upload().is_none());
            replies.extend(drain(&mut engine));
            assert_eq!(replies, drain(&mut expected));
            assert_eq!(row_text(&engine, 0), row_text(&expected, 0));
            assert_eq!(row_text(&engine, 1), row_text(&expected, 1));
            // The image ends the cursor on its own row, as in kitty.
            assert_eq!(row_text(&engine, 0), "before  after");
            assert_eq!(engine.cursor(), expected.cursor());
            assert_eq!(
                engine.graphics_placements().len(),
                expected.graphics_placements().len()
            );
        }
    }

    #[test]
    fn committing_an_upload_releases_history_read_while_it_decoded() {
        let mut engine = Engine::new(
            Size { cols: 20, rows: 2 },
            Options {
                scrollback_history: 100,
                ..Default::default()
            },
        );
        for line in 0..50 {
            engine.feed(format!("line {line}\r\n").as_bytes());
        }
        engine.compact_history();
        let input = b"\x1b_Ga=T,f=32,s=1,v=1,i=7,c=1,r=2;AQID/w==\x1b\\after";
        let consumed = engine.feed_deferring_uploads(input);
        let deferred = engine.take_deferred_upload().expect("upload is deferred");
        // A UI read (e.g. link hover) while the upload decodes off the lock.
        assert!(engine.line(-1).is_some());
        assert!(engine.state.grid.has_history_read_cache());
        engine.commit_upload(deferred.prepare());
        assert!(!engine.state.grid.has_history_read_cache());
        engine.feed(&input[consumed..]);
        assert_eq!(engine.graphics_placements().len(), 1);
    }

    #[test]
    fn deferred_upload_failures_reply_at_commit() {
        let mut engine = Engine::new(Size { cols: 20, rows: 4 }, Options::default());
        let input = b"\x1b_Ga=t,f=100,i=3;AQID/w==\x1b\\\x1b_Ga=t,f=32,s=1,v=1,i=4;!!\x1b\\";
        let consumed = engine.feed_deferring_uploads(input);
        let png = engine.take_deferred_upload().unwrap().prepare();
        assert!(drain(&mut engine).is_empty());
        engine.commit_upload(png);
        assert_eq!(
            drain(&mut engine),
            b"\x1b_Gi=3;EINVAL:invalid PNG image\x1b\\"
        );

        assert_eq!(
            engine.feed_deferring_uploads(&input[consumed..]),
            input.len() - consumed
        );
        let invalid = engine.take_deferred_upload().unwrap().prepare();
        engine.commit_upload(invalid);
        assert_eq!(
            drain(&mut engine),
            b"\x1b_Gi=4;EINVAL:invalid base64 payload\x1b\\"
        );
    }

    #[test]
    fn synchronized_updates_apply_uploads_atomically() {
        let mut engine = Engine::new(Size { cols: 20, rows: 4 }, Options::default());
        let input = b"\x1b[?2026h\x1b_Ga=T,f=32,s=1,v=1,i=7,q=2;AQID/w==\x1b\\after\x1b[?2026l";
        assert_eq!(engine.feed_deferring_uploads(input), input.len());
        assert!(engine.take_deferred_upload().is_none());
        assert_eq!(engine.graphics_placements().len(), 1);
        assert_eq!(row_text(&engine, 0), " after");
    }

    #[test]
    fn an_untaken_deferred_upload_commits_before_more_output() {
        let mut engine = Engine::new(Size { cols: 20, rows: 4 }, Options::default());
        let input = b"\x1b_Ga=T,f=32,s=1,v=1,i=7,q=2;AQID/w==\x1b\\after";
        let consumed = engine.feed_deferring_uploads(input);
        engine.feed(&input[consumed..]);
        assert_eq!(engine.graphics_placements().len(), 1);
        assert_eq!(row_text(&engine, 0), " after");
    }

    #[test]
    fn leaving_1049_clears_alternate_screen_images_with_its_text() {
        let mut engine = Engine::new(Size { cols: 10, rows: 4 }, Options::default());
        engine.feed(b"\x1b[?1049h\x1b_Ga=T,f=32,s=1,v=1,i=7,c=1,r=1,q=2;AQID/w==\x1b\\");
        engine.feed(b"\x1b_Ga=T,f=32,s=1,v=1,c=1,r=1,q=2;AQID/w==\x1b\\");
        assert_eq!(engine.graphics_placements().len(), 2);
        engine.feed(b"\x1b[?1049l");
        // Mode 47 reuses the alternate screen without clearing it, so only
        // the 1049 exit can have removed these.
        engine.feed(b"\x1b[?47h");
        assert!(engine.graphics_placements().is_empty());
        engine.feed(b"\x1b_Ga=p,i=7,c=1,r=1;\x1b\\");
        assert_eq!(engine.graphics_placements().len(), 1);
        assert_eq!(drain(&mut engine), b"\x1b_Gi=7;OK\x1b\\");
    }

    #[test]
    fn heavy_animation_edits_pause_deferring_feeds() {
        use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
        use std::io::Write as _;

        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&vec![0; 1024 * 1024 * 4]).unwrap();
        let root = BASE64.encode(encoder.finish().unwrap());
        let setup = format!(
            "\x1b_Ga=t,i=1,f=32,s=1024,v=1024,o=z,q=2;{root}\x1b\\\x1b_Ga=f,i=1,f=32,s=1,v=1,q=2;AQID/w==\x1b\\"
        );
        // Each composition blends one whole 1024x1024 frame.
        let compose = b"\x1b_Ga=c,i=1,r=1,c=2,q=2\x1b\\".as_slice();
        let input = [compose.repeat(10).as_slice(), b"after"].concat();
        let size = Size { cols: 20, rows: 4 };

        let mut expected = Engine::new(size, Options::default());
        expected.feed(setup.as_bytes());
        expected.feed(&input);

        let mut engine = Engine::new(size, Options::default());
        engine.feed(setup.as_bytes());
        let per_feed = super::ANIMATION_PIXELS_PER_FEED / (1024 * 1024);
        let mut offset = 0;
        let mut feeds = 0;
        while offset < input.len() {
            let consumed = engine.feed_deferring_uploads(&input[offset..]);
            assert!(engine.take_deferred_upload().is_none());
            offset += consumed;
            feeds += 1;
            if offset < input.len() {
                assert!(engine.animation_budget_spent());
                assert_eq!(consumed, per_feed * compose.len());
            }
        }
        assert_eq!(feeds, 10 / per_feed + 1);
        assert_eq!(row_text(&engine, 0), row_text(&expected, 0));
        assert!(row_text(&engine, 0).ends_with("after"));
        assert!(drain(&mut engine).is_empty());
    }

    fn inline_file(format: ::image::ImageFormat, width: u32, height: u32) -> Vec<u8> {
        let mut file = std::io::Cursor::new(Vec::new());
        ::image::DynamicImage::new_rgb8(width, height)
            .write_to(&mut file, format)
            .unwrap();
        file.into_inner()
    }

    fn osc_file(args: &str, file: &[u8]) -> Vec<u8> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let encoded = STANDARD.encode(file);
        [
            b"\x1b]1337;File=",
            args.as_bytes(),
            b":",
            encoded.as_bytes(),
            b"\x07",
        ]
        .concat()
    }

    #[test]
    fn osc_1337_places_inline_images_of_any_format_silently() {
        use ::image::ImageFormat;
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::WebP,
            ImageFormat::Tiff,
            ImageFormat::Bmp,
        ] {
            let mut engine = Engine::new(Size { cols: 20, rows: 6 }, Options::default());
            engine.feed(&osc_file("inline=1", &inline_file(format, 18, 36)));
            let placements = engine.graphics_placements();
            assert_eq!(placements.len(), 1, "{format:?}");
            assert_eq!(
                (placements[0].image_width, placements[0].image_height),
                (18, 36)
            );
            // 9x18 px cells: the natural size covers 2x2 cells.
            assert_eq!(
                (placements[0].occupied_cols, placements[0].occupied_rows),
                (2, 2)
            );
            // Like iTerm2, no reply reaches the program's input.
            assert!(drain(&mut engine).is_empty(), "{format:?}");
        }
    }

    #[test]
    fn osc_1337_images_survive_fragments_st_and_payloads_past_the_osc_limit() {
        // An uncompressed BMP, base64-encoded well past the 64 KiB OSC limit.
        let mut stream = osc_file(
            "inline=1;width=4",
            &inline_file(::image::ImageFormat::Bmp, 160, 160),
        );
        assert!(stream.len() > super::super::parser::MAX_STRING_BYTES);
        stream.pop();
        stream.extend_from_slice(b"\x1b\\after");
        let mut engine = Engine::new(Size { cols: 20, rows: 6 }, Options::default());
        for chunk in stream.chunks(7) {
            engine.feed(chunk);
        }
        let placements = engine.graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].display_cols, Some(4));
        assert_eq!(placements[0].occupied_rows, 2);
        // Text continues right of the image.
        assert_eq!(engine.cursor().col, 9);
    }

    #[test]
    fn osc_1337_multipart_files_join_parts_split_anywhere() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let encoded = STANDARD.encode(inline_file(::image::ImageFormat::Png, 9, 18));
        let mut stream = b"\x1b]1337;MultipartFile=inline=1;width=3\x07".to_vec();
        for part in encoded.as_bytes().chunks(5) {
            stream.extend_from_slice(&[b"\x1b]1337;FilePart=", part, b"\x1b\\"].concat());
        }
        stream.extend_from_slice(b"\x1b]1337;FileEnd\x07");
        let mut engine = Engine::new(Size { cols: 20, rows: 6 }, Options::default());
        engine.feed(&stream);
        let placements = engine.graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_height, 18);
        assert_eq!(placements[0].display_cols, Some(3));

        // Parts and ends without a started upload are ignored.
        engine.feed(b"\x1b]1337;FilePart=AAAA\x07\x1b]1337;FileEnd\x07");
        assert_eq!(engine.graphics_placements().len(), 1);
    }

    #[test]
    fn osc_1337_ignores_downloads_and_bad_files_and_can_keep_the_cursor() {
        let file = inline_file(::image::ImageFormat::Png, 9, 18);
        let mut engine = Engine::new(Size { cols: 20, rows: 6 }, Options::default());
        engine.feed(&osc_file("name=eA==;size=10", &file));
        engine.feed(&osc_file("inline=0", &file));
        engine.feed(&osc_file("inline=1", b"not an image"));
        engine.feed(b"\x1b]1337;File=inline=1\x07\x1b]1337;SetUserVar=a=Yg==\x07");
        assert!(engine.graphics_placements().is_empty());
        assert!(drain(&mut engine).is_empty());

        engine.feed(b"\x1b[2;3H");
        engine.feed(&osc_file("inline=1;doNotMoveCursor=1", &file));
        assert_eq!(engine.graphics_placements().len(), 1);
        assert_eq!((engine.cursor().row, engine.cursor().col), (1, 2));
    }
}
