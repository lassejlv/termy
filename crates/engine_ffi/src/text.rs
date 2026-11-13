//! Text extraction for selections and captures, and word/line boundaries
//! for double- and triple-click selection.

use std::fmt::Write;

use termy_core::terminal_engine::{Cell, Color, Engine, Line, Style, UnderlineStyle};

use crate::{
    TermyEngine, TermyEngineBytes, TermyEngineColors, TermyEngineStatus, guard, handle,
    termy_engine_effective_colors,
};

pub const FORMAT_PLAIN: u32 = 0;
pub const FORMAT_VT: u32 = 1;
pub const FORMAT_HTML: u32 = 2;

pub const TEXT_UNWRAP: u32 = 1;
pub const TEXT_TRIM: u32 = 1 << 1;

/// Buffer lines are numbered like the engine's: negative lines are
/// scrollback (-1 the newest), 0 the first live row. Columns are inclusive.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyEngineTextRange {
    pub start_line: i64,
    pub start_col: u32,
    pub end_line: i64,
    pub end_col: u32,
    /// A C `bool`, read as a byte; any nonzero value is true.
    pub rectangular: u8,
}

fn line_bounds(engine: &Engine) -> (i64, i64) {
    (
        -(engine.history_size() as i64),
        engine.size().rows as i64 - 1,
    )
}

fn get_line(engine: &Engine, line: i64) -> Option<Line<'_>> {
    i32::try_from(line).ok().and_then(|line| engine.line(line))
}

fn wrapped(engine: &Engine, line: i64) -> bool {
    i32::try_from(line).is_ok_and(|line| engine.line_wrapped(line))
}

/// Pen state while writing VT or HTML.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Pen {
    style: Style,
    underline_color: Color,
}

impl Default for Pen {
    fn default() -> Self {
        Self {
            style: Style::default(),
            underline_color: Color::DEFAULT,
        }
    }
}

fn sgr_color(out: &mut String, base: u8, color: Color) {
    if let Some(index) = color.as_indexed() {
        let _ = write!(out, ";{base}8;5;{index}");
    } else if let Some((r, g, b)) = color.as_rgb() {
        let _ = write!(out, ";{base}8;2;{r};{g};{b}");
    }
}

fn sgr(pen: Pen) -> String {
    let mut out = String::from("\x1b[0");
    let style = pen.style;
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
            let _ = write!(out, ";{code}");
        }
    }
    match style.underline {
        UnderlineStyle::None => {}
        UnderlineStyle::Single => out.push_str(";4"),
        UnderlineStyle::Double => out.push_str(";4:2"),
        UnderlineStyle::Curly => out.push_str(";4:3"),
        UnderlineStyle::Dotted => out.push_str(";4:4"),
        UnderlineStyle::Dashed => out.push_str(";4:5"),
    }
    sgr_color(&mut out, 3, style.foreground);
    sgr_color(&mut out, 4, style.background);
    sgr_color(&mut out, 5, pen.underline_color);
    out.push('m');
    out
}

fn html_escape(out: &mut String, character: char) {
    match character {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        character => out.push(character),
    }
}

/// Whether an OSC 8 URI may become an HTML `href`. Programs choose these
/// URIs, so schemes that run script (`javascript:`, `data:`) are dropped.
fn safe_href(uri: &str) -> bool {
    let Some((scheme, _)) = uri.split_once(':') else {
        return false;
    };
    [
        "http", "https", "mailto", "file", "ftp", "ftps", "sftp", "ssh",
    ]
    .iter()
    .any(|safe| scheme.eq_ignore_ascii_case(safe))
}

fn css_color(colors: &TermyEngineColors, color: Color, default: u32) -> u32 {
    if let Some(index) = color.as_indexed() {
        colors.palette[usize::from(index)]
    } else if let Some((r, g, b)) = color.as_rgb() {
        u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
    } else {
        default
    }
}

