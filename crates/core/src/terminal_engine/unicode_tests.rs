use super::{Cell, Engine, Options, Size};

fn engine(cols: usize) -> Engine {
    Engine::new(Size { cols, rows: 4 }, Options::default())
}

fn cell_text(cell: &Cell) -> String {
    format!("{}{}", cell.character, cell.combining())
}

#[test]
fn grapheme_sequences_have_the_same_width_at_every_input_split() {
    for (text, width) in [
        ("你好世界", 8),
        ("日本語かなカナ", 14),
        ("한글", 4),
        ("ｶﾀｶﾅ", 4),
        ("か\u{3099}", 2),
        ("e\u{301}", 1),
        ("👨‍👩‍👧‍👦", 2),
        ("❤️", 2),
        ("👩🏽‍💻", 2),
        ("🇩🇰", 2),
        ("1️⃣", 2),
        ("한", 2),
    ] {
        for split in 0..=text.len() {
            let mut term = engine(40);
            term.feed(&text.as_bytes()[..split]);
            term.feed(&text.as_bytes()[split..]);
            term.feed(b"X");
            assert_eq!(
                term.viewport_row(0).unwrap()[width].character,
                'X',
                "{text:?}, split {split}"
            );
        }
        let mut term = engine(40);
        for byte in text.as_bytes() {
            term.feed(&[*byte]);
        }
        term.feed(b"X");
        assert_eq!(
            term.viewport_row(0).unwrap()[width].character,
            'X',
            "bytewise {text:?}"
        );
    }
}

#[test]
fn emoji_promotion_wraps_and_survives_one_column_reflow() {
    let mut term = engine(4);
    term.feed("abc❤️X".as_bytes());
    assert_eq!(
        term.viewport_row(0).unwrap()[3].flags,
        Cell::LEADING_WIDE_SPACER
    );
    assert_eq!(cell_text(&term.viewport_row(1).unwrap()[0]), "❤️");
    assert_eq!(term.viewport_row(1).unwrap()[2].character, 'X');
    term.resize(Size { cols: 1, rows: 10 });
    term.resize(Size { cols: 8, rows: 4 });
    let row = term.viewport_row(0).unwrap();
    assert_eq!(cell_text(&row[3]), "❤️");
    assert_eq!(row[3].flags, Cell::WIDE);
    assert_eq!(row[4].flags, Cell::WIDE_SPACER);
    assert_eq!(row[5].character, 'X');
}

#[test]
fn deleting_or_overwriting_an_emoji_half_clears_the_whole_cluster() {
    for edit in ["\x1b[2G\x1b[X", "\x1b[2Gx"] {
        let mut term = engine(10);
        term.feed("👩🏽‍💻X".as_bytes());
        term.feed(edit.as_bytes());
        let row = term.viewport_row(0).unwrap();
        assert_eq!(row[0], Cell::default());
        assert_eq!(row[1].flags, 0);
        assert!(row[1].combining().is_empty());
        assert_eq!(row[2].character, 'X');
    }
}

#[test]
fn cursor_motion_ends_nonzero_grapheme_continuation() {
    let mut term = engine(10);
    term.feed("👩‍\x1b[1G\x1b[3G💻X".as_bytes());
    assert_eq!(term.viewport_row(0).unwrap()[2].character, '💻');
    assert_eq!(term.viewport_row(0).unwrap()[4].character, 'X');
}

#[test]
fn width_promotion_in_insert_mode_shifts_only_the_extra_cell() {
    let mut term = engine(10);
    term.feed("abcd\r\x1b[4h❤️X".as_bytes());
    let row = term.viewport_row(0).unwrap();
    assert_eq!(cell_text(&row[0]), "❤️");
    assert_eq!(row[2].character, 'X');
    assert_eq!(row[3].character, 'a');
    assert_eq!(row[6].character, 'd');
}

