//! Terminal state queries share the authoritative parser state.

use std::fmt::Write;

use super::{
    Color, CursorShape, MouseEncoding, MouseTracking, Style, UnderlineStyle, dispatch::State,
};

impl State {
    pub(super) fn mode_state(&self, private: bool, mode: u16) -> Option<bool> {
        if private {
            match mode {
                1 => Some(self.modes.application_cursor),
                3 => Some(self.column_mode),
                5 => Some(self.modes.reverse_video),
                6 => Some(self.grid.origin_mode),
                7 => Some(self.grid.autowrap),
                69 => Some(self.grid.lr_margin_mode()),
                9 => Some(self.modes.mouse_tracking == MouseTracking::Press),
                12 => Some(self.grid.cursor.blinking),
                25 => Some(self.grid.cursor.visible),
                47 | 1047 | 1049 => Some(self.alternate_screen),
                // Like xterm: whether the active screen holds a saved cursor.
                1048 => Some(self.grid.has_saved_cursor()),
                1000 => Some(self.modes.mouse_tracking == MouseTracking::Click),
                1002 => Some(self.modes.mouse_tracking == MouseTracking::Drag),
                1003 => Some(self.modes.mouse_tracking == MouseTracking::Motion),
                1004 => Some(self.modes.focus_events),
                1005 => Some(self.modes.mouse_encoding == MouseEncoding::Utf8),
                1006 => Some(self.modes.mouse_encoding == MouseEncoding::Sgr),
                1007 => Some(self.modes.alternate_scroll),
                1016 if self.pixel_mouse_reports => {
                    Some(self.modes.mouse_encoding == MouseEncoding::SgrPixels)
                }
                2004 => Some(self.modes.bracketed_paste),
                2026 => Some(self.modes.synchronized_update),
                5522 => Some(self.modes.clipboard_paste_events),
                _ => None,
            }
        } else {
            match mode {
                4 => Some(self.grid.insert_mode),
                20 => Some(self.grid.newline_mode),
                _ => None,
            }
        }
    }

    pub(super) fn report_mode(&mut self, private: bool, mode: u16) {
        let enabled = self.mode_state(private, mode);
        let status = if private && mode == 2027 {
            // Grapheme clustering (UAX #29) is always on and cannot be reset.
            3
        } else {
            enabled.map_or(0, |enabled| if enabled { 1 } else { 2 })
        };
        let prefix = if private { "?" } else { "" };
        let reply = format!("\x1b[{prefix}{mode};{status}$y");
        if private && mode == 5522 {
            // The kitty clipboard's paste-event mode lives only here.
            self.passthrough_reply(reply.as_bytes());
        } else {
            self.reply(reply.as_bytes());
        }
    }

    pub(super) fn device_control_query(&mut self, bytes: &[u8]) {
        if let Some(query) = bytes.strip_prefix(b"$q") {
            let response = match query {
                b"m" => Some(sgr(self.grid.pen.style, self.grid.pen.underline_color())),
                b"r" => {
                    let (top, bottom) = self.grid.scroll_region();
                    Some(format!("{};{bottom}r", top + 1))
                }
                b"s" => {
                    let (left, right) = self.grid.lr_margins();
                    Some(format!("{};{right}s", left + 1))
                }
                b" q" => {
                    let base = match self.grid.cursor.shape {
                        CursorShape::Block => 1,
                        CursorShape::Underline => 3,
                        CursorShape::Beam => 5,
                    };
                    Some(format!("{} q", base + u8::from(!self.grid.cursor.blinking)))
                }
                b"\"q" => Some(format!(
                    "{}\"q",
                    u8::from(self.grid.pen.style.attributes & Style::PROTECTED != 0)
                )),
                b"\"p" => Some("62;1\"p".to_owned()),
                _ => None,
            };
            if let Some(response) = response {
                self.reply(format!("\x1bP1$r{response}\x1b\\").as_bytes());
            } else {
                self.reply(b"\x1bP0$r\x1b\\");
            }
        } else if let Some(names) = bytes.strip_prefix(b"+q") {
            // Bound response work independently of the input string limit.
            for encoded in names.split(|byte| *byte == b';').take(32) {
                if encoded.len() > 128 || !encoded.iter().all(u8::is_ascii_hexdigit) {
                    continue;
                }
                let name = decode_hex(encoded);
                let value = match name.as_deref() {
                    Some(b"Co" | b"colors") => Some("256"),
                    Some(b"TN" | b"name") => Some("xterm-256color"),
                    Some(b"RGB") => Some("8"),
                    // Values use terminfo source notation, as kitty replies.
                    Some(b"Smulx") => Some(r"\E[4:%p1%dm"),
                    Some(b"Setulc") => {
                        Some(r"\E[58:2:%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m")
                    }
                    Some(b"Ms") => Some(r"\E]52;%p1%s;%p2%s\007"),
                    _ => None,
                };
                let mut reply = if value.is_some() {
                    String::from("\x1bP1+r")
                } else {
                    String::from("\x1bP0+r")
                };
                // ASCII hex was checked above.
                reply.push_str(std::str::from_utf8(encoded).unwrap_or_default());
                if let Some(value) = value {
                    reply.push('=');
                    for byte in value.bytes() {
                        let _ = write!(reply, "{byte:02x}");
                    }
                }
                reply.push_str("\x1b\\");
                self.reply(reply.as_bytes());
            }
        }
    }
}

