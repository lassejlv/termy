use std::sync::Arc;

use engine::{
    Cell, Color, DynamicColor, FrameUpdate, Key, KeyEvent, KeyEventKind, Modifiers, MouseButton,
    MouseEvent, MouseEventKind, MousePointerShape, RowMoveDirection, SearchDirection,
    SearchOptions, SelectionMode, SelectionPoint, SelectionRange, Terminal, TerminalConfig,
    TerminalEvent,
};

fn terminal(columns: usize, rows: usize) -> Terminal {
    let mut terminal = Terminal::new(TerminalConfig {
        columns,
        rows,
        scrollback_limit: 100,
    });
    let _ = terminal.frame_update(true);
    terminal
}

fn apply_update(retained: &mut Vec<Vec<Cell>>, update: &FrameUpdate) {
    if update.full
        || retained.len() != update.rows
        || retained.first().map(Vec::len) != Some(update.columns)
    {
        *retained = vec![vec![Cell::default(); update.columns]; update.rows];
    }
    for movement in &update.row_moves {
        let rows = &mut retained[movement.start_row..movement.end_row];
        match movement.direction {
            RowMoveDirection::Up => rows.rotate_left(movement.count),
            RowMoveDirection::Down => rows.rotate_right(movement.count),
        }
    }
    for row in &update.row_updates {
        let end = row.start_column + row.cells.len();
        retained[row.index][row.start_column..end].clone_from_slice(&row.cells);
    }
}

#[test]
fn printable_text_produces_a_partial_row_update() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"hello");
    let update = terminal.frame_update(false);
    assert!(!update.full);
    assert_eq!(update.row_updates.len(), 1);
    assert_eq!(update.row_updates[0].index, 0);
    assert_eq!(update.row_updates[0].start_column, 0);
    assert_eq!(update.row_updates[0].cells[0].text, "h");
    assert_eq!(update.row_updates[0].cells[4].text, "o");
    assert_eq!(update.cursor.column, 5);
}

#[test]
fn single_cell_rewrite_only_copies_the_changed_cell() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"abc");
    let _ = terminal.frame_update(false);

    terminal.feed(b"\x1b[2GZ");
    let update = terminal.frame_update(false);
    assert_eq!(update.row_updates.len(), 1);
    assert_eq!(update.row_updates[0].index, 0);
    assert_eq!(update.row_updates[0].start_column, 1);
    assert_eq!(update.row_updates[0].cells.len(), 1);
    assert_eq!(update.row_updates[0].cells[0].text, "Z");
}

#[test]
fn cursor_motion_is_metadata_only_damage() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"abc");
    let _ = terminal.frame_update(false);

    terminal.feed(b"\r");
    let update = terminal.frame_update(false);
    assert!(update.metadata_changed);
    assert!(update.row_updates.is_empty());
    assert_eq!(update.cursor.column, 0);
}

#[test]
fn erasing_either_half_of_a_wide_character_clears_the_whole_glyph() {
    let mut terminal = terminal(6, 2);
    terminal.feed("界".as_bytes());
    let _ = terminal.frame_update(false);

    terminal.feed(b"\x1b[2G\x1b[X");
    let update = terminal.frame_update(false);
    assert_eq!(update.row_updates[0].start_column, 0);
    assert_eq!(update.row_updates[0].cells.len(), 2);
    assert!(update.row_updates[0].cells.iter().all(|cell| {
        cell.text == " "
            && !cell
                .flags
                .intersects(engine::CellFlags::WIDE | engine::CellFlags::WIDE_SPACER)
    }));
}

#[test]
fn combining_marks_attach_to_the_wide_character_not_its_spacer() {
    let mut terminal = terminal(6, 2);
    terminal.feed("界\u{301}".as_bytes());
    let update = terminal.frame_update(false);
    assert_eq!(update.row_updates[0].cells[0].text, "界\u{301}");
    assert!(
        update.row_updates[0].cells[1]
            .flags
            .contains(engine::CellFlags::WIDE_SPACER)
    );
}

#[test]
fn origin_mode_addresses_and_clamps_to_the_scrolling_region() {
    let mut terminal = terminal(6, 5);
    terminal.feed(b"\x1b[2;4r\x1b[?6hX\x1b[99BY");
    let update = terminal.frame_update(true);
    assert_eq!(update.row_updates[1].cells[0].text, "X");
    assert_eq!(update.row_updates[3].cells[1].text, "Y");

    terminal.feed(b"\x1b[?6lZ");
    let update = terminal.frame_update(true);
    assert_eq!(update.row_updates[0].cells[0].text, "Z");
}

#[test]
fn metrics_report_actual_damage_and_cell_copy_volume() {
    let mut terminal = terminal(8, 3);
    terminal.reset_metrics();
    terminal.feed(b"x");
    let _ = terminal.frame_update(false);
    let _ = terminal.frame_update(false);

    let metrics = terminal.metrics();
    assert_eq!(metrics.feed_calls, 1);
    assert_eq!(metrics.bytes_fed, 1);
    assert_eq!(metrics.frame_requests, 2);
    assert_eq!(metrics.damaged_frames, 1);
    assert_eq!(metrics.full_frames, 0);
    assert_eq!(metrics.row_updates, 1);
    assert_eq!(metrics.cells_copied, 1);
}

#[test]
fn scroll_damage_metrics_remain_row_coalesced() {
    let mut terminal = Terminal::new(TerminalConfig {
        columns: 4,
        rows: 2,
        scrollback_limit: 1,
    });
    let _ = terminal.frame_update(true);
    terminal.reset_metrics();

    terminal.feed(b"a\r\nb\r\nc");
    let update = terminal.frame_update(false);

    assert_eq!(update.row_updates.len(), 2);
    let metrics = terminal.metrics();
    assert_eq!(metrics.feed_calls, 1);
    assert_eq!(metrics.damaged_frames, 1);
    assert_eq!(metrics.full_frames, 0);
    assert_eq!(metrics.row_moves, 1);
    assert_eq!(metrics.rows_moved, 1);
    assert_eq!(metrics.row_updates, 2);
    assert_eq!(metrics.cells_copied, 5);
}

