use std::{collections::VecDeque, sync::Arc};

use super::{
    CellExtra, Color, CursorShape, Event, Hyperlink, MAX_REPLY_BYTES, Modes, MouseEncoding,
    MouseTracking, Options, Size, Style, UnderlineStyle, enqueue,
    grid::Grid,
    parser::{Handler, Param},
};

const MAX_TITLE_STACK: usize = 64;
const MAX_KEYBOARD_STACK: usize = 32;

pub(super) struct State {
    pub(super) graphics: super::graphics::Graphics,
    pub(super) grid: Grid,
    pub(super) modes: Modes,
    pub(super) alternate_screen: bool,
    pub(super) events: VecDeque<Event>,
    pub(super) replies: Vec<u8>,
    pub(super) dropped_events: u64,
    pub(super) dropped_reply_bytes: u64,
    pub(super) palette: [Option<Color>; 256],
    pub(super) foreground: Option<Color>,
    pub(super) background: Option<Color>,
    pub(super) cursor_color: Option<Color>,
    pub(super) palette_revision: u64,
    pub(super) query_colors: crate::TerminalQueryColors,
    pub(super) default_cursor_shape: CursorShape,
    pub(super) cursor_shape_overridden: bool,
    pub(super) cell_pixels: (u16, u16),
    pub(super) program_status: crate::program_status::ProgramStatus,
    title: String,
    title_stack: Vec<String>,
    keyboard_stack: [Vec<u8>; 2],
    keyboard_flags: [u8; 2],
    charsets: [bool; 2],
    active_charset: usize,
    saved_charsets: [([bool; 2], usize); 2],
    saved_private_modes: Vec<(u16, bool)>,
    last_printed: Option<char>,
}

impl State {
    pub(super) fn new(size: Size, options: Options) -> Self {
        Self {
            graphics: super::graphics::Graphics::default(),
            grid: Grid::new(size, options.scrollback_history),
            modes: Modes::default(),
            alternate_screen: false,
            events: VecDeque::new(),
            replies: Vec::new(),
            dropped_events: 0,
            dropped_reply_bytes: 0,
            palette: [None; 256],
            foreground: None,
            background: None,
            cursor_color: None,
            palette_revision: 0,
            query_colors: crate::TerminalQueryColors::default(),
            default_cursor_shape: CursorShape::Block,
            cursor_shape_overridden: false,
            cell_pixels: (9, 18),
            program_status: crate::program_status::ProgramStatus::default(),
            title: String::new(),
            title_stack: Vec::new(),
            keyboard_stack: [Vec::new(), Vec::new()],
            keyboard_flags: [0, 0],
            charsets: [false; 2],
            active_charset: 0,
            saved_charsets: [([false; 2], 0); 2],
            saved_private_modes: Vec::new(),
            last_printed: None,
        }
    }

    fn event(&mut self, event: Event) {
        enqueue(
            &mut self.events,
            &mut self.dropped_events,
            self.modes.clipboard_paste_events,
            event,
        );
    }

    pub(super) fn reply(&mut self, bytes: &[u8]) {
        if self.replies.len().saturating_add(bytes.len()) <= MAX_REPLY_BYTES {
            self.replies.extend_from_slice(bytes);
        } else {
            self.dropped_reply_bytes = self.dropped_reply_bytes.saturating_add(bytes.len() as u64);
        }
    }

    fn save_cursor(&mut self) {
        self.grid.save_cursor();
        self.saved_charsets[usize::from(self.alternate_screen)] =
            (self.charsets, self.active_charset);
    }

    fn restore_cursor(&mut self) {
        self.grid.restore_cursor();
        (self.charsets, self.active_charset) =
            self.saved_charsets[usize::from(self.alternate_screen)];
    }

    fn reset_sgr(&mut self) {
        // DECSCA protection is not a graphic rendition attribute.
        let protected = self.grid.pen.style.attributes & Style::PROTECTED;
        self.grid.pen.style = Style {
            attributes: protected,
            ..Style::default()
        };
    }

