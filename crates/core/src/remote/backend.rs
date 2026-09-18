use super::*;
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
struct GraphicsCacheKey {
    revision: u64,
    generation: u64,
    cols: u16,
    rows: u16,
    display_offset: usize,
    history_size: usize,
    alternate_screen: bool,
}

impl GraphicsCacheKey {
    fn of(state: &RemoteState) -> Self {
        let metadata = state.render.metadata;
        Self {
            revision: state.graphics_revision,
            generation: metadata.generation,
            cols: metadata.cols,
            rows: metadata.rows,
            display_offset: metadata.display_offset,
            history_size: metadata.history_size,
            alternate_screen: state.alternate_screen,
        }
    }
}

#[derive(Default)]
pub(crate) struct LegacyGraphicsCache {
    key: Option<GraphicsCacheKey>,
    placements: Vec<KittyGraphicsRenderPlacement>,
}

impl LegacyGraphicsCache {
    pub(crate) fn update(
        &mut self,
        state: &mut RemoteState,
        fetch: impl FnOnce() -> anyhow::Result<(u64, Vec<KittyGraphicsRenderPlacement>)>,
    ) -> anyhow::Result<()> {
        let key = GraphicsCacheKey::of(state);
        if self.key != Some(key) {
            let (_, placements) = fetch()?;
            self.placements = placements;
            self.key = Some(key);
        }
        state.graphics = Some(self.placements.clone());
        Ok(())
    }
}

struct GraphicsCache {
    key: Option<GraphicsCacheKey>,
    snapshot: (u64, Vec<KittyGraphicsRenderPlacement>),
}

pub(crate) struct RemoteBackend {
    transport: Arc<dyn RemoteTransport>,
    last_render: Mutex<Option<Arc<RemoteState>>>,
    graphics: Mutex<GraphicsCache>,
}

impl RemoteBackend {
    pub(crate) fn new(transport: Arc<dyn RemoteTransport>) -> Self {
        Self {
            transport,
            last_render: Mutex::new(None),
            graphics: Mutex::new(GraphicsCache {
                key: None,
                snapshot: (u64::MAX, Vec::new()),
            }),
        }
    }

    fn request(&self, command: RemoteCommand) -> Option<RemoteReply> {
        match self.transport.request(command) {
            Ok(reply) => Some(reply),
            Err(error) => {
                log::error!("multiplexer request failed: {error}");
                None
            }
        }
    }

    pub(crate) fn feed_output(&self, bytes: &[u8]) {
        self.transport
            .send(RemoteCommand::FeedOutput(bytes.to_vec()));
    }
    pub(crate) fn hydrate_output(&self, bytes: &[u8]) {
        self.transport
            .send(RemoteCommand::HydrateOutput(bytes.to_vec()));
    }
    pub(crate) fn write(&self, bytes: &[u8]) {
        self.write_owned(bytes.to_vec());
    }
    pub(crate) fn write_owned(&self, bytes: Vec<u8>) {
        self.transport.send(RemoteCommand::Write(bytes));
    }
    pub(crate) fn write_str(&self, text: &str) {
        self.write(text.as_bytes());
    }
    pub(crate) fn child_pid(&self) -> Option<u32> {
        self.transport.state().child_pid
    }
    pub(crate) fn size(&self) -> TerminalSize {
        self.transport.state().size
    }
    pub(crate) fn resize(&mut self, size: TerminalSize) {
        self.transport.send(RemoteCommand::Resize(size));
    }
    pub(crate) fn nudge_resize(&self) {
        self.transport.send(RemoteCommand::NudgeResize);
    }
    pub(crate) fn set_wakeup_enabled(&self, enabled: bool) {
        self.transport.set_wakeup_enabled(enabled);
    }
    pub(crate) fn set_term_options(&self, options: TerminalOptions) {
        self.transport.send(RemoteCommand::SetOptions(options));
    }
    pub(crate) fn set_scrollback_history(&self, lines: usize) {
        self.transport.send(RemoteCommand::SetScrollback(lines));
    }
    pub(crate) fn set_query_colors(&mut self, colors: TerminalQueryColors) {
        self.transport.send(RemoteCommand::SetQueryColors(colors));
    }
    pub(crate) fn palette(&self) -> TerminalPalette {
        self.transport.state().render.palette.clone()
    }
    pub(crate) fn cursor_state(&self) -> Option<TerminalCursorState> {
        self.transport.state().render.metadata.cursor
    }
    pub(crate) fn cursor_position(&self) -> (usize, usize) {
        self.transport.state().cursor_position
    }
    pub(crate) fn mouse_mode(&self) -> TerminalMouseMode {
        self.transport.state().mouse_mode
    }
    pub(crate) fn keyboard_mode(&self) -> TerminalKeyboardMode {
        self.transport.state().keyboard_mode
    }
    pub(crate) fn bracketed_paste_mode(&self) -> bool {
        self.transport.state().bracketed_paste
    }
    pub(crate) fn alternate_screen_mode(&self) -> bool {
        self.transport.state().alternate_screen
    }
    pub(crate) fn kitty_clipboard_paste_events_enabled(&self) -> bool {
        self.transport.state().clipboard_paste_events
    }
    pub(crate) fn has_pending_events(&self) -> bool {
        self.transport.has_pending_events()
    }