#[test]
fn one_line_scroll_moves_rows_and_copies_only_the_exposed_payload() {
    let mut terminal = terminal(8, 50);
    for row in 1..=50 {
        terminal.feed(format!("\x1b[{row};1Hrow{row:02}").as_bytes());
    }
    let _ = terminal.frame_update(true);
    terminal.reset_metrics();

    terminal.feed(b"\x1b[50;1H\r\n");
    let update = terminal.frame_update(false);

    assert_eq!(update.row_moves.len(), 1);
    assert_eq!(update.row_moves[0].start_row, 0);
    assert_eq!(update.row_moves[0].end_row, 50);
    assert_eq!(update.row_moves[0].direction, RowMoveDirection::Up);
    assert_eq!(update.row_moves[0].count, 1);
    assert_eq!(update.row_updates.len(), 1);
    assert_eq!(update.row_updates[0].index, 49);
    assert_eq!(terminal.metrics().cells_copied, 8);
}

#[test]
fn partial_region_scroll_moves_and_damages_only_the_exposed_region_row() {
    let mut terminal = terminal(8, 6);
    terminal.feed(b"\x1b[2;5r\x1b[5;1H\n");
    let update = terminal.frame_update(false);

    assert_eq!(update.row_moves.len(), 1);
    assert_eq!(update.row_moves[0].start_row, 1);
    assert_eq!(update.row_moves[0].end_row, 5);
    assert_eq!(update.row_moves[0].direction, RowMoveDirection::Up);
    assert_eq!(update.row_updates.len(), 1);
    assert_eq!(update.row_updates[0].index, 4);
}

#[test]
fn incremental_row_moves_match_forced_frames_across_vt_scroll_operations() {
    let scenarios: [(&str, &[u8]); 8] = [
        ("line feed", b"\x1b[5;1H\r\n"),
        ("reverse index", b"\x1b[1;1H\x1bM"),
        ("insert line", b"\x1b[2;1H\x1b[L"),
        ("delete line", b"\x1b[2;1H\x1b[M"),
        ("partial CSI scroll up", b"\x1b[2;4r\x1b[S"),
        ("partial CSI scroll down", b"\x1b[2;4r\x1b[T"),
        (
            "synchronized scrolls",
            b"\x1b[?2026h\x1b[5;1H\r\nnext\r\n\x1b[?2026l",
        ),
        ("alternate screen fallback", b"\x1b[?1049halternate"),
    ];

    for (name, script) in scenarios {
        let mut terminal = terminal(12, 5);
        for row in 1..=5 {
            terminal.feed(format!("\x1b[{row};1Hseed-{row}").as_bytes());
        }
        let initial = terminal.frame_update(true);
        let mut retained = Vec::new();
        apply_update(&mut retained, &initial);

        terminal.feed(script);
        let incremental = terminal.frame_update(false);
        apply_update(&mut retained, &incremental);

        let full = terminal.frame_update(true);
        let mut expected = Vec::new();
        apply_update(&mut expected, &full);
        assert_eq!(retained, expected, "incremental mismatch after {name}");
    }

    let mut terminal = terminal(12, 5);
    let mut retained = Vec::new();
    apply_update(&mut retained, &terminal.frame_update(true));
    terminal.resize(15, 7);
    let resize = terminal.frame_update(false);
    assert!(
        resize.full,
        "resize must keep the explicit full-frame fallback"
    );
    apply_update(&mut retained, &resize);
    let mut expected = Vec::new();
    apply_update(&mut expected, &terminal.frame_update(true));
    assert_eq!(retained, expected);
}

#[test]
fn forced_full_frame_consumes_pending_partial_damage() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"x");
    let full = terminal.frame_update(true);
    assert!(full.full);
    assert!(!terminal.frame_update(false).has_damage());
}

#[test]
fn parser_is_invariant_to_arbitrary_pty_chunk_boundaries() {
    let bytes = concat!(
        "plain\r\n",
        "\x1b[38;2;12;34;56mcolor\x1b[0m",
        "\x1b]2;chunked title\x07",
        "\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\",
        "\x1b[2;5Hpositioned",
        "\x1b[?1049halt\r\nalternate\x1b[?1049l",
        "\x1b[3;1Hdone"
    )
    .as_bytes();
    let mut whole = terminal(32, 6);
    whole.feed(bytes);
    let whole_events = whole.drain_events();
    let whole_frame = whole.frame_update(true);

    let mut chunked = terminal(32, 6);
    let mut offset = 0;
    let mut chunk_size = 1;
    while offset < bytes.len() {
        let end = (offset + chunk_size).min(bytes.len());
        chunked.feed(&bytes[offset..end]);
        offset = end;
        chunk_size = chunk_size % 11 + 1;
    }

    assert_eq!(chunked.drain_events(), whole_events);
    let chunked_frame = chunked.frame_update(true);
    assert_eq!(chunked_frame.row_updates, whole_frame.row_updates);
    assert_eq!(chunked_frame.cursor, whole_frame.cursor);
}

#[test]
fn shell_prompt_redraw_erases_stale_input_tail() {
    let mut terminal = terminal(32, 3);
    terminal.feed(b"$ ccargo run --releasec");
    let _ = terminal.frame_update(false);
    terminal.feed(b"\r\x1b[2K$ cargo run --release");

    let update = terminal.frame_update(true);
    let row: String = update.row_updates[0]
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    assert_eq!(row.trim_end(), "$ cargo run --release");
}

#[test]
fn terminfo_clear_sequence_removes_all_previous_screen_content() {
    let mut terminal = terminal(20, 3);
    terminal.feed(b"old prompt\r\nstale tail");
    terminal.feed(b"\x1b[H\x1b[2J$ ");

    let update = terminal.frame_update(true);
    let rows: Vec<String> = update
        .row_updates
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.text.as_str()).collect())
        .collect();
    assert!(rows[0].starts_with("$ "));
    assert!(rows[0][2..].trim().is_empty());
    assert!(rows[1..].iter().all(|row| row.trim().is_empty()));
}

