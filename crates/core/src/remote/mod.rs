//! Renderer-neutral remote terminal boundary. The session host owns the engine;
//! clients cache only the viewport and send input or history queries to it.
mod backend;
pub mod frame;
pub mod graphics;
pub(crate) use crate::serde_deadline;
pub(crate) mod serde_image;
pub(crate) mod serde_palette;

use crate::*;
pub(crate) use backend::RemoteBackend;
pub use frame::RemoteDamage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteState {
    pub render: TerminalRenderRead,
    pub size: TerminalSize,
    pub child_pid: Option<u32>,
    pub cursor_position: (usize, usize),
    pub mouse_mode: TerminalMouseMode,
    pub keyboard_mode: TerminalKeyboardMode,
    pub bracketed_paste: bool,
    pub alternate_screen: bool,
    pub clipboard_paste_events: bool,
    pub graphics_revision: u64,
    #[serde(with = "serde_deadline")]
    pub graphics_deadline: Option<std::time::Instant>,
    /// Rows output has scrolled into history so far (see
    /// `terminal_engine::Engine::scrolled_into_history`). Viewport row `row`
    /// shows the line numbered `lines_scrolled - display_offset + row`, so
    /// two states of one screen tell how far their content moved.
    pub lines_scrolled: u64,
    /// Local snapshot that a client rebuilds from the graphics stream
    /// (`Update::GraphicsState`). Never encoded with the state itself.
    #[serde(skip)]
    pub graphics: Option<Vec<KittyGraphicsRenderPlacement>>,
}

impl RemoteState {
    /// Reads everything under one engine lock: images, cells and screen
    /// always describe the same moment (audit L20).
    pub fn capture(terminal: &Terminal) -> Self {
        terminal.remote_state()
    }

    /// Whether `other` shows the same thing, so that publishing it again
    /// would send an identical frame (audit I8).
    pub(crate) fn same_frame(&self, other: &Self) -> bool {
        self.render.metadata == other.render.metadata
            && self.size == other.size
            && self.child_pid == other.child_pid
            && self.cursor_position == other.cursor_position
            && self.mouse_mode == other.mouse_mode
            && self.keyboard_mode == other.keyboard_mode
            && self.bracketed_paste == other.bracketed_paste
            && self.alternate_screen == other.alternate_screen
            && self.clipboard_paste_events == other.clipboard_paste_events
            && self.graphics_revision == other.graphics_revision
            && self.graphics_deadline == other.graphics_deadline
            && self.lines_scrolled == other.lines_scrolled
            && self.render.palette == other.render.palette
            && self.render.cells == other.render.cells
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteCommand {
    Write(Vec<u8>),
    FeedOutput(Vec<u8>),
    HydrateOutput(Vec<u8>),
    Resize(TerminalSize),
    NudgeResize,
    Scroll(i32),
    ScrollToBottom,
    ClearScrollback,
    SetOptions(TerminalOptions),
    SetScrollback(usize),
    SetQueryColors(TerminalQueryColors),
    Snapshot,
    Lines {
        first: i32,
        last: i32,
    },
    Search {
        query: String,
        options: TermySearchOptions,
    },
    /// Retired: the hyperlink lookup in the host's current viewport. See
    /// [`Self::HyperlinkAt`].
    RetiredHyperlink,
    /// Retired: the link lookup in the host's current viewport. See
    /// [`Self::LinkAt`].
    RetiredLink,
    Graphics,
    ClipboardPaste {
        location: TerminalClipboardLocation,
        formats: Vec<String>,
    },
    /// A mouse-motion report. Like [`Terminal::write_mouse_motion`] it may
    /// fill only half of the input backlog, so hovering over a program that
    /// stopped reading cannot crowd out later keys (audit B22).
    WriteMotion(Vec<u8>),
    /// The text of buffer lines `first..=last`, numbered like
    /// [`Terminal::visit_line_cells`] (0 is the top screen row, -1 the newest
    /// history row). The host answers with [`RemoteReply::LineTexts`]: the
    /// newest lines of the range, at most [`LINE_TEXTS_REPLY_BYTES`] of text;
    /// ask again for the lines before them. `anchor` is a
    /// [`RemoteState::lines_scrolled`]: the host then numbers lines as they
    /// were at that state, although output scrolled meanwhile.
    LineTexts {
        first: i32,
        last: i32,
        anchor: Option<u64>,
    },
    /// The OSC 8 hyperlink at a cell of the frame described by `view`. The
    /// host finds that row in its current viewport, or answers `None` when
    /// it has left the screen (audit L19).
    HyperlinkAt {
        row: usize,
        col: usize,
        view: ViewportAnchor,
    },
    /// The detected link at a cell of the frame described by `view`; see
    /// [`Self::HyperlinkAt`].
    LinkAt {
        row: usize,
        col: usize,
        view: ViewportAnchor,
    },
}

/// The frame a client shows and a row the user clicked in it, so that the
/// host can find that row after output or scrolling moved its viewport
/// (audit L19).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewportAnchor {
    pub generation: u64,
    pub display_offset: usize,
    pub history_size: usize,
    /// [`RemoteState::lines_scrolled`] of the frame.
    pub lines_scrolled: u64,
    pub alternate_screen: bool,
    pub cols: u16,
    pub rows: u16,
    /// [`row_text_hash`] of the clicked row.
    pub row_text: u64,
}

impl ViewportAnchor {
    /// The anchor of viewport row `row` of `state`.
    pub fn at(state: &RemoteState, row: usize) -> Self {
        let metadata = state.render.metadata;
        let cols = usize::from(metadata.cols);
        let cells = state.render.cells.get(row * cols..(row + 1) * cols);
        Self {
            generation: metadata.generation,
            display_offset: metadata.display_offset,
            history_size: metadata.history_size,
            lines_scrolled: state.lines_scrolled,
            alternate_screen: state.alternate_screen,
            cols: metadata.cols,
            rows: metadata.rows,
            row_text: row_text_hash(
                cells
                    .unwrap_or_default()
                    .iter()
                    .map(|cell| cell.text.as_str()),
            ),
        }
    }

