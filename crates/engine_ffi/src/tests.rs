use std::ptr;

use crate::frame::*;
use crate::input::*;
use crate::text::*;
use crate::*;

struct Handle(*mut TermyEngine);

impl Handle {
    fn new(cols: u16, rows: u16, reply_mode: u32) -> Self {
        let mut options = termy_engine_options_default();
        options.cols = cols;
        options.rows = rows;
        options.reply_mode = reply_mode;
        options.scrollback_rows = 100;
        Self::with_options(&options)
    }

    fn with_options(options: &TermyEngineOptions) -> Self {
        let mut engine = ptr::null_mut();
        // SAFETY: valid options and out pointer.
        let status = unsafe { termy_engine_new(options, &raw mut engine) };
        assert_eq!(status, TermyEngineStatus::Ok);
        Self(engine)
    }

    fn feed(&self, bytes: &[u8]) -> Vec<u8> {
        let mut replies = TermyEngineBytes::EMPTY;
        // SAFETY: live handle, valid input and out buffer.
        let status =
            unsafe { termy_engine_feed(self.0, bytes.as_ptr(), bytes.len(), &raw mut replies) };
        assert_eq!(status, TermyEngineStatus::Ok);
        take(replies)
    }

    fn state(&self) -> TermyEngineState {
        let mut state = TermyEngineState::default();
        // SAFETY: live handle and out struct.
        assert_eq!(
            unsafe { termy_engine_get_state(self.0, &raw mut state) },
            TermyEngineStatus::Ok
        );
        state
    }

    fn frame(&self, force_full: bool) -> OwnedFrame {
        let mut frame = TermyEngineFrame::default();
        // SAFETY: live handle and out struct.
        assert_eq!(
            unsafe { termy_engine_frame(self.0, force_full, &raw mut frame) },
            TermyEngineStatus::Ok
        );
        OwnedFrame::copy(&frame)
    }

    fn mode(&self, mode: u16, ansi: bool) -> u8 {
        let mut report = 9;
        // SAFETY: live handle and out pointer.
        assert_eq!(
            unsafe { termy_engine_mode(self.0, mode, ansi, &raw mut report) },
            TermyEngineStatus::Ok
        );
        report
    }

    fn snapshot(&self) -> Vec<u8> {
        let mut bytes = TermyEngineBytes::EMPTY;
        // SAFETY: live handle and out buffer.
        assert_eq!(
            unsafe { termy_engine_snapshot_encode(self.0, &raw mut bytes) },
            TermyEngineStatus::Ok
        );
        take(bytes)
    }

    fn decode(snapshot: &[u8], reply_mode: u32) -> Self {
        let mut engine = ptr::null_mut();
        // SAFETY: valid input and out pointer.
        let status = unsafe {
            termy_engine_snapshot_decode(
                snapshot.as_ptr(),
                snapshot.len(),
                reply_mode,
                &raw mut engine,
            )
        };
        assert_eq!(status, TermyEngineStatus::Ok);
        Self(engine)
    }

    fn text(&self, range: Option<TermyEngineTextRange>, format: u32, options: u32) -> String {
        let mut out = TermyEngineBytes::EMPTY;
        let range_ptr = range.as_ref().map_or(ptr::null(), ptr::from_ref);
        // SAFETY: live handle, optional range and out buffer.
        assert_eq!(
            unsafe { termy_engine_read_text(self.0, range_ptr, format, options, &raw mut out) },
            TermyEngineStatus::Ok
        );
        String::from_utf8(take(out)).unwrap()
    }

    fn key(&self, key: &str, text: &str, mods: u32, consumed: u32, option_as_alt: u32) -> Vec<u8> {
        let event = TermyEngineKeyEvent {
            key_ptr: key.as_ptr(),
            key_len: key.len(),
            text_ptr: text.as_ptr(),
            text_len: text.len(),
            unshifted_codepoint: 0,
            mods,
            consumed_mods: consumed,
            action: KEY_PRESS,
            composing: 0,
        };
        let mut out = TermyEngineBytes::EMPTY;
        // SAFETY: live handle, event and out buffer.
        assert_eq!(
            unsafe {
                termy_engine_encode_key(self.0, &raw const event, option_as_alt, &raw mut out)
            },
            TermyEngineStatus::Ok
        );
        take(out)
    }

    fn mouse(&self, action: u8, button: u8, col: u32, row: u32, pixels: (u32, u32)) -> Vec<u8> {
        let event = TermyEngineMouseEvent {
            action,
            button,
            any_button_pressed: u8::from(button != MOUSE_BUTTON_NONE),
            mods: 0,
            col,
            row,
            x_px: pixels.0,
            y_px: pixels.1,
        };
        let mut out = TermyEngineBytes::EMPTY;
        // SAFETY: live handle, event and out buffer.
        assert_eq!(
            unsafe { termy_engine_encode_mouse(self.0, &raw const event, &raw mut out) },
            TermyEngineStatus::Ok
        );
        take(out)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: owned handle, freed once.
        unsafe { termy_engine_free(self.0) };
    }
}