#[test]
fn repeated_resize_keeps_every_frame_rectangular() {
    let mut terminal = terminal(8, 3);
    terminal.feed("ab界cd\r\nsecond".as_bytes());
    for (columns, rows) in [(3, 1), (40, 10), (2, 2), (9, 4), (80, 24)] {
        terminal.resize(columns, rows);
        assert_eq!(terminal.dimensions(), (columns.max(2), rows.max(1)));
        let update = terminal.frame_update(true);
        assert_eq!(update.columns, columns.max(2));
        assert_eq!(update.rows, rows.max(1));
        assert_eq!(update.row_updates.len(), rows.max(1));
        assert!(update.row_updates.iter().all(|row| {
            row.start_column == 0
                && row.cells.len() == columns.max(2)
                && !row
                    .cells
                    .first()
                    .is_some_and(|cell| cell.flags.contains(engine::CellFlags::WIDE_SPACER))
                && !row
                    .cells
                    .last()
                    .is_some_and(|cell| cell.flags.contains(engine::CellFlags::WIDE))
        }));
    }
}

#[test]
fn shrinking_rows_keeps_the_cursor_and_nearby_prompt_content() {
    let mut terminal = terminal(8, 4);
    terminal.feed(b"one\r\ntwo\r\nthree\r\nfour");
    terminal.resize(8, 2);

    let update = terminal.frame_update(true);
    let rows: Vec<String> = update
        .row_updates
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.text.as_str()).collect())
        .collect();
    assert!(rows[0].starts_with("three"));
    assert!(rows[1].starts_with("four"));
    assert_eq!(update.cursor.row, 1);
}

#[test]
fn sgr_truecolor_and_osc_hyperlinks_are_stored_on_cells() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b[38;2;1;2;3m\x1b]8;;https://example.com\x1b\\X\x1b]8;;\x1b\\");
    let update = terminal.frame_update(false);
    let cell = &update.row_updates[0].cells[0];
    assert_eq!(cell.text, "X");
    assert_eq!(cell.foreground, Color::Rgb(1, 2, 3));
    assert_eq!(
        cell.hyperlink,
        Some(Arc::new("https://example.com".to_owned()))
    );
    assert_eq!(
        terminal.hyperlink_at(SelectionPoint { column: 0, row: 0 }),
        Some(Arc::new("https://example.com".to_owned()))
    );
    assert_eq!(
        terminal.hyperlink_at(SelectionPoint { column: 1, row: 0 }),
        None
    );
}

#[test]
fn osc_title_directory_and_clipboard_become_events() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b]2;project\x07\x1b]7;file:///tmp\x1b\\\x1b]52;c;aGVsbG8=\x1b\\");
    assert_eq!(
        terminal.drain_events(),
        vec![
            TerminalEvent::Title("project".to_owned()),
            TerminalEvent::CurrentDirectory("file:///tmp".to_owned()),
            TerminalEvent::ClipboardStore {
                selection: "c".to_owned(),
                text: "hello".to_owned(),
            },
        ]
    );
}

#[test]
fn oversized_osc_metadata_is_ignored_without_leaking_prior_hyperlinks() {
    let mut terminal = terminal(8, 3);
    let oversized_title = "t".repeat(1_025);
    let oversized_directory = "d".repeat(4_097);
    let oversized_pointer = "p".repeat(513);
    let oversized_selection = "c".repeat(17);
    terminal.feed(format!("\x1b]2;{oversized_title}\x07").as_bytes());
    terminal.feed(format!("\x1b]7;{oversized_directory}\x07").as_bytes());
    terminal.feed(format!("\x1b]22;{oversized_pointer}\x07").as_bytes());
    terminal.feed(format!("\x1b]52;{oversized_selection};aGVsbG8=\x07").as_bytes());
    assert!(terminal.drain_events().is_empty());

    terminal.feed(b"\x1b]8;;https://example.com\x1b\\A");
    assert!(
        terminal
            .hyperlink_at(SelectionPoint { column: 0, row: 0 })
            .is_some()
    );
    let oversized_hyperlink = "h".repeat(8_193);
    terminal.feed(format!("\x1b]8;;{oversized_hyperlink}\x1b\\B").as_bytes());
    assert_eq!(
        terminal.hyperlink_at(SelectionPoint { column: 1, row: 0 }),
        None,
        "an invalid hyperlink must clear the previous hyperlink state"
    );
}

#[test]
fn osc_window_titles_drop_embedded_control_characters() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b]2;safe\n\t title\x07");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::Title("safe title".to_owned())]
    );
}

#[test]
fn alternate_screen_does_not_destroy_main_screen() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"main\x1b[?1049halt\x1b[?1049l");
    let update = terminal.frame_update(true);
    let text: String = update.row_updates[0]
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    assert!(text.starts_with("main"));
}

#[test]
fn scrollback_can_be_viewed_without_mutating_live_rows() {
    let mut terminal = terminal(4, 2);
    terminal.feed(b"one\r\ntwo\r\ntri");
    terminal.scroll_display(1);
    let history = terminal.frame_update(true);
    let first: String = history.row_updates[0]
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    assert!(first.starts_with("one"));
    assert_eq!(history.display_offset, 1);

    terminal.scroll_to_bottom();
    let live = terminal.frame_update(true);
    let last: String = live.row_updates[1]
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    assert!(last.starts_with("tri"));
}

#[test]
fn runtime_scrollback_shrink_releases_capacity_without_changing_live_rows() {
    let mut terminal = Terminal::new(TerminalConfig {
        columns: 8,
        rows: 2,
        scrollback_limit: 16,
    });
    let _ = terminal.frame_update(true);
    for _ in 0..32 {
        terminal.feed(b"line\r\n");
    }
    let before_frame = terminal.frame_update(true);
    let before = terminal.memory_stats();
    assert_eq!(before.scrollback_rows, 16);

    terminal.set_scrollback_limit(2);
    let after = terminal.memory_stats();
    assert_eq!(after.scrollback_rows, 2);
    assert!(after.scrollback_cell_capacity < before.scrollback_cell_capacity);
    assert!(after.cell_capacity_bytes() < before.cell_capacity_bytes());
    assert!(!terminal.frame_update(false).has_damage());

    let after_frame = terminal.frame_update(true);
    assert_eq!(after_frame.row_updates, before_frame.row_updates);

    terminal.set_scrollback_limit(0);
    let empty = terminal.memory_stats();
    assert_eq!(empty.scrollback_rows, 0);
    assert_eq!(empty.scrollback_cell_capacity, 0);
}

