use super::*;

fn engine(cols: usize, rows: usize, history: usize) -> Engine {
    Engine::new(
        Size { cols, rows },
        Options {
            scrollback_history: history,
            ..Default::default()
        },
    )
}

fn text(engine: &Engine, row: usize) -> String {
    let line = engine.viewport_row(row).unwrap();
    line.iter()
        .enumerate()
        .filter(|(_, cell)| cell.flags & Cell::WIDE_SPACER == 0)
        .map(|(col, cell)| {
            let mut text = cell.character.to_string();
            text.push_str(line.combining(col));
            text
        })
        .collect()
}

fn replies(engine: &mut Engine) -> Vec<u8> {
    let mut result = Vec::new();
    engine.drain_replies(&mut result);
    result
}

#[test]
fn clipboard_events_commit_in_stream_order_with_original_terminators() {
    let mut engine = engine(30, 3, 0);
    let input = b"\x1b[?2026h\x1b[?5522h\x1b]5522;type=read:id=a;Lg==\x07\x1b]5522;type=read:id=b;Lg==\x1b\\";
    for byte in input {
        engine.feed(&[*byte]);
    }
    assert!(engine.pop_event().is_none());
    assert!(!engine.modes().clipboard_paste_events);
    engine.feed(b"\x1b[?2026l");
    assert!(engine.modes().clipboard_paste_events);
    assert_eq!(
        engine.pop_event(),
        Some(Event::KittyClipboardControl(
            crate::KittyClipboardControl::Set(true)
        ))
    );
    for (body, terminator) in [
        (
            b"type=read:id=a;Lg==".as_slice(),
            crate::KittyClipboardOscTerminator::Bell,
        ),
        (
            b"type=read:id=b;Lg==".as_slice(),
            crate::KittyClipboardOscTerminator::StringTerminator,
        ),
    ] {
        assert_eq!(
            engine.pop_event(),
            Some(Event::KittyClipboard(crate::KittyClipboardOsc::from_body(
                body, terminator
            )))
        );
    }
    assert!(engine.pop_event().is_none());
}

#[test]
fn unicode_continuation_bytes_do_not_start_clipboard_protocols() {
    let mut engine = engine(30, 3, 0);
    for byte in "ŝ5522;ordinary".as_bytes() {
        engine.feed(&[*byte]);
    }
    assert_eq!(text(&engine, 0).trim_end(), "ŝ5522;ordinary");
    assert!(engine.pop_event().is_none());
}

#[test]
fn event_pressure_keeps_clipboard_resets_and_aborts_incomplete_writes() {
    let mut engine = engine(30, 3, 0);
    engine.feed(b"\x1b[?5522h");
    while engine.pop_event().is_some() {}
    engine.feed(&[7; MAX_EVENTS]);
    engine.feed(b"\x1b]5522;type=wdata;\x07");
    assert_eq!(
        engine.pop_event(),
        Some(Event::KittyClipboardControl(
            crate::KittyClipboardControl::Reset
        ))
    );
    assert_eq!(
        engine.pop_event(),
        Some(Event::KittyClipboardControl(
            crate::KittyClipboardControl::Set(true)
        ))
    );
    assert!(matches!(engine.pop_event(), Some(Event::KittyClipboard(_))));
    assert!(engine.pop_event().is_none());

    engine.feed(&[7; MAX_EVENTS]);
    engine.feed(b"\x1bc");
    assert_eq!(
        engine.pop_event(),
        Some(Event::KittyClipboardControl(
            crate::KittyClipboardControl::Reset
        ))
    );
    assert!(!engine.modes().clipboard_paste_events);
    assert!(engine.dropped_events() >= MAX_EVENTS as u64);
}

