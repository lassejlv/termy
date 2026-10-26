//! Damage snapshots, render reads and viewport visitors.

use super::*;

#[test]
fn terminal_damage_snapshot_is_full_for_new_terminal() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    assert!(matches!(
        terminal.take_damage_snapshot(),
        TerminalDamageSnapshot::Full
    ));
}

#[test]
fn terminal_damage_snapshot_resets_damage_after_read() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    let _ = terminal.take_damage_snapshot();
    let second = terminal.take_damage_snapshot();
    let third = terminal.take_damage_snapshot();
    assert!(matches!(second, TerminalDamageSnapshot::Partial(_)));
    assert_eq!(second, third);
}

#[test]
fn terminal_damage_snapshot_returns_partial_spans_for_output() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    let _ = terminal.take_damage_snapshot();
    terminal.feed_output(b"abc");
    assert!(matches!(
        terminal.take_damage_snapshot(),
        TerminalDamageSnapshot::Partial(spans) if !spans.is_empty()
    ));
}

#[test]
fn terminal_damage_snapshot_while_scrolled_returns_empty_partial_without_damage() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    let _ = terminal.take_damage_snapshot();
    terminal.feed_output(b"1\n2\n3\n4\n5\n6\n");
    let _ = terminal.take_damage_snapshot();

    assert!(terminal.scroll_display(1));
    assert!(terminal.scroll_state().0 > 0);

    assert!(matches!(
        terminal.take_damage_snapshot(),
        TerminalDamageSnapshot::Full
    ));
    assert_eq!(
        terminal.take_damage_snapshot(),
        TerminalDamageSnapshot::Partial(Vec::new())
    );
}

#[test]
fn terminal_damage_snapshot_while_scrolled_maps_damage_to_viewport_rows() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    let _ = terminal.take_damage_snapshot();
    terminal.feed_output(b"1\n2\n3\n4\n5\n6\n");
    let _ = terminal.take_damage_snapshot();

    assert!(terminal.scroll_display(1));
    let _ = terminal.take_damage_snapshot();
    let _ = terminal.take_damage_snapshot();

    terminal.feed_output(b"\x1b[1;1H");
    match terminal.take_damage_snapshot() {
        TerminalDamageSnapshot::Partial(spans) => {
            assert!(spans.iter().any(|span| span.row == 1), "spans: {spans:?}");
            assert!(spans.iter().all(|span| span.row < 4), "spans: {spans:?}");
        }
        TerminalDamageSnapshot::Full => {
            panic!("visible damage while scrolled should stay partial")
        }
    }
}

#[test]
fn terminal_damage_snapshot_while_scrolled_drops_damage_below_viewport() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    let _ = terminal.take_damage_snapshot();
    terminal.feed_output(b"1\n2\n3\n4\n5\n6\n");
    let _ = terminal.take_damage_snapshot();

    assert!(terminal.scroll_display(3));
    let _ = terminal.take_damage_snapshot();
    let _ = terminal.take_damage_snapshot();

    terminal.feed_output(b"x");
    match terminal.take_damage_snapshot() {
        TerminalDamageSnapshot::Partial(spans) => {
            assert!(spans.iter().all(|span| span.row < 4), "spans: {spans:?}");
        }
        TerminalDamageSnapshot::Full => {
            panic!("invisible damage while scrolled should stay partial")
        }
    }
}

#[test]
fn rich_render_read_preserves_text_colors_attributes_and_metadata() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    terminal.feed_output(
        "\x1b[38;2;1;2;3;48;5;4;4:3;58:2::7:8:9;1;2;3;7;8;9me\u{301}\x1b[0m 界".as_bytes(),
    );

    let read = terminal.render_read(true);
    assert_eq!((read.metadata.cols, read.metadata.rows), (32, 4));
    assert_eq!(read.metadata.generation, read.update.generation);
    assert_eq!(read.metadata.palette_revision, read.update.palette_revision);
    assert_eq!(read.metadata.palette_revision, read.palette.revision);
    assert!(matches!(read.update.damage, TerminalDamageSnapshot::Full));
    assert!(read.update.scrolls.is_empty());

    let cell = &read.cells[0];
    assert_eq!(cell.text, "e\u{301}");
    assert_eq!(
        cell.foreground,
        crate::TerminalRenderColor::Rgb(crate::TerminalColor { r: 1, g: 2, b: 3 })
    );
    assert_eq!(cell.background, crate::TerminalRenderColor::Indexed(4));
    assert_eq!(
        cell.underline_color,
        Some(crate::TerminalRenderColor::Rgb(crate::TerminalColor {
            r: 7,
            g: 8,
            b: 9,
        }))
    );
    assert_eq!(cell.underline_style, crate::TerminalUnderlineStyle::Curly);
    assert!(cell.bold);
    assert!(cell.dim);
    assert!(cell.italic);
    assert!(cell.inverse);
    assert!(cell.hidden);
    assert!(cell.strikethrough);
    assert!(read.cells.iter().any(|cell| cell.wide_character_spacer));
}