fn take(mut bytes: TermyEngineBytes) -> Vec<u8> {
    let out = if bytes.ptr.is_null() {
        Vec::new()
    } else {
        // SAFETY: a live buffer from the library.
        unsafe { std::slice::from_raw_parts(bytes.ptr, bytes.len) }.to_vec()
    };
    // SAFETY: a buffer from the library.
    unsafe { termy_engine_bytes_free(&raw mut bytes) };
    assert!(bytes.ptr.is_null());
    out
}

#[derive(Debug, PartialEq)]
struct OwnedFrame {
    cols: u16,
    rows: u16,
    damage: u32,
    spans: Vec<TermyEngineSpan>,
    cells: Vec<TermyEngineCell>,
    graphemes: Vec<u32>,
    links: Vec<(String, String)>,
    cursor: TermyEngineCursor,
}

impl OwnedFrame {
    fn copy(frame: &TermyEngineFrame) -> Self {
        // SAFETY: the frame's pointers are valid for their lengths until the
        // next frame call on the handle.
        unsafe {
            let slice = |ptr: *const u8, len: usize| {
                String::from_utf8(std::slice::from_raw_parts(ptr, len).to_vec()).unwrap()
            };
            Self {
                cols: frame.cols,
                rows: frame.rows,
                damage: frame.damage,
                spans: std::slice::from_raw_parts(frame.spans_ptr, frame.spans_len).to_vec(),
                cells: std::slice::from_raw_parts(frame.cells_ptr, frame.cells_len).to_vec(),
                graphemes: std::slice::from_raw_parts(frame.graphemes_ptr, frame.graphemes_len)
                    .to_vec(),
                links: std::slice::from_raw_parts(frame.hyperlinks_ptr, frame.hyperlinks_len)
                    .iter()
                    .map(|link| {
                        (
                            slice(link.id_ptr, link.id_len),
                            slice(link.uri_ptr, link.uri_len),
                        )
                    })
                    .collect(),
                cursor: frame.cursor,
            }
        }
    }

    fn cell(&self, row: usize, col: usize) -> TermyEngineCell {
        self.cells[row * usize::from(self.cols) + col]
    }

    fn row_text(&self, row: usize) -> String {
        (0..usize::from(self.cols))
            .map(|col| self.cell(row, col))
            .filter(|cell| cell.width != CELL_SPACER_TAIL)
            .flat_map(|cell| {
                std::iter::once(cell.codepoint).chain(
                    self.graphemes[cell.grapheme_start as usize..]
                        [..usize::from(cell.grapheme_len)]
                        .iter()
                        .copied(),
                )
            })
            .filter_map(char::from_u32)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }
}

#[test]
fn abi_and_snapshot_versions_are_exposed() {
    assert_eq!(termy_engine_abi_version(), TERMY_ENGINE_ABI_VERSION);
    assert_eq!(termy_engine_snapshot_version(), SNAPSHOT_VERSION);
    assert_eq!(termy_engine_snapshot_min_version(), MIN_SNAPSHOT_VERSION);
}

#[test]
fn scrollback_and_payload_limits_are_configurable() {
    let mut options = termy_engine_options_default();
    assert_eq!(options.scrollback_bytes, 16 * 1024 * 1024);
    assert_eq!(options.max_string_bytes, 64 * 1024);
    options.scrollback_rows = TERMY_ENGINE_SCROLLBACK_UNLIMITED;
    options.max_string_bytes = 16;
    let engine = Handle::with_options(&options);
    for line in 0..25_000 {
        engine.feed(format!("line {line}\r\n").as_bytes());
    }
    let state = engine.state();
    assert_eq!(state.scrollback_limit, TERMY_ENGINE_SCROLLBACK_UNLIMITED);
    assert_eq!(state.scrollback_bytes, 16 * 1024 * 1024);
    assert_eq!(state.history_size, 25_000 - 23);
    // An oversized title is dropped.
    engine.feed(b"\x1b]2;a title past sixteen bytes\x07");
    let mut title = TermyEngineBytes::EMPTY;
    // SAFETY: live handle and out buffer.
    unsafe { termy_engine_title(engine.0, &raw mut title) };
    assert!(take(title).is_empty());

    // SAFETY: live handle.
    unsafe {
        assert_eq!(
            termy_engine_set_scrollback_bytes(engine.0, 256 * 1024),
            TermyEngineStatus::Ok
        );
        assert_eq!(
            termy_engine_set_payload_limits(engine.0, 0, 0),
            TermyEngineStatus::Ok
        );
    }
    let state = engine.state();
    assert_eq!(state.scrollback_bytes, 256 * 1024);
    assert_eq!(state.scrollback_limit, TERMY_ENGINE_SCROLLBACK_UNLIMITED);
    assert!(state.history_size < 25_000 - 23);
    // Setting rows keeps the byte limit; the defaults are back.
    // SAFETY: live handle.
    unsafe { termy_engine_set_scrollback(engine.0, 100) };
    let state = engine.state();
    assert_eq!((state.scrollback_limit, state.history_size), (100, 100));
    assert_eq!(state.scrollback_bytes, 256 * 1024);
    engine.feed(b"\x1b]2;a title past sixteen bytes\x07");
    let mut title = TermyEngineBytes::EMPTY;
    // SAFETY: live handle and out buffer.
    unsafe { termy_engine_title(engine.0, &raw mut title) };
    assert_eq!(take(title), b"a title past sixteen bytes");
}