#[test]
fn kitty_keyboard_modes_set_push_pop_query_and_report_events() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b[=3u");
    let press = terminal.encode_key(&KeyEvent {
        key: Key::Character('a'),
        text: Some("a".to_owned()),
        shifted_key: None,
        base_layout: None,
        modifiers: Modifiers::CONTROL,
        kind: KeyEventKind::Press,
    });
    assert_eq!(press, b"\x1b[97;5:1u");

    let release = terminal.encode_key(&KeyEvent {
        key: Key::Character('a'),
        text: Some("a".to_owned()),
        shifted_key: None,
        base_layout: None,
        modifiers: Modifiers::CONTROL,
        kind: KeyEventKind::Release,
    });
    assert_eq!(release, b"\x1b[97;5:3u");

    terminal.feed(b"\x1b[>8u\x1b[?u");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::Reply(b"\x1b[?8u".to_vec())]
    );
    terminal.feed(b"\x1b[<u\x1b[?u");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::Reply(b"\x1b[?3u".to_vec())]
    );
}

#[test]
fn kitty_keyboard_mode_values_mask_unknown_bits_before_set_and_push() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b[=1u\x1b[=32;2u\x1b[?u");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::Reply(b"\x1b[?1u".to_vec())]
    );

    terminal.feed(b"\x1b[=33u\x1b[?u\x1b[>32u\x1b[?u\x1b[<u\x1b[?u");
    assert_eq!(
        terminal.drain_events(),
        vec![
            TerminalEvent::Reply(b"\x1b[?1u".to_vec()),
            TerminalEvent::Reply(b"\x1b[?0u".to_vec()),
            TerminalEvent::Reply(b"\x1b[?1u".to_vec()),
        ]
    );
}

#[test]
fn xterm_modify_other_keys_translates_x_modifier_exclusion_masks() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b[>4;2m\x1b[>4:8m\x1b[?4m");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::Reply(b"\x1b[>4;2m".to_vec())]
    );
    assert_eq!(
        terminal.encode_key(&KeyEvent {
            key: Key::Character('a'),
            text: Some("a".to_owned()),
            shifted_key: None,
            base_layout: None,
            modifiers: Modifiers::ALT,
            kind: KeyEventKind::Press,
        }),
        b"\x1ba"
    );

    terminal.feed(b"\x1b[>4:4m");
    assert_eq!(
        terminal.encode_key(&KeyEvent {
            key: Key::Character('.'),
            text: Some(".".to_owned()),
            shifted_key: None,
            base_layout: None,
            modifiers: Modifiers::CONTROL,
            kind: KeyEventKind::Press,
        }),
        b"."
    );

    terminal.feed(b"\x1b[>4:1m");
    assert_eq!(
        terminal.encode_key(&KeyEvent {
            key: Key::Character('a'),
            text: Some("A".to_owned()),
            shifted_key: None,
            base_layout: None,
            modifiers: Modifiers::SHIFT,
            kind: KeyEventKind::Press,
        }),
        b"A"
    );
}

#[test]
fn bracketed_paste_and_sgr_mouse_follow_negotiated_modes() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b[?2004h\x1b[?1000h\x1b[?1006h");
    assert_eq!(terminal.encode_paste("hello"), b"\x1b[200~hello\x1b[201~");
    let mouse = terminal
        .encode_mouse(MouseEvent {
            button: MouseButton::Left,
            kind: MouseEventKind::Press,
            column: 1,
            row: 2,
            pixel_x: 0,
            pixel_y: 0,
            modifiers: Modifiers::empty(),
        })
        .expect("mouse tracking enabled");
    assert_eq!(mouse, b"\x1b[<0;2;3M");
}

#[test]
fn sgr_mouse_release_preserves_the_released_button_code() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b[?1002h\x1b[?1006h");

    let release = terminal
        .encode_mouse(MouseEvent {
            button: MouseButton::Right,
            kind: MouseEventKind::Release,
            column: 4,
            row: 1,
            pixel_x: 0,
            pixel_y: 0,
            modifiers: Modifiers::SHIFT,
        })
        .expect("mouse tracking enabled");

    assert_eq!(release, b"\x1b[<6;5;2m");
}

#[test]
fn any_motion_reports_buttonless_motion_and_press_mode_filters_motion() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b[?1003h\x1b[?1006h");
    let motion = terminal
        .encode_mouse(MouseEvent {
            button: MouseButton::None,
            kind: MouseEventKind::Motion,
            column: 2,
            row: 1,
            pixel_x: 0,
            pixel_y: 0,
            modifiers: Modifiers::empty(),
        })
        .expect("any-motion tracking enabled");
    assert_eq!(motion, b"\x1b[<35;3;2M");

    terminal.feed(b"\x1b[?1003l\x1b[?1000h");
    assert_eq!(
        terminal.encode_mouse(MouseEvent {
            button: MouseButton::None,
            kind: MouseEventKind::Motion,
            column: 2,
            row: 1,
            pixel_x: 0,
            pixel_y: 0,
            modifiers: Modifiers::empty(),
        }),
        None
    );
}

#[test]
fn sgr_pixel_mouse_negotiates_reports_status_and_uses_physical_coordinates() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b[?1016$p");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::Reply(b"\x1b[?1016;2$y".to_vec())]
    );

    terminal.feed(b"\x1b[?1003;1016h\x1b[?1016$p");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::Reply(b"\x1b[?1016;1$y".to_vec())]
    );
    let motion = terminal
        .encode_mouse(MouseEvent {
            button: MouseButton::None,
            kind: MouseEventKind::Motion,
            column: 2,
            row: 1,
            pixel_x: 40,
            pixel_y: 25,
            modifiers: Modifiers::empty(),
        })
        .expect("pixel mouse tracking enabled");
    assert_eq!(motion, b"\x1b[<35;41;26M");
}

#[test]
fn pixel_size_query_reports_the_host_surface_dimensions() {
    let mut terminal = terminal(8, 3);
    terminal.set_pixel_size(1280, 720);
    terminal.feed(b"\x1b[14t");

    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::Reply(b"\x1b[4;720;1280t".to_vec())]
    );
}

#[test]
fn osc_22_emits_only_supported_mouse_pointer_shapes_and_reset() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b]22;pointer\x1b\\\x1b]22;not-a-real-shape\x07\x1b]22;\x1b\\");

    assert_eq!(
        terminal.drain_events(),
        vec![
            TerminalEvent::MousePointerShape(MousePointerShape::Pointer),
            TerminalEvent::MousePointerShape(MousePointerShape::Text),
        ]
    );
}