#[test]
fn underline_color_follows_text_into_spacers_marks_links_and_history() {
    let red = Color::rgb(255, 0, 0);
    let mut engine = engine(40, 2, 10);
    engine.feed("\x1b[4;58:2::255:0:0m界e\u{301}".as_bytes());
    let row = engine.viewport_row(0).unwrap();
    assert_eq!(row[1].flags, Cell::WIDE_SPACER);
    assert!((0..3).all(|col| row.underline_color(col) == red));
    assert_eq!(row.combining(2), "\u{301}");

    // OSC 8 keeps the color; SGR 0 clears it but keeps the link.
    engine.feed(b"\x1b]8;;https://example.test\x1b\\L\x1b[0mM");
    let row = engine.viewport_row(0).unwrap();
    assert_eq!(row.underline_color(3), red);
    assert!(row.hyperlink(3).is_some());
    assert_eq!(row.underline_color(4), Color::DEFAULT);
    assert!(row.hyperlink(4).is_some());

    // Packed history rows keep colors, flags and marks.
    engine.feed(b"\r\n\r\n\r\n");
    engine.compact_history();
    assert!(engine.state.grid.row(-2).unwrap().is_packed());
    let line = engine.line(-2).unwrap();
    assert_eq!(line[1].flags, Cell::WIDE_SPACER);
    assert_eq!(line.combining(2), "\u{301}");
    assert!((0..4).all(|col| line.underline_color(col) == red));

    engine.feed(b"\x1b[58:5:9m\x1bP$qm\x1b\\\x1b[59m\x1bP$qm\x1b\\");
    assert_eq!(
        replies(&mut engine),
        b"\x1bP1$r0;58;5;9m\x1b\\\x1bP1$r0m\x1b\\"
    );
}

#[test]
fn cell_layout_and_screen_dimensions_stay_bounded() {
    assert_eq!(size_of::<Cell>(), 16, "hot cell size changed");
    assert_eq!(size_of::<Color>(), 4);
    assert_eq!(
        Size { cols: 0, rows: 0 }.clamped(),
        Size { cols: 1, rows: 1 }
    );
    let size = Size {
        cols: usize::MAX,
        rows: usize::MAX,
    }
    .clamped();
    assert!(size.cols * size.rows <= Size::MAX_CELLS);
}

#[test]
fn wraps_only_when_next_character_arrives() {
    let mut engine = engine(4, 2, 10);
    engine.feed(b"abcd");
    assert_eq!(text(&engine, 0), "abcd");
    assert_eq!((engine.cursor().row, engine.cursor().col), (0, 3));
    engine.feed(b"e");
    assert_eq!(text(&engine, 1), "e   ");
    assert!(engine.viewport_row_wrapped(0));
    assert_eq!(engine.history_size(), 0);
}

#[test]
fn cursor_editing_and_scroll_regions_preserve_outside_rows() {
    let mut engine = engine(8, 4, 10);
    engine.feed(b"header\r\none\r\ntwo\r\nfooter\x1b[2;3r\x1b[3;1H\nnew");
    assert_eq!(text(&engine, 0), "header  ");
    assert_eq!(text(&engine, 1), "two     ");
    assert_eq!(text(&engine, 2), "new     ");
    assert_eq!(text(&engine, 3), "footer  ");
    assert_eq!(engine.history_size(), 0);
    engine.feed(b"\x1b[r\x1b[2;2H\x1b[2@AB");
    assert_eq!(text(&engine, 1), "tABwo   ");
    engine.feed(b"\x1b[2;2H\x1b[2P");
    assert_eq!(text(&engine, 1), "two     ");
}

