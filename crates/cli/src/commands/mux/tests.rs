use super::*;

fn bytes(spec: &str) -> Vec<u8> {
    encode_key(
        spec,
        TermyModifiers::default(),
        TerminalKeyboardMode::default(),
    )
    .unwrap()
}

/// Audit B25: every name and chord has one encoding, on every platform.
#[test]
fn keys_encode_the_same_on_every_platform() {
    for (spec, expected) in [
        ("enter", &b"\r"[..]),
        ("Return", b"\r"),
        ("ESC", b"\x1b"),
        ("bs", b"\x7f"),
        ("Tab", b"\t"),
        ("shift+tab", b"\x1b[Z"),
        ("space", b" "),
        ("PgUp", b"\x1b[5~"),
        ("pgdn", b"\x1b[6~"),
        ("del", b"\x1b[3~"),
        ("up", b"\x1b[A"),
        ("f5", b"\x1b[15~"),
        ("c", b"c"),
        ("C", b"C"),
        ("shift+c", b"C"),
        ("ctrl+c", b"\x03"),
        ("CTRL+C", b"\x03"),
        ("ctrl+left", b"\x1b[1;5D"),
        ("alt+left", b"\x1b[1;3D"),
        ("ctrl+backspace", b"\x7f"),
        ("alt+b", b"\x1bb"),
        ("alt+B", b"\x1bB"),
        ("alt+enter", b"\x1b\r"),
        ("ctrl+alt+x", b"\x1b\x18"),
        ("+", b"+"),
        ("alt++", b"\x1b+"),
        ("-", b"-"),
        ("é", "é".as_bytes()),
    ] {
        assert_eq!(bytes(spec), expected, "{spec}");
    }
}

#[test]
fn held_modifiers_apply_to_every_key() {
    let control = TermyModifiers {
        control: true,
        ..TermyModifiers::default()
    };
    let encode = |spec| encode_key(spec, control, TerminalKeyboardMode::default()).unwrap();
    assert_eq!(encode("c"), b"\x03");
    assert_eq!(encode("alt+x"), b"\x1b\x18");
    let shift = TermyModifiers {
        shift: true,
        ..TermyModifiers::default()
    };
    assert_eq!(
        encode_key("a", shift, TerminalKeyboardMode::default()).unwrap(),
        b"A"
    );
}

#[test]
fn keys_follow_the_kitty_keyboard_protocol_when_enabled() {
    let disambiguate = TerminalKeyboardMode::from_flags(false, true, false, false, false, false);
    let encode = |spec| encode_key(spec, TermyModifiers::default(), disambiguate).unwrap();
    assert_eq!(encode("ctrl+c"), b"\x1b[99;5u");
    assert_eq!(encode("alt+b"), b"\x1b[98;3u");
    assert_eq!(encode("esc"), b"\x1b[27u");
}

#[test]
fn unknown_keys_and_modifiers_are_invalid_arguments() {
    for spec in ["fn", "hyper+a", "ctrl+", "f13"] {
        let error = encode_key(
            spec,
            TermyModifiers::default(),
            TerminalKeyboardMode::default(),
        )
        .unwrap_err();
        assert_eq!(Code::of(&error), Code::InvalidArgument, "{spec}: {error}");
    }
}

fn row(text: &str, wrapped: bool) -> LineText {
    LineText {
        text: text.into(),
        columns: vec![1; text.len()],
        wrapped,
    }
}

/// Lines numbered as when 10 lines had scrolled, read from a 4-row screen
/// after `scrolled` had.
fn texts(first: i32, scrolled: u64, rows: Vec<LineText>) -> LineTexts {
    let moved = scrolled as i32 - 10;
    LineTexts {
        bounds: (-100 + moved, 3 + moved, 6),
        first,
        lines_scrolled: 10,
        scrolled,
        rows: 4,
        lines: rows,
    }
}

#[test]
fn soft_wrapped_rows_join_into_one_line() {
    let joined = join(
        -1,
        &[row("ab cd", true), row("ef", false), row("gh", false)],
        6,
    );
    // The blank at the wrap belongs to the line.
    assert_eq!(joined.text, "ab cd ef\ngh");
    assert_eq!(joined.rows, [(0, -1), (6, 0), (9, 1)]);
}