    fn soft_reset(&mut self) {
        self.grid.soft_reset(self.default_cursor_shape);
        self.cursor_shape_overridden = false;
        self.modes.application_cursor = false;
        self.modes.application_keypad = false;
        self.charsets = [false; 2];
        self.active_charset = 0;
        self.saved_charsets[usize::from(self.alternate_screen)] = ([false; 2], 0);
        self.last_printed = None;
    }

    fn save_private_mode(&mut self, mode: u16) {
        // Only recognized, stateful modes enter the one-level cache. Its size
        // is bounded by the modes supported by mode_state, not the input.
        let Some(enabled) = self.mode_state(true, mode) else {
            return;
        };
        if let Some((_, saved)) = self
            .saved_private_modes
            .iter_mut()
            .find(|(saved_mode, _)| *saved_mode == mode)
        {
            *saved = enabled;
        } else {
            self.saved_private_modes.push((mode, enabled));
        }
    }

    pub(super) fn saved_private_mode(&self, mode: u16) -> Option<bool> {
        self.saved_private_modes
            .iter()
            .find_map(|&(saved_mode, enabled)| (saved_mode == mode).then_some(enabled))
    }

    fn mouse_tracking(&mut self, mode: MouseTracking, enabled: bool) {
        if enabled {
            self.modes.mouse_tracking = mode;
        } else if self.modes.mouse_tracking == mode {
            self.modes.mouse_tracking = MouseTracking::None;
        }
    }

    fn mouse_encoding(&mut self, mode: MouseEncoding, enabled: bool) {
        if enabled {
            self.modes.mouse_encoding = mode;
        } else if self.modes.mouse_encoding == mode {
            self.modes.mouse_encoding = MouseEncoding::Default;
        }
    }