#[test]
fn sgr_semicolon_and_colon_forms_keep_style_and_hyperlink_independent() {
    let mut engine = engine(10, 2, 0);
    engine.feed(b"\x1b]8;id=build;https://example.com\x1b\\\x1b[1;3;4:3;38:2::1:2:3;48;5;42;58:2::4:5:6mA\x1b[0mB\x1b]8;;\x1b\\C");
    let cells = engine.viewport_row(0).unwrap();
    assert_eq!(cells[0].style.foreground.as_rgb(), Some((1, 2, 3)));
    assert_eq!(cells[0].style.background.as_indexed(), Some(42));
    assert_eq!(cells.underline_color(0).as_rgb(), Some((4, 5, 6)));
    assert_eq!(cells.underline_color(1), Color::DEFAULT);
    assert_eq!(cells[0].style.underline, UnderlineStyle::Curly);
    assert_eq!(cells[0].style.attributes, Style::BOLD | Style::ITALIC);
    assert_eq!(cells[1].style, Style::default());
    assert_eq!(cells.hyperlink(1).unwrap().uri, "https://example.com");
    assert!(cells.hyperlink(2).is_none());
}

#[test]
fn reopened_links_and_wide_spacers_reuse_shared_metadata() {
    use std::sync::Arc;

    let extra = |engine: &Engine, col: usize| {
        Arc::clone(
            engine
                .state
                .grid
                .visible_row(0)
                .unwrap()
                .extra(col)
                .unwrap(),
        )
    };
    let mut engine = engine(20, 2, 0);
    // Reopening the pen's current link keeps its metadata allocation.
    engine.feed(b"\x1b]8;;https://example.test\x1b\\A\x1b]8;;https://example.test\x1b\\B");
    assert!(Arc::ptr_eq(&extra(&engine, 0), &extra(&engine, 1)));
    // Closing a link that isn't open is a no-op, and keeps the color.
    engine.feed(b"\x1b]8;;\x1b\\\x1b[58:5:1m\x1b]8;;\x1b\\C");
    let cells = engine.viewport_row(0).unwrap();
    assert!(cells.hyperlink(2).is_none());
    assert_eq!(cells.underline_color(2), Color::indexed(1));
    // Spacers of linked, colored wide glyphs share one color-only extra.
    engine.feed("\x1b]8;;https://example.test\x1b\\界界".as_bytes());
    let (first, second) = (extra(&engine, 4), extra(&engine, 6));
    assert!(Arc::ptr_eq(&first, &second));
    assert!(first.hyperlink.is_none());
    assert_eq!(first.underline_color, Color::indexed(1));
    // A new color gets its own spacer extra.
    engine.feed("\x1b[58:5:2m界".as_bytes());
    let cells = engine.viewport_row(0).unwrap();
    assert_eq!(cells.underline_color(8), Color::indexed(2));
    assert!(!Arc::ptr_eq(&first, &extra(&engine, 8)));
}

// Single printable bytes take a shortcut past the parser; any byte the full
// path would treat differently must still reach it.
#[test]
fn byte_at_a_time_output_matches_one_write() {
    let input = concat!(
        "plain text that wraps past the edge\r\n",
        "\x1b[31mred\x1b[0m \x1b(0lqk\x1b(B x\r\n",
        "\x1b[4hins\x1b[4l\x1b[1;3Hover\x1b[3;1H",
        "\x1b]8;;https://example.test\x07link\x1b]8;;\x07 ",
        "\x1b[?2026hsync\x1b[?2026l e\u{301}界\x1b[?7lno wrap at the margin here\x1b[?7h",
        "\r\nmore\r\nlines\r\nscroll\r\nthe\r\nscreen\x1b[2;5r\x1b[5;1Hregion\n\n",
        // Control sequences the parser's fast paths take whole, and their
        // edge cases: intermediates, controls, cancels, escapes and deletes
        // inside, subparameters and too many parameters.
        "\x1b[38;2;120;180;255;48;5;234;1mstatus\x1b[0m\x1b[2;1H\x1b[Kbuild\x1b[H",
        "\x1b[4 q\x1b[2\r;3HA\x1b[31\x18mB\x1b[3\x1b[1mC\x1b[4:3mD\x1b[3\x7f2mE",
        "\x1b[1;2;3;4;5;6;7;8;9;1;2;3;4;5;6;7;8;9;1;2;3;4;5;6;7;8;9;1;2;3;4;5;6;7mF\x1b[m",
    );
    let mut whole = engine(16, 4, 8);
    whole.feed(input.as_bytes());
    let mut bytewise = engine(16, 4, 8);
    for byte in input.as_bytes() {
        bytewise.feed(std::slice::from_ref(byte));
    }
    for line in -(whole.history_size() as i32)..4 {
        assert_eq!(whole.line(line), bytewise.line(line), "line {line}");
    }
    assert_eq!(whole.cursor(), bytewise.cursor());
    assert_eq!(whole.modes(), bytewise.modes());
    assert_eq!(whole.history_size(), bytewise.history_size());
}