#[test]
fn viewport_visitors_allow_reentrant_terminal_reads() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    terminal.feed_output(b"abc");

    let mut viewport_reentered = false;
    terminal.visit_viewport_cells(|_, _, _, _| {
        if !viewport_reentered {
            viewport_reentered = true;
            assert_eq!(terminal.line_bounds().1, 3);
        }
    });
    assert!(viewport_reentered);

    let update = terminal.take_render_damage_snapshot();
    let spans = match &update.damage {
        TerminalDamageSnapshot::Partial(spans) if !spans.is_empty() => spans.clone(),
        TerminalDamageSnapshot::Full | TerminalDamageSnapshot::Partial(_) => {
            vec![crate::TerminalDirtySpan {
                row: 0,
                left_col: 0,
                right_col: 0,
            }]
        }
    };
    let mut range_reentered = false;
    assert!(terminal.visit_viewport_ranges_at_generation(
        update.generation,
        &spans,
        |_, _, _, _, _| {
            if !range_reentered {
                range_reentered = true;
                let _ = terminal.palette();
            }
        },
    ));
    assert!(range_reentered);
}

#[test]
fn locked_viewport_visitors_stream_cells_and_reject_stale_ranges() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    terminal.feed_output(b"abc");

    let mut first_row = String::new();
    let mut visited = 0;
    let metadata = terminal.visit_viewport_cells_locked(|viewport_row, _, _, cell| {
        visited += 1;
        if viewport_row == 0 {
            first_row.push_str(&cell.text);
        }
    });
    assert!(first_row.starts_with("abc"));
    assert_eq!(
        visited,
        usize::from(test_terminal_size().cols) * usize::from(test_terminal_size().rows)
    );

    let spans = [crate::TerminalDirtySpan {
        row: 0,
        left_col: 0,
        right_col: 2,
    }];
    let mut selected = String::new();
    assert!(terminal.visit_viewport_ranges_locked_at_generation(
        metadata.generation,
        &spans,
        |_, _, _, _, cell| selected.push_str(&cell.text),
    ));
    assert_eq!(selected, "abc");

    terminal.feed_output(b"d");
    let mut stale_visited = false;
    assert!(!terminal.visit_viewport_ranges_locked_at_generation(
        metadata.generation,
        &spans,
        |_, _, _, _, _| stale_visited = true,
    ));
    assert!(!stale_visited);
}

#[test]
fn rich_line_visitor_streams_cells_in_buffer_order() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    terminal.feed_output(b"abc\r\ndef");

    let mut visited = Vec::new();
    let bounds = terminal.visit_line_cells(0, 1, |range, line, col, cell| {
        if col < 3 {
            visited.push((range, line, col, cell.text.to_string()));
        }
    });

    assert_eq!(bounds, (0, 3, 32));
    assert_eq!(
        visited,
        vec![
            ((0, 3, 32), 0, 0, "a".to_string()),
            ((0, 3, 32), 0, 1, "b".to_string()),
            ((0, 3, 32), 0, 2, "c".to_string()),
            ((0, 3, 32), 1, 0, "d".to_string()),
            ((0, 3, 32), 1, 1, "e".to_string()),
            ((0, 3, 32), 1, 2, "f".to_string()),
        ]
    );
}

#[test]
fn rich_render_cell_keeps_common_text_inline() {
    let terminal = Terminal::new_display(test_terminal_size(), None);
    terminal.feed_output(b"plain");

    let read = terminal.render_read(true);
    assert!(read.cells.iter().all(|cell| !cell.text.is_heap_allocated()));

    terminal.feed_output("\rcombined e\u{301}".as_bytes());
    let read = terminal.render_read(true);
    assert!(
        read.cells
            .iter()
            .filter(|cell| !cell.text.is_empty())
            .all(|cell| !cell.text.is_heap_allocated())
    );
}

#[test]
fn rich_render_read_reports_wrap_generation_palette_and_partial_cells() {
    let terminal = Terminal::new_display(
        TerminalSize {
            cols: 4,
            rows: 2,
            ..test_terminal_size()
        },
        None,
    );
    let initial = terminal.render_read(true);
    let _ = terminal.take_render_damage_snapshot();
    terminal.feed_output(b"abcde");

    let update = terminal.take_render_damage_snapshot();
    assert!(update.generation > initial.metadata.generation);
    assert!(matches!(
        update.damage,
        TerminalDamageSnapshot::Partial(ref spans) if !spans.is_empty()
    ));
    let spans = match &update.damage {
        TerminalDamageSnapshot::Partial(spans) => spans,
        TerminalDamageSnapshot::Full => unreachable!(),
    };
    let mut visited = Vec::new();
    assert!(terminal.visit_viewport_ranges_at_generation(
        update.generation,
        spans,
        |row, display_offset, line, col, cell| {
            visited.push((row, display_offset, line, col, cell.text.clone()));
        },
    ));
    assert!(!visited.is_empty());

    terminal.feed_output(b"\x1b]4;1;#123456\x07");
    let read = terminal.render_read(true);
    assert!(read.cells[3].line_wrapped);
    assert_eq!(
        read.palette.indexed[1],
        Some(crate::TerminalColor {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        })
    );
    assert!(read.metadata.palette_revision > initial.metadata.palette_revision);
}