#[test]
fn presentation_selectors_preserve_the_base_style_across_sgr() {
    let mut term = engine(10);
    term.feed("❤\x1b[31m\u{fe0f}X".as_bytes());
    let row = term.viewport_row(0).unwrap();
    assert_eq!(cell_text(&row[0]), "❤️");
    assert_eq!(row[0].style.foreground, super::Color::DEFAULT);
    assert_eq!(row[2].character, 'X');
    assert_eq!(row[2].style.foreground, super::Color::indexed(1));
}

#[test]
fn cold_history_preserves_graphemes_wrapping_and_damage() {
    let mut term = engine(40);
    for _ in 0..20 {
        term.feed("你好 👩🏽‍💻 ❤️ 🇯🇵\r\n".as_bytes());
    }
    let before: Vec<_> = (-(term.history_size() as i32)..4)
        .map(|line| (term.line(line).unwrap().to_vec(), term.line_wrapped(line)))
        .collect();
    term.take_damage();
    let generation = term.generation();
    term.compact_history();
    assert_eq!(term.generation(), generation);
    assert_eq!(term.take_damage(), super::Damage::Partial(Vec::new()));
    for (index, line) in (-(term.history_size() as i32)..4).enumerate() {
        assert_eq!(term.line(line).unwrap(), before[index].0);
        assert_eq!(term.line_wrapped(line), before[index].1);
    }
    // A subsequent feed releases borrowed read caches, while later reads
    // still decode exactly the same retained row.
    term.feed(b"x");
    assert_eq!(term.line(-1).unwrap(), before[term.history_size() - 1].0);
}

#[test]
fn emoji_at_right_margin_is_retained_when_autowrap_is_disabled() {
    let mut term = engine(4);
    term.feed("\x1b[?7labc❤️".as_bytes());
    assert_eq!(cell_text(&term.viewport_row(0).unwrap()[3]), "❤️");
}

#[test]
fn dropped_emoji_at_right_margin_does_not_extend_the_previous_cell() {
    for text in ["abc👩🏽", "abc👩‍💻", "abc👩\u{fe0f}"] {
        let mut term = engine(4);
        term.feed(b"\x1b[?7l");
        term.feed(text.as_bytes());
        let row = term.viewport_row(0).unwrap();
        assert_eq!(cell_text(&row[2]), "c", "{text:?}");
        assert_eq!(row[2].flags, 0, "{text:?}");
        assert_eq!(cell_text(&row[3]), " ", "{text:?}");
    }
}

#[test]
fn overlong_combining_suffix_still_prints_the_next_base() {
    let mut term = engine(40);
    term.feed("❤".as_bytes());
    for _ in 0..200 {
        term.feed("\u{fe0f}".as_bytes());
    }
    term.feed(b"X");
    let row = term.viewport_row(0).unwrap();
    assert!(row[0].combining().len() <= super::types::MAX_COMBINING_BYTES);
    assert_eq!(row[2].character, 'X');
}

#[test]
fn text_presentation_damages_the_erased_wide_spacer_with_a_hidden_cursor() {
    for prefix in ["", "abc"] {
        for visible in [true, false] {
            let mut term = engine(5);
            if !visible {
                term.feed(b"\x1b[?25l");
            }
            term.feed(prefix.as_bytes());
            term.feed("⌚".as_bytes());
            let spacer = prefix.len() + 1;
            assert_eq!(
                term.viewport_row(0).unwrap()[spacer].flags,
                Cell::WIDE_SPACER
            );
            term.take_damage();
            term.feed("\u{fe0e}".as_bytes());
            assert_eq!(term.viewport_row(0).unwrap()[spacer], Cell::default());
            let damage = term.take_damage();
            assert!(
                match &damage {
                    super::Damage::Full => true,
                    super::Damage::Partial(spans) => spans
                        .iter()
                        .any(|span| { span.row == 0 && span.start <= spacer && spacer < span.end }),
                },
                "erased spacer omitted from {damage:?}, visible={visible}, prefix={prefix:?}"
            );
        }
    }
}