#[test]
fn wide_and_combining_text_survive_bytewise_input() {
    let mut engine = engine(8, 2, 0);
    for byte in "A界e\u{301}🙂".as_bytes() {
        engine.feed(&[*byte]);
    }
    let cells = engine.viewport_row(0).unwrap();
    assert_eq!(cells[1].character, '界');
    assert_ne!(cells[2].flags & Cell::WIDE_SPACER, 0);
    assert_eq!(cells[3].character, 'e');
    assert_eq!(cells.combining(3), "\u{301}");
    assert_eq!(cells[4].character, '🙂');
    assert_ne!(cells[5].flags & Cell::WIDE_SPACER, 0);
}

#[test]
fn alternate_screen_preserves_primary_and_keyboard_stack_is_per_screen() {
    let mut engine = engine(10, 3, 10);
    engine.feed(b"shell\x1b[>3u\x1b[?1049h\x1b[>7uTUI");
    assert!(engine.alternate_screen());
    assert_eq!(engine.modes().kitty_keyboard, 7);
    assert_eq!(text(&engine, 0), "TUI       ");
    engine.feed(b"\x1b[?1049l");
    assert_eq!(engine.modes().kitty_keyboard, 3);
    assert_eq!(text(&engine, 0), "shell     ");
    assert_eq!(engine.cursor().col, 5);
    engine.feed(b"\x1b[<u\x1b[?u");
    assert_eq!(replies(&mut engine), b"\x1b[?0u");
}

#[test]
fn color_scheme_is_reported_on_query_and_on_change_when_asked() {
    let dark = crate::TerminalQueryColors::default();
    let light = crate::TerminalQueryColors {
        foreground: crate::TerminalColor {
            r: 0x20,
            g: 0x20,
            b: 0x20,
        },
        background: crate::TerminalColor {
            r: 0xfa,
            g: 0xfa,
            b: 0xf9,
        },
        ..dark
    };
    let mut engine = engine(20, 2, 0);
    engine.set_query_colors(dark);
    engine.feed(b"\x1b[?996n");
    assert_eq!(replies(&mut engine), b"\x1b[?997;1n");

    // Without ?2031 a change stays quiet; the program can still ask.
    engine.set_query_colors(light);
    assert!(replies(&mut engine).is_empty());
    engine.feed(b"\x1b[?996n\x1b[?2031$p");
    assert_eq!(replies(&mut engine), b"\x1b[?997;2n\x1b[?2031;2$y");

    engine.feed(b"\x1b[?2031h\x1b[?2031$p");
    assert_eq!(replies(&mut engine), b"\x1b[?2031;1$y");
    engine.set_query_colors(dark);
    assert_eq!(replies(&mut engine), b"\x1b[?997;1n");
    // Same colors again: nothing to report.
    engine.set_query_colors(dark);
    assert!(replies(&mut engine).is_empty());

    engine.feed(b"\x1b[?2031l");
    engine.set_query_colors(light);
    assert!(replies(&mut engine).is_empty());
}

#[test]
fn terminal_queries_report_cursor_and_dimensions() {
    let mut engine = engine(80, 24, 0);
    engine.feed(b"\x1b[4;9H\x1b[6n\x1b[5n\x1b[18t");
    assert_eq!(replies(&mut engine), b"\x1b[4;9R\x1b[0n\x1b[8;24;80t");
    assert!(replies(&mut engine).is_empty());
}

