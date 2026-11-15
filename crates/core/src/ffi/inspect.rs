//! Reading a terminal's state for tools that drive it, such as Termy Next's
//! plugin API: the modes programs turned on, and the screen and scrollback as
//! text.

use std::fmt::Write as _;

use super::{
    TermyFfiBytes, TermyFfiStatus, TermyFfiTerminal, ffi_bytes_from_string, ffi_status_guard,
};
use crate::{TerminalRenderCell, TerminalRenderColor, TerminalUnderlineStyle};

/// The modes programs turned on, as a JSON object: `alternate_screen`,
/// `bracketed_paste`, `mouse` (`enabled`, `report_click`, `report_drag`,
/// `report_motion`, `sgr_encoding`, `utf8_encoding`) and `keyboard`
/// (`application_keypad`, `disambiguate_escape_codes`, `report_event_types`,
/// `report_alternate_keys`, `report_all_keys_as_esc`,
/// `report_associated_text`). Free with `termy_buffer_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_modes_json(
    terminal: *mut TermyFfiTerminal,
    out_json: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_json.is_null() {
            return TermyFfiStatus::Null;
        }
        let terminal = unsafe { &(*terminal).terminal };
        let mouse = terminal.mouse_mode();
        let keyboard = terminal.keyboard_mode();
        let json = serde_json::json!({
            "alternate_screen": terminal.alternate_screen_mode(),
            "bracketed_paste": terminal.bracketed_paste_mode(),
            "mouse": {
                "enabled": mouse.enabled,
                "report_click": mouse.report_click,
                "report_drag": mouse.report_drag,
                "report_motion": mouse.report_motion,
                "sgr_encoding": mouse.sgr_encoding,
                "utf8_encoding": mouse.utf8_encoding,
            },
            "keyboard": {
                "application_keypad": keyboard.application_keypad(),
                "disambiguate_escape_codes": keyboard.disambiguate_escape_codes(),
                "report_event_types": keyboard.report_event_types(),
                "report_alternate_keys": keyboard.report_alternate_keys(),
                "report_all_keys_as_esc": keyboard.report_all_keys_as_esc(),
                "report_associated_text": keyboard.report_associated_text(),
            },
        });
        unsafe { *out_json = ffi_bytes_from_string(json.to_string()) };
        TermyFfiStatus::Ok
    })
}

/// How `termy_terminal_text` writes cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFormat {
    Plain,
    /// With SGR escape sequences for colors and styles.
    Vt,
    /// An HTML `<pre>` with inline styles.
    Html,
}

/// `TERMY_TEXT_UNWRAP`: lines the terminal wrapped join into one.
pub const TERMY_TEXT_UNWRAP: u32 = 1 << 8;

/// The screen and scrollback as text: the last `max_lines` lines (0 for
/// all of them), oldest first. `flags` is the format (0 plain, 1 VT, 2
/// HTML), or'd with `TERMY_TEXT_UNWRAP`. Free with `termy_buffer_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_text(
    terminal: *mut TermyFfiTerminal,
    flags: u32,
    max_lines: u32,
    out_text: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_text.is_null() {
            return TermyFfiStatus::Null;
        }
        let format = match flags & 0xff {
            0 => TextFormat::Plain,
            1 => TextFormat::Vt,
            2 => TextFormat::Html,
            _ => return TermyFfiStatus::InvalidArgument,
        };
        let terminal = unsafe { &(*terminal).terminal };
        let (first, last) = terminal.line_bounds();
        let first = if max_lines == 0 {
            first
        } else {
            first.max(last.saturating_sub(max_lines as i32).saturating_add(1))
        };
        let mut lines: Vec<Vec<TerminalRenderCell>> = Vec::new();
        let mut current = i32::MIN;
        terminal.visit_line_cells(first, last, |_, line, _, cell| {
            if line != current {
                current = line;
                lines.push(Vec::new());
            }
            if let Some(cells) = lines.last_mut() {
                cells.push(cell.clone());
            }
        });
        let text = format_lines(&lines, format, flags & TERMY_TEXT_UNWRAP != 0);
        unsafe { *out_text = ffi_bytes_from_string(text) };
        TermyFfiStatus::Ok
    })
}