#[test]
fn osc_22_set_push_pop_and_empty_reset_follow_the_pointer_stack() {
    let mut terminal = terminal(8, 3);

    terminal.feed(b"\x1b]22;>pointer,wait\x1b\\");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::MousePointerShape(MousePointerShape::Wait)]
    );

    // A simple set replaces the top entry rather than discarding the stack below it.
    terminal.feed(b"\x1b]22;=crosshair\x1b\\\x1b]22;<ignored,names\x1b\\");
    assert_eq!(
        terminal.drain_events(),
        vec![
            TerminalEvent::MousePointerShape(MousePointerShape::Crosshair),
            TerminalEvent::MousePointerShape(MousePointerShape::Pointer),
        ]
    );

    // Empty set means no application shape. The terminal's own text cursor is restored,
    // whereas the explicit CSS `default` shape remains an arrow.
    terminal.feed(b"\x1b]22;\x1b\\\x1b]22;?__current__\x1b\\");
    assert_eq!(
        terminal.drain_events(),
        vec![
            TerminalEvent::MousePointerShape(MousePointerShape::Text),
            TerminalEvent::Reply(b"\x1b]22;0\x1b\\".to_vec()),
        ]
    );
    terminal.feed(b"\x1b]22;default\x1b\\");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::MousePointerShape(MousePointerShape::Default)]
    );
}

#[test]
fn osc_22_queries_current_defaults_and_supported_shapes() {
    let mut terminal = terminal(8, 3);
    terminal.feed(
        b"\x1b]22;?__current__,__default__,__grabbed__\x1b\\\
          \x1b]22;?pointer,crosshair,no-such-name,wait\x1b\\",
    );

    assert_eq!(
        terminal.drain_events(),
        vec![
            TerminalEvent::Reply(b"\x1b]22;0,text,default\x1b\\".to_vec()),
            TerminalEvent::Reply(b"\x1b]22;1,1,0,1\x1b\\".to_vec()),
        ]
    );
}

#[test]
fn osc_22_main_and_alternate_screens_have_independent_stacks() {
    let mut terminal = terminal(8, 3);
    terminal.feed(
        b"\x1b]22;>pointer\x1b\\\
          \x1b[?1049h\
          \x1b]22;>wait\x1b\\\
          \x1b[?1049l\
          \x1b[?1049h\
          \x1b]22;<\x1b\\\
          \x1b[?1049l",
    );

    assert_eq!(
        terminal.drain_events(),
        vec![
            TerminalEvent::MousePointerShape(MousePointerShape::Pointer),
            TerminalEvent::MousePointerShape(MousePointerShape::Text),
            TerminalEvent::MousePointerShape(MousePointerShape::Wait),
            TerminalEvent::MousePointerShape(MousePointerShape::Pointer),
            TerminalEvent::MousePointerShape(MousePointerShape::Wait),
            TerminalEvent::MousePointerShape(MousePointerShape::Text),
            TerminalEvent::MousePointerShape(MousePointerShape::Pointer),
        ]
    );
}

#[test]
fn osc_22_stack_keeps_the_latest_sixteen_shapes() {
    let mut terminal = terminal(8, 3);
    terminal.feed(
        b"\x1b]22;>alias,cell,copy,crosshair,default,e-resize,ew-resize,grab,grabbing,help,move,n-resize,ne-resize,nesw-resize,no-drop,not-allowed,ns-resize\x1b\\",
    );
    let _ = terminal.drain_events();

    // Fifteen pops leave the second supplied entry (`cell`) because the first was evicted.
    for _ in 0..15 {
        terminal.feed(b"\x1b]22;<\x1b\\");
    }
    terminal.feed(b"\x1b]22;?__current__\x1b\\");
    let events = terminal.drain_events();
    assert_eq!(
        events.last(),
        Some(&TerminalEvent::Reply(b"\x1b]22;cell\x1b\\".to_vec()))
    );

    terminal.feed(b"\x1b]22;<\x1b\\\x1b]22;?__current__\x1b\\");
    assert_eq!(
        terminal.drain_events(),
        vec![
            TerminalEvent::MousePointerShape(MousePointerShape::Text),
            TerminalEvent::Reply(b"\x1b]22;0\x1b\\".to_vec()),
        ]
    );

    // Further pops are harmless, but still re-assert the terminal's own cursor.
    terminal.feed(b"\x1b]22;<\x1b\\");
    assert_eq!(
        terminal.drain_events(),
        vec![TerminalEvent::MousePointerShape(MousePointerShape::Text)]
    );
}