    pub(crate) fn drain_events(
        &self,
        host: &mut impl TerminalReplyHost,
    ) -> (Vec<TerminalEvent>, bool) {
        for (id, request) in self.transport.take_host_requests() {
            self.transport.reply_to_host(id, request.execute(host));
        }
        (
            self.transport.take_events(),
            self.transport.has_pending_events(),
        )
    }

    pub(crate) fn send_kitty_clipboard_paste_event(
        &self,
        location: TerminalClipboardLocation,
        formats: &[String],
    ) -> bool {
        matches!(
            self.request(RemoteCommand::ClipboardPaste {
                location,
                formats: formats.to_vec()
            }),
            Some(RemoteReply::Changed(true))
        )
    }

    pub(crate) fn kitty_graphics_revision(&self) -> u64 {
        self.transport.state().graphics_revision
    }
    pub(crate) fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        let state = self.transport.state();
        if let Some(placements) = &state.graphics {
            return (state.graphics_revision, placements.clone());
        }
        // Placements are viewport-relative. History scrolling and Unicode
        // placeholder redraws can move them without changing image storage.
        let key = GraphicsCacheKey::of(&state);
        let mut cached = self.graphics.lock().unwrap();
        if cached.key != Some(key)
            && let Some(RemoteReply::Graphics(revision, placements)) =
                self.request(RemoteCommand::Graphics)
        {
            // Tag the reply with the state that prompted the request. If a
            // newer frame arrives during the RPC, its key must trigger a refresh.
            cached.key = Some(key);
            cached.snapshot = (revision, placements);
        }
        cached.snapshot.clone()
    }
    pub(crate) fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        self.kitty_graphics_snapshot().1
    }

    pub(crate) fn scroll_state(&self) -> (usize, usize) {
        let metadata = self.transport.state().render.metadata;
        (metadata.display_offset, metadata.history_size)
    }
    pub(crate) fn scroll_display(&self, lines: i32) -> bool {
        matches!(
            self.request(RemoteCommand::Scroll(lines)),
            Some(RemoteReply::Changed(true))
        )
    }
    pub(crate) fn scroll_to_bottom(&self) -> bool {
        matches!(
            self.request(RemoteCommand::ScrollToBottom),
            Some(RemoteReply::Changed(true))
        )
    }
    pub(crate) fn clear_scrollback(&self) -> bool {
        matches!(
            self.request(RemoteCommand::ClearScrollback),
            Some(RemoteReply::Changed(true))
        )
    }

    fn damage(&self, state: &Arc<RemoteState>, force_full: bool) -> TerminalRenderDamageSnapshot {
        let mut previous = self.last_render.lock().unwrap();
        let metadata = state.render.metadata;
        let damage = match previous.as_ref().filter(|old| {
            !force_full
                && old.render.metadata.cols == metadata.cols
                && old.render.metadata.rows == metadata.rows
                && old.render.metadata.display_offset == metadata.display_offset
                && old.render.metadata.history_size == metadata.history_size
                && old.render.palette == state.render.palette
        }) {
            None => TerminalDamageSnapshot::Full,
            Some(old) if Arc::ptr_eq(old, state) => TerminalDamageSnapshot::Partial(Vec::new()),
            Some(old) => {
                let cols = usize::from(metadata.cols);
                let mut spans = Vec::new();
                for (row, cells) in state.render.cells.chunks(cols).enumerate() {
                    let start = row * cols;
                    let mut left = None;
                    for (col, cell) in cells.iter().enumerate() {
                        if old.render.cells.get(start + col) != Some(cell) {
                            left.get_or_insert(col);
                        } else if let Some(left_col) = left.take() {
                            spans.push(TerminalDirtySpan {
                                row,
                                left_col,
                                right_col: col - 1,
                            });
                        }
                    }
                    if let Some(left_col) = left {
                        spans.push(TerminalDirtySpan {
                            row,
                            left_col,
                            right_col: cells.len() - 1,
                        });
                    }
                }
                TerminalDamageSnapshot::Partial(spans)
            }
        };
        *previous = Some(Arc::clone(state));
        TerminalRenderDamageSnapshot {
            damage,
            scrolls: Vec::new(),
            generation: metadata.generation,
            palette_revision: metadata.palette_revision,
        }
    }

    pub(crate) fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        self.damage(&self.transport.state(), false)
    }
    pub(crate) fn take_damage_snapshot(&self) -> TerminalDamageSnapshot {
        self.take_render_damage_snapshot().damage
    }
    pub(crate) fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        self.render_read_with_screen(force_full).0
    }
    pub(crate) fn render_read_with_screen(&self, force_full: bool) -> (TerminalRenderRead, bool) {
        let state = self.transport.state();
        let mut read = state.render.clone();
        read.update = self.damage(&state, force_full);
        (read, state.alternate_screen)
    }

    pub(crate) fn visit_viewport_cells(
        &self,
        mut visitor: impl FnMut(usize, i32, usize, &TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        let state = self.transport.state();
        let metadata = state.render.metadata;
        let cols = usize::from(metadata.cols);
        for (index, cell) in state.render.cells.iter().enumerate() {
            visitor(
                metadata.display_offset,
                (index / cols) as i32 - metadata.display_offset as i32,
                index % cols,
                cell,
            );
        }
        metadata
    }

    pub(crate) fn visit_viewport_ranges_at_generation(
        &self,
        generation: u64,
        spans: &[TerminalDirtySpan],
        mut visitor: impl FnMut(usize, usize, i32, usize, &TerminalRenderCell),
    ) -> bool {
        let state = self.transport.state();
        let metadata = state.render.metadata;
        if metadata.generation != generation {
            return false;
        }
        let cols = usize::from(metadata.cols);
        for span in spans {
            if span.row >= usize::from(metadata.rows)
                || span.left_col > span.right_col
                || span.right_col >= cols
            {
                return false;
            }
        }
        for span in spans {
            for col in span.left_col..=span.right_col {
                visitor(
                    span.row,
                    metadata.display_offset,
                    span.row as i32 - metadata.display_offset as i32,
                    col,
                    &state.render.cells[span.row * cols + col],
                );
            }
        }
        true
    }

    pub(crate) fn line_bounds(&self) -> (i32, i32) {
        let state = self.transport.state();
        (
            -(state.render.metadata.history_size as i32),
            i32::from(state.size.rows) - 1,
        )
    }

    pub(crate) fn visit_line_cells(
        &self,
        first: i32,
        last: i32,
        mut visitor: impl FnMut((i32, i32, usize), i32, usize, &TerminalRenderCell),
    ) -> (i32, i32, usize) {
        let state = self.transport.state();
        let metadata = state.render.metadata;
        let cols = usize::from(metadata.cols);
        let bounds = (
            -(metadata.history_size as i32),
            i32::from(metadata.rows) - 1,
            cols,
        );
        if first > last {
            return bounds;
        }
        let viewport_first = -(metadata.display_offset as i32);
        let viewport_last = viewport_first + i32::from(metadata.rows) - 1;
        if first >= viewport_first && last <= viewport_last {
            // Selection belongs to the frame the user can see. Querying the
            // host here both blocks the UI and may read newer, shifted lines.
            for line in first..=last {
                let start = (line - viewport_first) as usize * cols;
                for (col, cell) in state.render.cells[start..start + cols].iter().enumerate() {
                    visitor(bounds, line, col, cell);
                }
            }
            return bounds;
        }
        if let Some(RemoteReply::Lines { bounds, cells }) =
            self.request(RemoteCommand::Lines { first, last })
        {
            let start = first.max(bounds.0).min(bounds.1);
            for (index, cell) in cells.iter().enumerate() {
                visitor(
                    bounds,
                    start + (index / bounds.2) as i32,
                    index % bounds.2,
                    cell,
                );
            }
            bounds
        } else {
            let (first, last) = self.line_bounds();
            (first, last, usize::from(self.size().cols))
        }
    }

    pub(crate) fn search(&self, query: &str) -> Vec<TermySearchMatch> {
        self.search_with_options(query, TermySearchOptions::default())
    }
    pub(crate) fn search_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySearchMatch> {
        if let Some(RemoteReply::Search(matches)) = self.request(RemoteCommand::Search {
            query: query.to_owned(),
            options,
        }) {
            matches
        } else {
            Vec::new()
        }
    }
    pub(crate) fn search_shared(&self, query: &str) -> Vec<TermySharedSearchMatch> {
        self.search_shared_with_options(query, TermySearchOptions::default())
    }
    pub(crate) fn search_shared_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySharedSearchMatch> {
        let mut lines = std::collections::HashMap::new();
        self.search_with_options(query, options)
            .into_iter()
            .map(|item| {
                let line = lines.entry(item.row).or_insert_with(|| Arc::new(item.line));
                TermySharedSearchMatch {
                    row: item.row,
                    start_col: item.start_col,
                    end_col: item.end_col,
                    line: Arc::clone(line),
                }
            })
            .collect()
    }
    pub(crate) fn hyperlink_at(&self, row: usize, col: usize) -> Option<DetectedLink> {
        let state = self.transport.state();
        let cols = usize::from(state.render.metadata.cols);
        if col >= cols
            || row >= usize::from(state.render.metadata.rows)
            || !state.render.cells[row * cols + col].hyperlink
        {
            return None;
        }
        match self.request(RemoteCommand::Hyperlink { row, col }) {
            Some(RemoteReply::Hyperlink(link)) => link,
            _ => None,
        }
    }
    pub(crate) fn link_at(&self, row: usize, col: usize) -> Option<DetectedViewportLink> {
        match self.request(RemoteCommand::Link { row, col }) {
            Some(RemoteReply::Link(link)) => link,
            _ => None,
        }
    }
    pub(crate) fn snapshot(&self) -> TermyFrame {
        if let Some(RemoteReply::Frame(frame)) = self.request(RemoteCommand::Snapshot) {
            frame
        } else {
            let metadata = self.transport.state().render.metadata;
            TermyFrame {
                cols: metadata.cols,
                rows: metadata.rows,
                cells: Vec::new(),
                cursor: metadata.cursor,
                display_offset: metadata.display_offset,
                history_size: metadata.history_size,
            }
        }
    }

    pub(crate) fn frame_update(&self, _force_full: bool) -> TermyFrameUpdate {
        let frame = self.snapshot();
        TermyFrameUpdate {
            cols: frame.cols,
            rows: frame.rows,
            cells: frame.cells,
            cursor: frame.cursor,
            display_offset: frame.display_offset,
            history_size: frame.history_size,
            damage: TerminalDamageSnapshot::Full,
        }
    }
}