/// Writes `lines` of cells as `format`. Trailing blank cells and blank lines
/// at the end are dropped.
pub fn format_lines(lines: &[Vec<TerminalRenderCell>], format: TextFormat, unwrap: bool) -> String {
    let mut out = String::new();
    if format == TextFormat::Html {
        out.push_str("<pre class=\"termy\">");
    }
    let mut rendered: Vec<(String, bool)> = lines
        .iter()
        .map(|cells| {
            let wrapped = cells.last().is_some_and(|cell| cell.line_wrapped);
            (format_line(cells, format, wrapped), wrapped)
        })
        .collect();
    while rendered.last().is_some_and(|(line, _)| line.is_empty()) {
        rendered.pop();
    }
    for (index, (line, wrapped)) in rendered.iter().enumerate() {
        out.push_str(line);
        let last = index + 1 == rendered.len();
        if !(unwrap && *wrapped) || last {
            out.push('\n');
        }
    }
    if format == TextFormat::Html {
        out.push_str("</pre>\n");
    }
    out
}

pub(super) fn format_line(
    cells: &[TerminalRenderCell],
    format: TextFormat,
    wrapped: bool,
) -> String {
    // A wrapped line keeps its trailing spaces: they are part of the text.
    let end = if wrapped {
        cells.len()
    } else {
        cells
            .iter()
            .rposition(|cell| !is_blank(cell))
            .map_or(0, |index| index + 1)
    };
    let mut out = String::new();
    let mut style: Option<Style> = None;
    let mut open = false;
    for cell in &cells[..end] {
        if cell.wide_character_spacer || cell.leading_wide_character_spacer {
            continue;
        }
        let text = if cell.hidden || cell.text.as_str().is_empty() {
            " "
        } else {
            cell.text.as_str()
        };
        match format {
            TextFormat::Plain => out.push_str(text),
            TextFormat::Vt => {
                let next = Style::of(cell);
                if style != Some(next) {
                    out.push_str(&next.sgr());
                    style = Some(next);
                }
                out.push_str(text);
            }
            TextFormat::Html => {
                let next = Style::of(cell);
                if style != Some(next) {
                    if open {
                        out.push_str("</span>");
                    }
                    let css = next.css();
                    open = !css.is_empty();
                    if open {
                        let _ = write!(out, "<span style=\"{css}\">");
                    }
                    style = Some(next);
                }
                escape_html(text, &mut out);
            }
        }
    }
    match format {
        TextFormat::Vt if style.is_some_and(|style| style != Style::default()) => {
            out.push_str("\x1b[0m");
        }
        TextFormat::Html if open => out.push_str("</span>"),
        _ => {}
    }
    out
}

fn is_blank(cell: &TerminalRenderCell) -> bool {
    let text = cell.text.as_str();
    (text.is_empty() || text == " ")
        && matches!(cell.background, TerminalRenderColor::DefaultBackground)
        && !cell.inverse
}

