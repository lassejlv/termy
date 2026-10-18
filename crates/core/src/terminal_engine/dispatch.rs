use std::{collections::VecDeque, sync::Arc};

use super::{
    CellExtra, Color, CursorShape, Event, MAX_REPLY_BYTES, Modes, MouseEncoding, MouseTracking,
    Options, Size, Style, UnderlineStyle, enqueue,
    grid::Grid,
    parser::{Handler, Param},
};

const MAX_TITLE_STACK: usize = 64;
const MAX_KEYBOARD_STACK: usize = 32;
/// ASCII runs at most this long try `Grid::write_short_ascii` first.
const SHORT_ASCII_RUN: usize = 16;

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
    pub(super) pixel_mouse_reports: bool,
    pub(super) program_status: crate::program_status::ProgramStatus,
    title: String,
    title_stack: Vec<String>,
    /// The last OSC 7 (or OSC 9;9) working directory.
    working_directory: Option<String>,
    keyboard_stack: [Vec<u8>; 2],
    keyboard_flags: [u8; 2],
    charsets: [bool; 2],
    active_charset: usize,
    saved_charsets: [([bool; 2], usize); 2],
    saved_private_modes: Vec<(u16, bool)>,
    pub(super) column_mode: bool,
    last_printed: Option<char>,
    hyperlinks: super::hyperlinks::Hyperlinks,
}