#[test]
fn snapshots_encode_for_older_readers() {
    let server = Handle::new(24, 6, TERMY_ENGINE_REPLIES_RETURN);
    server.feed("old client \u{4e2d}\r\n".as_bytes());
    let mut bytes = TermyEngineBytes::EMPTY;
    // SAFETY: live handle and out buffer.
    let status = unsafe { termy_engine_snapshot_encode_version(server.0, 1, &raw mut bytes) };
    assert_eq!(status, TermyEngineStatus::Ok);
    let v1 = take(bytes);
    let mut version = 0;
    // SAFETY: valid input and out pointer.
    unsafe { termy_engine_snapshot_peek_version(v1.as_ptr(), v1.len(), &raw mut version) };
    assert_eq!(version, 1);
    let client = Handle::decode(&v1, TERMY_ENGINE_REPLIES_DISCARD);
    assert_eq!(client.frame(true), server.frame(true));

    let mut bytes = TermyEngineBytes::EMPTY;
    // SAFETY: live handle and out buffer.
    let status = unsafe {
        termy_engine_snapshot_encode_version(server.0, SNAPSHOT_VERSION + 1, &raw mut bytes)
    };
    assert_eq!(status, TermyEngineStatus::UnsupportedSnapshotVersion);
    assert!(take(bytes).is_empty());
}

#[test]
fn authoritative_replies_are_synchronous_and_ordered() {
    let engine = Handle::new(20, 5, TERMY_ENGINE_REPLIES_RETURN);
    let replies = engine.feed(b"\x1b[5n\x1b[c\x1b[?2004$p");
    let text = String::from_utf8(replies).unwrap();
    let status = text.find("\x1b[0n").expect("status report");
    let attributes = text.find("\x1b[?").expect("device attributes");
    let mode = text.find("\x1b[?2004;2$y").expect("mode report");
    assert!(status < attributes && attributes < mode, "{text:?}");
    // Nothing is left queued.
    assert!(engine.feed(b"").is_empty());
}

#[test]
fn queued_replies_wait_for_the_next_caller_in_order() {
    let engine = Handle::new(20, 5, TERMY_ENGINE_REPLIES_RETURN);
    // SAFETY: live handle; no out buffer leaves replies queued.
    unsafe { termy_engine_feed(engine.0, b"\x1b[5n".as_ptr(), 4, ptr::null_mut()) };
    let replies = engine.feed(b"\x1b[6n");
    assert_eq!(replies, b"\x1b[0n\x1b[1;1R");
}

#[test]
fn replicas_parse_queries_without_replying_or_changing_state() {
    let authoritative = Handle::new(30, 6, TERMY_ENGINE_REPLIES_RETURN);
    let replica = Handle::new(30, 6, TERMY_ENGINE_REPLIES_DISCARD);
    let stream: &[u8] =
        b"hi\x1b[5n\x1b[c\x1b]10;?\x07\x1b[>q\x1bP$qm\x1b\\\x1b[?2004h\x1b[6n there";
    assert!(!authoritative.feed(stream).is_empty());
    assert!(replica.feed(stream).is_empty());
    assert_eq!(replica.snapshot(), authoritative.snapshot());
    assert_eq!(replica.frame(true), authoritative.frame(true));
}