#[test]
fn palette_queries_and_reset_are_engine_owned() {
    let mut engine = engine(10, 2, 0);
    engine.feed(b"\x1b]4;42;#123456\x1b\\\x1b]4;42;?\x1b\\\x1b]10;rgb:ff/80/00\x07");
    assert_eq!(engine.palette()[42], Some(Color::rgb(0x12, 0x34, 0x56)));
    assert_eq!(engine.foreground(), Some(Color::rgb(255, 128, 0)));
    assert_eq!(replies(&mut engine), b"\x1b]4;42;rgb:1212/3434/5656\x1b\\");
    let revision = engine.palette_revision();
    engine.feed(b"\x1b]104;42\x1b\\\x1b]110\x1b\\");
    assert_eq!(engine.palette()[42], None);
    assert_eq!(engine.foreground(), None);
    assert!(engine.palette_revision() > revision);
}

#[test]
fn events_and_replies_have_backpressure_limits() {
    let mut engine = engine(10, 2, 0);
    engine.feed(&[7; MAX_EVENTS + 10]);
    assert_eq!(engine.dropped_events(), 10);
    let mut count = 0;
    while engine.pop_event().is_some() {
        count += 1;
    }
    assert_eq!(count, MAX_EVENTS);
    for _ in 0..20_000 {
        engine.feed(b"\x1b[5n");
    }
    let output = replies(&mut engine);
    assert!(output.len() <= MAX_REPLY_BYTES);
    assert!(engine.dropped_reply_bytes() > 0);
    assert_eq!(
        output.len() % b"\x1b[0n".len(),
        0,
        "never queue a partial reply"
    );
}

#[test]
fn all_chunk_boundaries_produce_identical_terminal_state() {
    let input = "\x1b]2;editor\x07plain\r\n界e\u{301}\x1b[38:2::7:8:9mXYZ\x1b[0m\x1b[2;3H\x1b[2@!!\x1b[6n\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\".as_bytes();
    let mut whole = engine(16, 4, 5);
    whole.feed(input);
    let expected_rows: Vec<_> = (0..4)
        .map(|row| whole.viewport_row(row).unwrap().snapshot())
        .collect();
    let expected_reply = replies(&mut whole);
    for split in 0..=input.len() {
        let mut fragmented = engine(16, 4, 5);
        fragmented.feed(&input[..split]);
        fragmented.feed(&input[split..]);
        assert_eq!(fragmented.cursor(), whole.cursor(), "split {split}");
        for (row, expected) in expected_rows.iter().enumerate() {
            assert_eq!(
                &fragmented.viewport_row(row).unwrap().snapshot(),
                expected,
                "split {split}, row {row}"
            );
        }
        assert_eq!(replies(&mut fragmented), expected_reply, "split {split}");
    }
}

#[test]
fn dec_line_drawing_and_character_set_switching() {
    let mut engine = engine(12, 2, 0);
    engine.feed(b"\x1b(0lqk\x1b(Babc\x1b)0\x0ex\x0fq");
    assert_eq!(text(&engine, 0), "┌─┐abc│q    ");
}

