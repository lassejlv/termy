use std::{fs, path::PathBuf};

use engine::{CursorShape, Terminal, TerminalConfig};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Corpus {
    streams: Vec<StreamFixture>,
}

#[derive(Debug, Deserialize)]
struct StreamFixture {
    id: String,
    origin: String,
    columns: usize,
    rows: usize,
    chunks: Vec<String>,
    expected_cells: Vec<ExpectedCell>,
    cursor: ExpectedCursor,
}

#[derive(Debug, Deserialize)]
struct ExpectedCell {
    row: usize,
    column: usize,
    text: String,
}

#[derive(Debug, Deserialize)]
struct ExpectedCursor {
    row: usize,
    column: usize,
    shape: String,
    blinking: bool,
}

#[test]
fn external_terminal_stream_regressions_keep_exact_cell_and_cursor_alignment() {
    let path = fixture_path();
    let source = fs::read_to_string(&path).expect("read terminal stream fixture corpus");
    let corpus: Corpus =
        serde_json::from_str(&source).expect("parse terminal stream fixture corpus");

    for fixture in corpus.streams {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: fixture.columns,
            rows: fixture.rows,
            scrollback_limit: 0,
        });
        for chunk in &fixture.chunks {
            terminal.feed(chunk.as_bytes());
        }
        let frame = terminal.frame_update(true);

        for expected in fixture.expected_cells {
            let row = frame
                .row_updates
                .iter()
                .find(|row| row.index == expected.row)
                .unwrap_or_else(|| panic!("{}: missing row {}", fixture.id, expected.row));
            let cell = row
                .cells
                .get(expected.column)
                .unwrap_or_else(|| panic!("{}: missing column {}", fixture.id, expected.column));
            assert_eq!(
                cell.text, expected.text,
                "{}: {} at row {}, column {}",
                fixture.id, fixture.origin, expected.row, expected.column
            );
        }

        assert_eq!(
            frame.cursor.row, fixture.cursor.row,
            "{} cursor row",
            fixture.id
        );
        assert_eq!(
            frame.cursor.column, fixture.cursor.column,
            "{} cursor column",
            fixture.id
        );
        assert_eq!(
            frame.cursor.shape,
            parse_cursor_shape(&fixture.cursor.shape),
            "{} cursor shape",
            fixture.id
        );
        assert_eq!(
            frame.cursor.blinking, fixture.cursor.blinking,
            "{} cursor blink state",
            fixture.id
        );
    }
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/terminal-streams/streams.json")
}

fn parse_cursor_shape(shape: &str) -> CursorShape {
    match shape {
        "block" => CursorShape::Block,
        "underline" => CursorShape::Underline,
        "bar" => CursorShape::Bar,
        other => panic!("unknown fixture cursor shape {other:?}"),
    }
}