#[test]
fn snapshots_round_trip_through_the_c_abi() {
    let server = Handle::new(24, 6, TERMY_ENGINE_REPLIES_RETURN);
    server.feed("\x1b[1;4:3mstyled\x1b[0m wide \u{4e2d} e\u{301}\r\n".as_bytes());
    server.feed(b"\x1b]0;title\x07\x1b]7;file://h/srv\x07\x1b[?1049h\x1b[3;3Halt\x1b[");
    let snapshot = server.snapshot();
    let mut version = 0;
    // SAFETY: valid input and out pointer.
    let status = unsafe {
        termy_engine_snapshot_peek_version(snapshot.as_ptr(), snapshot.len(), &raw mut version)
    };
    assert_eq!(status, TermyEngineStatus::Ok);
    assert_eq!(version, SNAPSHOT_VERSION);

    let replica = Handle::decode(&snapshot, TERMY_ENGINE_REPLIES_DISCARD);
    // The parser was mid-CSI: both finish the same sequence.
    let tail = b"2;5Hmore\x1b[?1049l\x1b[5n tail";
    assert!(!server.feed(tail).is_empty());
    assert!(replica.feed(tail).is_empty());
    assert_eq!(replica.frame(true), server.frame(true));
    assert_eq!(replica.state().cursor_col, server.state().cursor_col);
    assert_eq!(replica.snapshot(), server.snapshot());

    let mut engine = ptr::null_mut();
    // SAFETY: valid input and out pointer.
    let status =
        unsafe { termy_engine_snapshot_decode(b"TRMYSNAP".as_ptr(), 8, 0, &raw mut engine) };
    assert_eq!(status, TermyEngineStatus::TruncatedSnapshot);
    assert!(engine.is_null());
}

#[test]
fn frames_carry_graphemes_wide_roles_styles_and_links() {
    let engine = Handle::new(12, 3, TERMY_ENGINE_REPLIES_RETURN);
    engine.feed(
        "e\u{301}\u{4e2d}\x1b[2;4:3;58;5;9;38;2;1;2;3mX\x1b[0m\x1b]8;id=k;http://a\x07L\x1b]8;;\x07"
            .as_bytes(),
    );
    let frame = engine.frame(false);
    assert_eq!(frame.damage, DAMAGE_FULL);
    assert_eq!(frame.cells.len(), 36);
    let accent = frame.cell(0, 0);
    assert_eq!(accent.codepoint, u32::from('e'));
    assert_eq!(
        &frame.graphemes[accent.grapheme_start as usize..][..usize::from(accent.grapheme_len)],
        &[0x301]
    );
    assert_eq!(frame.cell(0, 1).width, CELL_WIDE);
    assert_eq!(frame.cell(0, 2).width, CELL_SPACER_TAIL);
    let styled = frame.cell(0, 3);
    assert_eq!(styled.underline, 3);
    assert_eq!(styled.underline_color, 0x0100_0009);
    assert_eq!(styled.foreground, 0x0201_0203);
    assert_ne!(styled.flags & FLAG_FAINT, 0);
    assert_ne!(styled.flags & FLAG_STYLED, 0);
    let link = frame.cell(0, 4);
    assert_eq!(link.hyperlink, 1);
    assert_eq!(frame.links, vec![("k".to_owned(), "http://a".to_owned())]);
    assert_ne!(frame.cell(1, 0).flags & FLAG_BLANK, 0);
    assert_eq!(frame.row_text(0), "e\u{301}\u{4e2d}XL");

    // Small output is partial; a resize is full again.
    engine.feed(b"!");
    let update = engine.frame(false);
    assert_eq!(update.damage, DAMAGE_PARTIAL);
    assert!(!update.spans.is_empty());
    let span_cells: usize = update
        .spans
        .iter()
        .map(|span| usize::from(span.end_col - span.start_col))
        .sum();
    assert_eq!(update.cells.len(), span_cells);
    // SAFETY: live handle.
    assert_eq!(
        unsafe { termy_engine_resize(engine.0, 10, 4) },
        TermyEngineStatus::Ok
    );
    assert_eq!(engine.frame(false).damage, DAMAGE_FULL);
}

#[test]
fn soft_wrapped_rows_mark_their_last_cell() {
    let engine = Handle::new(5, 3, TERMY_ENGINE_REPLIES_RETURN);
    engine.feed(b"abcdefg");
    let frame = engine.frame(true);
    assert_ne!(frame.cell(0, 4).flags & FLAG_WRAPPED, 0);
    assert_eq!(frame.cell(1, 4).flags & FLAG_WRAPPED, 0);
}

#[test]
fn mode_queries_follow_decrqm() {
    let mut options = termy_engine_options_default();
    options.pixel_mouse_reports = 1;
    let engine = Handle::with_options(&options);
    assert_eq!(engine.mode(2004, false), 2);
    engine.feed(b"\x1b[?2004h\x1b[?1007h\x1b[?1016h\x1b[4h");
    assert_eq!(engine.mode(2004, false), 1);
    assert_eq!(engine.mode(1007, false), 1);
    assert_eq!(engine.mode(1016, false), 1);
    assert_eq!(engine.mode(2027, false), 3);
    assert_eq!(engine.mode(4, true), 1);
    assert_eq!(engine.mode(9999, false), 0);
    let state = engine.state();
    assert!(state.bracketed_paste && state.alternate_scroll);
    assert_eq!(state.mouse_encoding, 3);

    // Without the option ?1016 stays unrecognized.
    let plain = Handle::new(10, 2, TERMY_ENGINE_REPLIES_RETURN);
    plain.feed(b"\x1b[?1016h");
    assert_eq!(plain.mode(1016, false), 0);
    assert_eq!(plain.state().mouse_encoding, 0);
}