fn decode_hex(bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

fn sgr(style: Style, underline_color: Color) -> String {
    let mut output = String::from("0");
    for (flag, code) in [
        (Style::BOLD, 1),
        (Style::DIM, 2),
        (Style::ITALIC, 3),
        (Style::BLINK, 5),
        (Style::INVERSE, 7),
        (Style::HIDDEN, 8),
        (Style::STRIKE, 9),
    ] {
        if style.attributes & flag != 0 {
            let _ = write!(output, ";{code}");
        }
    }
    let underline = match style.underline {
        UnderlineStyle::None => None,
        UnderlineStyle::Single => Some(";4"),
        UnderlineStyle::Double => Some(";4:2"),
        UnderlineStyle::Curly => Some(";4:3"),
        UnderlineStyle::Dotted => Some(";4:4"),
        UnderlineStyle::Dashed => Some(";4:5"),
    };
    if let Some(underline) = underline {
        output.push_str(underline);
    }
    for (code, color) in [
        (38, style.foreground),
        (48, style.background),
        (58, underline_color),
    ] {
        if color == Color::DEFAULT {
            continue;
        }
        if let Some(index) = color.as_indexed() {
            let _ = write!(output, ";{code};5;{index}");
        } else if let Some((r, g, b)) = color.as_rgb() {
            let _ = write!(output, ";{code};2;{r};{g};{b}");
        }
    }
    output.push('m');
    output
}

#[cfg(test)]
mod tests {
    use crate::terminal_engine::{Engine, Options, Size};

    fn replies(input: &[u8]) -> Vec<u8> {
        let mut engine = Engine::new(Size { cols: 80, rows: 24 }, Options::default());
        // Query parsing must survive arbitrary PTY fragmentation.
        for byte in input {
            engine.feed(&[*byte]);
        }
        let mut output = Vec::new();
        engine.drain_replies(&mut output);
        output
    }

    #[test]
    fn mode_queries_report_live_state_and_unknown_modes() {
        assert_eq!(
            replies(b"\x1b[?2004$p\x1b[?2004h\x1b[?2004$p\x1b[?9999$p\x1b[4h\x1b[4$p"),
            b"\x1b[?2004;2$y\x1b[?2004;1$y\x1b[?9999;0$y\x1b[4;1$y"
        );
    }

    #[test]
    fn status_strings_roundtrip_styles_margins_and_cursor() {
        assert_eq!(replies(b"\x1b[1;4:3;38;2;1;2;3m\x1bP$qm\x1b\\\x1b[3;20r\x1bP$qr\x1b\\\x1b[6 q\x1bP$q q\x1b\\\x1bP$q?\x1b\\"), b"\x1bP1$r0;1;4:3;38;2;1;2;3m\x1b\\\x1bP1$r3;20r\x1b\\\x1bP1$r6 q\x1b\\\x1bP0$r\x1b\\");
    }

    #[test]
    fn capability_queries_distinguish_supported_and_unknown_names() {
        assert_eq!(
            replies(b"\x1bP+q436f;524742;6e6f7065\x1b\\"),
            b"\x1bP1+r436f=323536\x1b\\\x1bP1+r524742=38\x1b\\\x1bP0+r6e6f7065\x1b\\"
        );
    }

    #[test]
    fn capability_queries_advertise_styled_underlines_and_clipboard_writes() {
        let hex = |text: &str| -> String { text.bytes().map(|b| format!("{b:02x}")).collect() };
        let names = ["Smulx", "Setulc", "Ms"];
        let query: Vec<String> = names.iter().map(|name| hex(name)).collect();
        let reply = replies(format!("\x1bP+q{}\x1b\\", query.join(";")).as_bytes());
        let expected: String = names
            .iter()
            .zip([
                r"\E[4:%p1%dm",
                r"\E[58:2:%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m",
                r"\E]52;%p1%s;%p2%s\007",
            ])
            .map(|(name, value)| format!("\x1bP1+r{}={}\x1b\\", hex(name), hex(value)))
            .collect();
        assert_eq!(String::from_utf8(reply).unwrap(), expected);
    }
}