    /// The row that shows viewport row `row` of this frame in `now`, or
    /// `None` when it is no longer on screen. The scroll count tells where
    /// output moved the row; `row_text` (the hash of a row of `now`) must
    /// then still match, because scrolling inside a region, on the alternate
    /// screen or without scrollback does not count.
    pub(crate) fn row_in(
        &self,
        row: usize,
        now: &ViewportAnchor,
        row_text: impl FnOnce(usize) -> u64,
    ) -> Option<usize> {
        if (self.cols, self.rows, self.alternate_screen)
            != (now.cols, now.rows, now.alternate_screen)
            || row >= usize::from(self.rows)
        {
            return None;
        }
        if self.generation == now.generation {
            return Some(row);
        }
        let top =
            |view: &ViewportAnchor| i128::from(view.lines_scrolled) - view.display_offset as i128;
        let moved = top(now) - top(self);
        let current = row as i128 - moved;
        let current = (0..i128::from(now.rows))
            .contains(&current)
            .then_some(current as usize)?;
        (row_text(current) == self.row_text).then_some(current)
    }
}

/// A hash of a row's cell texts (FNV-1a), the same on every platform.
pub(crate) fn row_text_hash<'a>(cells: impl Iterator<Item = &'a str>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for cell in cells {
        for byte in cell.bytes().chain([0xff]) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

/// The most text one [`RemoteReply::LineTexts`] carries.
pub const LINE_TEXTS_REPLY_BYTES: usize = 2 * 1024 * 1024;
/// The most cells one [`RemoteReply::Lines`] carries; a client asks again
/// for the lines after them.
pub const LINES_REPLY_CELLS: usize = 64 * 1024;

/// One buffer line as plain text, for search and copying (audit B13).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineText {
    /// The text of the line's cells, left to right. A blank, hidden or
    /// control cell reads as a space; trailing blank cells are left out.
    pub text: String,
    /// The length in bytes of each cell's text in `text`, up to the last
    /// cell that is not blank. The spacer after a wide character has none.
    pub columns: Vec<u8>,
    /// Whether the line continues on the next one (a soft wrap).
    pub wrapped: bool,
}

impl LineText {
    /// Each column's text; `""` for the spacer after a wide character and
    /// `" "` past the last cell that is not blank.
    pub fn cells(&self, cols: usize) -> impl Iterator<Item = &str> {
        let mut start = 0;
        let mut lengths = self.columns.iter();
        (0..cols).map(move |_| match lengths.next() {
            Some(&length) => {
                let end = (start + usize::from(length)).min(self.text.len());
                let cell = self.text.get(start..end).unwrap_or(" ");
                start = end;
                cell
            }
            None => " ",
        })
    }
}

/// Lines read with [`RemoteCommand::LineTexts`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineTexts {
    /// The first and last line of the buffer, and its width.
    pub bounds: (i32, i32, usize),
    /// The number of the first line in `lines`.
    pub first: i32,
    /// The [`RemoteState::lines_scrolled`] the lines are numbered by; the
    /// anchor that keeps the next request's line numbers.
    pub lines_scrolled: u64,
    pub lines: Vec<LineText>,
}