#[test]
fn title_and_working_directory_are_getters() {
    let engine = Handle::new(10, 2, TERMY_ENGINE_REPLIES_RETURN);
    engine.feed(b"\x1b]2;hello\x07\x1b]7;file://host/tmp/a%20b\x07");
    let mut title = TermyEngineBytes::EMPTY;
    let mut path = TermyEngineBytes::EMPTY;
    let mut found = false;
    // SAFETY: live handle and out pointers.
    unsafe {
        termy_engine_title(engine.0, &raw mut title);
        termy_engine_working_directory(engine.0, &raw mut found, &raw mut path);
    }
    assert_eq!(take(title), b"hello");
    assert!(found);
    assert_eq!(take(path), b"/tmp/a b");

    // A replica restored from a snapshot sees them without events.
    let replica = Handle::decode(&engine.snapshot(), TERMY_ENGINE_REPLIES_DISCARD);
    let mut title = TermyEngineBytes::EMPTY;
    // SAFETY: live handle and out buffer.
    unsafe { termy_engine_title(replica.0, &raw mut title) };
    assert_eq!(take(title), b"hello");
}

#[test]
fn events_are_drained_one_at_a_time() {
    let engine = Handle::new(10, 2, TERMY_ENGINE_REPLIES_RETURN);
    engine.feed(b"\x07\x1b]2;t\x07\x1b]9;4;1;40\x07");
    let mut kinds = Vec::new();
    loop {
        let mut found = false;
        let mut event = TermyEngineEvent {
            kind: 0,
            value: 0,
            payload: TermyEngineBytes::EMPTY,
            data: TermyEngineBytes::EMPTY,
        };
        // SAFETY: live handle and out pointers.
        unsafe { termy_engine_next_event(engine.0, &raw mut found, &raw mut event) };
        if !found {
            break;
        }
        kinds.push((event.kind, event.value));
        // SAFETY: an event from the library.
        unsafe { termy_engine_event_free(&raw mut event) };
    }
    assert_eq!(
        kinds,
        vec![
            (TERMY_ENGINE_EVENT_BELL, 0),
            (TERMY_ENGINE_EVENT_TITLE, 0),
            (TERMY_ENGINE_EVENT_PROGRESS, 1 << 8 | 40)
        ]
    );
}

#[test]
fn scrollbar_and_viewport_scrolling() {
    let engine = Handle::new(10, 3, TERMY_ENGINE_REPLIES_RETURN);
    for line in 0..20 {
        engine.feed(format!("line {line}\r\n").as_bytes());
    }
    let scrollbar = |engine: &Handle| {
        let mut out = TermyEngineScrollbar::default();
        // SAFETY: live handle and out struct.
        unsafe { termy_engine_scrollbar(engine.0, &raw mut out) };
        out
    };
    let history = engine.state().history_size as u64;
    assert_eq!(
        scrollbar(&engine),
        TermyEngineScrollbar {
            total: history + 3,
            offset: history,
            len: 3
        }
    );
    let mut changed = false;
    // SAFETY: live handle and out flag.
    unsafe { termy_engine_scroll_viewport(engine.0, TERMY_ENGINE_SCROLL_TOP, 0, &raw mut changed) };
    assert!(changed);
    assert_eq!(scrollbar(&engine).offset, 0);
    assert_eq!(engine.frame(false).row_text(0), "line 0");
    // SAFETY: live handle and out flag.
    unsafe {
        termy_engine_scroll_viewport(engine.0, TERMY_ENGINE_SCROLL_OFFSET, 5, &raw mut changed)
    };
    assert_eq!(scrollbar(&engine).offset, 5);
    assert_eq!(engine.frame(false).row_text(0), "line 5");
    // SAFETY: live handle and out flag.
    unsafe {
        termy_engine_scroll_viewport(engine.0, TERMY_ENGINE_SCROLL_BOTTOM, 0, &raw mut changed);
    }
    assert_eq!(scrollbar(&engine).offset, history);
}