#[test]
fn terminal_reset_clears_both_osc_22_stacks() {
    let mut terminal = terminal(8, 3);
    terminal.feed(
        b"\x1b]22;pointer\x1b\\\x1b[?1049h\x1b]22;wait\x1b\\\x1bc\
          \x1b]22;?__current__\x1b\\\x1b[?1049h\x1b]22;?__current__\x1b\\",
    );

    let events = terminal.drain_events();
    assert!(events.contains(&TerminalEvent::MousePointerShape(MousePointerShape::Text)));
    let replies = events
        .into_iter()
        .filter_map(|event| match event {
            TerminalEvent::Reply(reply) => Some(reply),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        replies,
        vec![b"\x1b]22;0\x1b\\".to_vec(), b"\x1b]22;0\x1b\\".to_vec(),]
    );
}

#[test]
fn selection_normalizes_reverse_drag_updates_frames_and_extracts_text() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"alpha\r\nbeta\r\ngamma");
    let _ = terminal.frame_update(false);

    terminal.begin_selection(SelectionPoint { column: 3, row: 1 });
    terminal.update_selection(SelectionPoint { column: 2, row: 0 });

    let update = terminal.frame_update(false);
    assert!(update.metadata_changed);
    assert_eq!(
        update.selection,
        Some(SelectionRange {
            start: SelectionPoint { column: 2, row: 0 },
            end: SelectionPoint { column: 3, row: 1 },
        })
    );
    assert_eq!(terminal.selected_text().as_deref(), Some("pha\nbeta"));

    terminal.clear_selection();
    assert_eq!(terminal.selected_text(), None);
    assert_eq!(terminal.frame_update(false).selection, None);
}

#[test]
fn word_selection_expands_terminal_paths_and_drags_by_whole_words() {
    let mut terminal = terminal(32, 1);
    terminal.feed(b"alpha /tmp/foo-bar.rs tail");

    terminal.begin_selection_with_mode(SelectionPoint { column: 10, row: 0 }, SelectionMode::Word);
    assert_eq!(terminal.selected_text().as_deref(), Some("/tmp/foo-bar.rs"));
    assert_eq!(
        terminal.frame_update(false).selection,
        Some(SelectionRange {
            start: SelectionPoint { column: 6, row: 0 },
            end: SelectionPoint { column: 20, row: 0 },
        })
    );

    terminal.update_selection(SelectionPoint { column: 24, row: 0 });
    assert_eq!(
        terminal.selected_text().as_deref(),
        Some("/tmp/foo-bar.rs tail")
    );
}

#[test]
fn word_selection_treats_wide_glyph_spacers_as_part_of_the_word() {
    let mut terminal = terminal(12, 1);
    terminal.feed("界面 test".as_bytes());

    terminal.begin_selection_with_mode(SelectionPoint { column: 1, row: 0 }, SelectionMode::Word);

    assert_eq!(terminal.selected_text().as_deref(), Some("界面"));
    assert_eq!(
        terminal.frame_update(false).selection,
        Some(SelectionRange {
            start: SelectionPoint { column: 0, row: 0 },
            end: SelectionPoint { column: 3, row: 0 },
        })
    );
}

#[test]
fn line_selection_expands_and_drags_by_whole_visible_lines() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"one\r\ntwo\r\nthree");

    terminal.begin_selection_with_mode(SelectionPoint { column: 2, row: 1 }, SelectionMode::Line);
    assert_eq!(terminal.selected_text().as_deref(), Some("two"));

    terminal.update_selection(SelectionPoint { column: 1, row: 0 });
    assert_eq!(terminal.selected_text().as_deref(), Some("one\ntwo"));
    assert_eq!(
        terminal.frame_update(false).selection,
        Some(SelectionRange {
            start: SelectionPoint { column: 0, row: 0 },
            end: SelectionPoint { column: 7, row: 1 },
        })
    );
}

#[test]
fn selection_order_is_row_major_and_copy_survives_lazy_history_resize() {
    let mut terminal = terminal(8, 2);
    terminal.feed(b"one\r\ntwo\r\ntri");
    terminal.scroll_display(1);
    terminal.begin_selection(SelectionPoint { column: 0, row: 1 });
    terminal.update_selection(SelectionPoint { column: 7, row: 0 });
    assert_eq!(
        terminal.frame_update(false).selection,
        Some(SelectionRange {
            start: SelectionPoint { column: 7, row: 0 },
            end: SelectionPoint { column: 0, row: 1 },
        })
    );

    terminal.clear_selection();
    terminal.resize(3, 2);
    terminal.begin_selection(SelectionPoint { column: 0, row: 0 });
    terminal.update_selection(SelectionPoint { column: 2, row: 0 });
    assert_eq!(terminal.selected_text().as_deref(), Some("one"));
}

#[test]
fn selection_normalizes_wide_spacers_for_copy_in_both_directions() {
    let cases = [
        (
            SelectionPoint { column: 1, row: 0 },
            SelectionPoint { column: 1, row: 0 },
            SelectionRange {
                start: SelectionPoint { column: 0, row: 0 },
                end: SelectionPoint { column: 0, row: 0 },
            },
            "界",
        ),
        (
            SelectionPoint { column: 1, row: 0 },
            SelectionPoint { column: 2, row: 0 },
            SelectionRange {
                start: SelectionPoint { column: 0, row: 0 },
                end: SelectionPoint { column: 2, row: 0 },
            },
            "界A",
        ),
        (
            SelectionPoint { column: 2, row: 0 },
            SelectionPoint { column: 1, row: 0 },
            SelectionRange {
                start: SelectionPoint { column: 0, row: 0 },
                end: SelectionPoint { column: 2, row: 0 },
            },
            "界A",
        ),
    ];

    for (anchor, head, expected_range, expected_text) in cases {
        let mut terminal = terminal(4, 1);
        terminal.feed("界A".as_bytes());
        let _ = terminal.frame_update(false);
        terminal.begin_selection(anchor);
        terminal.update_selection(head);

        assert_eq!(terminal.frame_update(false).selection, Some(expected_range));
        assert_eq!(terminal.selected_text().as_deref(), Some(expected_text));
    }
}

#[test]
fn output_scroll_clears_viewport_relative_selection_before_copy_can_drift() {
    let mut terminal = terminal(4, 2);
    terminal.feed(b"one\r\ntwo");
    let _ = terminal.frame_update(false);
    terminal.begin_selection(SelectionPoint { column: 0, row: 0 });
    terminal.update_selection(SelectionPoint { column: 2, row: 0 });
    let _ = terminal.frame_update(false);
    assert_eq!(terminal.selected_text().as_deref(), Some("one"));

    terminal.feed(b"\r\nnew");

    assert_eq!(terminal.selected_text(), None);
    let update = terminal.frame_update(false);
    assert!(update.metadata_changed);
    assert_eq!(update.selection, None);
}