    fn mode(&mut self, private: bool, value: u16, enabled: bool) {
        if !private {
            match value {
                4 => self.grid.insert_mode = enabled,
                20 => self.grid.newline_mode = enabled,
                _ => {}
            }
            return;
        }
        let old_cursor = self.grid.cursor;
        match value {
            1 => self.modes.application_cursor = enabled,
            3 => {
                self.grid.set_scroll_region(0, self.grid.size().rows);
                self.grid.erase_display(2, false);
                self.grid.goto(0, 0);
            }
            6 => {
                self.grid.origin_mode = enabled;
                self.grid.goto(0, 0);
            }
            7 => self.grid.autowrap = enabled,
            9 => self.mouse_tracking(MouseTracking::Press, enabled),
            12 => self.grid.cursor.blinking = enabled,
            25 => self.grid.cursor.visible = enabled,
            47 | 1047 | 1049 => {
                if enabled != self.alternate_screen {
                    if enabled && value == 1049 {
                        self.save_cursor();
                    }
                    // 1047 clears the alternate buffer on exit; 1049 clears
                    // on entry and restores the saved primary cursor on exit.
                    if !enabled && value == 1047 {
                        self.grid.erase_display(2, false);
                    }
                    self.grid
                        .set_alternate(enabled, value == 1049, value == 1049);
                    self.alternate_screen = enabled;
                    if !enabled && value == 1049 {
                        self.restore_cursor();
                    }
                    self.modes.kitty_keyboard = self.keyboard_flags[usize::from(enabled)];
                }
            }
            1048 => {
                if enabled {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            1000 => self.mouse_tracking(MouseTracking::Click, enabled),
            1002 => self.mouse_tracking(MouseTracking::Drag, enabled),
            1003 => self.mouse_tracking(MouseTracking::Motion, enabled),
            1004 => self.modes.focus_events = enabled,
            1005 => self.mouse_encoding(MouseEncoding::Utf8, enabled),
            1006 => self.mouse_encoding(MouseEncoding::Sgr, enabled),
            1015 => self.mouse_encoding(MouseEncoding::Urxvt, enabled),
            1016 => self.mouse_encoding(MouseEncoding::SgrPixels, enabled),
            2004 => self.modes.bracketed_paste = enabled,
            2026 => self.modes.synchronized_update = enabled,
            5522 => {
                self.modes.clipboard_paste_events = enabled;
                self.event(Event::KittyClipboardControl(
                    crate::KittyClipboardControl::Set(enabled),
                ));
            }
            _ => {}
        }
        if self.grid.cursor != old_cursor {
            self.grid.cursor_changed(old_cursor);
        }
    }

    fn sgr(&mut self, params: &[Param]) {
        if params.is_empty() {
            self.reset_sgr();
            return;
        }
        let mut index = 0;
        while let Some(param) = params.get(index) {
            let value = param.value().unwrap_or(0);
            if !param.subparams().is_empty() && !matches!(value, 4 | 38 | 48 | 58) {
                index += 1;
                continue;
            }
            match value {
                0 => self.reset_sgr(),
                1 => self.grid.pen.style.set(Style::BOLD, true),
                2 => self.grid.pen.style.set(Style::DIM, true),
                3 => self.grid.pen.style.set(Style::ITALIC, true),
                4 if param.subparams().len() <= 1 => {
                    if let Some(underline) =
                        match param.subparams().first().copied().flatten().unwrap_or(1) {
                            0 => Some(UnderlineStyle::None),
                            1 => Some(UnderlineStyle::Single),
                            2 => Some(UnderlineStyle::Double),
                            3 => Some(UnderlineStyle::Curly),
                            4 => Some(UnderlineStyle::Dotted),
                            5 => Some(UnderlineStyle::Dashed),
                            _ => None,
                        }
                    {
                        self.grid.pen.style.underline = underline;
                    }
                }
                5 | 6 => self.grid.pen.style.set(Style::BLINK, true),
                7 => self.grid.pen.style.set(Style::INVERSE, true),
                8 => self.grid.pen.style.set(Style::HIDDEN, true),
                9 => self.grid.pen.style.set(Style::STRIKE, true),
                21 => self.grid.pen.style.underline = UnderlineStyle::Double,
                22 => self.grid.pen.style.set(Style::BOLD | Style::DIM, false),
                23 => self.grid.pen.style.set(Style::ITALIC, false),
                24 => self.grid.pen.style.underline = UnderlineStyle::None,
                25 => self.grid.pen.style.set(Style::BLINK, false),
                27 => self.grid.pen.style.set(Style::INVERSE, false),
                28 => self.grid.pen.style.set(Style::HIDDEN, false),
                29 => self.grid.pen.style.set(Style::STRIKE, false),
                30..=37 => self.grid.pen.style.foreground = Color::indexed((value - 30) as u8),
                40..=47 => self.grid.pen.style.background = Color::indexed((value - 40) as u8),
                90..=97 => self.grid.pen.style.foreground = Color::indexed((value - 90 + 8) as u8),
                100..=107 => {
                    self.grid.pen.style.background = Color::indexed((value - 100 + 8) as u8);
                }
                39 => self.grid.pen.style.foreground = Color::DEFAULT,
                49 => self.grid.pen.style.background = Color::DEFAULT,
                59 => self.grid.pen.style.underline_color = Color::DEFAULT,
                38 | 48 | 58 => {
                    if let Some(color) = extended_color(params, &mut index) {
                        match value {
                            38 => self.grid.pen.style.foreground = color,
                            48 => self.grid.pen.style.background = color,
                            _ => self.grid.pen.style.underline_color = color,
                        }
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn keyboard(&mut self, params: &[Param], private: u8) {
        let screen = usize::from(self.alternate_screen);
        let flags = (value(params, 0, 0) & 31) as u8;
        match private {
            b'?' => self.reply(format!("\x1b[?{}u", self.modes.kitty_keyboard).as_bytes()),
            b'>' => {
                let stack = &mut self.keyboard_stack[screen];
                if stack.len() == MAX_KEYBOARD_STACK {
                    stack.remove(0);
                }
                stack.push(self.modes.kitty_keyboard);
                self.modes.kitty_keyboard = flags;
            }
            b'<' => {
                for _ in 0..usize::from(value(params, 0, 1)).min(MAX_KEYBOARD_STACK + 1) {
                    self.modes.kitty_keyboard = self.keyboard_stack[screen].pop().unwrap_or(0);
                }
            }
            b'=' => match value(params, 1, 1) {
                1 => self.modes.kitty_keyboard = flags,
                2 => self.modes.kitty_keyboard |= flags,
                3 => self.modes.kitty_keyboard &= !flags,
                _ => {}
            },
            _ => {}
        }
        self.keyboard_flags[screen] = self.modes.kitty_keyboard;
    }

    fn osc_color(&mut self, command: u16, rest: &str) {
        if command == 4 {
            let mut fields = rest.split(';');
            while let (Some(index), Some(spec)) = (fields.next(), fields.next()) {
                if let Ok(index) = index.parse::<u8>() {
                    if spec == "?" {
                        let color = self.palette[usize::from(index)].unwrap_or_else(|| {
                            terminal_color(self.query_colors.indexed_color(index))
                        });
                        self.color_reply(&format!("4;{index}"), color);
                    } else if let Some(color) = parse_color(spec) {
                        self.palette[usize::from(index)] = Some(color);
                        self.palette_revision = self.palette_revision.wrapping_add(1);
                    }
                }
            }
        } else if command == 104 {
            if rest.is_empty() {
                self.palette.fill(None);
            } else {
                for index in rest.split(';').filter_map(|s| s.parse::<u8>().ok()) {
                    self.palette[usize::from(index)] = None;
                }
            }
            self.palette_revision = self.palette_revision.wrapping_add(1);
        } else {
            for (offset, spec) in rest.split(';').enumerate() {
                let cmd = usize::from(command).saturating_add(offset);
                let current = match cmd {
                    10 => self.foreground,
                    11 => self.background,
                    12 => self.cursor_color,
                    _ => break,
                };
                if spec == "?" {
                    let color = current.or_else(|| match cmd {
                        10 => Some(terminal_color(self.query_colors.foreground)),
                        11 => Some(terminal_color(self.query_colors.background)),
                        _ => None,
                    });
                    if let Some(color) = color {
                        self.color_reply(&cmd.to_string(), color);
                    }
                } else if let Some(color) = parse_color(spec) {
                    match cmd {
                        10 => self.foreground = Some(color),
                        11 => self.background = Some(color),
                        _ => self.cursor_color = Some(color),
                    }
                    self.palette_revision = self.palette_revision.wrapping_add(1);
                }
            }
        }
    }

    fn color_reply(&mut self, command: &str, color: Color) {
        if let Some((r, g, b)) = color.as_rgb() {
            self.reply(
                format!(
                    "\x1b]{command};rgb:{:04x}/{:04x}/{:04x}\x1b\\",
                    u16::from(r) * 257,
                    u16::from(g) * 257,
                    u16::from(b) * 257
                )
                .as_bytes(),
            );
        }
    }
}

impl Handler for State {
    fn pause_requested(&self) -> bool {
        self.modes.synchronized_update
    }

    fn print(&mut self, character: char) {
        let character = if self.charsets[self.active_charset] {
            dec_graphic(character)
        } else {
            character
        };
        self.grid.put_char(character);
        self.last_printed = Some(character);
    }

    fn print_ascii(&mut self, bytes: &[u8]) {
        if self.charsets[self.active_charset] {
            for &byte in bytes {
                self.print(char::from(byte));
            }
        } else {
            self.grid.write_ascii(bytes);
            if let Some(&byte) = bytes.last() {
                self.last_printed = Some(char::from(byte));
            }
        }
    }

    fn execute(&mut self, byte: u8) {
        self.grid.end_grapheme();
        match byte {
            0x07 => self.event(Event::Bell),
            0x08 => self.grid.backspace(),
            0x09 => self.grid.tab(),
            0x0a..=0x0c => self.grid.linefeed(),
            0x0d => self.grid.carriage_return(),
            0x0e => self.active_charset = 1,
            0x0f => self.active_charset = 0,
            _ => {}
        }
    }

    fn escape(&mut self, intermediates: &[u8], final_byte: u8) {
        self.grid.end_grapheme();
        match (intermediates, final_byte) {
            ([], b'D') => {
                let newline_mode = self.grid.newline_mode;
                self.grid.newline_mode = false;
                self.grid.linefeed();
                self.grid.newline_mode = newline_mode;
            }
            ([], b'E') => {
                self.grid.linefeed();
                self.grid.carriage_return();
            }
            ([], b'M') => self.grid.reverse_index(),
            ([], b'H') => self.grid.set_tab(),
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([], b'=') => self.modes.application_keypad = true,
            ([], b'>') => self.modes.application_keypad = false,
            ([], b'Z') => self.reply(b"\x1b[?62;22c"),
            ([], b'c') => {
                self.grid.reset();
                self.program_status.clear();
                self.grid.cursor.shape = self.default_cursor_shape;
                self.cursor_shape_overridden = false;
                self.modes = Modes::default();
                self.alternate_screen = false;
                self.charsets = [false; 2];
                self.active_charset = 0;
                self.saved_charsets = [([false; 2], 0); 2];
                self.saved_private_modes.clear();
                self.last_printed = None;
                self.keyboard_stack.iter_mut().for_each(Vec::clear);
                self.keyboard_flags = [0; 2];
                self.palette.fill(None);
                self.foreground = None;
                self.background = None;
                self.cursor_color = None;
                self.palette_revision = self.palette_revision.wrapping_add(1);
                self.title.clear();
                self.title_stack.clear();
                self.event(Event::ResetTitle);
                self.event(Event::KittyClipboardControl(
                    crate::KittyClipboardControl::Reset,
                ));
            }
            ([b'('], b'0' | b'B') => self.charsets[0] = final_byte == b'0',
            ([b')'], b'0' | b'B') => self.charsets[1] = final_byte == b'0',
            _ => {}
        }
    }

    fn csi(&mut self, params: &[Param], private: Option<u8>, intermediates: &[u8], final_byte: u8) {
        if final_byte != b'm' || private.is_some() || !intermediates.is_empty() {
            self.grid.end_grapheme();
        }
        let count = count(params, 0);
        if intermediates == b"$" && final_byte == b'p' && matches!(private, None | Some(b'?')) {
            for param in params {
                if param.subparams().is_empty() {
                    self.report_mode(private.is_some(), param.value().unwrap_or(0));
                }
            }
            return;
        }
        if final_byte == b'u'
            && intermediates.is_empty()
            && let Some(private) = private
        {
            self.keyboard(params, private);
            return;
        }
        if intermediates == b" " && private.is_none() && final_byte == b'q' {
            let style = value(params, 0, 0);
            if style <= 6 {
                let old_cursor = self.grid.cursor;
                self.grid.cursor.shape = match style {
                    0 => self.default_cursor_shape,
                    3 | 4 => CursorShape::Underline,
                    5 | 6 => CursorShape::Beam,
                    _ => CursorShape::Block,
                };
                self.cursor_shape_overridden = style != 0;
                self.grid.cursor.blinking = style == 0 || style % 2 == 1;
                if self.grid.cursor != old_cursor {
                    self.grid.cursor_changed(old_cursor);
                }
            }
            return;
        }
        if intermediates == b"\"" && private.is_none() && final_byte == b'q' {
            match value(params, 0, 0) {
                0 | 2 => self.grid.pen.style.set(Style::PROTECTED, false),
                1 => self.grid.pen.style.set(Style::PROTECTED, true),
                _ => {}
            }
            return;
        }
        if !intermediates.is_empty() {
            if intermediates == b"!" && private.is_none() && final_byte == b'p' {
                self.soft_reset();
            }
            return;
        }
        match (private, final_byte) {
            (None, b'A') => self.grid.move_cursor(-(count as isize), 0),
            (None, b'B' | b'e') => self.grid.move_cursor(count as isize, 0),
            (None, b'C' | b'a') => self.grid.move_cursor(0, count as isize),
            (None, b'D') => self.grid.move_cursor(0, -(count as isize)),
            (None, b'E') => {
                self.grid.move_cursor(count as isize, 0);
                self.grid.carriage_return();
            }
            (None, b'F') => {
                self.grid.move_cursor(-(count as isize), 0);
                self.grid.carriage_return();
            }
            (None, b'G' | b'`') => {
                self.grid.carriage_return();
                self.grid.move_cursor(0, count.saturating_sub(1) as isize);
            }
            (None, b'H' | b'f') => self.grid.goto(
                count.saturating_sub(1),
                super::dispatch::count(params, 1).saturating_sub(1),
            ),
            (None, b'd') => self
                .grid
                .goto(count.saturating_sub(1), self.grid.cursor.col),
            (None, b'I') => {
                for _ in 0..count.min(self.grid.size().cols) {
                    self.grid.tab();
                }
            }
            (None, b'Z') => {
                for _ in 0..count.min(self.grid.size().cols) {
                    self.grid.backtab();
                }
            }
            (None, b'g') => match value(params, 0, 0) {
                0 => self.grid.clear_tab(false),
                3 => self.grid.clear_tab(true),
                _ => {}
            },
            (None | Some(b'?'), b'J') => self
                .grid
                .erase_display(value(params, 0, 0), private.is_some()),
            (None | Some(b'?'), b'K') => {
                self.grid.erase_line(value(params, 0, 0), private.is_some());
            }
            (None, b'X') => self.grid.erase_chars(count),
            (None, b'@') => self.grid.insert_chars(count),
            (None, b'P') => self.grid.delete_chars(count),
            (None, b'L') => self.grid.insert_lines(count),
            (None, b'M') => self.grid.delete_lines(count),
            (None, b'S') => self.grid.scroll_up(count),
            (None, b'T') => self.grid.scroll_down(count),
            (None, b'b') => {
                if let Some(character) = self.last_printed {
                    for _ in 0..count {
                        self.grid.put_char(character);
                    }
                }
            }
            (Some(b'?'), b's' | b'r') => {
                for param in params {
                    if !param.subparams().is_empty() {
                        continue;
                    }
                    let mode = param.value().unwrap_or(0);
                    if final_byte == b's' {
                        self.save_private_mode(mode);
                    } else if let Some(enabled) = self.saved_private_mode(mode) {
                        self.mode(true, mode, enabled);
                    }
                }
            }
            (None, b'r') => {
                let bottom = value(params, 1, 0);
                let bottom = if bottom == 0 {
                    self.grid.size().rows
                } else {
                    usize::from(bottom)
                };
                self.grid.set_scroll_region(count.saturating_sub(1), bottom);
            }
            (None, b's') => self.save_cursor(),
            (None, b'u') => self.restore_cursor(),
            (None, b'm') => self.sgr(params),
            (None | Some(b'?'), b'h' | b'l') => {
                for param in params {
                    if !param.subparams().is_empty() {
                        continue;
                    }
                    self.mode(
                        private.is_some(),
                        param.value().unwrap_or(0),
                        final_byte == b'h',
                    );
                }
            }
            (None, b'c') if value(params, 0, 0) == 0 => self.reply(b"\x1b[?62;22c"),
            (Some(b'>'), b'c') if value(params, 0, 0) == 0 => self.reply(b"\x1b[>1;1;0c"),
            (None, b'n') if value(params, 0, 0) == 5 => self.reply(b"\x1b[0n"),
            (None | Some(b'?'), b'n') if value(params, 0, 0) == 6 => {
                let private = if private.is_some() { "?" } else { "" };
                let top = if self.grid.origin_mode {
                    self.grid.scroll_region().0
                } else {
                    0
                };
                let row = self.grid.cursor.row.saturating_sub(top) + 1;
                self.reply(format!("\x1b[{private}{row};{}R", self.grid.cursor.col + 1).as_bytes());
            }
            (None, b't') => match value(params, 0, 0) {
                14 => self.reply(
                    format!(
                        "\x1b[4;{};{}t",
                        usize::from(self.cell_pixels.1) * self.grid.size().rows,
                        usize::from(self.cell_pixels.0) * self.grid.size().cols
                    )
                    .as_bytes(),
                ),
                16 => self.reply(
                    format!("\x1b[6;{};{}t", self.cell_pixels.1, self.cell_pixels.0).as_bytes(),
                ),
                18 => self.reply(
                    format!(
                        "\x1b[8;{};{}t",
                        self.grid.size().rows,
                        self.grid.size().cols
                    )
                    .as_bytes(),
                ),
                22 => {
                    if self.title_stack.len() < MAX_TITLE_STACK {
                        self.title_stack.push(self.title.clone());
                    }
                }
                23 => {
                    if let Some(title) = self.title_stack.pop() {
                        self.title.clone_from(&title);
                        self.event(Event::Title(title));
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn osc_terminated(&mut self, bytes: &[u8], bell: bool) {
        if let Some(body) = bytes.strip_prefix(b"5522;") {
            let terminator = if bell {
                crate::KittyClipboardOscTerminator::Bell
            } else {
                crate::KittyClipboardOscTerminator::StringTerminator
            };
            self.event(Event::KittyClipboard(crate::KittyClipboardOsc::from_body(
                body, terminator,
            )));
        } else {
            self.osc(bytes);
        }
    }

    fn osc(&mut self, bytes: &[u8]) {
        self.grid.end_grapheme();
        let Ok(text) = std::str::from_utf8(bytes) else {
            return;
        };
        let (command, rest) = text.split_once(';').unwrap_or((text, ""));
        let Ok(command) = command.parse::<u16>() else {
            return;
        };
        let palette_revision = self.palette_revision;
        match command {
            0 | 2 => {
                self.title.clear();
                self.title.push_str(rest);
                self.event(Event::Title(rest.to_owned()));
            }
            4 | 10..=12 | 104 => self.osc_color(command, rest),
            110..=112 => {
                match command {
                    110 => self.foreground = None,
                    111 => self.background = None,
                    _ => self.cursor_color = None,
                }
                self.palette_revision = self.palette_revision.wrapping_add(1);
            }
            7 => {
                let path = rest
                    .strip_prefix("file://")
                    .and_then(|rest| rest.find('/').map(|offset| &rest[offset..]))
                    .unwrap_or(rest);
                self.event(Event::WorkingDirectory(path.to_owned()));
            }
            9 => {
                if let Some(rest) = rest.strip_prefix("4;") {
                    let mut parts = rest.split(';');
                    if let Some(state) = parts.next().and_then(|s| s.parse::<u8>().ok()) {
                        let progress = parts.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                        self.event(Event::Progress(crate::ProgressState::from_osc(
                            state, progress,
                        )));
                    }
                } else if let Some(path) = rest.strip_prefix("9;") {
                    let path = path.trim().trim_matches('"');
                    if !path.is_empty() {
                        self.event(Event::WorkingDirectory(path.to_owned()));
                    }
                }
            }
            8 => {
                let Some((params, uri)) = rest.split_once(';') else {
                    return;
                };
                if uri.is_empty() {
                    self.grid.pen.extra = None;
                } else {
                    let id = params
                        .split(':')
                        .find_map(|param| param.strip_prefix("id="))
                        .unwrap_or("");
                    self.grid.pen.extra = Some(Arc::new(CellExtra {
                        combining: String::new(),
                        hyperlink: Some(Arc::new(Hyperlink {
                            id: id.to_owned(),
                            uri: uri.to_owned(),
                        })),
                    }));
                }
            }
            52 => {
                if let Some((selection, data)) = rest.split_once(';') {
                    self.event(Event::Clipboard {
                        selection: selection.to_owned(),
                        data: data.to_owned(),
                    });
                }
            }
            133 => {
                if rest.split(';').next() == Some("A") {
                    self.program_status.finish();
                }
                self.event(Event::ShellIntegration(rest.to_owned()));
            }
            7501 if rest == "?" => self.reply(b"\x1b]7501;?\x1b\\"),
            7501 => self.program_status.report(rest),
            _ => {}
        }
        if self.palette_revision != palette_revision {
            self.grid.mark_full_damage();
        }
    }

    fn dcs(&mut self, bytes: &[u8]) {
        self.device_control_query(bytes);
    }
    fn apc(&mut self, bytes: &[u8]) {
        self.apply_graphics(bytes);
    }
}

fn value(params: &[Param], index: usize, default: u16) -> u16 {
    params.get(index).and_then(Param::value).unwrap_or(default)
}

fn count(params: &[Param], index: usize) -> usize {
    usize::from(value(params, index, 1).max(1))
}

fn extended_color(params: &[Param], index: &mut usize) -> Option<Color> {
    let sub = params[*index].subparams();
    if let Some((&mode, components)) = sub.split_first() {
        return color_components(mode?, components);
    }

    *index += 1;
    let selector = params.get(*index)?;
    let mode = selector.value()?;
    // Legacy clients may use a semicolon before the color selector, then
    // colon-separated components (38;2::r:g:b). After a colon, fields must
    // remain within that parameter rather than becoming independent SGRs.
    if !selector.subparams().is_empty() {
        return color_components(mode, selector.subparams());
    }
    let component_count = match mode {
        5 => 1,
        2 => 3,
        _ => return None,
    };
    let start = *index + 1;
    *index += component_count;
    let mut components = [None; 3];
    for (index, component) in components[..component_count].iter_mut().enumerate() {
        let param = params.get(start + index)?;
        if !param.subparams().is_empty() {
            return None;
        }
        *component = param.value();
    }
    color_components(mode, &components[..component_count])
}

fn color_components(mode: u16, components: &[Option<u16>]) -> Option<Color> {
    match (mode, components) {
        (5, [Some(index)]) => Some(Color::indexed(u8::try_from(*index).ok()?)),
        (2, [Some(r), Some(g), Some(b)] | [_, Some(r), Some(g), Some(b)]) => Some(Color::rgb(
            u8::try_from(*r).ok()?,
            u8::try_from(*g).ok()?,
            u8::try_from(*b).ok()?,
        )),
        _ => None,
    }
}

fn parse_color(spec: &str) -> Option<Color> {
    if let Some(hex) = spec.strip_prefix('#') {
        if !matches!(hex.len(), 3 | 6 | 9 | 12) || !hex.is_ascii() {
            return None;
        }
        let width = hex.len() / 3;
        return Some(Color::rgb(
            parse_component(&hex[..width])?,
            parse_component(&hex[width..2 * width])?,
            parse_component(&hex[2 * width..])?,
        ));
    }
    let mut components = spec.strip_prefix("rgb:")?.split('/');
    let color = Color::rgb(
        parse_component(components.next()?)?,
        parse_component(components.next()?)?,
        parse_component(components.next()?)?,
    );
    components.next().is_none().then_some(color)
}

fn parse_component(component: &str) -> Option<u8> {
    if component.is_empty()
        || component.len() > 4
        || !component.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let value = u32::from_str_radix(component, 16).ok()?;
    let max = (1u32 << (component.len() * 4)) - 1;
    Some((value * 255 / max) as u8)
}

fn terminal_color(color: crate::TerminalColor) -> Color {
    Color::rgb(color.r, color.g, color.b)
}

fn dec_graphic(character: char) -> char {
    const GRAPHICS: [char; 31] = [
        '◆', '▒', '␉', '␌', '␍', '␊', '°', '±', '␤', '␋', '┘', '┐', '┌', '└', '┼', '⎺', '⎻', '─',
        '⎼', '⎽', '├', '┤', '┴', '┬', '│', '≤', '≥', 'π', '≠', '£', '·',
    ];
    if ('`'..='~').contains(&character) {
        GRAPHICS[character as usize - '`' as usize]
    } else {
        character
    }
}

#[cfg(test)]
#[path = "dispatch/tests.rs"]
mod tests;