/// Audit B18: a marker wider than the pane is found across its wrap.
#[test]
fn markers_longer_than_a_row_match_across_wraps() {
    let start = texts(0, 10, vec![row("$ run", false), row("", false)]);
    let mut search = Search::new("READY_NOW", 0, &start, false);
    assert_eq!(search.scan(start.clone()), None);
    let found = search
        .scan(texts(
            0,
            10,
            vec![row("$ run", false), row("READY_", true), row("NOW", false)],
        ))
        .unwrap();
    assert_eq!(
        found,
        Found {
            line: 1,
            text: "READY_NOW".into()
        }
    );
}

/// Audit B18: output that scrolled into history between two reads is
/// still searched, and kept rows join the rows read after them.
#[test]
fn markers_that_scrolled_into_history_between_reads_match() {
    let start = texts(0, 10, vec![row("$ ", false)]);
    let mut search = Search::new("MARK_ER", 0, &start, false);
    assert_eq!(search.scan(start.clone()), None);
    assert_eq!(search.next_line(), 0);
    // Ten lines scrolled: the marker is now history (line 2 is -8).
    let mut rows = vec![row("$ ", false), row("x", false), row("MARK_ER", false)];
    rows.extend((0..12).map(|_| row("y", false)));
    let found = search.scan(texts(0, 20, rows)).unwrap();
    assert_eq!(
        found,
        Found {
            line: -8,
            text: "MARK_ER".into()
        }
    );

    // A marker split across the frozen rows and a later read.
    let start = texts(0, 10, vec![row("", false)]);
    let mut search = Search::new("SPLIT_MARK", 0, &start, false);
    let mut rows: Vec<_> = (0..10).map(|_| row("z", false)).collect();
    rows.push(row("SPLIT_", true));
    assert_eq!(search.scan(texts(0, 21, rows)), None);
    // Only the rows the marker could still continue from are kept, and
    // the next read starts after them.
    assert_eq!(search.kept_first, 8);
    assert_eq!(search.next_line(), 11);
    let found = search
        .scan(texts(11, 21, vec![row("MARK", false)]))
        .unwrap();
    assert_eq!(
        found,
        Found {
            line: -1,
            text: "SPLIT_MARK".into()
        }
    );
}

/// A wide character wrapped to the next row leaves a spacer that is no
/// text, so the joined line has no gap.
#[test]
fn wide_characters_join_across_a_wrap() {
    let wrapped = LineText {
        text: "ab日".into(),
        columns: vec![1, 1, 3, 0, 0],
        wrapped: true,
    };
    let rest = LineText {
        text: "本語".into(),
        columns: vec![3, 0, 3, 0],
        wrapped: false,
    };
    assert_eq!(join(0, &[wrapped, rest], 5).text, "ab日本語");
}

/// Audit B18: output that left the scrollback before it was read is
/// reported, both between reads and before the first one of `--after`.
#[test]
fn output_evicted_before_it_was_read_is_reported() {
    let start = texts(0, 10, vec![row("$ ", false)]);
    let mut search = Search::new("NEVER", 0, &start, false);
    assert_eq!(search.scan(start.clone()), None);
    assert!(!search.lost);
    // Lines 0..5 scrolled away and were dropped before this read.
    assert_eq!(search.scan(texts(5, 40, vec![row("y", false)])), None);
    assert!(search.lost);

    let evicted = texts(3, 40, vec![row("y", false)]);
    assert!(Search::new("NEVER", 0, &evicted, true).lost);
    // History that never held the lines a screen origin asks for.
    assert!(!Search::new("NEVER", -50, &evicted, false).lost);
}

/// Audit B18: a resize reflows lines, so the search restarts from the
/// current screen with the history output added since it began.
#[test]
fn a_resize_restarts_the_search() {
    let start = texts(-2, 10, vec![row("$ ", false)]);
    let search = Search::new("X", -2, &start, false);
    let mut later = texts(0, 25, vec![row("y", false)]);
    assert!(!search.resized(&later));
    later.rows = 5;
    assert!(search.resized(&later));
    assert_eq!(search.restart_line(&later), -17);
}
