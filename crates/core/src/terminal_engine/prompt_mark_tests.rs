//! OSC 133 prompt marks on rows: where they land, and how they follow rows
//! into history, through reflow and out again when rows are reused.

use super::prompt_mark::{OUTPUT, PROMPT};
use super::*;

const A: &[u8] = b"\x1b]133;A\x07";
const C: &[u8] = b"\x1b]133;C\x07";

fn engine(cols: usize, rows: usize, history: usize) -> Engine {
    Engine::new(
        Size { cols, rows },
        Options {
            scrollback_history: history,
            ..Default::default()
        },
    )
}

fn feed(engine: &mut Engine, parts: &[&[u8]]) {
    for part in parts {
        engine.feed(part);
    }
}

fn all_marks(engine: &Engine) -> Vec<(i32, u8)> {
    engine.prompt_marks(i32::MIN, i32::MAX)
}

fn line_text(engine: &Engine, line: i32) -> String {
    let text: String = engine
        .line(line)
        .unwrap()
        .iter()
        .map(|cell| cell.character)
        .collect();
    text.trim_end().to_owned()
}

#[test]
fn prompt_and_output_marks_land_on_the_cursor_row() {
    let mut engine = engine(20, 5, 100);
    feed(
        &mut engine,
        &[A, b"$ ls\r\n", C, b"file\r\n", b"\x1b[3;5H", A, C],
    );
    assert_eq!(engine.line_prompt_marks(0), PROMPT);
    assert_eq!(engine.line_prompt_marks(1), OUTPUT);
    // One row can carry both.
    assert_eq!(engine.line_prompt_marks(2), PROMPT | OUTPUT);
    assert_eq!(engine.line_prompt_marks(3), 0);
    // Other OSC 133 commands leave no mark.
    feed(&mut engine, &[b"\x1b[4;1H\x1b]133;B\x07\x1b]133;D;0\x07"]);
    assert_eq!(engine.line_prompt_marks(3), 0);
    assert_eq!(
        all_marks(&engine),
        [(0, PROMPT), (1, OUTPUT), (2, PROMPT | OUTPUT)]
    );
}

#[test]
fn marks_follow_rows_into_history_and_through_packing() {
    let mut engine = engine(20, 3, 100);
    feed(
        &mut engine,
        &[A, b"$ make\r\n", C, b"one\r\ntwo\r\nthree\r\n"],
    );
    feed(&mut engine, &[b"four\r\nfive\r\n"]);
    // Four of seven rows scrolled off a three-row screen.
    assert_eq!(engine.history_size(), 4);
    assert_eq!(all_marks(&engine), [(-4, PROMPT), (-3, OUTPUT)]);
    assert_eq!(line_text(&engine, -4), "$ make");
    // Packing cold history keeps the marks.
    engine.compact_history();
    assert_eq!(all_marks(&engine), [(-4, PROMPT), (-3, OUTPUT)]);
    // A flood of output scrolls them further.
    for _ in 0..40 {
        engine.feed(b"line of output\r\n");
    }
    assert_eq!(all_marks(&engine), [(-44, PROMPT), (-43, OUTPUT)]);
    assert_eq!(line_text(&engine, -44), "$ make");
}

#[test]
fn marks_are_cleared_when_an_evicted_row_is_reused() {
    for cols in [10, 20] {
        let mut engine = engine(cols, 2, 2);
        feed(&mut engine, &[A, b"$ x\r\n", C, b"y\r\n"]);
        assert_eq!(all_marks(&engine).len(), 2);
        for _ in 0..12 {
            engine.feed(b"z\r\n");
        }
        assert_eq!(all_marks(&engine), [], "{cols} columns");
        engine.compact_history();
        for _ in 0..12 {
            engine.feed(b"w\r\n");
        }
        assert_eq!(all_marks(&engine), [], "{cols} columns");
    }
}

#[test]
fn marks_survive_a_reflow_that_rewraps_their_line() {
    let mut engine = engine(20, 6, 100);
    feed(
        &mut engine,
        &[A, b"$ abcdefghijklmnop\r\n", C, b"output\r\n", A, b"$ "],
    );
    assert_eq!(all_marks(&engine), [(0, PROMPT), (1, OUTPUT), (2, PROMPT)]);

    // Narrower: the first line takes two rows, and only its first is marked.
    engine.resize(Size { cols: 10, rows: 6 });
    assert_eq!(line_text(&engine, 0), "$ abcdefgh");
    assert!(engine.line_wrapped(0));
    assert_eq!(all_marks(&engine), [(0, PROMPT), (2, OUTPUT), (3, PROMPT)]);

    // Wider again: the line joins, and the marks move with their rows.
    engine.resize(Size { cols: 20, rows: 6 });
    assert_eq!(line_text(&engine, 0), "$ abcdefghijklmnop");
    assert_eq!(all_marks(&engine), [(0, PROMPT), (1, OUTPUT), (2, PROMPT)]);
}