#[test]
fn read_lines_caps_rows_to_the_frame_row_type() {
    let mut options = termy_engine_options_default();
    options.cols = 2;
    options.rows = 1;
    options.scrollback_rows = 70_000;
    let engine = Handle::with_options(&options);
    engine.feed(&b"x\r\n".repeat(70_000));
    let history = engine.state().history_size;
    assert!(history > u32::from(u16::MAX));
    let mut frame = TermyEngineFrame::default();
    // SAFETY: live handle and out struct.
    let status =
        unsafe { termy_engine_read_lines(engine.0, -i64::from(history), u32::MAX, &raw mut frame) };
    assert_eq!(status, TermyEngineStatus::Ok);
    // The row count must agree with the cells; more rows need another call.
    assert_eq!(frame.rows, u16::MAX);
    assert_eq!(
        frame.cells_len,
        usize::from(frame.rows) * usize::from(frame.cols)
    );
    // A first line past the end reads nothing instead of overflowing.
    // SAFETY: live handle and out struct.
    let status = unsafe { termy_engine_read_lines(engine.0, i64::MAX, 10, &raw mut frame) };
    assert_eq!(status, TermyEngineStatus::Ok);
    assert_eq!((frame.rows, frame.cells_len), (0, 0));
}

#[test]
fn effective_colors_apply_osc_overrides() {
    let engine = Handle::new(10, 2, TERMY_ENGINE_REPLIES_RETURN);
    let palette: Vec<u32> = (0..256).map(|index| index * 0x10101).collect();
    let cursor = 0x00ff00;
    // SAFETY: live handle, one cursor color and 256 palette entries.
    unsafe {
        termy_engine_set_default_colors(engine.0, 0xeeeeee, 0x111111, &cursor, palette.as_ptr());
    }
    engine.feed(b"\x1b]4;3;rgb:12/34/56\x07\x1b]11;#222222\x07");
    let mut colors = TermyEngineColors {
        foreground: 0,
        background: 0,
        cursor: 0,
        cursor_set: false,
        reverse_video: false,
        palette: [0; 256],
    };
    // SAFETY: live handle and out struct.
    unsafe { termy_engine_effective_colors(engine.0, &raw mut colors) };
    assert_eq!(colors.foreground, 0xeeeeee);
    assert_eq!(colors.background, 0x222222);
    assert_eq!(colors.cursor, 0x00ff00);
    assert_eq!(colors.palette[3], 0x123456);
    assert_eq!(colors.palette[200], 200 * 0x10101);
    // Queries answer with the host's colors too.
    let reply = engine.feed(b"\x1b]4;1;?\x07");
    assert_eq!(reply, b"\x1b]4;1;rgb:0101/0101/0101\x07");
}

#[test]
fn key_encoder_handles_modes_and_option_sides() {
    let engine = Handle::new(10, 2, TERMY_ENGINE_REPLIES_RETURN);
    assert_eq!(engine.key("enter", "\r", 0, 0, 0), b"\r");
    assert_eq!(engine.key("c", "c", MOD_CTRL, 0, 0), b"\x03");
    assert_eq!(engine.key("up", "", 0, 0, 0), b"\x1b[A");
    engine.feed(b"\x1b[?1h");
    assert_eq!(engine.key("up", "", 0, 0, 0), b"\x1bOA");
    // Left Option as Alt: ESC prefix. Right Option composes text.
    let left = MOD_ALT | MOD_LEFT_ALT;
    let right = MOD_ALT | MOD_RIGHT_ALT;
    assert_eq!(
        engine.key("a", "\u{e5}", left, MOD_ALT, OPTION_AS_ALT_LEFT),
        b"\x1ba"
    );
    assert_eq!(
        engine.key("a", "\u{e5}", right, MOD_ALT, OPTION_AS_ALT_LEFT),
        "\u{e5}".as_bytes()
    );
    // Option+Space composes a no-break space unless Option is Alt.
    assert_eq!(
        engine.key("space", "\u{a0}", right, MOD_ALT, OPTION_AS_ALT_LEFT),
        "\u{a0}".as_bytes()
    );
    assert_eq!(engine.key("space", "\u{a0}", 0, 0, 0), "\u{a0}".as_bytes());
    assert_eq!(engine.key("space", " ", 0, 0, 0), b" ");
    assert_eq!(
        engine.key("space", "\u{a0}", left, MOD_ALT, OPTION_AS_ALT_LEFT),
        b"\x1b "
    );
    // Kitty disambiguation reports Escape as CSI u.
    engine.feed(b"\x1b[>1u");
    assert_eq!(engine.key("escape", "", 0, 0, 0), b"\x1b[27u");
    // Composition sends nothing.
    let event = TermyEngineKeyEvent {
        key_ptr: b"a".as_ptr(),
        key_len: 1,
        text_ptr: ptr::null(),
        text_len: 0,
        unshifted_codepoint: 0,
        mods: 0,
        consumed_mods: 0,
        action: KEY_PRESS,
        // Any nonzero byte from the host means true.
        composing: 2,
    };
    let mut out = TermyEngineBytes::EMPTY;
    // SAFETY: live handle, event and out buffer.
    unsafe { termy_engine_encode_key(engine.0, &raw const event, 0, &raw mut out) };
    assert!(take(out).is_empty());
}