fn html_style(colors: &TermyEngineColors, pen: Pen) -> String {
    let style = pen.style;
    let mut foreground = css_color(colors, style.foreground, colors.foreground);
    let mut background = css_color(colors, style.background, colors.background);
    if style.attributes & Style::INVERSE != 0 {
        std::mem::swap(&mut foreground, &mut background);
    }
    let mut css = format!("color:#{foreground:06x}");
    if style.background != Color::DEFAULT || style.attributes & Style::INVERSE != 0 {
        let _ = write!(css, ";background-color:#{background:06x}");
    }
    if style.attributes & Style::BOLD != 0 {
        css.push_str(";font-weight:bold");
    }
    if style.attributes & Style::DIM != 0 {
        css.push_str(";opacity:0.6");
    }
    if style.attributes & Style::ITALIC != 0 {
        css.push_str(";font-style:italic");
    }
    if style.attributes & Style::HIDDEN != 0 {
        css.push_str(";visibility:hidden");
    }
    let mut decorations = Vec::new();
    if style.underline != UnderlineStyle::None {
        decorations.push("underline");
    }
    if style.attributes & Style::STRIKE != 0 {
        decorations.push("line-through");
    }
    if !decorations.is_empty() {
        let _ = write!(css, ";text-decoration:{}", decorations.join(" "));
        let line_style = match style.underline {
            UnderlineStyle::Double => Some("double"),
            UnderlineStyle::Curly => Some("wavy"),
            UnderlineStyle::Dotted => Some("dotted"),
            UnderlineStyle::Dashed => Some("dashed"),
            _ => None,
        };
        if let Some(line_style) = line_style {
            let _ = write!(css, ";text-decoration-style:{line_style}");
        }
        if pen.underline_color != Color::DEFAULT {
            let color = css_color(colors, pen.underline_color, foreground);
            let _ = write!(css, ";text-decoration-color:#{color:06x}");
        }
    }
    css
}

struct Formatter<'a> {
    format: u32,
    colors: Option<&'a TermyEngineColors>,
    out: String,
    pen: Pen,
    link: Option<String>,
    span_open: bool,
}

impl Formatter<'_> {
    fn set(&mut self, pen: Pen, link: Option<&str>) {
        let link = link.filter(|link| self.format != FORMAT_HTML || safe_href(link));
        match self.format {
            FORMAT_VT => {
                if pen != self.pen {
                    self.out.push_str(&sgr(pen));
                }
                if link != self.link.as_deref() {
                    let _ = write!(self.out, "\x1b]8;;{}\x1b\\", link.unwrap_or_default());
                }
            }
            FORMAT_HTML => {
                if pen == self.pen && link == self.link.as_deref() {
                    return;
                }
                self.close_html();
                if let Some(link) = link {
                    self.out.push_str("<a href=\"");
                    link.chars().for_each(|c| html_escape(&mut self.out, c));
                    self.out.push_str("\">");
                }
                if pen != Pen::default()
                    && let Some(colors) = self.colors
                {
                    let _ = write!(self.out, "<span style=\"{}\">", html_style(colors, pen));
                    self.span_open = true;
                }
            }
            _ => {}
        }
        self.pen = pen;
        self.link = link.map(str::to_owned);
    }

    fn close_html(&mut self) {
        if self.span_open {
            self.out.push_str("</span>");
            self.span_open = false;
        }
        if self.link.is_some() {
            self.out.push_str("</a>");
        }
    }

    fn text(&mut self, character: char) {
        if self.format == FORMAT_HTML {
            html_escape(&mut self.out, character);
        } else {
            self.out.push(character);
        }
    }

    fn newline(&mut self) {
        match self.format {
            FORMAT_VT => self.out.push_str("\r\n"),
            FORMAT_HTML => {
                // Styles end at the line break so copied fragments stay valid.
                self.set(Pen::default(), None);
                self.out.push('\n');
            }
            _ => self.out.push('\n'),
        }
    }

    fn finish(mut self) -> String {
        match self.format {
            FORMAT_VT => {
                self.set(Pen::default(), None);
                if !self.out.is_empty() {
                    self.out.push_str("\x1b[0m");
                }
            }
            FORMAT_HTML => {
                self.close_html();
                self.out = format!(
                    "<pre class=\"termy\" style=\"{}\">{}</pre>",
                    self.colors.map_or(String::new(), |colors| format!(
                        "color:#{:06x};background-color:#{:06x}",
                        colors.foreground, colors.background
                    )),
                    self.out
                );
            }
            _ => {}
        }
        self.out
    }
}