#[test]
fn a_mark_on_a_continuation_row_joins_the_line_it_continues() {
    let mut engine = engine(10, 4, 100);
    // Wraps onto the second row, where the mark is set.
    feed(&mut engine, &[b"0123456789ab", C, b"\r\nnext"]);
    assert_eq!(all_marks(&engine), [(1, OUTPUT)]);
    engine.resize(Size { cols: 20, rows: 4 });
    assert_eq!(line_text(&engine, 0), "0123456789ab");
    assert_eq!(all_marks(&engine), [(0, OUTPUT)]);
}

#[test]
fn marks_in_history_survive_reflow() {
    let mut engine = engine(20, 3, 100);
    feed(&mut engine, &[A, b"$ abcdefghijklmnop\r\n", C, b"out\r\n"]);
    for _ in 0..4 {
        engine.feed(b"x\r\n");
    }
    assert_eq!(all_marks(&engine), [(-4, PROMPT), (-3, OUTPUT)]);
    engine.compact_history();
    engine.resize(Size { cols: 10, rows: 3 });
    assert_eq!(all_marks(&engine), [(-5, PROMPT), (-3, OUTPUT)]);
    assert_eq!(line_text(&engine, -5), "$ abcdefgh");
    engine.resize(Size { cols: 30, rows: 3 });
    assert_eq!(all_marks(&engine), [(-4, PROMPT), (-3, OUTPUT)]);
    assert_eq!(line_text(&engine, -4), "$ abcdefghijklmnop");
}

#[test]
fn the_alternate_screen_is_never_marked() {
    let mut engine = engine(20, 4, 100);
    feed(&mut engine, &[A, b"$ vim\r\n"]);
    feed(&mut engine, &[b"\x1b[?1049h", A, b"editor", C, b"\r\n", A]);
    assert!(engine.alternate_screen());
    assert_eq!(all_marks(&engine), []);
    feed(&mut engine, &[b"\x1b[?1049l"]);
    // The primary screen kept its mark, and gained none.
    assert_eq!(all_marks(&engine), [(0, PROMPT)]);
}

#[test]
fn ranges_report_marked_lines_by_number() {
    let mut engine = engine(20, 3, 100);
    for command in ["a", "b", "c"] {
        feed(
            &mut engine,
            &[A, b"$ ", command.as_bytes(), b"\r\n", C, b"out\r\n"],
        );
    }
    feed(&mut engine, &[A, b"$ "]);
    // Six rows, one live prompt: four scrolled into history.
    assert_eq!(engine.history_size(), 4);
    assert_eq!(
        all_marks(&engine),
        [
            (-4, PROMPT),
            (-3, OUTPUT),
            (-2, PROMPT),
            (-1, OUTPUT),
            (0, PROMPT),
            (1, OUTPUT),
            (2, PROMPT),
        ]
    );
    assert_eq!(engine.prompt_marks(-3, -2), [(-3, OUTPUT), (-2, PROMPT)]);
    assert_eq!(engine.prompt_marks(-1, 0), [(-1, OUTPUT), (0, PROMPT)]);
    assert_eq!(engine.prompt_marks(2, 50), [(2, PROMPT)]);
    assert_eq!(engine.prompt_marks(-100, -4), [(-4, PROMPT)]);
    assert_eq!(engine.prompt_marks(1, 0), []);
    assert_eq!(engine.prompt_marks(3, 9), []);
    assert_eq!(line_text(&engine, -2), "$ b");
}

#[test]
fn snapshots_drop_marks() {
    let mut engine = engine(20, 3, 100);
    feed(&mut engine, &[A, b"$ a\r\n", C, b"out\r\nmore\r\nend"]);
    assert_eq!(all_marks(&engine).len(), 2);
    let restored = Engine::decode_snapshot(&engine.encode_snapshot()).unwrap();
    assert_eq!(line_text(&restored, -1), "$ a");
    assert_eq!(all_marks(&restored), []);
}