#[test]
fn mouse_encoder_covers_modes_buttons_and_pixels() {
    let mut options = termy_engine_options_default();
    options.pixel_mouse_reports = 1;
    let engine = Handle::with_options(&options);
    // No tracking: nothing.
    assert!(
        engine
            .mouse(MOUSE_PRESS, MOUSE_BUTTON_LEFT, 1, 2, (0, 0))
            .is_empty()
    );
    engine.feed(b"\x1b[?1000h");
    assert_eq!(
        engine.mouse(MOUSE_PRESS, MOUSE_BUTTON_LEFT, 1, 2, (0, 0)),
        b"\x1b[M \"#"
    );
    assert_eq!(
        engine.mouse(MOUSE_RELEASE, MOUSE_BUTTON_LEFT, 1, 2, (0, 0)),
        b"\x1b[M#\"#"
    );
    // ?1000 reports no motion; ?1002 only with a button held.
    assert!(
        engine
            .mouse(MOUSE_MOTION, MOUSE_BUTTON_LEFT, 1, 2, (0, 0))
            .is_empty()
    );
    engine.feed(b"\x1b[?1002h\x1b[?1006h");
    assert_eq!(
        engine.mouse(MOUSE_MOTION, MOUSE_BUTTON_LEFT, 4, 5, (0, 0)),
        b"\x1b[<32;5;6M"
    );
    assert!(
        engine
            .mouse(MOUSE_MOTION, MOUSE_BUTTON_NONE, 4, 5, (0, 0))
            .is_empty()
    );
    engine.feed(b"\x1b[?1003h");
    assert_eq!(
        engine.mouse(MOUSE_MOTION, MOUSE_BUTTON_NONE, 4, 5, (0, 0)),
        b"\x1b[<35;5;6M"
    );
    assert_eq!(
        engine.mouse(MOUSE_RELEASE, MOUSE_BUTTON_RIGHT, 0, 0, (0, 0)),
        b"\x1b[<2;1;1m"
    );
    assert_eq!(
        engine.mouse(MOUSE_PRESS, MOUSE_WHEEL_DOWN, 0, 0, (0, 0)),
        b"\x1b[<65;1;1M"
    );
    assert_eq!(
        engine.mouse(MOUSE_PRESS, MOUSE_BUTTON_8, 0, 0, (0, 0)),
        b"\x1b[<128;1;1M"
    );
    assert!(
        engine
            .mouse(MOUSE_RELEASE, MOUSE_WHEEL_UP, 0, 0, (0, 0))
            .is_empty()
    );
    engine.feed(b"\x1b[?1016h");
    assert_eq!(
        engine.mouse(MOUSE_PRESS, MOUSE_BUTTON_LEFT, 4, 5, (37, 91)),
        b"\x1b[<0;38;92M"
    );
    // X10 reports presses only, without modifiers.
    engine.feed(b"\x1b[?1003l\x1b[?1016l\x1b[?1006l\x1b[?9h");
    assert_eq!(
        engine.mouse(MOUSE_PRESS, MOUSE_BUTTON_MIDDLE, 0, 0, (0, 0)),
        b"\x1b[M!!!"
    );
    assert!(
        engine
            .mouse(MOUSE_RELEASE, MOUSE_BUTTON_MIDDLE, 0, 0, (0, 0))
            .is_empty()
    );
    // Positions the encoding cannot carry are not reported, without overflow.
    for far in [223, u32::MAX] {
        assert!(
            engine
                .mouse(MOUSE_PRESS, MOUSE_BUTTON_LEFT, far, 0, (0, 0))
                .is_empty()
        );
        assert!(
            engine
                .mouse(MOUSE_PRESS, MOUSE_BUTTON_LEFT, 0, far, (0, 0))
                .is_empty()
        );
    }
    engine.feed(b"\x1b[?1005h");
    assert!(
        engine
            .mouse(MOUSE_PRESS, MOUSE_BUTTON_LEFT, u32::MAX, u32::MAX, (0, 0))
            .is_empty()
    );
}

#[test]
fn paste_and_focus_encoders() {
    let paste = |text: &[u8], bracketed| {
        let mut out = TermyEngineBytes::EMPTY;
        // SAFETY: valid input and out buffer.
        unsafe { termy_engine_encode_paste(text.as_ptr(), text.len(), bracketed, &raw mut out) };
        take(out)
    };
    assert_eq!(paste(b"a\r\nb\nc", false), b"a\rb\rc");
    assert_eq!(
        paste(b"x\x1b[201~rm\x03\ty", true),
        b"\x1b[200~x[201~rm\ty\x1b[201~"
    );
    assert_eq!(paste("\u{9b}31m".as_bytes(), false), b"31m");
    // A nested marker must not collapse into a real end marker.
    assert_eq!(
        paste(b"x\x1b[20\x1b[201~1~rm", true),
        b"\x1b[200~x[20[201~1~rm\x1b[201~"
    );
    let mut out = TermyEngineBytes::EMPTY;
    // SAFETY: valid out buffer.
    unsafe { termy_engine_encode_focus(true, &raw mut out) };
    assert_eq!(take(out), b"\x1b[I");
}