impl State {
    pub(super) fn new(size: Size, options: Options) -> Self {
        Self {
            graphics: super::graphics::Graphics::default(),
            grid: Grid::with_limits(size, options.scrollback_history, options.scrollback_bytes),
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
            pixel_mouse_reports: false,
            program_status: crate::program_status::ProgramStatus::default(),
            title: String::new(),
            title_stack: Vec::new(),
            working_directory: None,
            keyboard_stack: [Vec::new(), Vec::new()],
            keyboard_flags: [0, 0],
            charsets: [false; 2],
            active_charset: 0,
            saved_charsets: [([false; 2], 0); 2],
            saved_private_modes: Vec::new(),
            column_mode: false,
            last_printed: None,
            hyperlinks: super::hyperlinks::Hyperlinks::default(),
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

    fn set_working_directory(&mut self, path: String) {
        self.working_directory = Some(path.clone());
        self.event(Event::WorkingDirectory(path));
    }

    pub(super) fn title(&self) -> &str {
        &self.title
    }

    pub(super) fn working_directory(&self) -> Option<&str> {
        self.working_directory.as_deref()
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
        self.grid.pen.set_underline_color(Color::DEFAULT);
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
        // DECCOLM and 1048 are reportable but act rather than hold state, so
        // restoring them would clear the screen or move the cursor.
        if matches!(mode, 3 | 1048) {
            return;
        }
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
        // Like xterm, resetting any tracking mode turns tracking off.
        self.modes.mouse_tracking = if enabled { mode } else { MouseTracking::None };
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
                // The width never changes, but DECCOLM still clears and homes.
                self.column_mode = enabled;
                self.grid.set_scroll_region(0, self.grid.size().rows);
                self.grid.reset_lr_margins();
                self.grid.erase_display(2, false);
                self.grid.goto(0, 0);
            }
            5 => {
                if self.modes.reverse_video != enabled {
                    self.modes.reverse_video = enabled;
                    // Renderers swap the default colors, so repaint like a
                    // palette change.
                    self.palette_revision = self.palette_revision.wrapping_add(1);
                    self.grid.mark_full_damage();
                }
            }
            6 => {
                self.grid.origin_mode = enabled;
                self.grid.goto(0, 0);
            }
            7 => self.grid.autowrap = enabled,
            69 => self.grid.set_lr_margin_mode(enabled),
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
                } else if enabled && value == 1049 {
                    // Like xterm, 1049 on the alternate screen (entered with
                    // 47 or 1047) still saves the cursor, now in the
                    // alternate screen's slot, and clears the screen.
                    self.save_cursor();
                    self.grid.erase_display(2, false);
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
            1007 => self.modes.alternate_scroll = enabled,
            // 1016 needs a host that reports pixel positions; see
            // `Engine::set_pixel_mouse_reports`. 1015 (urxvt) is not
            // implemented. Unrecognized modes keep the current encoding and
            // make DECRQM answer 0.
            1016 if self.pixel_mouse_reports => {
                self.mouse_encoding(MouseEncoding::SgrPixels, enabled);
            }
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
                59 => self.grid.pen.set_underline_color(Color::DEFAULT),
                38 | 48 | 58 => {
                    if let Some(color) = extended_color(params, &mut index) {
                        match value {
                            38 => self.grid.pen.style.foreground = color,
                            48 => self.grid.pen.style.background = color,
                            _ => self.grid.pen.set_underline_color(color),
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

    /// Replies end with the query's terminator, BEL or ST, like xterm.
    fn osc_color(&mut self, command: u16, rest: &str, bell: bool) {
        if command == 4 {
            let mut fields = rest.split(';');
            while let (Some(index), Some(spec)) = (fields.next(), fields.next()) {
                if let Ok(index) = index.parse::<u8>() {
                    if spec == "?" {
                        let color = self.palette[usize::from(index)].unwrap_or_else(|| {
                            terminal_color(self.query_colors.indexed_color(index))
                        });
                        self.color_reply(&format!("4;{index}"), color, bell);
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
                    let color = current.unwrap_or_else(|| match cmd {
                        10 => terminal_color(self.query_colors.foreground),
                        11 => terminal_color(self.query_colors.background),
                        // Like xterm, an unset cursor color is the foreground.
                        _ => self.query_colors.cursor.map_or_else(
                            || {
                                self.foreground
                                    .unwrap_or(terminal_color(self.query_colors.foreground))
                            },
                            terminal_color,
                        ),
                    });
                    self.color_reply(&cmd.to_string(), color, bell);
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

    fn color_reply(&mut self, command: &str, color: Color, bell: bool) {
        if let Some((r, g, b)) = color.as_rgb() {
            let terminator = if bell { "\x07" } else { "\x1b\\" };
            self.reply(
                format!(
                    "\x1b]{command};rgb:{:04x}/{:04x}/{:04x}{terminator}",
                    u16::from(r) * 257,
                    u16::from(g) * 257,
                    u16::from(b) * 257
                )
                .as_bytes(),
            );
        }
    }

    /// `bell` is true when the OSC ended with BEL rather than ST.
    fn osc_with_terminator(&mut self, bytes: &[u8], bell: bool) {
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
            4 | 10..=12 | 104 => self.osc_color(command, rest, bell),
            110..=112 => {
                match command {
                    110 => self.foreground = None,
                    111 => self.background = None,
                    _ => self.cursor_color = None,
                }
                self.palette_revision = self.palette_revision.wrapping_add(1);
            }
            7 => {
                let path = match split_url(rest) {
                    // vte.sh and fish percent-encode the path of a file URL.
                    // Other shells send a literal `%`, so keep malformed
                    // escapes as written.
                    Some((scheme, url)) if scheme.eq_ignore_ascii_case("file") => {
                        url.find('/').map(|offset| file_url_path(&url[offset..]))
                    }
                    Some(_) => None,
                    None => Some(rest.to_owned()),
                };
                if let Some(path) = path {
                    self.set_working_directory(path);
                }
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
                        self.set_working_directory(path.to_owned());
                    }
                }
            }
            8 => {
                let Some((params, uri)) = rest.split_once(';') else {
                    return;
                };
                let id = params
                    .split(':')
                    .find_map(|param| param.strip_prefix("id="))
                    .unwrap_or("");
                // Oversized links, or links past the table budget, print as text.
                let link = (!uri.is_empty())
                    .then(|| self.hyperlinks.intern(id, uri))
                    .flatten();
                // Reopening the pen's link keeps its metadata, so cells on
                // either side keep sharing one allocation.
                let unchanged = match (&link, self.grid.pen.hyperlink()) {
                    (Some(link), Some(current)) => Arc::ptr_eq(link, current),
                    (None, None) => true,
                    _ => false,
                };
                if unchanged {
                    return;
                }
                let underline_color = self.grid.pen.underline_color();
                self.grid.pen.extra = link.map(|link| {
                    Arc::new(CellExtra {
                        hyperlink: Some(link),
                        ..CellExtra::default()
                    })
                });
                self.grid.pen.set_underline_color(underline_color);
            }
            52 => {
                if let Some((selection, data)) = rest.split_once(';') {
                    // Replies echo the selection, so keep only xterm's target
                    // letters, once each, instead of program-chosen text.
                    let mut targets = String::new();
                    for target in selection.chars().filter(|c| "cpqs01234567".contains(*c)) {
                        if !targets.contains(target) {
                            targets.push(target);
                        }
                    }
                    self.event(Event::Clipboard {
                        selection: targets,
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
}

impl State {
    /// `print_ascii` of one byte when the grid writes it in place; see
    /// `Grid::write_ascii_cell`. Returns false, having changed nothing,
    /// otherwise.
    #[inline]
    pub(super) fn print_ascii_cell(&mut self, byte: u8) -> bool {
        if self.charsets[self.active_charset] || !self.grid.write_ascii_cell(byte) {
            return false;
        }
        self.last_printed = Some(char::from(byte));
        true
    }

    #[inline(never)]
    fn print_ascii_run(&mut self, bytes: &[u8]) {
        if self.charsets[self.active_charset] {
            for &byte in bytes {
                self.print(char::from(byte));
            }
        } else {
            // Short runs (words between other text) mostly fit the row
            // as they are; longer ones are stored in vector blocks.
            if bytes.len() > SHORT_ASCII_RUN || !self.grid.write_short_ascii(bytes) {
                self.grid.write_ascii(bytes);
            }
            if let Some(&byte) = bytes.last() {
                self.last_printed = Some(char::from(byte));
            }
        }
    }
}

impl Handler for State {
    fn pause_requested(&self) -> bool {
        self.modes.synchronized_update
            || self.has_deferred_upload()
            || self.pauses_for_animation_work()
    }

    fn print(&mut self, character: char) {
        self.grid.flush_pending_clear();
        let character = if self.charsets[self.active_charset] {
            dec_graphic(character)
        } else {
            character
        };
        self.grid.put_char(character);
        self.last_printed = Some(character);
    }

    fn print_utf8(&mut self, bytes: &[u8]) -> usize {
        if self.charsets[self.active_charset] {
            return 0;
        }
        let (written, last) = self.grid.write_utf8_run(bytes);
        if let Some(character) = last {
            self.last_printed = Some(character);
        }
        written
    }

    // Out of line, as the parser's loop runs faster without it on some
    // targets; left to the inliner, unrelated changes to the crate flip it.
    #[inline(never)]
    fn print_ascii(&mut self, bytes: &[u8]) {
        // Byte-at-a-time output (echo, small PTY reads) writes one cell.
        if let &[byte] = bytes
            && self.print_ascii_cell(byte)
        {
            return;
        }
        self.print_ascii_run(bytes);
    }

    fn execute(&mut self, byte: u8) {
        self.grid.flush_pending_clear();
        self.grid.end_grapheme();
        match byte {
            0x07 => self.event(Event::Bell),
            0x08 => self.grid.backspace(),
            0x09 => self.grid.tab(),
            0x0a..=0x0c => self.grid.linefeed_deferring_clear(),
            0x0d => self.grid.carriage_return(),
            0x0e => self.active_charset = 1,
            0x0f => self.active_charset = 0,
            _ => {}
        }
    }

    fn escape(&mut self, intermediates: &[u8], final_byte: u8) {
        self.grid.flush_pending_clear();
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
            ([b'#'], b'8') => self.grid.screen_alignment_test(),
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
                self.column_mode = false;
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
            // Only DEC special graphics is translated. Other designations,
            // such as UK or national sets, select an ASCII-equivalent set.
            ([b'('], _) => self.charsets[0] = final_byte == b'0',
            ([b')'], _) => self.charsets[1] = final_byte == b'0',
            _ => {}
        }
    }

    fn csi(&mut self, params: &[Param], private: Option<u8>, intermediates: &[u8], final_byte: u8) {
        if final_byte != b'm' || private.is_some() || !intermediates.is_empty() {
            self.grid.flush_pending_clear();
            self.grid.end_grapheme();
        }
        // SGR only changes the pen. A pending clear keeps the blank chosen
        // when its line feed scrolled, so it may stay pending across colors.
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
                // Like kitty and Ghostty, 0 restores the startup cursor,
                // which is steady, instead of xterm's blinking block.
                self.grid.cursor.blinking = if style == 0 {
                    super::Cursor::default().blinking
                } else {
                    style % 2 == 1
                };
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
            (None, b'G' | b'`') => self.grid.goto_col(count.saturating_sub(1)),
            (None, b'H' | b'f') => self.grid.goto(
                count.saturating_sub(1),
                super::dispatch::count(params, 1).saturating_sub(1),
            ),
            (None, b'd') => self.grid.goto_row(count.saturating_sub(1)),
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
                    self.grid.repeat_char(character, count);
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
            // With DECLRMM set, `CSI s` is DECSLRM instead of SCOSC.
            (None, b's') if self.grid.lr_margin_mode() => {
                let right = value(params, 1, 0);
                let right = if right == 0 {
                    self.grid.size().cols
                } else {
                    usize::from(right)
                };
                self.grid.set_lr_margins(count.saturating_sub(1), right);
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
            // XTVERSION reports the engine crate, which every host embeds.
            (Some(b'>'), b'q') if value(params, 0, 0) == 0 => {
                self.reply(
                    concat!("\x1bP>|Termy ", env!("CARGO_PKG_VERSION"), "\x1b\\").as_bytes(),
                );
            }
            (None, b'n') if value(params, 0, 0) == 5 => self.reply(b"\x1b[0n"),
            (None | Some(b'?'), b'n') if value(params, 0, 0) == 6 => {
                let private = if private.is_some() { "?" } else { "" };
                let (top, left) = if self.grid.origin_mode {
                    (self.grid.scroll_region().0, self.grid.lr_margins().0)
                } else {
                    (0, 0)
                };
                let row = self.grid.cursor.row.saturating_sub(top) + 1;
                let col = self.grid.cursor.col.saturating_sub(left) + 1;
                self.reply(format!("\x1b[{private}{row};{col}R").as_bytes());
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
        self.grid.flush_pending_clear();
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
            self.osc_with_terminator(bytes, bell);
        }
    }

    fn osc(&mut self, bytes: &[u8]) {
        self.grid.flush_pending_clear();
        self.osc_with_terminator(bytes, false);
    }

    fn dcs(&mut self, bytes: &[u8]) {
        self.grid.flush_pending_clear();
        self.device_control_query(bytes);
    }
    fn apc(&mut self, bytes: &mut Vec<u8>) {
        self.grid.flush_pending_clear();
        self.apply_graphics(bytes);
    }
}

/// Split `scheme://rest`; `None` for a bare path, including `C:\dir`.
fn split_url(text: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = text.split_once("://")?;
    let mut bytes = scheme.bytes();
    (bytes.next()?.is_ascii_alphabetic()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.')))
    .then_some((scheme, rest))
}

/// The working directory named by the path of a `file://host/path` URL.
fn file_url_path(path: &str) -> String {
    let path = percent_decode(path).unwrap_or_else(|| path.to_owned());
    if cfg!(windows) {
        windows_file_url_path(path)
    } else {
        path
    }
}

/// A decoded `file://` URL path as a Windows path: `/C:/dir` becomes `C:\dir`
/// and `//server/share/dir` (`file://host//server/share` or
/// `file:////server/share`) becomes the UNC path `\\server\share\dir`.
///
/// A plain path is kept as sent even when the URL names another host: that is
/// usually a shell over SSH or in WSL reporting its own `/home/me`, and reading
/// it as `\\host\home\me` would make every cwd lookup try an SMB connection.
fn windows_file_url_path(path: String) -> String {
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
        && matches!(bytes.get(3).copied(), None | Some(b'/' | b'\\'));
    if drive {
        let mut path = path[1..].replace('/', "\\");
        // `C:` alone is the drive's current directory, not its root.
        if path.len() == 2 {
            path.push('\\');
        }
        return path;
    }
    if let Some(share) = path.strip_prefix("//") {
        let share = share.trim_start_matches('/');
        if !share.is_empty() {
            return format!(r"\\{}", share.replace('/', "\\"));
        }
    }
    path
}

/// Decode `%XX` escapes; `None` for malformed escapes or non-UTF-8 results.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
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
    match character {
        '_' => ' ',
        '`'..='~' => GRAPHICS[character as usize - '`' as usize],
        _ => character,
    }
}

#[cfg(test)]
#[path = "dispatch/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "dispatch/margin_tests.rs"]
mod margin_tests;

#[path = "dispatch/snapshot.rs"]
mod snapshot;
