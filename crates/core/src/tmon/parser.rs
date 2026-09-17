mod graphics;

use crate::tmon::grid::{Charset, Grid, Rgb, UnderlineStyle};
use crate::tmon::{
    ClipboardRequest, ClipboardTarget, KittyClipboardPacket, Osc52, Size, decode_base64,
    graphics::{GraphicsCommand, GraphicsState, MAX_COMMAND_BYTES, MAX_CONTROL_BYTES},
};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_CSI_PARAMS: usize = 32;
const MAX_OSC_PARAMS: usize = 16;
const MAX_OSC_BYTES: usize = 64 * 1024;
const MAX_DCS_BYTES: usize = 64 * 1024;
const MAX_SYNC_BYTES: usize = 2 * 1024 * 1024;
const SYNC_TIMEOUT: Duration = Duration::from_millis(150);
const SYNC_ESCAPE_LEN: usize = 8;
const BSU_CSI: &[u8; SYNC_ESCAPE_LEN] = b"\x1b[?2026h";
const ESU_CSI: &[u8; SYNC_ESCAPE_LEN] = b"\x1b[?2026l";
const TITLE_STACK_MAX_DEPTH: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryColors {
    pub ansi: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Option<Rgb>,
}

impl Default for QueryColors {
    fn default() -> Self {
        Self {
            ansi: [
                Rgb {
                    r: 0x00,
                    g: 0x00,
                    b: 0x00,
                },
                Rgb {
                    r: 0xcd,
                    g: 0x00,
                    b: 0x00,
                },
                Rgb {
                    r: 0x00,
                    g: 0xcd,
                    b: 0x00,
                },
                Rgb {
                    r: 0xcd,
                    g: 0xcd,
                    b: 0x00,
                },
                Rgb {
                    r: 0x00,
                    g: 0x00,
                    b: 0xee,
                },
                Rgb {
                    r: 0xcd,
                    g: 0x00,
                    b: 0xcd,
                },
                Rgb {
                    r: 0x00,
                    g: 0xcd,
                    b: 0xcd,
                },
                Rgb {
                    r: 0xe5,
                    g: 0xe5,
                    b: 0xe5,
                },
                Rgb {
                    r: 0x7f,
                    g: 0x7f,
                    b: 0x7f,
                },
                Rgb {
                    r: 0xff,
                    g: 0x00,
                    b: 0x00,
                },
                Rgb {
                    r: 0x00,
                    g: 0xff,
                    b: 0x00,
                },
                Rgb {
                    r: 0xff,
                    g: 0xff,
                    b: 0x00,
                },
                Rgb {
                    r: 0x5c,
                    g: 0x5c,
                    b: 0xff,
                },
                Rgb {
                    r: 0xff,
                    g: 0x00,
                    b: 0xff,
                },
                Rgb {
                    r: 0x00,
                    g: 0xff,
                    b: 0xff,
                },
                Rgb {
                    r: 0xff,
                    g: 0xff,
                    b: 0xff,
                },
            ],
            foreground: Rgb {
                r: 0xe5,
                g: 0xe5,
                b: 0xe5,
            },
            background: Rgb {
                r: 0x1e,
                g: 0x1e,
                b: 0x1e,
            },
            cursor: None,
        }
    }
}