#[test]
fn text_formats_and_selection_helpers() {
    let engine = Handle::new(8, 4, TERMY_ENGINE_REPLIES_RETURN);
    engine.feed(b"\x1b[1mbold\x1b[0m <&>\r\nfoo/bar baz\r\n");
    let all = engine.text(None, FORMAT_PLAIN, TEXT_TRIM);
    assert_eq!(all, "bold <&>\nfoo/bar \nbaz\n");
    assert_eq!(
        engine.text(None, FORMAT_PLAIN, TEXT_TRIM | TEXT_UNWRAP),
        "bold <&>\nfoo/bar baz\n"
    );
    let first = Some(TermyEngineTextRange {
        start_line: 0,
        start_col: 0,
        end_line: 0,
        end_col: 7,
        rectangular: 0,
    });
    let vt = engine.text(first, FORMAT_VT, TEXT_TRIM);
    assert!(vt.starts_with("\x1b[0;1mbold\x1b[0m <&>"), "{vt:?}");
    let html = engine.text(first, FORMAT_HTML, TEXT_TRIM);
    assert!(
        html.contains("font-weight:bold\">bold</span> &lt;&amp;&gt;"),
        "{html}"
    );

    let mut range = TermyEngineTextRange::default();
    // SAFETY: live handle and out struct.
    unsafe { termy_engine_select_word(engine.0, 1, 2, &raw mut range) };
    assert_eq!(
        (
            range.start_line,
            range.start_col,
            range.end_line,
            range.end_col
        ),
        (1, 0, 1, 6)
    );
    // The word continues across the soft wrap into the next row.
    // SAFETY: live handle and out struct.
    unsafe { termy_engine_select_word(engine.0, 2, 0, &raw mut range) };
    assert_eq!(
        (
            range.start_line,
            range.start_col,
            range.end_line,
            range.end_col
        ),
        (2, 0, 2, 2)
    );
    // SAFETY: live handle and out struct.
    unsafe { termy_engine_select_line(engine.0, 2, &raw mut range) };
    assert_eq!((range.start_line, range.end_line), (1, 2));
}

#[test]
fn html_links_allow_only_safe_schemes() {
    let engine = Handle::new(40, 2, TERMY_ENGINE_REPLIES_RETURN);
    engine.feed(
        b"\x1b]8;;javascript:alert(1)\x1b\\js\x1b]8;;\x1b\\ \
          \x1b]8;;HTTPS://e.x/?a=\"b\"&c\x1b\\ok\x1b]8;;\x1b\\ \
          \x1b]8;; java\tscript:x\x1b\\ws\x1b]8;;\x1b\\",
    );
    let html = engine.text(None, FORMAT_HTML, TEXT_TRIM);
    assert!(!html.contains("script:"), "{html}");
    assert!(
        html.contains("<a href=\"HTTPS://e.x/?a=&quot;b&quot;&amp;c\">ok</a>"),
        "{html}"
    );
    // Dropped links keep their text and leave no dangling tags.
    assert!(html.contains(">js ") || html.contains("\"js "), "{html}");
    assert_eq!(html.matches("<a ").count(), html.matches("</a>").count());
}

#[test]
fn null_arguments_are_rejected() {
    // SAFETY: deliberately null arguments.
    unsafe {
        assert_eq!(
            termy_engine_new(ptr::null(), ptr::null_mut()),
            TermyEngineStatus::Null
        );
        assert_eq!(
            termy_engine_feed(ptr::null_mut(), ptr::null(), 0, ptr::null_mut()),
            TermyEngineStatus::Null
        );
        termy_engine_free(ptr::null_mut());
        termy_engine_bytes_free(ptr::null_mut());
    }
    let engine = Handle::new(10, 2, TERMY_ENGINE_REPLIES_RETURN);
    // SAFETY: a null pointer with a non-zero length is rejected.
    assert_eq!(
        unsafe { termy_engine_feed(engine.0, ptr::null(), 3, ptr::null_mut()) },
        TermyEngineStatus::Null
    );
    // SAFETY: zero size is an invalid argument.
    assert_eq!(
        unsafe { termy_engine_resize(engine.0, 0, 3) },
        TermyEngineStatus::InvalidArgument
    );
}