fn escape_html(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Style {
    foreground: Option<TerminalRenderColor>,
    background: Option<TerminalRenderColor>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
    strikethrough: bool,
}

impl Style {
    fn of(cell: &TerminalRenderCell) -> Self {
        let color = |color: TerminalRenderColor| match color {
            TerminalRenderColor::DefaultForeground | TerminalRenderColor::DefaultBackground => None,
            other => Some(other),
        };
        Style {
            foreground: color(cell.foreground),
            background: color(cell.background),
            bold: cell.bold,
            dim: cell.dim,
            italic: cell.italic,
            underline: cell.underline_style != TerminalUnderlineStyle::None,
            inverse: cell.inverse,
            strikethrough: cell.strikethrough,
        }
    }

    fn sgr(&self) -> String {
        let mut codes = vec!["0".to_owned()];
        for (on, code) in [
            (self.bold, "1"),
            (self.dim, "2"),
            (self.italic, "3"),
            (self.underline, "4"),
            (self.inverse, "7"),
            (self.strikethrough, "9"),
        ] {
            if on {
                codes.push(code.to_owned());
            }
        }
        if let Some(color) = self.foreground {
            codes.push(sgr_color(color, 38));
        }
        if let Some(color) = self.background {
            codes.push(sgr_color(color, 48));
        }
        format!("\x1b[{}m", codes.join(";"))
    }

    fn css(&self) -> String {
        let (mut foreground, mut background) = (self.foreground, self.background);
        if self.inverse {
            std::mem::swap(&mut foreground, &mut background);
        }
        let mut css = String::new();
        if let Some(color) = foreground.and_then(css_color) {
            let _ = write!(css, "color:{color};");
        } else if self.inverse {
            css.push_str("color:var(--termy-background,#000);");
        }
        if let Some(color) = background.and_then(css_color) {
            let _ = write!(css, "background-color:{color};");
        } else if self.inverse {
            css.push_str("background-color:var(--termy-foreground,#fff);");
        }
        for (on, rule) in [
            (self.bold, "font-weight:bold;"),
            (self.dim, "opacity:0.6;"),
            (self.italic, "font-style:italic;"),
            (self.underline, "text-decoration:underline;"),
            (self.strikethrough, "text-decoration:line-through;"),
        ] {
            if on {
                css.push_str(rule);
            }
        }
        css
    }
}

fn sgr_color(color: TerminalRenderColor, base: u8) -> String {
    match color {
        TerminalRenderColor::Indexed(index) | TerminalRenderColor::DimIndexed(index) => {
            format!("{base};5;{index}")
        }
        TerminalRenderColor::Rgb(rgb) => format!("{base};2;{};{};{}", rgb.r, rgb.g, rgb.b),
        _ => format!("{}", base + 1),
    }
}

/// The xterm palette, for colors a program picked by index.
fn css_color(color: TerminalRenderColor) -> Option<String> {
    let (r, g, b) = match color {
        TerminalRenderColor::Rgb(rgb) => (rgb.r, rgb.g, rgb.b),
        TerminalRenderColor::Indexed(index) | TerminalRenderColor::DimIndexed(index) => {
            xterm_color(index)
        }
        _ => return None,
    };
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn xterm_color(index: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    match index {
        0..=15 => BASE[usize::from(index)],
        16..=231 => {
            let n = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TerminalRenderText;

    fn cell(text: &str) -> TerminalRenderCell {
        TerminalRenderCell {
            text: TerminalRenderText::new(text),
            ..TerminalRenderCell::default()
        }
    }

    fn line(text: &str, width: usize, wrapped: bool) -> Vec<TerminalRenderCell> {
        let mut cells: Vec<_> = text.chars().map(|c| cell(&c.to_string())).collect();
        while cells.len() < width {
            cells.push(cell(" "));
        }
        if let Some(last) = cells.last_mut() {
            last.line_wrapped = wrapped;
        }
        cells
    }

    #[test]
    fn plain_text_drops_trailing_blanks_and_joins_wrapped_lines_when_asked() {
        let lines = vec![
            line("hello", 5, true),
            line("world", 8, false),
            line("", 8, false),
        ];
        assert_eq!(
            format_lines(&lines, TextFormat::Plain, false),
            "hello\nworld\n"
        );
        assert_eq!(
            format_lines(&lines, TextFormat::Plain, true),
            "helloworld\n"
        );
    }

    #[test]
    fn vt_and_html_carry_colors() {
        let mut red = cell("x");
        red.foreground = TerminalRenderColor::Indexed(1);
        red.bold = true;
        let lines = vec![vec![red, cell("y")]];
        assert_eq!(
            format_lines(&lines, TextFormat::Vt, false),
            "\x1b[0;1;38;5;1mx\x1b[0my\n"
        );
        assert_eq!(
            format_lines(&lines, TextFormat::Html, false),
            "<pre class=\"termy\"><span style=\"color:#cd0000;font-weight:bold;\">x</span>y\n</pre>\n"
        );
    }
}