impl QueryColors {
    fn indexed(self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[usize::from(index)],
            16..=231 => {
                let index = index - 16;
                let component = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
                Rgb {
                    r: component((index / 36) % 6),
                    g: component((index / 6) % 6),
                    b: component(index % 6),
                }
            }
            232..=255 => {
                let gray = 8 + (index - 232) * 10;
                Rgb {
                    r: gray,
                    g: gray,
                    b: gray,
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Progress {
    #[default]
    Clear,
    InProgress(u8),
    Error(u8),
    Indeterminate,
    Warning(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParsedEvent {
    Title(String),
    ResetTitle,
    Bell,
    ClipboardStore(String),
    ClipboardLoad(ClipboardRequest),
    KittyClipboard(KittyClipboardPacket),
    KittyClipboardMode(bool),
    KittyClipboardReset,
    ShellPromptStart,
    ShellCommandStart,
    ShellCommandExecuting,
    ShellCommandFinished(Option<i32>),
    Progress(Progress),
    WorkingDirectory(String),
}

#[derive(Debug, Default)]
pub(crate) struct ParseOutput {
    pub(crate) events: Vec<ParsedEvent>,
    pub(crate) replies: Vec<u8>,
    pub(crate) synchronized_update_active: bool,
    pub(crate) synchronized_update_deadline: Option<Instant>,
    pub(crate) synchronized_update_refreshed: bool,
    pub(crate) synchronized_update_committed: bool,
    pub(crate) unsynchronized_activity: bool,
}

impl ParseOutput {
    fn append(&mut self, mut other: Self) {
        self.events.append(&mut other.events);
        self.replies.append(&mut other.replies);
        self.synchronized_update_active = other.synchronized_update_active;
        self.synchronized_update_deadline = other.synchronized_update_deadline;
        self.synchronized_update_refreshed |= other.synchronized_update_refreshed;
        self.synchronized_update_committed |= other.synchronized_update_committed;
        self.unsynchronized_activity |= other.unsynchronized_activity;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    Dcs,
    ApcStart,
    ApcStart8Bit,
    Kitty,
    KittyEscape,
    IgnoreString,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DcsPhase {
    #[default]
    Entry,
    Param,
    Intermediate,
    Ignore,
    Passthrough,
}

#[derive(Debug, Default)]
struct Utf8Decoder {
    bytes: [u8; 4],
    len: usize,
    expected: usize,
}

impl Utf8Decoder {
    fn reset(&mut self) {
        self.len = 0;
        self.expected = 0;
    }

    fn is_pending(&self) -> bool {
        self.expected != 0
    }

    fn start(&mut self, byte: u8) -> bool {
        self.reset();
        self.expected = match byte {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => return false,
        };
        self.bytes[0] = byte;
        self.len = 1;
        true
    }

    fn push_continuation(&mut self, byte: u8) -> Option<char> {
        if !byte.is_ascii() && (byte & 0xc0) == 0x80 && self.len < self.expected {
            self.bytes[self.len] = byte;
            self.len += 1;
            if self.len == self.expected {
                let value = std::str::from_utf8(&self.bytes[..self.len])
                    .ok()
                    .and_then(|text| text.chars().next())
                    .unwrap_or('\u{fffd}');
                self.reset();
                return Some(value);
            }
        }
        None
    }
}

#[derive(Debug)]
pub(crate) struct Parser {
    state: State,
    params: [u16; MAX_CSI_PARAMS],
    separators: [u8; MAX_CSI_PARAMS],
    param_count: usize,
    private_marker: u8,
    intermediate: u8,
    csi_has_params: bool,
    csi_intermediate_count: u8,
    csi_ignored: bool,
    osc: Vec<u8>,
    osc_oversized: bool,
    dcs: Vec<u8>,
    dcs_oversized: bool,
    dcs_phase: DcsPhase,
    kitty: Vec<u8>,
    kitty_oversized: bool,
    kitty_payload_started: bool,
    utf8: Utf8Decoder,
    last_printed: Option<char>,
    active_charset: usize,
    single_shift: Option<usize>,
    designating_charset: Option<u8>,
    escape_intermediate: u8,
    escape_intermediate_count: u8,
    escape_ignored: bool,
    query_colors: QueryColors,
    osc52: Osc52,
    synchronized_update_started: Option<Instant>,
    synchronized_update_buffer: Vec<u8>,
    kitty_clipboard_paste_events: bool,
    title: Option<Arc<str>>,
    title_stack: VecDeque<Option<Arc<str>>>,
    graphics: GraphicsState,
    size: Size,
}

impl Default for Parser {
    fn default() -> Self {
        Self {
            state: State::Ground,
            params: [0; MAX_CSI_PARAMS],
            separators: [0; MAX_CSI_PARAMS],
            param_count: 0,
            private_marker: 0,
            intermediate: 0,
            csi_has_params: false,
            csi_intermediate_count: 0,
            csi_ignored: false,
            osc: Vec::with_capacity(256),
            osc_oversized: false,
            dcs: Vec::with_capacity(128),
            dcs_oversized: false,
            dcs_phase: DcsPhase::Entry,
            kitty: Vec::with_capacity(MAX_CONTROL_BYTES),
            kitty_oversized: false,
            kitty_payload_started: false,
            utf8: Utf8Decoder::default(),
            last_printed: None,
            active_charset: 0,
            single_shift: None,
            designating_charset: None,
            escape_intermediate: 0,
            escape_intermediate_count: 0,
            escape_ignored: false,
            query_colors: QueryColors::default(),
            osc52: Osc52::default(),
            synchronized_update_started: None,
            synchronized_update_buffer: Vec::new(),
            kitty_clipboard_paste_events: false,
            title: None,
            title_stack: VecDeque::with_capacity(8),
            graphics: GraphicsState::default(),
            size: Size::default(),
        }
    }
}

impl Parser {
    pub(crate) fn set_query_colors(&mut self, colors: QueryColors) {
        self.query_colors = colors;
    }

    pub(crate) fn set_size(&mut self, size: Size) {
        self.graphics.resize(size);
        self.size = size;
    }

    pub(crate) fn set_osc52(&mut self, osc52: Osc52) {
        self.osc52 = osc52;
    }

    pub(crate) fn kitty_clipboard_paste_events_mode(&self) -> bool {
        self.kitty_clipboard_paste_events
    }

    pub(crate) fn sync_grid_effects(&mut self, grid: &mut Grid) -> bool {
        let mut changed = false;
        while let Some(effect) = grid.pop_effect() {
            changed |= self.graphics.apply_grid_effect(effect);
        }
        if changed {
            // Overlay revision is enough. Text damage for the same effect is already
            // on the grid; marking full here flickers the whole screen on image updates.
            self.graphics.bump_revision();
        }
        changed
    }

    pub(crate) fn advance(&mut self, grid: &mut Grid, bytes: &[u8]) -> ParseOutput {
        let mut output = ParseOutput::default();
        if self
            .synchronized_update_started
            .is_some_and(|started| started.elapsed() >= SYNC_TIMEOUT)
        {
            output.append(self.stop_synchronized_update(grid));
        }

        let mut offset = 0;
        let mut candidate_sequence_start = None;
        while offset < bytes.len() {
            if self.synchronized_update_started.is_some() {
                if self
                    .synchronized_update_buffer
                    .len()
                    .saturating_add(bytes.len() - offset)
                    >= MAX_SYNC_BYTES - 1
                {
                    output.append(self.stop_synchronized_update(grid));
                    continue;
                }

                let new_bytes = bytes.len() - offset;
                self.synchronized_update_buffer
                    .extend_from_slice(&bytes[offset..]);
                offset = bytes.len();
                if let Some(bsu_offset) = self.scan_synchronized_update(new_bytes, &mut output) {
                    output.append(self.commit_synchronized_update(grid, bsu_offset));
                }
                continue;
            }

            if self.state == State::Ground && !self.utf8.is_pending() {
                if self.single_shift.is_none()
                    && grid.charset(self.active_charset) == Charset::Ascii
                {
                    let (consumed, last_printed) = grid.put_default_text_lines(&bytes[offset..]);
                    if consumed > 0 {
                        if let Some(last_printed) = last_printed {
                            self.last_printed = Some(last_printed);
                        }
                        offset += consumed;
                        continue;
                    }
                }
                let consumed = self.advance_ground_text(grid, &bytes[offset..], &mut output);
                if consumed > 0 {
                    offset += consumed;
                    continue;
                }
            }
            if self.state == State::Ground && bytes[offset] == 0x1b {
                let consumed = self.advance_complete_csi(grid, &bytes[offset..], &mut output);
                if consumed > 0 {
                    offset += consumed;
                    continue;
                }
            }
            if self.state == State::Ground && bytes[offset] == 0x1b {
                candidate_sequence_start = Some(offset);
            }
            let was_synchronized = self.synchronized_update_started.is_some();
            self.advance_byte(grid, bytes[offset], &mut output);
            offset += 1;
            if !was_synchronized && self.synchronized_update_started.is_some() {
                output.unsynchronized_activity |= candidate_sequence_start
                    .is_some_and(|start| start > 0)
                    || !output.events.is_empty()
                    || !output.replies.is_empty();
            }
        }
        self.sync_grid_effects(grid);
        output.synchronized_update_active = self.synchronized_update_started.is_some();
        output.synchronized_update_deadline = self
            .synchronized_update_started
            .map(|started| started + SYNC_TIMEOUT);
        output
    }

    /// Commit a pending synchronized update immediately.
    ///
    /// The timeout and capacity paths deliberately use the same replay path as
    /// an exact ESU. This keeps grid changes, events, and protocol replies on a
    /// single commit boundary.
    pub(crate) fn stop_synchronized_update(&mut self, grid: &mut Grid) -> ParseOutput {
        if self.synchronized_update_started.is_none() {
            return ParseOutput::default();
        }
        self.commit_synchronized_update(grid, None)
    }

    /// Clear read-boundary state after persistence hydration.
    ///
    /// Completed semantic state lives in the grid, graphics store, title stack,
    /// and charset selection and is intentionally retained. Incomplete control
    /// strings, UTF-8, single-shift, and synchronized bytes belong to the replay
    /// parser and must not leak into the subsequent live PTY stream.
    pub(crate) fn reset_transient_state_after_hydration(&mut self) {
        self.state = State::Ground;
        self.params.fill(0);
        self.separators.fill(0);
        self.param_count = 0;
        self.private_marker = 0;
        self.intermediate = 0;
        self.csi_has_params = false;
        self.csi_intermediate_count = 0;
        self.csi_ignored = false;
        self.osc.clear();
        self.osc_oversized = false;
        self.dcs.clear();
        self.dcs_oversized = false;
        self.dcs_phase = DcsPhase::Entry;
        self.kitty.clear();
        self.kitty_oversized = false;
        self.kitty_payload_started = false;
        self.graphics.clear_pending_upload();
        self.utf8.reset();
        self.last_printed = None;
        self.single_shift = None;
        self.designating_charset = None;
        self.reset_escape_intermediates();
        self.synchronized_update_started = None;
        self.synchronized_update_buffer.clear();
    }

    /// Scan only newly appended synchronized bytes for exact raw BSU/ESU.
    /// The overlap matches vte's local scan, so an escape split at any byte
    /// boundary is still recognized without rescanning the full 2 MiB buffer.
    fn scan_synchronized_update(
        &mut self,
        new_bytes: usize,
        output: &mut ParseOutput,
    ) -> Option<Option<usize>> {
        let buffer_len = self.synchronized_update_buffer.len();
        let start_offset = (buffer_len - new_bytes).saturating_sub(SYNC_ESCAPE_LEN - 1);
        let end_offset = buffer_len.saturating_sub(SYNC_ESCAPE_LEN - 1);
        let mut bsu_offset = None;

        for offset in (start_offset..end_offset)
            .rev()
            .filter(|&offset| self.synchronized_update_buffer[offset] == 0x1b)
        {
            let escape = &self.synchronized_update_buffer[offset..offset + SYNC_ESCAPE_LEN];
            if escape == BSU_CSI {
                self.synchronized_update_started = Some(Instant::now());
                output.synchronized_update_refreshed = true;
                bsu_offset = Some(offset);
            } else if escape == ESU_CSI {
                return Some(bsu_offset);
            }
        }
        None
    }

    fn commit_synchronized_update(
        &mut self,
        grid: &mut Grid,
        bsu_offset: Option<usize>,
    ) -> ParseOutput {
        let mut buffer = std::mem::take(&mut self.synchronized_update_buffer);
        let replay_end = bsu_offset.unwrap_or(buffer.len());
        let mut output = ParseOutput {
            synchronized_update_committed: true,
            ..ParseOutput::default()
        };
        self.advance_unbuffered(grid, &buffer[..replay_end], &mut output);

        if let Some(offset) = bsu_offset {
            let retained_len = buffer.len() - offset;
            buffer.copy_within(offset.., 0);
            buffer.truncate(retained_len);
            self.synchronized_update_buffer = buffer;
            self.synchronized_update_started = Some(Instant::now());
            output.synchronized_update_refreshed = true;
        } else {
            buffer.clear();
            self.synchronized_update_buffer = buffer;
            self.synchronized_update_started = None;
        }
        self.sync_grid_effects(grid);
        output.synchronized_update_active = self.synchronized_update_started.is_some();
        output.synchronized_update_deadline = self
            .synchronized_update_started
            .map(|started| started + SYNC_TIMEOUT);
        output
    }

    /// Parse bytes without entering the outer synchronized staging loop. This
    /// is used exclusively to replay a committed batch.
    fn advance_unbuffered(&mut self, grid: &mut Grid, bytes: &[u8], output: &mut ParseOutput) {
        let mut offset = 0;
        while offset < bytes.len() {
            if self.state == State::Ground && !self.utf8.is_pending() {
                let consumed = self.advance_ground_text(grid, &bytes[offset..], output);
                if consumed > 0 {
                    offset += consumed;
                    continue;
                }
            }
            if self.state == State::Ground && bytes[offset] == 0x1b {
                let consumed = self.advance_complete_csi(grid, &bytes[offset..], output);
                if consumed > 0 {
                    offset += consumed;
                    continue;
                }
            }
            self.advance_byte(grid, bytes[offset], output);
            offset += 1;
        }
    }

    fn advance_ground_text(
        &mut self,
        grid: &mut Grid,
        bytes: &[u8],
        output: &mut ParseOutput,
    ) -> usize {
        if self.single_shift.is_none() && grid.charset(self.active_charset) == Charset::Ascii {
            let ascii_len = bytes
                .iter()
                .take_while(|&&byte| (0x20..=0x7e).contains(&byte))
                .count();
            if ascii_len > 0 {
                self.print_ascii(grid, &bytes[..ascii_len]);
                return ascii_len;
            }
        }

        let span_len = bytes
            .iter()
            .position(|byte| *byte < 0x20 || *byte == 0x7f)
            .unwrap_or(bytes.len());
        if span_len == 0 {
            return 0;
        }

        let candidate = &bytes[..span_len];
        let mut consumed = 0;
        while consumed < candidate.len() {
            match std::str::from_utf8(&candidate[consumed..]) {
                Ok(text) => {
                    self.print_text(grid, text);
                    return span_len;
                }
                Err(error) => {
                    let valid_len = error.valid_up_to();
                    if valid_len > 0 {
                        let text = std::str::from_utf8(&candidate[consumed..consumed + valid_len])
                            .expect("the UTF-8 error prefix is always valid");
                        self.print_text(grid, text);
                        consumed += valid_len;
                    }

                    self.advance_byte(grid, candidate[consumed], output);
                    consumed += 1;
                    if self.state != State::Ground {
                        return consumed;
                    }
                    while consumed < candidate.len() && self.utf8.is_pending() {
                        self.advance_byte(grid, candidate[consumed], output);
                        consumed += 1;
                    }
                }
            }
        }
        span_len
    }

    fn print_text(&mut self, grid: &mut Grid, mut text: &str) {
        let ascii_fast_path =
            self.single_shift.is_none() && grid.charset(self.active_charset) == Charset::Ascii;
        while !text.is_empty() {
            if ascii_fast_path {
                let ascii_len = text
                    .as_bytes()
                    .iter()
                    .take_while(|&&byte| (0x20..=0x7e).contains(&byte))
                    .count();
                if ascii_len > 0 {
                    let ascii = &text.as_bytes()[..ascii_len];
                    self.print_ascii(grid, ascii);
                    text = &text[ascii_len..];
                    continue;
                }
            }

            let character = text
                .chars()
                .next()
                .expect("non-empty text always has a first character");
            self.print(grid, character);
            text = &text[character.len_utf8()..];
        }
    }

    fn print_ascii(&mut self, grid: &mut Grid, ascii: &[u8]) {
        let mut consumed_ascii = 0;
        while consumed_ascii < ascii.len() {
            let consumed = grid.put_ascii_run(&ascii[consumed_ascii..]);
            if consumed == 0 {
                self.print(grid, char::from(ascii[consumed_ascii]));
                consumed_ascii += 1;
            } else {
                consumed_ascii += consumed;
                self.last_printed = Some(char::from(ascii[consumed_ascii - 1]));
            }
        }
    }

    fn advance_byte(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        if matches!(byte, 0x18 | 0x1a) && self.state != State::Ground {
            if self.state == State::Osc {
                self.finish_osc(grid, output, b"\x1b\\");
            }
            self.state = State::Ground;
            self.utf8.reset();
            self.osc.clear();
            self.dcs.clear();
            self.kitty.clear();
            self.osc_oversized = false;
            self.dcs_oversized = false;
            self.dcs_phase = DcsPhase::Entry;
            self.kitty_oversized = false;
            self.kitty_payload_started = false;
            return;
        }
        match self.state {
            State::Ground => self.advance_ground(grid, byte, output),
            State::Escape => self.advance_escape(grid, byte, output),
            State::EscapeIntermediate => self.advance_escape_intermediate(grid, byte, output),
            State::Csi => self.advance_csi(grid, byte, output),
            State::Osc => self.advance_osc(grid, byte, output),
            State::Dcs => self.advance_dcs(grid, byte, output),
            State::ApcStart => {
                if byte == b'G' {
                    self.kitty.clear();
                    self.kitty_oversized = false;
                    self.kitty_payload_started = false;
                    self.state = State::Kitty;
                } else if byte == 0x1b {
                    self.state = State::Escape;
                } else {
                    self.state = State::IgnoreString;
                }
            }
            State::ApcStart8Bit => {
                if byte == b'G' {
                    self.kitty.clear();
                    self.kitty_oversized = false;
                    self.kitty_payload_started = false;
                    self.state = State::Kitty;
                } else {
                    // Termy's native Kitty interceptor forwards non-Kitty
                    // 8-bit APC input to vte. The raw C1 introducer is ignored
                    // in UTF-8 mode and this byte is parsed as ordinary input.
                    self.state = State::Ground;
                    if byte != 0x9f {
                        self.advance_ground(grid, byte, output);
                    }
                }
            }
            State::Kitty => match byte {
                0x1b => {
                    self.state = State::KittyEscape;
                }
                0x9c => {
                    self.finish_kitty(grid, output);
                    self.state = State::Ground;
                }
                _ => self.push_kitty(byte),
            },
            State::KittyEscape => {
                if byte == b'\\' {
                    self.finish_kitty(grid, output);
                    self.state = State::Ground;
                } else {
                    self.push_kitty(0x1b);
                    self.push_kitty(byte);
                    self.state = State::Kitty;
                }
            }
            State::IgnoreString => {
                if byte == 0x1b {
                    self.state = State::Escape;
                }
            }
        }
    }

    fn advance_ground(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        if self.utf8.is_pending() {
            if (byte & 0xc0) == 0x80 {
                if let Some(character) = self.utf8.push_continuation(byte) {
                    self.print(grid, character);
                }
                return;
            }
            self.utf8.reset();
            self.print(grid, '\u{fffd}');
            self.advance_ground(grid, byte, output);
            return;
        }

        match byte {
            0x00 | 0x18 | 0x7f => {}
            0x07 => output.events.push(ParsedEvent::Bell),
            0x08 => grid.backspace(),
            0x09 => grid.tab(),
            0x0a..=0x0c => grid.line_feed(),
            0x0d => grid.carriage_return(),
            0x0e => self.active_charset = 1,
            0x0f => self.active_charset = 0,
            0x1a => {}
            0x1b => self.state = State::Escape,
            0x20..=0x7e => self.print(grid, char::from(byte)),
            // Termy's native Kitty interceptor accepts the 8-bit APC introducer
            // even though other standalone C1 bytes are ignored in UTF-8 mode.
            0x9f => self.state = State::ApcStart8Bit,
            // The PTY stream is UTF-8. Match Alacritty by ignoring standalone
            // C1 bytes instead of interpreting their legacy 8-bit forms.
            0x80..=0x9f => {}
            _ => {
                if !self.utf8.start(byte) {
                    self.print(grid, '\u{fffd}');
                }
            }
        }
    }

    fn print(&mut self, grid: &mut Grid, character: char) {
        // VTE dispatches valid UTF-8 encodings of C1 code points as controls.
        // Alacritty ignores those controls in UTF-8 mode, so they must not be
        // retained as zero-width combining text or become REP's last glyph.
        if ('\u{80}'..='\u{9f}').contains(&character) {
            return;
        }
        self.last_printed = Some(character);
        let charset_index = self.single_shift.take().unwrap_or(self.active_charset);
        let charset = grid.charset(charset_index);
        let character = match charset {
            Charset::DecSpecial => dec_special_character(character),
            Charset::Uk if character == '#' => '£',
            Charset::Ascii | Charset::Uk => character,
        };
        grid.put_char(character);
    }

    fn advance_escape(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        self.utf8.reset();
        if self.execute_control(grid, byte, output) {
            return;
        }
        match byte {
            b'[' => self.start_csi(),
            b']' => self.start_osc(),
            b'P' => self.start_dcs(),
            b'_' => self.state = State::ApcStart,
            b'X' | b'^' => self.state = State::IgnoreString,
            b'7' => {
                grid.save_cursor();
                self.state = State::Ground;
            }
            b'8' => {
                grid.restore_cursor();
                self.single_shift = None;
                self.state = State::Ground;
            }
            b'D' => {
                grid.line_feed();
                self.state = State::Ground;
            }
            b'E' => {
                grid.next_line();
                self.state = State::Ground;
            }
            b'M' => {
                grid.reverse_index();
                self.state = State::Ground;
            }
            b'c' => {
                grid.reset();
                self.kitty_clipboard_paste_events = false;
                self.active_charset = 0;
                self.single_shift = None;
                self.title = None;
                self.title_stack.clear();
                output.events.push(ParsedEvent::ResetTitle);
                output.events.push(ParsedEvent::KittyClipboardReset);
                self.state = State::Ground;
            }
            b'H' => {
                grid.set_tab_stop();
                self.state = State::Ground;
            }
            b'Z' => {
                // DECID is the legacy primary-device-attributes request. It
                // does not inherit a private marker from an earlier CSI.
                output.replies.extend_from_slice(b"\x1b[?6c");
                self.state = State::Ground;
            }
            b'=' => {
                grid.set_keypad_application_mode(true);
                self.state = State::Ground;
            }
            b'>' => {
                grid.set_keypad_application_mode(false);
                self.state = State::Ground;
            }
            b'\\' => self.state = State::Ground,
            b'N' | b'O' => {
                self.single_shift = Some(if byte == b'N' { 2 } else { 3 });
                self.state = State::Ground;
            }
            b'n' | b'o' => {
                self.active_charset = if byte == b'n' { 2 } else { 3 };
                self.state = State::Ground;
            }
            0x20..=0x2f => {
                self.designating_charset = Some(match byte {
                    b'(' => 0,
                    b')' | b'-' => 1,
                    b'*' | b'.' => 2,
                    b'+' | b'/' => 3,
                    _ => {
                        self.escape_intermediate = byte;
                        self.escape_intermediate_count = 1;
                        self.escape_ignored = false;
                        self.state = State::EscapeIntermediate;
                        return;
                    }
                });
                self.escape_intermediate = byte;
                self.escape_intermediate_count = 1;
                self.escape_ignored = false;
                self.state = State::EscapeIntermediate;
            }
            0x1b => {
                self.reset_escape_intermediates();
                self.state = State::Escape;
            }
            0x30..=0x7e => self.state = State::Ground,
            _ => {}
        }
    }

    fn advance_escape_intermediate(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        if self.execute_control(grid, byte, output) {
            return;
        }
        match byte {
            0x20..=0x2f => {
                self.escape_intermediate_count = self.escape_intermediate_count.saturating_add(1);
                self.escape_ignored = true;
                self.designating_charset = None;
            }
            0x30..=0x7e => {
                if !self.escape_ignored {
                    if let Some(slot) = self.designating_charset.take() {
                        let charset = match byte {
                            b'0' => Some(Charset::DecSpecial),
                            b'A' => Some(Charset::Uk),
                            b'B' => Some(Charset::Ascii),
                            _ => None,
                        };
                        if let Some(charset) = charset {
                            grid.set_charset(usize::from(slot), charset);
                        }
                    } else if self.escape_intermediate == b'#' && byte == b'8' {
                        grid.alignment_test();
                    }
                }
                self.reset_escape_intermediates();
                self.state = State::Ground;
            }
            0x1b => {
                self.reset_escape_intermediates();
                self.state = State::Escape;
            }
            _ => {}
        }
    }

    fn reset_escape_intermediates(&mut self) {
        self.designating_charset = None;
        self.escape_intermediate = 0;
        self.escape_intermediate_count = 0;
        self.escape_ignored = false;
    }

    fn execute_control(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) -> bool {
        match byte {
            0x00..=0x06 | 0x10..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f => {}
            0x07 => output.events.push(ParsedEvent::Bell),
            0x08 => grid.backspace(),
            0x09 => grid.tab(),
            0x0a..=0x0c => grid.line_feed(),
            0x0d => grid.carriage_return(),
            0x0e => self.active_charset = 1,
            0x0f => self.active_charset = 0,
            _ => return false,
        }
        true
    }

    fn start_csi(&mut self) {
        self.state = State::Csi;
        self.params.fill(0);
        self.separators.fill(0);
        self.param_count = 1;
        self.private_marker = 0;
        self.intermediate = 0;
        self.csi_has_params = false;
        self.csi_intermediate_count = 0;
        self.csi_ignored = false;
    }

    fn advance_complete_csi(
        &mut self,
        grid: &mut Grid,
        bytes: &[u8],
        output: &mut ParseOutput,
    ) -> usize {
        if bytes.get(0..2) != Some(b"\x1b[") {
            return 0;
        }
        let Some(relative_final) = bytes[2..]
            .iter()
            .position(|byte| (0x40..=0x7e).contains(byte))
        else {
            return 0;
        };
        let final_index = relative_final + 2;
        let final_byte = bytes[final_index];
        if matches!(final_byte, b'h' | b'l') {
            return 0;
        }
        let body = &bytes[2..final_index];
        if body.iter().any(|byte| !matches!(byte, 0x20..=0x3f)) {
            return 0;
        }

        self.start_csi();
        for &byte in body {
            match byte {
                b'0'..=b'9' => {
                    if self.csi_intermediate_count != 0 {
                        self.csi_ignored = true;
                        continue;
                    }
                    self.csi_has_params = true;
                    let index = self.param_count.saturating_sub(1);
                    if index < MAX_CSI_PARAMS {
                        self.params[index] = self.params[index]
                            .saturating_mul(10)
                            .saturating_add(u16::from(byte - b'0'));
                    }
                }
                b';' | b':' => {
                    if self.csi_intermediate_count != 0 {
                        self.csi_ignored = true;
                        continue;
                    }
                    self.csi_has_params = true;
                    if self.param_count < MAX_CSI_PARAMS {
                        self.separators[self.param_count] = byte;
                        self.param_count += 1;
                    } else {
                        self.csi_ignored = true;
                    }
                }
                b'?' | b'>' | b'<' | b'='
                    if !self.csi_has_params
                        && self.csi_intermediate_count == 0
                        && self.private_marker == 0 =>
                {
                    self.private_marker = byte;
                    self.csi_has_params = true;
                }
                b'?' | b'>' | b'<' | b'=' => self.csi_ignored = true,
                0x20..=0x2f => {
                    self.csi_intermediate_count = self.csi_intermediate_count.saturating_add(1);
                    if self.csi_intermediate_count == 1 {
                        self.intermediate = byte;
                    } else {
                        self.csi_ignored = true;
                    }
                }
                _ => unreachable!("complete CSI preflight accepts only parameter bytes"),
            }
        }
        if !self.csi_ignored {
            self.dispatch_csi(grid, final_byte, output);
        }
        self.state = State::Ground;
        final_index + 1
    }

    fn advance_csi(&mut self, grid: &mut Grid, byte: u8, output: &mut ParseOutput) {
        if self.csi_ignored {
            match byte {
                0x07 => output.events.push(ParsedEvent::Bell),
                0x08 => grid.backspace(),
                0x09 => grid.tab(),
                0x0a..=0x0c => grid.line_feed(),
                0x0d => grid.carriage_return(),
                0x0e => self.active_charset = 1,
                0x0f => self.active_charset = 0,
                0x1b => self.state = State::Escape,
                0x40..=0x7e => self.state = State::Ground,
                _ => {}
            }
            return;
        }

        match byte {
            0x00 | 0x7f => {}
            0x07 => output.events.push(ParsedEvent::Bell),
            0x08 => grid.backspace(),
            0x09 => grid.tab(),
            0x0a..=0x0c => grid.line_feed(),
            0x0d => grid.carriage_return(),
            0x0e => self.active_charset = 1,
            0x0f => self.active_charset = 0,
            b'0'..=b'9' => {
                if self.csi_intermediate_count != 0 {
                    self.csi_ignored = true;
                    return;
                }
                self.csi_has_params = true;
                let index = self.param_count.saturating_sub(1);
                if index < MAX_CSI_PARAMS {
                    self.params[index] = self.params[index]
                        .saturating_mul(10)
                        .saturating_add(u16::from(byte - b'0'));
                }
            }
            b';' | b':' => {
                if self.csi_intermediate_count != 0 {
                    self.csi_ignored = true;
                    return;
                }
                self.csi_has_params = true;
                if self.param_count < MAX_CSI_PARAMS {
                    self.separators[self.param_count] = byte;
                    self.param_count += 1;
                } else {
                    self.csi_ignored = true;
                }
            }
            b'?' | b'>' | b'<' | b'='
                if !self.csi_has_params
                    && self.csi_intermediate_count == 0
                    && self.private_marker == 0 =>
            {
                self.private_marker = byte;
                self.csi_has_params = true;
            }
            b'?' | b'>' | b'<' | b'=' => {
                self.csi_ignored = true;
            }
            0x20..=0x2f => {
                self.csi_intermediate_count = self.csi_intermediate_count.saturating_add(1);
                if self.csi_intermediate_count == 1 {
                    self.intermediate = byte;
                } else {
                    self.csi_ignored = true;
                }
            }
            0x40..=0x7e => {
                self.dispatch_csi(grid, byte, output);
                self.state = State::Ground;
            }
            0x1b => self.state = State::Escape,
            _ => {}
        }
    }

    fn dispatch_csi(&mut self, grid: &mut Grid, final_byte: u8, output: &mut ParseOutput) {
        if self.intermediate != 0
            && !matches!(
                (final_byte, self.intermediate),
                (b'q', b' ' | b'"') | (b'p', b'$')
            )
        {
            return;
        }
        if self.private_marker != 0
            && !matches!(
                (final_byte, self.private_marker, self.intermediate),
                (b'h' | b'l' | b'J' | b'K' | b'W', b'?', _)
                    | (b'u', b'=' | b'>' | b'<' | b'?', _)
                    | (b'p', b'?', b'$')
                    | (b'c', b'>', _)
            )
        {
            return;
        }
        let first = self.param_or(0, 1) as usize;
        match final_byte {
            b'A' => grid.cursor_up(first),
            b'B' | b'e' => grid.cursor_down(first),
            b'C' | b'a' => grid.cursor_forward(first),
            b'D' => grid.cursor_backward(first),
            b'E' => grid.cursor_next_line(first),
            b'F' => grid.cursor_previous_line(first),
            b'G' | b'`' => grid.set_cursor_col(first.saturating_sub(1)),
            b'd' => grid.set_cursor_row(first.saturating_sub(1)),
            b'H' | b'f' => {
                let row = self.param_or(0, 1) as usize;
                let col = self.param_or(1, 1) as usize;
                grid.set_cursor_position(row.saturating_sub(1), col.saturating_sub(1));
            }
            b'J' => grid.erase_display(self.param(0), self.private_marker == b'?'),
            b'K' => grid.erase_line(self.param(0), self.private_marker == b'?'),
            b'I' => grid.move_forward_tabs(first),
            b'Z' => grid.move_backward_tabs(first),
            b'W' if self.private_marker == b'?' && self.param(0) == 5 => {
                grid.reset_tab_stops();
            }
            b'g' => grid.clear_tab_stop(self.param(0)),
            b'X' => grid.erase_chars(first),
            b'@' => grid.insert_blank_chars(first),
            b'P' => grid.delete_chars(first),
            b'L' => grid.insert_lines(first),
            b'M' => grid.delete_lines(first),
            b'S' => grid.scroll_up(first),
            b'T' => grid.scroll_down(first),
            b'b' => {
                if let Some(character) = self.last_printed {
                    for _ in 0..first {
                        self.print(grid, character);
                    }
                }
            }
            b'm' => self.dispatch_sgr(grid),
            b'h' | b'l' => {
                let enabled = final_byte == b'h';
                let private = self.private_marker == b'?';
                let mut index = 0;
                while index < self.param_count {
                    let mode = self.params[index];
                    if private && mode == 2026 {
                        if enabled {
                            self.synchronized_update_started = Some(Instant::now());
                            self.synchronized_update_buffer.clear();
                            output.synchronized_update_refreshed = true;
                            output.unsynchronized_activity |= self.top_level_param_count() > 1;
                        } else {
                            self.synchronized_update_started = None;
                            self.synchronized_update_buffer.clear();
                        }
                    } else if private && mode == 5522 {
                        self.kitty_clipboard_paste_events = enabled;
                        output.events.push(ParsedEvent::KittyClipboardMode(enabled));
                    } else {
                        grid.set_mode(private, mode, enabled);
                    }

                    index += 1;
                    while index < self.param_count && self.separators[index] == b':' {
                        index += 1;
                    }
                }
            }
            b'r' => {
                let rows = grid.rows();
                let top = self.param_or(0, 1) as usize;
                let bottom = self.param_or(1, rows as u16) as usize;
                if top == 1 && bottom == rows {
                    grid.reset_scroll_region();
                } else {
                    grid.set_scroll_region(top.saturating_sub(1), bottom.saturating_sub(1));
                }
            }
            b's' => {
                grid.save_cursor();
            }
            b'u' if self.private_marker == b'=' => {
                let flags = self.param(0);
                let mode = self.param_or(1, 1);
                grid.set_kitty_keyboard_flags(flags, mode);
            }
            b'u' if self.private_marker == b'>' => {
                grid.push_kitty_keyboard_flags(self.param(0));
            }
            b'u' if self.private_marker == b'<' => {
                grid.pop_kitty_keyboard_flags(self.param_or(0, 1) as usize);
            }
            b'u' if self.private_marker == b'?' => {
                output.replies.extend_from_slice(
                    format!("\x1b[?{}u", grid.kitty_keyboard_report_flags()).as_bytes(),
                );
            }
            b'u' => {
                grid.restore_cursor();
                self.single_shift = None;
            }
            b'q' if self.intermediate == b' ' => grid.set_cursor_style(self.param(0)),
            b'q' if self.intermediate == b'"' => {
                grid.set_character_protection(self.param(0));
            }
            b'p' if self.intermediate == b'$' => {
                let mode = self.param(0);
                let private = self.private_marker == b'?';
                let marker = if private { "?" } else { "" };
                let state = if private && mode == 2026 {
                    if self.synchronized_update_started.is_some() {
                        1
                    } else {
                        2
                    }
                } else if private && mode == 5522 {
                    if self.kitty_clipboard_paste_events {
                        1
                    } else {
                        2
                    }
                } else {
                    grid.mode_state(private, mode)
                };
                output
                    .replies
                    .extend_from_slice(format!("\x1b[{marker}{mode};{state}$y").as_bytes());
            }
            b't' => match self.param_or(0, 1) {
                14 => {
                    let (height, width) = grid.text_area_size_pixels();
                    output
                        .replies
                        .extend_from_slice(format!("\x1b[4;{height};{width}t").as_bytes());
                }
                18 => output.replies.extend_from_slice(
                    format!("\x1b[8;{};{}t", grid.rows(), grid.cols()).as_bytes(),
                ),
                22 => {
                    if self.title_stack.len() >= TITLE_STACK_MAX_DEPTH {
                        self.title_stack.pop_front();
                    }
                    self.title_stack.push_back(self.title.clone());
                }
                23 => {
                    if let Some(title) = self.title_stack.pop_back() {
                        self.title = title.clone();
                        output.events.push(match title {
                            Some(title) => ParsedEvent::Title(title.to_string()),
                            None => ParsedEvent::ResetTitle,
                        });
                    }
                }
                _ => {}
            },
            b'n' => self.device_status_report(grid, output),
            b'c' if self.param(0) == 0 => self.device_attributes(output),
            _ => {}
        }
    }

    fn device_status_report(&self, grid: &Grid, output: &mut ParseOutput) {
        if self.private_marker != 0 {
            return;
        }
        match self.param(0) {
            5 => output.replies.extend_from_slice(b"\x1b[0n"),
            6 => {
                let (col, row) = grid.cursor_position();
                output
                    .replies
                    .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            _ => {}
        }
    }

    fn dispatch_sgr(&self, grid: &mut Grid) {
        let mut normalized = [0u16; MAX_CSI_PARAMS];
        let mut underline_styles = [None; MAX_CSI_PARAMS];
        let mut output_len = 0usize;
        let mut index = 0usize;
        while index < self.param_count && output_len < MAX_CSI_PARAMS {
            let code = self.params[index];
            let colon_group = index + 1 < self.param_count && self.separators[index + 1] == b':';
            if !colon_group {
                normalized[output_len] = code;
                output_len += 1;
                index += 1;
                continue;
            }

            let mut group_end = index + 1;
            while group_end < self.param_count && self.separators[group_end] == b':' {
                group_end += 1;
            }
            match code {
                4 => {
                    let style = match &self.params[index..group_end] {
                        [4, 0] => UnderlineStyle::None,
                        [4, 2] => UnderlineStyle::Double,
                        [4, 3] => UnderlineStyle::Curly,
                        [4, 4] => UnderlineStyle::Dotted,
                        [4, 5] => UnderlineStyle::Dashed,
                        [4, ..] => UnderlineStyle::Single,
                        _ => unreachable!("colon underline groups always start with SGR 4"),
                    };
                    normalized[output_len] = if style == UnderlineStyle::None { 24 } else { 4 };
                    underline_styles[output_len] = Some(style);
                    output_len += 1;
                }
                38 | 48 | 58 if index + 1 < group_end => {
                    let mode = self.params[index + 1];
                    match mode {
                        2 => {
                            let values = &self.params[index + 2..group_end];
                            let start = usize::from(values.len() >= 4);
                            if values.len().saturating_sub(start) >= 3
                                && output_len + 5 <= MAX_CSI_PARAMS
                            {
                                normalized[output_len..output_len + 5].copy_from_slice(&[
                                    code,
                                    2,
                                    values[start],
                                    values[start + 1],
                                    values[start + 2],
                                ]);
                                output_len += 5;
                            }
                        }
                        5 if index + 2 < group_end && output_len + 3 <= MAX_CSI_PARAMS => {
                            normalized[output_len..output_len + 3].copy_from_slice(&[
                                code,
                                5,
                                self.params[index + 2],
                            ]);
                            output_len += 3;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            index = group_end;
        }
        grid.sgr_with_underline_styles(&normalized[..output_len], &underline_styles[..output_len]);
    }

    fn device_attributes(&self, output: &mut ParseOutput) {
        match self.private_marker {
            b'>' => output.replies.extend_from_slice(b"\x1b[>0;2600;1c"),
            _ => output.replies.extend_from_slice(b"\x1b[?6c"),
        }
    }

    fn param(&self, index: usize) -> u16 {
        let mut group = 0;
        for raw_index in 0..self.param_count {
            if raw_index > 0 && self.separators[raw_index] != b':' {
                group += 1;
            }
            if group == index {
                return self.params[raw_index];
            }
        }
        0
    }

    fn top_level_param_count(&self) -> usize {
        self.separators[..self.param_count]
            .iter()
            .filter(|&&separator| separator != b':')
            .count()
    }

    fn param_or(&self, index: usize, default: u16) -> u16 {
        match self.param(index) {
            0 => default,
            value => value,
        }
    }
}

mod control_strings;
mod helpers;

use helpers::*;

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
