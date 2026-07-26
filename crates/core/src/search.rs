use crate::frame::TermyFrame;
use std::sync::Arc;
use termy_search::{SearchConfig, SearchEngine, SearchMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermySearchMatch {
    pub row: usize,
    pub start_col: usize,
    /// Inclusive end column for Swift/FFI consumers.
    pub end_col: usize,
    pub line: String,
}

/// Allocation-efficient search result whose line text is shared by matches on
/// the same row. Use this for large result sets; [`TermySearchMatch`] remains
/// available for callers that require independently owned strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermySharedSearchMatch {
    pub row: usize,
    pub start_col: usize,
    /// Inclusive end column for Swift/FFI consumers.
    pub end_col: usize,
    /// Shared by every match on this row so common queries do not duplicate
    /// the complete line for each match.
    pub line: Arc<String>,
}

impl From<TermySharedSearchMatch> for TermySearchMatch {
    fn from(search_match: TermySharedSearchMatch) -> Self {
        Self {
            row: search_match.row,
            start_col: search_match.start_col,
            end_col: search_match.end_col,
            line: Arc::unwrap_or_clone(search_match.line),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermySearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
}

pub fn search_frame(frame: &TermyFrame, query: &str) -> Vec<TermySearchMatch> {
    search_frame_with_options(frame, query, TermySearchOptions::default())
}

pub fn search_frame_with_options(
    frame: &TermyFrame,
    query: &str,
    options: TermySearchOptions,
) -> Vec<TermySearchMatch> {
    search_frame_shared_with_options(frame, query, options)
        .into_iter()
        .map(Into::into)
        .collect()
}

pub fn search_frame_shared(frame: &TermyFrame, query: &str) -> Vec<TermySharedSearchMatch> {
    search_frame_shared_with_options(frame, query, TermySearchOptions::default())
}

pub fn search_frame_shared_with_options(
    frame: &TermyFrame,
    query: &str,
    options: TermySearchOptions,
) -> Vec<TermySharedSearchMatch> {
    if query.is_empty() || frame.cols == 0 {
        return Vec::new();
    }

    let cols = usize::from(frame.cols);
    let rows = usize::from(frame.rows);
    search_lines_shared(
        (0..rows).map(|row| (row, line_text(frame, row, cols))),
        query,
        options,
    )
}

pub(crate) fn search_lines_shared(
    lines: impl IntoIterator<Item = (usize, String)>,
    query: &str,
    options: TermySearchOptions,
) -> Vec<TermySharedSearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }

    let mut engine = SearchEngine::new(SearchConfig {
        case_sensitive: options.case_sensitive,
        mode: if options.regex {
            SearchMode::Regex
        } else {
            SearchMode::Literal
        },
    });
    if engine.set_pattern(query).is_err() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for (row, line) in lines {
        let line_matches = engine.search_line(row as i32, &line);
        if line_matches.is_empty() {
            continue;
        }

        let line = Arc::new(line);
        for search_match in line_matches {
            if search_match.end_col <= search_match.start_col {
                continue;
            }
            matches.push(TermySharedSearchMatch {
                row,
                start_col: search_match.start_col,
                end_col: search_match.end_col.saturating_sub(1),
                line: Arc::clone(&line),
            });
        }
    }

    matches
}

fn line_text(frame: &TermyFrame, row: usize, cols: usize) -> String {
    let start = row.saturating_mul(cols);
    let end = start.saturating_add(cols);
    if end > frame.cells.len() {
        return String::new();
    }

    let mut text = frame.cells[start..end]
        .iter()
        .map(|cell| if cell.render_text { cell.char } else { ' ' })
        .collect::<String>();
    let trimmed_len = text.trim_end().len();
    text.truncate(trimmed_len);
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{TermyCell, TermyColor};

    #[test]
    fn searches_visible_frame_rows_case_insensitively() {
        let frame = frame_from_rows(12, &["hello", "world hello"]);

        let matches = search_frame(&frame, "HELLO");

        assert_eq!(
            matches,
            vec![
                TermySearchMatch {
                    row: 0,
                    start_col: 0,
                    end_col: 4,
                    line: "hello".to_string(),
                },
                TermySearchMatch {
                    row: 1,
                    start_col: 6,
                    end_col: 10,
                    line: "world hello".to_string(),
                },
            ]
        );
    }

    #[test]
    fn matches_on_the_same_row_share_line_text() {
        let frame = frame_from_rows(20, &["needle needle", "needle"]);

        let matches = search_frame_shared(&frame, "needle");

        assert_eq!(matches.len(), 3);
        assert!(Arc::ptr_eq(&matches[0].line, &matches[1].line));
        assert!(!Arc::ptr_eq(&matches[0].line, &matches[2].line));
    }

    #[test]
    fn empty_query_returns_no_matches() {
        assert!(search_frame(&frame_from_rows(4, &["abc"]), "").is_empty());
    }

    #[test]
    fn search_options_can_enable_case_sensitive_matching() {
        let frame = frame_from_rows(12, &["Hello HELLO"]);

        let matches = search_frame_with_options(
            &frame,
            "HELLO",
            TermySearchOptions {
                case_sensitive: true,
                regex: false,
            },
        );

        assert_eq!(
            matches,
            vec![TermySearchMatch {
                row: 0,
                start_col: 6,
                end_col: 10,
                line: "Hello HELLO".to_string(),
            }]
        );
    }

    #[test]
    fn search_options_can_enable_regex_matching() {
        let frame = frame_from_rows(16, &["foo 123 bar"]);

        let matches = search_frame_with_options(
            &frame,
            r"\d+",
            TermySearchOptions {
                case_sensitive: false,
                regex: true,
            },
        );

        assert_eq!(
            matches,
            vec![TermySearchMatch {
                row: 0,
                start_col: 4,
                end_col: 6,
                line: "foo 123 bar".to_string(),
            }]
        );
    }

    #[test]
    fn invalid_regex_returns_no_matches() {
        let frame = frame_from_rows(12, &["hello"]);

        let matches = search_frame_with_options(
            &frame,
            "[",
            TermySearchOptions {
                case_sensitive: false,
                regex: true,
            },
        );

        assert!(matches.is_empty());
    }

    fn frame_from_rows(cols: u16, rows: &[&str]) -> TermyFrame {
        let color = TermyColor {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        };
        let mut cells = Vec::new();
        for text in rows {
            let mut chars = text.chars();
            for _ in 0..usize::from(cols) {
                let char = chars.next().unwrap_or(' ');
                cells.push(TermyCell {
                    char,
                    fg: color,
                    bg: TermyColor::default(),
                    uses_terminal_default_bg: true,
                    bold: false,
                    italic: false,
                    underline: false,
                    strikethrough: false,
                    render_text: char != ' ',
                    wide_character_spacer: false,
                    line_wrapped: false,
                });
            }
        }

        TermyFrame {
            cols,
            rows: rows.len() as u16,
            cells,
            cursor: None,
            display_offset: 0,
            history_size: 0,
        }
    }
}