/// The text of `range` in `format`; see the header for `options`.
pub(crate) fn read_text(
    engine: &Engine,
    colors: Option<&TermyEngineColors>,
    range: TermyEngineTextRange,
    format: u32,
    options: u32,
) -> String {
    let (top, bottom) = line_bounds(engine);
    let (mut start, mut end) = (
        (range.start_line, range.start_col as usize),
        (range.end_line, range.end_col as usize),
    );
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    let (left, right) = if range.rectangular != 0 {
        (start.1.min(end.1), start.1.max(end.1))
    } else {
        (0, usize::MAX)
    };
    let mut formatter = Formatter {
        format,
        colors,
        out: String::new(),
        pen: Pen::default(),
        link: None,
        span_open: false,
    };
    let first = start.0.max(top);
    let last = end.0.min(bottom);
    let mut line = first;
    // Packed history is decoded into scratch: `Engine::line` would keep a
    // dense copy of every row it expands until the next output, so a
    // whole-buffer capture would leave all of scrollback expanded.
    let mut scratch = Vec::new();
    while line <= last {
        let Some(number) = i32::try_from(line).ok() else {
            break;
        };
        let read = engine.with_line(number, &mut scratch, |cells| {
            let (from, to) = if range.rectangular != 0 {
                (left, right)
            } else {
                (
                    if line == start.0 { start.1 } else { 0 },
                    if line == end.0 { end.1 } else { usize::MAX },
                )
            };
            let to = to.min(cells.len().saturating_sub(1));
            let soft_wrapped = wrapped(engine, line) && range.rectangular == 0;
            // Trailing blanks of a hard line end are padding, not text.
            let mut stop = to + 1;
            if options & TEXT_TRIM != 0 && !(soft_wrapped && to + 1 == cells.len()) {
                while stop > from
                    && cells[stop - 1].character == ' '
                    && cells.extra(stop - 1).is_none()
                    && (format == FORMAT_PLAIN
                        || cells[stop - 1].style.background == Color::DEFAULT)
                {
                    stop -= 1;
                }
            }
            for col in from..stop.max(from) {
                let Some(cell) = cells.get(col) else {
                    break;
                };
                if cell.flags & (Cell::WIDE_SPACER | Cell::LEADING_WIDE_SPACER) != 0 {
                    continue;
                }
                let extra = cells.extra(col);
                formatter.set(
                    Pen {
                        style: cell.style,
                        underline_color: extra
                            .map_or(Color::DEFAULT, |extra| extra.underline_color),
                    },
                    extra
                        .and_then(|extra| extra.hyperlink.as_ref())
                        .map(|link| link.uri.as_str()),
                );
                formatter.text(cell.character);
                if let Some(extra) = extra {
                    extra.combining.chars().for_each(|c| formatter.text(c));
                }
            }
            let joined = options & TEXT_UNWRAP != 0 && soft_wrapped && to + 1 == cells.len();
            if line != last && !joined {
                formatter.newline();
            }
        });
        if read.is_none() {
            break;
        }
        line += 1;
    }
    formatter.finish()
}

fn is_word(character: char) -> bool {
    !character.is_whitespace()
        && !matches!(
            character,
            '"' | '\''
                | '`'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
                | ','
                | ';'
                | '|'
                | '│'
        )
}

/// The character a cell shows, with spacers resolved to their wide glyph.
fn character_at(engine: &Engine, line: i64, col: usize) -> Option<char> {
    let cells = get_line(engine, line)?;
    let cell = cells.get(col)?;
    if cell.flags & Cell::WIDE_SPACER != 0 && col > 0 {
        return cells.get(col - 1).map(|cell| cell.character);
    }
    Some(cell.character)
}

fn line_len(engine: &Engine, line: i64) -> usize {
    get_line(engine, line).map_or(0, |cells| cells.len())
}