#[test]
fn ordinary_output_preserves_selection_until_viewport_rows_move() {
    let mut terminal = terminal(4, 2);
    terminal.feed(b"one");
    terminal.begin_selection(SelectionPoint { column: 0, row: 0 });
    terminal.update_selection(SelectionPoint { column: 2, row: 0 });
    assert_eq!(terminal.selected_text().as_deref(), Some("one"));

    terminal.feed(b"\x1b[2;1HX");

    assert_eq!(terminal.selected_text().as_deref(), Some("one"));
}

#[test]
fn actual_grid_resize_clears_viewport_relative_selection() {
    let mut terminal = terminal(8, 2);
    terminal.feed(b"selected");
    terminal.begin_selection(SelectionPoint { column: 0, row: 0 });
    terminal.update_selection(SelectionPoint { column: 7, row: 0 });
    assert_eq!(terminal.selected_text().as_deref(), Some("selected"));

    terminal.resize(8, 2);
    assert_eq!(terminal.selected_text().as_deref(), Some("selected"));

    terminal.resize(7, 2);
    assert_eq!(terminal.selected_text(), None);
    assert_eq!(terminal.frame_update(false).selection, None);
}

#[test]
fn full_buffer_search_reveals_highlights_and_advances_through_scrollback() {
    let mut terminal = terminal(16, 3);
    terminal.feed(b"Alpha one\r\nmiddle\r\nalpha two\r\nlast alpha\r\nend");
    let _ = terminal.frame_update(false);

    let latest = terminal
        .search_with_options("ALPHA", SearchOptions::default())
        .unwrap();
    assert_eq!(latest.display_offset, 0);
    assert_eq!(latest.range.start, SelectionPoint { column: 5, row: 1 });
    assert_eq!(terminal.selected_text().as_deref(), Some("alpha"));

    let previous = terminal
        .search_with_options("ALPHA", SearchOptions::default())
        .unwrap();
    assert_eq!(previous.range.start, SelectionPoint { column: 0, row: 0 });
    assert_eq!(previous.display_offset, 0);

    let history = terminal
        .search_with_options("ALPHA", SearchOptions::default())
        .unwrap();
    assert_eq!(history.display_offset, 2);
    assert_eq!(history.range.start, SelectionPoint { column: 0, row: 0 });
    let update = terminal.frame_update(false);
    assert!(!update.full);
    assert_eq!(update.row_moves.len(), 1);
    assert_eq!(update.row_moves[0].count, 2);
    assert_eq!(update.row_updates.len(), 2);
    assert_eq!(update.selection, Some(history.range));

    let wrapped = terminal
        .search_with_options("ALPHA", SearchOptions::default())
        .unwrap();
    assert_eq!(wrapped.display_offset, 0);
    assert_eq!(wrapped.range.start, SelectionPoint { column: 5, row: 1 });
}

#[test]
fn search_direction_case_sensitivity_and_no_wrap_are_explicit() {
    let mut terminal = terminal(12, 2);
    terminal.feed(b"Alpha\r\nalpha");

    let options = SearchOptions {
        direction: SearchDirection::Forward,
        case_sensitive: true,
        wrap: false,
    };
    let first = terminal.search_with_options("Alpha", options).unwrap();
    assert_eq!(first.range.start, SelectionPoint { column: 0, row: 0 });
    assert_eq!(terminal.search_with_options("Alpha", options), None);

    let lowercase = terminal.search_with_options("alpha", options).unwrap();
    assert_eq!(lowercase.range.start, SelectionPoint { column: 0, row: 1 });
}

#[test]
fn search_maps_unicode_and_wide_glyphs_back_to_terminal_cells() {
    let mut terminal = terminal(12, 1);
    terminal.feed("start 界面 END".as_bytes());

    let found = terminal
        .search_with_options("界面", SearchOptions::default())
        .unwrap();
    assert_eq!(
        found.range,
        SelectionRange {
            start: SelectionPoint { column: 6, row: 0 },
            end: SelectionPoint { column: 9, row: 0 },
        }
    );
    assert_eq!(terminal.selected_text().as_deref(), Some("界面"));
}

#[test]
fn terminal_output_invalidates_a_search_highlight_but_not_an_ordinary_selection() {
    let mut terminal = terminal(12, 2);
    terminal.feed(b"find me");
    terminal.search_with_options("find", SearchOptions::default());
    assert_eq!(terminal.selected_text().as_deref(), Some("find"));

    terminal.feed(b"!");
    assert_eq!(terminal.selected_text(), None);

    terminal.begin_selection(SelectionPoint { column: 0, row: 0 });
    terminal.update_selection(SelectionPoint { column: 3, row: 0 });
    terminal.feed(b"x");
    assert_eq!(terminal.selected_text().as_deref(), Some("find"));
}

#[test]
fn device_queries_generate_replies() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"abc\x1b[6n\x1b[c");
    assert_eq!(
        terminal.drain_events(),
        vec![
            TerminalEvent::Reply(b"\x1b[1;4R".to_vec()),
            TerminalEvent::Reply(b"\x1b[?62;22c".to_vec()),
        ]
    );
}

#[test]
fn synchronized_output_defers_partial_frames_until_the_mode_is_released() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b[?2026hhello");
    assert!(!terminal.frame_update(false).has_damage());

    terminal.feed(b"\x1b[?2026l");
    let update = terminal.frame_update(false);
    assert!(update.has_damage());
    assert_eq!(update.row_updates[0].cells[0].text, "h");
}

