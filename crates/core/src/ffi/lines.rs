//! Buffer lines by number: the range a terminal holds, the lines OSC 133
//! shell integration marked, and the text of a range of lines. Lines are
//! numbered as by `Terminal::visit_line_cells`: 0 is the top screen row, -1
//! the newest history row.

use super::{
    TermyFfiBytes, TermyFfiStatus, TermyFfiTerminal, ffi_bytes_from_string, ffi_status_guard,
    inspect::{TextFormat, format_line},
    leak_vec,
};
use crate::TerminalRenderCell;

/// A line OSC 133 marked; see `termy_terminal_prompt_marks`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiPromptMark {
    pub line: i32,
    /// `TERMY_PROMPT_MARK_PROMPT` and/or `TERMY_PROMPT_MARK_OUTPUT`.
    pub kinds: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyFfiPromptMarkBatch {
    pub marks_ptr: *mut TermyFfiPromptMark,
    pub marks_len: usize,
    pub marks_capacity: usize,
}

/// OSC 133;A: the shell's prompt starts on the line.
pub const TERMY_PROMPT_MARK_PROMPT: u8 = crate::remote::PromptMark::PROMPT;
/// OSC 133;C: the command's output starts on the line.
pub const TERMY_PROMPT_MARK_OUTPUT: u8 = crate::remote::PromptMark::OUTPUT;

/// The first and last line of the buffer, history included.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_line_bounds(
    terminal: *mut TermyFfiTerminal,
    out_first: *mut i32,
    out_last: *mut i32,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_first.is_null() || out_last.is_null() {
            return TermyFfiStatus::Null;
        }
        let (first, last) = unsafe { (*terminal).terminal.line_bounds() };
        unsafe {
            *out_first = first;
            *out_last = last;
        }
        TermyFfiStatus::Ok
    })
}

/// The lines of `first..=last` that OSC 133;A or 133;C marked, oldest first.
/// `*out_supported` is false, with no marks, when the terminal cannot tell:
/// it views a pane of a session host of another build. Free the batch with
/// `termy_prompt_mark_batch_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_prompt_marks(
    terminal: *mut TermyFfiTerminal,
    first: i32,
    last: i32,
    out_supported: *mut bool,
    out_batch: *mut TermyFfiPromptMarkBatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_supported.is_null() || out_batch.is_null() {
            return TermyFfiStatus::Null;
        }
        let marks = match unsafe { (*terminal).terminal.prompt_marks(first, last) } {
            Ok(marks) => marks,
            Err(error) => return super::multiplexer::status_for(&error),
        };
        let supported = marks.is_some();
        let marks: Vec<_> = marks
            .unwrap_or_default()
            .into_iter()
            .map(|mark| TermyFfiPromptMark {
                line: mark.line,
                kinds: mark.kinds,
            })
            .collect();
        let (marks_ptr, marks_len, marks_capacity) = leak_vec(marks);
        unsafe {
            *out_supported = supported;
            *out_batch = TermyFfiPromptMarkBatch {
                marks_ptr,
                marks_len,
                marks_capacity,
            };
        }
        TermyFfiStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_prompt_mark_batch_free(
    batch: *mut TermyFfiPromptMarkBatch,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if batch.is_null() {
            return TermyFfiStatus::Null;
        }
        let batch = unsafe { &mut *batch };
        if !batch.marks_ptr.is_null() {
            drop(unsafe {
                Vec::from_raw_parts(batch.marks_ptr, batch.marks_len, batch.marks_capacity)
            });
        }
        *batch = TermyFfiPromptMarkBatch::default();
        TermyFfiStatus::Ok
    })
}

/// The text of lines `first..=last` (clamped to the buffer): a line the
/// terminal soft-wrapped joins the next without a newline, every other line
/// ends with one (none after the last), and trailing blanks of each line are
/// dropped. Free with `termy_buffer_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_terminal_line_range_text(
    terminal: *mut TermyFfiTerminal,
    first: i32,
    last: i32,
    out_text: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if terminal.is_null() || out_text.is_null() {
            return TermyFfiStatus::Null;
        }
        let terminal = unsafe { &(*terminal).terminal };
        let mut lines: Vec<Vec<TerminalRenderCell>> = Vec::new();
        if first <= last {
            let mut current = None;
            terminal.visit_line_cells(first, last, |_, line, _, cell| {
                if current != Some(line) {
                    current = Some(line);
                    lines.push(Vec::new());
                }
                if let Some(cells) = lines.last_mut() {
                    cells.push(cell.clone());
                }
            });
        }
        unsafe { *out_text = ffi_bytes_from_string(join_lines(&lines)) };
        TermyFfiStatus::Ok
    })
}