pub(crate) fn select_word(engine: &Engine, line: i64, col: usize) -> Option<TermyEngineTextRange> {
    let (top, bottom) = line_bounds(engine);
    let target = character_at(engine, line, col)?;
    let word = is_word(target);
    let same = |c: char| if word { is_word(c) } else { c == target };
    let (mut start_line, mut start_col) = (line, col);
    loop {
        if start_col > 0 {
            match character_at(engine, start_line, start_col - 1) {
                Some(c) if same(c) => start_col -= 1,
                _ => break,
            }
        } else if start_line > top && wrapped(engine, start_line - 1) {
            let previous = line_len(engine, start_line - 1);
            match character_at(engine, start_line - 1, previous.saturating_sub(1)) {
                Some(c) if previous != 0 && same(c) => {
                    start_line -= 1;
                    start_col = previous - 1;
                }
                _ => break,
            }
        } else {
            break;
        }
    }
    let (mut end_line, mut end_col) = (line, col);
    loop {
        let len = line_len(engine, end_line);
        if end_col + 1 < len {
            match character_at(engine, end_line, end_col + 1) {
                Some(c) if same(c) => end_col += 1,
                _ => break,
            }
        } else if end_line < bottom && wrapped(engine, end_line) {
            match character_at(engine, end_line + 1, 0) {
                Some(c) if same(c) => {
                    end_line += 1;
                    end_col = 0;
                }
                _ => break,
            }
        } else {
            break;
        }
    }
    Some(TermyEngineTextRange {
        start_line,
        start_col: start_col as u32,
        end_line,
        end_col: end_col as u32,
        rectangular: 0,
    })
}

pub(crate) fn select_line(engine: &Engine, line: i64) -> Option<TermyEngineTextRange> {
    let (top, bottom) = line_bounds(engine);
    if !(top..=bottom).contains(&line) {
        return None;
    }
    let mut start = line;
    while start > top && wrapped(engine, start - 1) {
        start -= 1;
    }
    let mut end = line;
    while end < bottom && wrapped(engine, end) {
        end += 1;
    }
    Some(TermyEngineTextRange {
        start_line: start,
        start_col: 0,
        end_line: end,
        end_col: line_len(engine, end).saturating_sub(1) as u32,
        rectangular: 0,
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_read_text(
    engine: *mut TermyEngine,
    range: *const TermyEngineTextRange,
    format: u32,
    options: u32,
    out_text: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a writable buffer struct.
        let Some(out) = (unsafe { out_text.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        if format > FORMAT_HTML || options & !(TEXT_UNWRAP | TEXT_TRIM) != 0 {
            return TermyEngineStatus::InvalidArgument;
        }
        let mut colors = TermyEngineColors {
            foreground: 0,
            background: 0,
            cursor: 0,
            cursor_set: false,
            reverse_video: false,
            palette: [0; 256],
        };
        // SAFETY: null or a live handle; `colors` is a valid out struct.
        let status = unsafe { termy_engine_effective_colors(engine, &raw mut colors) };
        if status != TermyEngineStatus::Ok {
            return status;
        }
        // SAFETY: checked by the colors call above.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a valid range.
        let range = unsafe { range.as_ref() }.copied().unwrap_or_else(|| {
            // The whole buffer, for captures.
            let (top, bottom) = line_bounds(&engine.engine);
            TermyEngineTextRange {
                start_line: top,
                start_col: 0,
                end_line: bottom,
                end_col: u32::MAX,
                rectangular: 0,
            }
        });
        let text = read_text(&engine.engine, Some(&colors), range, format, options);
        *out = TermyEngineBytes::from_vec(text.into_bytes());
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_select_word(
    engine: *mut TermyEngine,
    line: i64,
    col: u32,
    out_range: *mut TermyEngineTextRange,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable struct.
        let Some(out) = (unsafe { out_range.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        match select_word(&engine.engine, line, col as usize) {
            Some(range) => {
                *out = range;
                TermyEngineStatus::Ok
            }
            None => TermyEngineStatus::InvalidArgument,
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_select_line(
    engine: *mut TermyEngine,
    line: i64,
    out_range: *mut TermyEngineTextRange,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable struct.
        let Some(out) = (unsafe { out_range.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        match select_line(&engine.engine, line) {
            Some(range) => {
                *out = range;
                TermyEngineStatus::Ok
            }
            None => TermyEngineStatus::InvalidArgument,
        }
    })
}