#[test]
fn opentui_capability_handshake_gets_immediate_terminal_replies() {
    let mut terminal = terminal(80, 24);
    terminal.feed(
        concat!(
            "\x1b]10;?\x07",
            "\x1b]11;?\x07",
            "\x1b[>0q",
            "\x1bP+q4d73\x1b\\",
            "\x1b[?1016$p",
            "\x1b[?2027$p",
            "\x1b[?2031$p",
            "\x1b[?1004$p",
            "\x1b[?2004$p",
            "\x1b[?2026$p",
            "\x1b[?u",
        )
        .as_bytes(),
    );

    assert_eq!(
        terminal.drain_events(),
        vec![
            TerminalEvent::Reply(b"\x1b]10;rgb:d8d8/dede/e9e9\x1b\\".to_vec()),
            TerminalEvent::Reply(b"\x1b]11;rgb:0b0b/0d0d/0f0f\x1b\\".to_vec()),
            TerminalEvent::Reply(b"\x1bP>|Tmon 0.1.0\x1b\\".to_vec()),
            TerminalEvent::Reply(b"\x1bP0+r4d73\x1b\\".to_vec()),
            TerminalEvent::Reply(b"\x1b[?1016;2$y".to_vec()),
            TerminalEvent::Reply(b"\x1b[?2027;0$y".to_vec()),
            TerminalEvent::Reply(b"\x1b[?2031;0$y".to_vec()),
            TerminalEvent::Reply(b"\x1b[?1004;2$y".to_vec()),
            TerminalEvent::Reply(b"\x1b[?2004;2$y".to_vec()),
            TerminalEvent::Reply(b"\x1b[?2026;2$y".to_vec()),
            TerminalEvent::Reply(b"\x1b[?0u".to_vec()),
        ]
    );
}

#[test]
fn opentui_modify_other_keys_setup_does_not_leak_sgr_styles_into_the_frame() {
    let mut terminal = terminal(16, 3);
    terminal.feed(
        concat!(
            "\x1b[?1049h",
            "\x1b[>4;1m",
            "\x1b[?2026h",
            "\x1b[1;1H",
            "\x1b[38;2;238;238;238m",
            "\x1b[48;2;10;10;10m",
            "OpenCode",
            "\x1b[0m",
            "\x1b[?2026l",
        )
        .as_bytes(),
    );

    let update = terminal.frame_update(true);
    let row = &update.row_updates[0].cells;
    assert_eq!(row[0].text, "O");
    assert_eq!(row[7].text, "e");
    assert_eq!(row[0].foreground, Color::Rgb(238, 238, 238));
    assert_eq!(row[0].background, Color::Rgb(10, 10, 10));
    assert!(!row[0].flags.intersects(
        engine::CellFlags::BOLD
            | engine::CellFlags::UNDERLINE
            | engine::CellFlags::DOUBLE_UNDERLINE
    ));
}

#[test]
fn opentui_dynamic_colors_set_query_and_reset_consistently() {
    let mut terminal = terminal(16, 3);
    terminal.feed(
        concat!(
            "\x1b]11;#1e1e1e\x1b\\",
            "\x1b]11;?\x07",
            "\x1b]12;rgb:ff/80/00\x1b\\",
            "\x1b]12;?\x07",
            "\x1b]111\x07",
            "\x1b]11;?\x07",
            "\x1b]112\x07",
        )
        .as_bytes(),
    );

    assert_eq!(
        terminal.drain_events(),
        vec![
            TerminalEvent::SetDynamicColor {
                target: DynamicColor::Background,
                color: [30, 30, 30],
            },
            TerminalEvent::Reply(b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\".to_vec()),
            TerminalEvent::SetDynamicColor {
                target: DynamicColor::Cursor,
                color: [255, 128, 0],
            },
            TerminalEvent::Reply(b"\x1b]12;rgb:ffff/8080/0000\x1b\\".to_vec()),
            TerminalEvent::ResetDynamicColor {
                target: DynamicColor::Background,
            },
            TerminalEvent::Reply(b"\x1b]11;rgb:0b0b/0d0d/0f0f\x1b\\".to_vec()),
            TerminalEvent::ResetDynamicColor {
                target: DynamicColor::Cursor,
            },
        ]
    );
}

#[test]
fn decscusr_preserves_steady_and_blinking_cursor_styles() {
    let mut terminal = terminal(8, 3);

    for (sequence, shape, blinking) in [
        ("\x1b[1 q", engine::CursorShape::Block, true),
        ("\x1b[2 q", engine::CursorShape::Block, false),
        ("\x1b[3 q", engine::CursorShape::Underline, true),
        ("\x1b[4 q", engine::CursorShape::Underline, false),
        ("\x1b[5 q", engine::CursorShape::Bar, true),
        ("\x1b[6 q", engine::CursorShape::Bar, false),
    ] {
        terminal.feed(sequence.as_bytes());
        let cursor = terminal.frame_update(false).cursor;
        assert_eq!(cursor.shape, shape, "sequence {sequence:?}");
        assert_eq!(cursor.blinking, blinking, "sequence {sequence:?}");
    }
}

#[test]
fn primary_device_attributes_do_not_advertise_unimplemented_graphics() {
    let mut terminal = terminal(8, 3);
    terminal.feed(b"\x1b[c");

    let events = terminal.drain_events();
    assert_eq!(events, vec![TerminalEvent::Reply(b"\x1b[?62;22c".to_vec())]);
    let TerminalEvent::Reply(reply) = &events[0] else {
        unreachable!("asserted reply event above");
    };
    assert!(!reply.windows(2).any(|window| window == b";4"));
    assert!(!reply.windows(2).any(|window| window == b";6"));
}

#[test]
fn unsupported_osc_66_probe_has_no_artifacts_and_forces_opentui_fallback() {
    let mut terminal = terminal(12, 3);
    terminal
        .feed(b"base\x1b[s\x1b[H\x1b]66;w=1; \x1b\\\x1b[6n\x1b[H\x1b]66;s=2; \x1b\\\x1b[6n\x1b[u");

    assert_eq!(
        terminal.drain_events(),
        vec![
            TerminalEvent::Reply(b"\x1b[1;1R".to_vec()),
            TerminalEvent::Reply(b"\x1b[1;1R".to_vec()),
        ]
    );
    let frame = terminal.frame_update(true);
    let row: String = frame.row_updates[0]
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    assert!(row.starts_with("base"));
    assert_eq!(frame.cursor.column, 4);
}