/// `lines` as `termy_terminal_line_range_text` writes them.
fn join_lines(lines: &[Vec<TerminalRenderCell>]) -> String {
    let mut out = String::new();
    for (index, cells) in lines.iter().enumerate() {
        let last = index + 1 == lines.len();
        // A soft-wrapped line keeps its trailing spaces: the next line
        // continues it.
        let joins = !last && cells.last().is_some_and(|cell| cell.line_wrapped);
        let text = format_line(cells, TextFormat::Plain, joins);
        if joins {
            out.push_str(&text);
            continue;
        }
        out.push_str(text.trim_end_matches(' '));
        if !last {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    fn display_terminal(cols: u16, rows: u16) -> *mut TermyFfiTerminal {
        let mut terminal = ptr::null_mut();
        let size = TermyFfiSize {
            cols,
            rows,
            ..termy_size_default()
        };
        assert_eq!(
            unsafe { termy_display_terminal_new(size, &mut terminal) },
            TermyFfiStatus::Ok
        );
        terminal
    }

    fn feed(terminal: *mut TermyFfiTerminal, bytes: &[u8]) {
        assert_eq!(
            unsafe { termy_terminal_feed_output(terminal, bytes.as_ptr(), bytes.len()) },
            TermyFfiStatus::Ok
        );
    }

    fn text(terminal: *mut TermyFfiTerminal, first: i32, last: i32) -> String {
        let mut bytes = termy_null_buffer();
        assert_eq!(
            unsafe { termy_terminal_line_range_text(terminal, first, last, &mut bytes) },
            TermyFfiStatus::Ok
        );
        let text = if bytes.ptr.is_null() {
            String::new()
        } else {
            String::from_utf8(unsafe { slice::from_raw_parts(bytes.ptr, bytes.len) }.to_vec())
                .unwrap()
        };
        unsafe { termy_buffer_free(bytes) };
        text
    }

    #[test]
    fn prompt_marks_bounds_and_text_read_lines_by_number() {
        let terminal = display_terminal(10, 3);
        feed(
            terminal,
            b"\x1b]133;A\x07$ ls\r\n\x1b]133;C\x07one\r\n0123456789abc\r\n\x1b]133;A\x07$ ",
        );
        // Five rows on a three-row screen.
        let (mut first, mut last) = (0, 0);
        assert_eq!(
            unsafe { termy_terminal_line_bounds(terminal, &mut first, &mut last) },
            TermyFfiStatus::Ok
        );
        assert_eq!((first, last), (-2, 2));

        let mut supported = false;
        let mut batch = TermyFfiPromptMarkBatch::default();
        assert_eq!(
            unsafe {
                termy_terminal_prompt_marks(terminal, first, last, &mut supported, &mut batch)
            },
            TermyFfiStatus::Ok
        );
        assert!(supported);
        let marks = unsafe { slice::from_raw_parts(batch.marks_ptr, batch.marks_len) };
        assert_eq!(
            marks,
            [
                TermyFfiPromptMark {
                    line: -2,
                    kinds: TERMY_PROMPT_MARK_PROMPT,
                },
                TermyFfiPromptMark {
                    line: -1,
                    kinds: TERMY_PROMPT_MARK_OUTPUT,
                },
                TermyFfiPromptMark {
                    line: 2,
                    kinds: TERMY_PROMPT_MARK_PROMPT,
                },
            ]
        );
        assert_eq!(
            unsafe { termy_prompt_mark_batch_free(&mut batch) },
            TermyFfiStatus::Ok
        );
        assert!(batch.marks_ptr.is_null());

        // The wrapped line joins; the last line has no newline.
        assert_eq!(text(terminal, -2, 2), "$ ls\none\n0123456789abc\n$");
        assert_eq!(text(terminal, -1, 0), "one\n0123456789");
        assert_eq!(text(terminal, -50, -2), "$ ls");
        assert_eq!(text(terminal, 3, 9), "");
        assert_eq!(
            unsafe { termy_terminal_prompt_marks(terminal, 0, 0, ptr::null_mut(), &mut batch) },
            TermyFfiStatus::Null
        );
        unsafe { termy_terminal_free(terminal) };
    }

    #[test]
    fn blank_lines_keep_their_newlines() {
        let terminal = display_terminal(10, 4);
        feed(terminal, b"a   \r\n\r\nb");
        assert_eq!(text(terminal, 0, 3), "a\n\nb\n");
        unsafe { termy_terminal_free(terminal) };
    }
}