/// Reads lines `first..=last` with as many [`RemoteCommand::LineTexts`]
/// requests as their size needs, newest first. Every chunk is numbered as of
/// `anchor` (a [`RemoteState::lines_scrolled`], the shown frame's for a
/// view), or as of the first request, even when output scrolls between
/// requests. History that output drops meanwhile only shortens the result
/// at its start. `None` when a request fails.
pub(crate) fn read_line_texts(
    first: i32,
    last: i32,
    anchor: Option<u64>,
    mut request: impl FnMut(RemoteCommand) -> Option<RemoteReply>,
) -> Option<LineTexts> {
    let mut chunks: Vec<LineTexts> = Vec::new();
    let mut anchor = anchor;
    let mut next = last;
    loop {
        let Some(RemoteReply::LineTexts(chunk)) = request(RemoteCommand::LineTexts {
            first,
            last: next,
            anchor,
        }) else {
            return None;
        };
        anchor = Some(chunk.lines_scrolled);
        let done = chunk.lines.is_empty() || chunk.first <= first.max(chunk.bounds.0);
        next = chunk.first - 1;
        chunks.push(chunk);
        if done {
            break;
        }
    }
    let newest = chunks.first()?;
    let oldest = chunks.last()?;
    let mut texts = LineTexts {
        bounds: (oldest.bounds.0, newest.bounds.1, newest.bounds.2),
        first: oldest.first,
        lines_scrolled: newest.lines_scrolled,
        lines: Vec::with_capacity(chunks.iter().map(|chunk| chunk.lines.len()).sum()),
    };
    for chunk in chunks.into_iter().rev() {
        texts.lines.extend(chunk.lines);
    }
    Some(texts)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteReply {
    Ok,
    Changed(bool),
    Frame(TermyFrame),
    Lines {
        bounds: (i32, i32, usize),
        cells: Vec<TerminalRenderCell>,
    },
    Search(Vec<TermySearchMatch>),
    Hyperlink(Option<DetectedLink>),
    Link(Option<DetectedViewportLink>),
    Graphics(
        u64,
        #[serde(with = "serde_image::placements")] Vec<KittyGraphicsRenderPlacement>,
    ),
    LineTexts(LineTexts),
}

/// A clipboard operation is executed by the attached UI using its existing
/// permission policy. Detached sessions have no clipboard host.
pub type RemoteHostRequest = TerminalClipboardRequest;
pub type RemoteHostReply = TerminalClipboardReply;

/// Implemented by the IPC client, keeping sockets, process discovery, and
/// session lifetime outside the terminal engine. Dropping a transport detaches;
/// destroying the remote process is an explicit session-host operation.
pub trait RemoteTransport: Send + Sync {
    fn state(&self) -> Arc<RemoteState>;
    /// The state, with the viewport damage since the previous call when the
    /// transport knows it (from frame deltas, audit I1). Without it, the
    /// renderer's damage comes from comparing the cells with the frame it
    /// read before.
    fn state_with_damage(&self) -> (Arc<RemoteState>, Option<RemoteDamage>) {
        (self.state(), None)
    }
    fn request(&self, command: RemoteCommand) -> anyhow::Result<RemoteReply>;
    fn send(&self, command: RemoteCommand);
    /// Sends a command behind the commands queued by [`Self::send`] and
    /// waits for its reply, for commands that write to the PTY and must not
    /// overtake queued input. An error means the command was not sent;
    /// `None` means it was, but its reply did not come, so it may still run.
    fn send_and_wait(&self, command: RemoteCommand) -> anyhow::Result<Option<RemoteReply>> {
        self.request(command).map(Some)
    }
    fn take_events(&self) -> Vec<TerminalEvent>;
    fn take_host_requests(&self) -> Vec<(u64, RemoteHostRequest)>;
    fn reply_to_host(&self, id: u64, reply: RemoteHostReply);
    fn has_pending_events(&self) -> bool;
    fn set_wakeup_enabled(&self, enabled: bool);
    /// `true` when another client resized the pane after a frame showed the
    /// size this client last asked for (or attached at). A transport with a
    /// single client never reports one.
    fn host_size_overridden(&self) -> bool {
        false
    }
    /// Whether the host confirmed that it stopped sending frames to this
    /// hidden view (see [`Self::set_wakeup_enabled`]).
    fn frames_paused(&self) -> bool {
        false
    }
}

/// Execute a client operation against the authoritative terminal. No output
/// transcript is needed: both screen buffers, parser state, and history stay
/// in this engine for the lifetime of its PTY.
pub fn execute(terminal: &mut Terminal, command: RemoteCommand) -> RemoteReply {
    use RemoteCommand as C;
    match command {
        C::Write(bytes) => terminal.write_owned(bytes),
        C::FeedOutput(bytes) => terminal.feed_output(&bytes),
        C::HydrateOutput(bytes) => terminal.hydrate_output(&bytes),
        C::Resize(size) => terminal.resize(size),
        C::NudgeResize => terminal.nudge_resize(),
        C::Scroll(lines) => return RemoteReply::Changed(terminal.scroll_display(lines)),
        C::ScrollToBottom => return RemoteReply::Changed(terminal.scroll_to_bottom()),
        C::ClearScrollback => return RemoteReply::Changed(terminal.clear_scrollback()),
        C::SetOptions(options) => terminal.set_term_options(options),
        C::SetScrollback(lines) => terminal.set_scrollback_history(lines),
        C::SetQueryColors(colors) => terminal.set_query_colors(colors),
        C::Snapshot => return RemoteReply::Frame(terminal.snapshot()),
        C::Lines { first, last } => {
            // Bounded, so a long range cannot exceed the message limit; the
            // client asks again from the line after the last one returned.
            let cols = usize::from(terminal.size().cols).max(1);
            let start = first.max(terminal.line_bounds().0);
            let last = last.min(start.saturating_add((LINES_REPLY_CELLS / cols).max(1) as i32 - 1));
            let mut cells = Vec::new();
            let bounds =
                terminal.visit_line_cells(first, last, |_, _, _, cell| cells.push(cell.clone()));
            return RemoteReply::Lines { bounds, cells };
        }
        C::LineTexts {
            first,
            last,
            anchor,
        } => {
            return RemoteReply::LineTexts(terminal.line_texts(
                first,
                last,
                anchor,
                LINE_TEXTS_REPLY_BYTES,
            ));
        }
        C::Search { query, options } => {
            return RemoteReply::Search(terminal.search_with_options(&query, options));
        }
        C::HyperlinkAt { row, col, view } => {
            return RemoteReply::Hyperlink(terminal.hyperlink_at_view(row, col, view));
        }
        C::LinkAt { row, col, view } => {
            return RemoteReply::Link(terminal.link_at_view(row, col, view));
        }
        C::RetiredHyperlink => return RemoteReply::Hyperlink(None),
        C::RetiredLink => return RemoteReply::Link(None),
        C::WriteMotion(bytes) => terminal.write_mouse_motion(bytes),
        C::Graphics => {
            let (revision, placements) = terminal.kitty_graphics_snapshot();
            return RemoteReply::Graphics(revision, placements);
        }
        C::ClipboardPaste { location, formats } => {
            return RemoteReply::Changed(
                terminal.send_kitty_clipboard_paste_event(location, &formats),
            );
        }
    }
    RemoteReply::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit L20: a capture reads images and cells under one engine lock, so
    /// it never pairs the primary screen's images with the alternate screen.
    #[test]
    fn captures_never_mix_the_images_of_one_screen_with_the_other() {
        let terminal = Arc::new(Terminal::new_display(TerminalSize::default(), None));
        terminal.feed_output(b"\x1b_Ga=T,i=1,f=32,s=1,v=1,C=1,q=2;AQID/w==\x1b\\");
        let feeder = {
            let terminal = Arc::clone(&terminal);
            std::thread::spawn(move || {
                for _ in 0..300 {
                    terminal.feed_output(b"\x1b[?1049h");
                    terminal.feed_output(b"\x1b[?1049l");
                }
            })
        };
        let mut captures = 0;
        while !feeder.is_finished() {
            let state = RemoteState::capture(&terminal);
            let images = state.graphics.as_ref().unwrap().len();
            assert_eq!(
                images,
                usize::from(!state.alternate_screen),
                "capture {captures} mixed screens"
            );
            captures += 1;
        }
        feeder.join().unwrap();
    }

    fn display(cols: u16, rows: u16) -> Terminal {
        Terminal::new_display(
            TerminalSize {
                cols,
                rows,
                ..TerminalSize::default()
            },
            None,
        )
    }

    /// Audit B13: lines travel as text with wrap flags and cell widths.
    #[test]
    fn line_texts_keep_cell_columns_and_soft_wraps() {
        let terminal = display(6, 3);
        terminal.feed_output("e\u{301}界\x1b[8mhid\x1b[0m\r\nabcdefgh".as_bytes());
        let texts = terminal.line_texts(0, 2, None, usize::MAX);
        assert_eq!(texts.bounds, (0, 2, 6));
        assert_eq!(texts.first, 0);
        let [first, wrapped, rest] = texts.lines.as_slice() else {
            panic!("{texts:?}");
        };
        // Hidden text reads as blanks, and trailing blanks are left out.
        assert_eq!(first.text, "e\u{301}界");
        assert_eq!(first.columns, [3, 3, 0]);
        assert!(!first.wrapped);
        assert_eq!(
            first.cells(6).collect::<Vec<_>>(),
            ["e\u{301}", "界", "", " ", " ", " "]
        );
        assert_eq!(wrapped.text, "abcdef");
        assert!(wrapped.wrapped);
        assert_eq!(rest.text, "gh");
        assert!(!rest.wrapped);
    }

    /// Audit B13: a long range is read in bounded chunks, all numbered like
    /// the first one although output scrolls between them.
    #[test]
    fn chunked_line_texts_stay_anchored_while_output_scrolls() {
        let terminal = display(10, 4);
        for line in 0..200 {
            terminal.feed_output(format!("line {line}\r\n").as_bytes());
        }
        // History, which output does not rewrite, unlike the screen rows.
        let (first, last) = (terminal.line_bounds().0, -1);
        let expected: Vec<_> = terminal
            .line_texts(first, last, None, usize::MAX)
            .lines
            .into_iter()
            .map(|line| line.text)
            .collect();
        let mut requests = 0;
        let texts = read_line_texts(first, last, None, |command| {
            let RemoteCommand::LineTexts {
                first,
                last,
                anchor,
            } = command
            else {
                panic!("unexpected {command:?}");
            };
            requests += 1;
            let reply = terminal.line_texts(first, last, anchor, 100);
            terminal.feed_output(format!("more {requests}\r\n").as_bytes());
            Some(RemoteReply::LineTexts(reply))
        })
        .unwrap();
        assert!(requests > 10, "{requests} requests");
        assert_eq!(texts.first, first);
        let read: Vec<_> = texts.lines.into_iter().map(|line| line.text).collect();
        assert_eq!(read, expected);
    }

    /// Audit B13: when output drops old history faster than a long read
    /// proceeds, the result loses only its oldest lines: it still ends with
    /// the newest ones, numbered as of the frame the read started from.
    #[test]
    fn chunked_line_texts_keep_the_newest_lines_while_history_is_dropped() {
        let terminal = display(10, 4);
        terminal.set_scrollback_history(60);
        for line in 0..100 {
            terminal.feed_output(format!("line {line}\r\n").as_bytes());
        }
        let shown = RemoteState::capture(&terminal);
        let (first, last) = (terminal.line_bounds().0, -1);
        let newest = terminal.line_texts(last, last, None, usize::MAX).lines[0]
            .text
            .clone();
        let mut more = 0;
        let texts = read_line_texts(first, last, Some(shown.lines_scrolled), |command| {
            let RemoteCommand::LineTexts {
                first,
                last,
                anchor,
            } = command
            else {
                panic!("unexpected {command:?}");
            };
            let reply = terminal.line_texts(first, last, anchor, 60);
            for _ in 0..8 {
                more += 1;
                terminal.feed_output(format!("more {more}\r\n").as_bytes());
            }
            Some(RemoteReply::LineTexts(reply))
        })
        .unwrap();
        assert!(texts.first > first, "no history was dropped meanwhile");
        let numbers: Vec<u32> = texts
            .lines
            .iter()
            .map(|line| line.text.strip_prefix("line ").unwrap().parse().unwrap())
            .collect();
        assert_eq!(texts.lines.last().unwrap().text, newest);
        assert!(
            numbers.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "{numbers:?}"
        );
        assert_eq!(texts.first + texts.lines.len() as i32 - 1, last);
    }

    /// Audit B13: styled history replies are bounded too.
    #[test]
    fn styled_line_replies_are_chunked() {
        let mut terminal = display(40, 4);
        for line in 0..3000 {
            terminal.feed_output(format!("line {line}\r\n").as_bytes());
        }
        let (first, last) = terminal.line_bounds();
        let RemoteReply::Lines { bounds, cells } =
            execute(&mut terminal, RemoteCommand::Lines { first, last })
        else {
            panic!("expected lines");
        };
        assert_eq!((bounds.0, bounds.1), (first, last));
        assert!(!cells.is_empty() && cells.len() <= LINES_REPLY_CELLS);
        assert_eq!(cells.len() % 40, 0);
    }

    #[test]
    fn anchors_follow_rows_as_output_scrolls() {
        let then = ViewportAnchor {
            generation: 1,
            display_offset: 0,
            history_size: 10,
            lines_scrolled: 10,
            alternate_screen: false,
            cols: 80,
            rows: 24,
            row_text: 7,
        };
        let now = |lines_scrolled, display_offset| ViewportAnchor {
            generation: 2,
            lines_scrolled,
            display_offset,
            row_text: 0,
            ..then
        };
        let same = |_| 7;
        assert_eq!(then.row_in(5, &then, |_| 0), Some(5), "same frame");
        // Two lines of output moved row 5 up to row 3.
        assert_eq!(then.row_in(5, &now(12, 0), same), Some(3));
        assert_eq!(then.row_in(1, &now(12, 0), same), None);
        // Scrolled back by three lines: row 5 is now row 8.
        assert_eq!(then.row_in(5, &now(10, 3), same), Some(8));
        assert_eq!(then.row_in(23, &now(10, 3), same), None);
        // The row there now shows other text: the click is stale.
        assert_eq!(then.row_in(5, &now(12, 0), |_| 8), None);
        // Another screen or size: the row is gone.
        let alternate = ViewportAnchor {
            alternate_screen: true,
            ..now(10, 0)
        };
        assert_eq!(then.row_in(5, &alternate, same), None);
        let wider = ViewportAnchor {
            cols: 81,
            ..now(10, 0)
        };
        assert_eq!(then.row_in(5, &wider, same), None);
    }
}