#[test]
fn damage_covers_every_changed_cell_across_screen_operations() {
    let mut engine = engine(12, 5, 20);
    let operations: &[&[u8]] = &[
        b"hello",
        b"\r\nnext",
        "界e\u{301}".as_bytes(),
        b"\x1b[1;2H!",
        b"\x1b[2@",
        b"\x1b[P",
        b"\x1b[K",
        b"\x1b[2J",
        b"first\r\nsecond\r\nthird\r\nfourth\r\nfifth\r\nsixth",
        b"\x1b[2;4r\x1b[2;1H\x1b[L",
        b"\x1b[M",
        b"\x1b[S",
        b"\x1b[T",
        b"\x1b[r",
        b"\x1b[?1049hfull screen",
        b"\x1b[?1049l",
        b"\x1b[H\x1b[3X",
    ];
    for operation in operations.iter().cycle().take(100) {
        let before: Vec<_> = (0..5)
            .map(|row| engine.viewport_row(row).unwrap().snapshot())
            .collect();
        engine.take_damage();
        engine.feed(operation);
        let damage = engine.take_damage();
        if let Damage::Partial(spans) = damage {
            for (row, (cells, extras)) in before.iter().enumerate() {
                let line = engine.viewport_row(row).unwrap();
                for (col, cell) in cells.iter().enumerate() {
                    let extra = extras
                        .iter()
                        .find(|(at, _)| *at == col)
                        .map(|(_, extra)| extra);
                    if cell != &line[col] || extra != line.extra(col) {
                        assert!(
                            spans
                                .iter()
                                .any(|span| span.row == row && span.start <= col && col < span.end),
                            "missing damage at {row}:{col} for {operation:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn history_limit_updates_trim_history_and_clamp_viewport() {
    let mut engine = engine(8, 3, 20);
    for _ in 0..30 {
        engine.feed(b"line\r\n");
    }
    assert_eq!(engine.history_size(), 20);
    assert!(engine.scroll_display(20));
    engine.set_options(Options {
        scrollback_history: 2,
        ..Default::default()
    });
    assert_eq!(engine.history_size(), 2);
    assert!(engine.display_offset() <= 2);
    for row in 0..engine.size().rows {
        assert!(engine.viewport_row(row).is_some());
    }
}

#[test]
fn arbitrary_bytes_and_resizes_keep_grid_invariants() {
    let mut engine = engine(13, 7, 17);
    let mut random = 0x1234_5678u32;
    for step in 0..2000 {
        let mut bytes = [0; 31];
        for byte in &mut bytes {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            *byte = random as u8;
        }
        engine.feed(&bytes);
        if step % 17 == 0 {
            engine.resize(Size {
                cols: (random as usize % 31) + 1,
                rows: (random as usize % 9) + 1,
            });
        }
        let size = engine.size();
        assert!(engine.cursor().row < size.rows);
        assert!(engine.cursor().col < size.cols);
        for row in 0..size.rows {
            let cells = engine.viewport_row(row).unwrap();
            assert_eq!(cells.len(), size.cols);
            for (col, cell) in cells.iter().enumerate() {
                if cell.flags & Cell::WIDE_SPACER != 0 {
                    assert!(col > 0 && cells[col - 1].flags & Cell::WIDE != 0);
                }
                if cell.flags & Cell::WIDE != 0 {
                    assert!(col + 1 < size.cols && cells[col + 1].flags & Cell::WIDE_SPACER != 0);
                }
            }
        }
    }
}

// A line feed blanks the row it scrolls in lazily: the next text write fills
// only around itself. Recycled history content must never show through,
// whatever follows the line feed or wherever a feed ends.
#[test]
fn rows_scrolled_in_by_line_feeds_never_show_their_recycled_content() {
    let full_row = format!("{}\r\n", "x".repeat(20));
    let cases = [
        ("ab\r\n", "ab"),
        ("\r\n", ""),
        ("\x1b[31mab\x1b[0m\r\n", "ab"),
        ("界a\r\n", "界a"),
        ("e\u{301}\r\n", "e\u{301}"),
        ("\x1b[3Cab\r\n", "   ab"),
        ("\x1b]0;title\x07ab\r\n", "ab"),
        // LF without CR keeps the column, so the text starts mid-row.
        ("ab\n", "ab"),
        // Consecutive runs (ASCII, then UTF-8, across SGR) each take their
        // part of the row; only what no run covers is blanked.
        (
            "ab caf\u{e9} \u{3b5}\u{3bb} \u{65e5}z\r\n",
            "ab caf\u{e9} \u{3b5}\u{3bb} \u{65e5}z",
        ),
        ("\u{e9}a\u{e9}a\r\n", "\u{e9}a\u{e9}a"),
        ("ab\x1b[1m\u{e9}\u{65e5}\x1b[0mz\r\n", "ab\u{e9}\u{65e5}z"),
    ];
    for (line, visible) in cases {
        for chunk in [1, 3, usize::MAX] {
            let mut engine = engine(20, 3, 2);
            engine.feed(full_row.repeat(6).as_bytes());
            let input = line.repeat(4);
            for part in input.as_bytes().chunks(chunk.min(input.len())) {
                engine.feed(part);
            }
            for row in 0..2 {
                let text = text(&engine, row);
                assert!(
                    !text.contains('x'),
                    "{line:?} in chunks of {chunk}: {text:?}"
                );
                assert_eq!(text.trim(), visible.trim(), "{line:?} in chunks of {chunk}");
            }
            assert_eq!(text(&engine, 2).trim(), "", "{line:?} in chunks of {chunk}");
        }
    }
}

#[test]
fn payload_limits_drop_oversized_osc_dcs_and_clipboard_strings() {
    let mut engine = Engine::new(
        Size { cols: 20, rows: 3 },
        Options {
            max_string_bytes: 64,
            max_clipboard_bytes: 32,
            ..Default::default()
        },
    );
    let title = |len: usize| format!("\x1b]2;{}\x07", "t".repeat(len - 2));
    engine.feed(title(64).as_bytes());
    assert_eq!(engine.title().len(), 62);
    engine.feed(b"\x1b]2;x\x07");
    engine.feed(title(65).as_bytes());
    assert_eq!(engine.title(), "x");
    // A dropped DCS gets no reply; one within the limit does.
    engine.feed(format!("\x1bP$q{}\x1b\\", "m".repeat(64)).as_bytes());
    assert!(replies(&mut engine).is_empty());
    engine.feed(b"\x1bP$qm\x1b\\");
    assert!(!replies(&mut engine).is_empty());
    while engine.pop_event().is_some() {}
    // OSC 52 has its own limit, counted over the whole payload.
    engine.feed(b"\x1b]52;c;aGVsbG8gd29ybGQ=\x07");
    assert!(matches!(engine.pop_event(), Some(Event::Clipboard { .. })));
    engine.feed(format!("\x1b]52;c;{}\x07", "QUFB".repeat(8)).as_bytes());
    assert!(engine.pop_event().is_none());
    // Limits clamp to their defaults, which are also the maxima.
    engine.set_options(Options {
        max_string_bytes: usize::MAX,
        max_clipboard_bytes: 0,
        ..engine.options()
    });
    assert_eq!(engine.options().max_string_bytes, MAX_STRING_PAYLOAD_BYTES);
    assert_eq!(engine.options().max_clipboard_bytes, 1);
}

#[test]
fn scrollback_is_bounded_by_bytes_alone_without_a_row_limit() {
    let mut engine = Engine::new(
        Size { cols: 80, rows: 4 },
        Options {
            scrollback_history: usize::MAX,
            ..Default::default()
        },
    );
    for line in 0..30_000 {
        engine.feed(format!("line {line}\r\n").as_bytes());
    }
    // Past the old 20,000-row cap.
    assert_eq!(engine.history_size(), 30_000 - 3);
    // Lowering the byte limit evicts the oldest rows at once.
    engine.set_options(Options {
        scrollback_bytes: 1024 * 1024,
        ..engine.options()
    });
    let kept = engine.history_size();
    assert!(kept < 30_000 - 3 && kept > 1000, "{kept}");
    assert!(engine.state.grid.history_bytes() <= 1024 * 1024);
    let oldest: String = engine
        .line(-(kept as i32))
        .unwrap()
        .iter()
        .map(|cell| cell.character)
        .collect();
    assert_eq!(oldest.trim_end(), format!("line {}", 30_000 - 3 - kept));
    for line in 0..5000 {
        engine.feed(format!("more {line}\r\n").as_bytes());
    }
    assert!(engine.state.grid.history_bytes() <= 1024 * 1024);
}
