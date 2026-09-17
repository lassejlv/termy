use super::*;

#[test]
fn display_terminal_uses_independent_parser_and_grid() {
    let terminal = Terminal::new_display(
        Size {
            cols: 5,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    terminal.feed_output(b"ok\x1b[31m!");
    let frame = terminal.snapshot();
    assert_eq!(frame.cells[0].character, 'o');
    assert_eq!(frame.cells[1].character, 'k');
    assert_eq!(frame.cells[2].foreground, Color::Indexed(1));
    assert_eq!(terminal.drain_events().0, vec![Event::Wakeup]);
}

#[test]
fn state_snapshot_collects_engine_metadata_from_one_state() {
    let terminal = Terminal::new_display(
        Size {
            cols: 5,
            rows: 2,
            ..Size::default()
        },
        Config {
            scrollback_history: 8,
            ..Config::default()
        },
    );
    terminal.feed_output(b"one\r\ntwo\r\nthree\x1b[?2004h\x1b[?1000h");

    let state = terminal.state_snapshot();

    assert_eq!(state.size, terminal.size());
    assert_eq!(
        (state.display_offset, state.history_size),
        terminal.scroll_state()
    );
    assert_eq!(state.cursor_state, terminal.cursor_state());
    assert!(state.bracketed_paste_mode);
    assert_eq!(
        state.alternate_screen_mode,
        terminal.alternate_screen_mode()
    );
    assert_eq!(state.mouse_mode, terminal.mouse_mode());
    assert_eq!(state.keyboard_mode, terminal.keyboard_mode());
    assert_eq!(state.child_pid, None);
}

#[test]
fn snapshot_and_viewport_metadata_match_the_legacy_visitor() {
    let terminal = Terminal::new_display(
        Size {
            cols: 5,
            rows: 3,
            ..Size::default()
        },
        Config::default(),
    );
    terminal.feed_output("A\u{301}B\r\nC\r\nD\r\nE".as_bytes());
    assert!(terminal.scroll_display(1));

    let mut legacy_cells = Vec::new();
    let legacy_offset = terminal.for_each_viewport_cell(|offset, line, col, cell, combining| {
        legacy_cells.push((
            offset,
            line,
            col,
            *cell,
            combining.map(Combining::to_owned_string),
        ));
    });
    let mut visited_cells = Vec::new();
    let metadata = terminal.visit_viewport_cells(|offset, line, col, cell, combining| {
        visited_cells.push((
            offset,
            line,
            col,
            *cell,
            combining.map(Combining::to_owned_string),
        ));
    });

    assert_eq!(visited_cells, legacy_cells);
    assert_eq!(metadata.display_offset, legacy_offset);
    assert_eq!(
        visited_cells.len(),
        usize::from(metadata.cols) * usize::from(metadata.rows)
    );
    assert!(
        visited_cells
            .iter()
            .all(|(offset, _, _, _, _)| *offset == metadata.display_offset)
    );

    let snapshot = terminal.snapshot();
    let expected_cells = visited_cells
        .iter()
        .map(|(_, _, _, cell, _)| *cell)
        .collect::<Vec<_>>();
    assert_eq!(snapshot.cells, expected_cells);
    assert_eq!(snapshot.cols, metadata.cols);
    assert_eq!(snapshot.rows, metadata.rows);
    assert_eq!(snapshot.cursor, metadata.cursor);
    assert_eq!(snapshot.display_offset, metadata.display_offset);
    assert_eq!(snapshot.history_size, metadata.history_size);

    terminal.feed_output(b"\x1b[?1049hALT");
    let alternate_snapshot = terminal.snapshot();
    let mut alternate_cells = Vec::new();
    let alternate_metadata = terminal.visit_viewport_cells(|_, _, _, cell, _| {
        alternate_cells.push(*cell);
    });
    assert_eq!(alternate_snapshot.cells, alternate_cells);
    assert_eq!(alternate_snapshot.cols, alternate_metadata.cols);
    assert_eq!(alternate_snapshot.rows, alternate_metadata.rows);
    assert_eq!(alternate_snapshot.cursor, alternate_metadata.cursor);
    assert_eq!(
        alternate_snapshot.display_offset,
        alternate_metadata.display_offset
    );
    assert_eq!(
        alternate_snapshot.history_size,
        alternate_metadata.history_size
    );
}

#[test]
fn viewport_metadata_stays_coherent_after_scroll_and_resize() {
    let terminal = Terminal::new_display(
        Size {
            cols: 4,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    terminal.feed_output(b"a\r\nb\r\nc\r\nd");
    assert!(terminal.scroll_display(i32::MAX));

    let mut scrolled_positions = Vec::new();
    let scrolled = terminal.visit_viewport_cells(|offset, line, col, _, _| {
        scrolled_positions.push((offset, line, col));
    });
    assert_eq!(scrolled.display_offset, scrolled.history_size);
    assert_eq!(scrolled.cols, 4);
    assert_eq!(scrolled.rows, 2);
    assert_eq!(scrolled_positions.len(), 8);
    assert!(scrolled_positions.iter().all(|(offset, line, _)| {
        *offset == scrolled.display_offset && *line >= -(scrolled.display_offset as i32)
    }));

    terminal.resize(Size {
        cols: 6,
        rows: 3,
        ..Size::default()
    });
    let mut resized_positions = Vec::new();
    let resized = terminal.visit_viewport_cells(|offset, line, col, _, _| {
        resized_positions.push((offset, line, col));
    });

    assert_eq!(resized.cols, 6);
    assert_eq!(resized.rows, 3);
    assert_eq!(resized_positions.len(), 18);
    assert!(resized.display_offset <= resized.history_size);
    assert!(resized_positions.iter().all(|(offset, line, col)| {
        *offset == resized.display_offset
            && *line >= -(resized.display_offset as i32)
            && *col < usize::from(resized.cols)
    }));
    if let Some(cursor) = resized.cursor {
        assert!(cursor.col < usize::from(resized.cols));
        assert!(cursor.row < usize::from(resized.rows));
    }
}

#[test]
fn line_range_visits_bounds_dimensions_and_cells_from_one_state() {
    let terminal = Terminal::new_display(
        Size {
            cols: 4,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    terminal.feed_output(b"a\r\nb\r\nc\r\nd");

    let mut visited = Vec::new();
    let captured =
        terminal.for_each_line_cell_range(i32::MIN, i32::MAX, |range, line, col, cell, _| {
            visited.push((range, line, col, cell.character));
        });

    assert_eq!(
        captured,
        TerminalLineRange {
            first_line: -2,
            last_line: 1,
            columns: 4,
        }
    );
    assert_eq!(visited.len(), 16);
    assert!(visited.iter().all(|(range, line, col, _)| {
        *range == captured
            && (*line >= captured.first_line && *line <= captured.last_line)
            && *col < captured.columns
    }));
    assert_eq!(
        visited
            .iter()
            .filter(|(_, _, col, _)| *col == 0)
            .map(|(_, line, _, character)| (*line, *character))
            .collect::<Vec<_>>(),
        vec![(-2, 'a'), (-1, 'b'), (0, 'c'), (1, 'd')]
    );
}

#[test]
fn dense_line_callback_materializes_compact_history_for_legacy_callers() {
    let terminal = Terminal::new_display(
        Size {
            cols: 6,
            rows: 2,
            ..Size::default()
        },
        Config {
            scrollback_history: 8,
            ..Config::default()
        },
    );
    terminal.feed_output("A\u{301}B\r\nC\r\nD".as_bytes());
    let first = terminal.line_bounds().0;
    assert!(first < 0);

    let dense = terminal
        .with_line_cells(first, <[Cell]>::to_vec)
        .expect("oldest history line exists");
    let mut visited = Vec::new();
    assert!(terminal.for_each_line_cell(first, |_, cell, _| visited.push(*cell)));

    assert_eq!(dense.len(), 6);
    assert_eq!(dense, visited);
    assert_eq!(dense[0].character, 'A');
    assert_eq!(dense[1].character, 'B');
    assert!(dense[2..].iter().all(|cell| *cell == Cell::default()));
}

#[test]
fn renderer_damage_keeps_scroll_order_while_legacy_damage_stays_full() {
    let size = Size {
        cols: 4,
        rows: 2,
        ..Size::default()
    };
    let renderer = Terminal::new_display(size, Config::default());
    assert!(matches!(
        renderer.take_render_damage_snapshot().damage,
        DamageSnapshot::Full
    ));
    renderer.feed_output(b"a\r\nb\r\nc");
    let update = renderer.take_render_damage_snapshot();
    assert!(matches!(update.damage, DamageSnapshot::Partial(_)));
    assert_eq!(
        update.scrolls,
        vec![ScrollDamage {
            top: 0,
            bottom: 1,
            count: 1,
            direction: ScrollDirection::Up,
        }]
    );

    let legacy = Terminal::new_display(size, Config::default());
    assert_eq!(legacy.take_damage_snapshot(), DamageSnapshot::Full);
    legacy.feed_output(b"a\r\nb\r\nc");
    assert_eq!(legacy.take_damage_snapshot(), DamageSnapshot::Full);
}

#[test]
fn stale_render_generation_refuses_incremental_cell_visits() {
    let terminal = Terminal::new_display(
        Size {
            cols: 4,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    let _ = terminal.take_render_damage_snapshot();
    terminal.feed_output(b"x");
    let update = terminal.take_render_damage_snapshot();
    terminal.feed_output(b"y");

    let mut visited = 0usize;
    assert_eq!(
        terminal.for_each_viewport_range_at_generation(
            update.generation,
            [(0, 0, 1)],
            |_, _, _, _, _, _| visited += 1,
        ),
        None
    );
    assert_eq!(visited, 0);

    let current = terminal.take_render_damage_snapshot();
    assert_eq!(
        terminal.for_each_viewport_range_at_generation(
            current.generation,
            [(0, 0, 1)],
            |_, _, _, _, _, _| visited += 1,
        ),
        Some(0)
    );
    assert_eq!(visited, 2);
}

#[test]
fn coherent_frame_update_visits_and_clears_only_represented_damage() {
    let terminal = Terminal::new_display(
        Size {
            cols: 4,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    let _ = terminal.visit_frame_update_with_options(true, |_, _, _, _, _, _| {});
    terminal.feed_output(b"a\r\nb\r\nc");

    let mut visited = Vec::new();
    let update = terminal.visit_frame_update(|row, _, _, col, cell, _| {
        visited.push((row, col, cell.character));
    });

    assert_eq!(
        update.render.scrolls,
        vec![ScrollDamage {
            top: 0,
            bottom: 1,
            count: 1,
            direction: ScrollDirection::Up,
        }]
    );
    assert!(matches!(update.render.damage, DamageSnapshot::Partial(_)));
    assert_eq!(update.size, terminal.size());
    assert_eq!(update.viewport.cursor, terminal.cursor_state());
    assert_eq!(
        (update.viewport.display_offset, update.viewport.history_size),
        terminal.scroll_state()
    );
    assert_eq!(update.palette, terminal.palette());
    assert!(
        visited
            .iter()
            .any(|(row, _, character)| *row == 0 && *character == 'b')
    );
    assert!(
        visited
            .iter()
            .any(|(row, _, character)| *row == 1 && *character == 'c')
    );

    terminal.feed_output(b"d");
    let next = terminal.visit_frame_update(|_, _, _, _, _, _| {});
    assert_eq!(
        next.render.damage,
        DamageSnapshot::Partial(vec![DirtySpan {
            row: 1,
            left_col: 1,
            right_col: 1,
        }])
    );
    assert!(next.render.scrolls.is_empty());
}

#[test]
fn coherent_frame_update_preserves_damage_when_the_visitor_panics() {
    let terminal = Terminal::new_display(
        Size {
            cols: 4,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    let _ = terminal.visit_frame_update_with_options(true, |_, _, _, _, _, _| {});
    terminal.feed_output(b"a\r\nb\r\nc");
    let expected = terminal
        .engine
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .grid
        .render_damage_snapshot();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        terminal.visit_frame_update(|_, _, _, _, _, _| panic!("visitor failed"));
    }));
    assert!(result.is_err());

    let mut visited = 0;
    let retry = terminal.visit_frame_update(|_, _, _, _, _, _| visited += 1);
    assert_eq!((retry.render.damage, retry.render.scrolls), expected);
    assert!(visited > 0);
}

#[test]
fn coherent_frame_update_holds_output_until_its_visit_finishes() {
    let terminal = Arc::new(Terminal::new_display(
        Size {
            cols: 4,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    ));
    let _ = terminal.visit_frame_update_with_options(true, |_, _, _, _, _, _| {});
    terminal.feed_output(b"a");

    let (start_tx, start_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let update_terminal = terminal.clone();
    let update = std::thread::spawn(move || {
        update_terminal.visit_frame_update(|_, _, _, _, _, _| {
            let _ = start_tx.send(());
            let _ = release_rx.recv();
        })
    });
    start_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("frame visitation should start");

    let (fed_tx, fed_rx) = std::sync::mpsc::channel();
    let output_terminal = terminal.clone();
    let output = std::thread::spawn(move || {
        output_terminal.feed_output(b"b");
        let _ = fed_tx.send(());
    });
    assert!(fed_rx.recv_timeout(Duration::from_millis(50)).is_err());
    release_tx.send(()).unwrap();
    let update = update.join().unwrap();
    fed_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("output should finish after visitation releases the engine lock");
    output.join().unwrap();

    assert_eq!(
        update.render.damage,
        DamageSnapshot::Partial(vec![DirtySpan {
            row: 0,
            left_col: 0,
            right_col: 0,
        }])
    );
    let next = terminal.visit_frame_update(|_, _, _, _, _, _| {});
    assert_eq!(
        next.render.damage,
        DamageSnapshot::Partial(vec![DirtySpan {
            row: 0,
            left_col: 1,
            right_col: 1,
        }])
    );
}

#[test]
fn full_viewport_fallback_consumes_damage_represented_by_the_rebuild() {
    let terminal = Terminal::new_display(
        Size {
            cols: 4,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    let _ = terminal.take_render_damage_snapshot();
    terminal.feed_output(b"a\r\nb\r\nc");

    let mut cells = 0usize;
    let metadata = terminal.visit_viewport_cells_and_clear_damage(|_, _, _, _, _| {
        cells = cells.saturating_add(1);
    });
    assert_eq!(cells, 8);
    assert_eq!(metadata.display_offset, 0);
    let update = terminal.take_render_damage_snapshot();
    assert_eq!(update.damage, DamageSnapshot::Partial(Vec::new()));
    assert!(update.scrolls.is_empty());
}

#[test]
fn full_viewport_fallback_preserves_damage_when_the_visitor_panics() {
    let terminal = Terminal::new_display(
        Size {
            cols: 4,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    let _ = terminal.take_render_damage_snapshot();
    terminal.feed_output(b"a\r\nb\r\nc");
    let expected = terminal
        .engine
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .grid
        .render_damage_snapshot();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        terminal.visit_viewport_cells_and_clear_damage(|_, _, _, _, _| {
            panic!("visitor failed");
        });
    }));
    assert!(result.is_err());
    let preserved = terminal
        .engine
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .grid
        .render_damage_snapshot();
    assert_eq!(preserved, expected);
    assert!(matches!(preserved.0, DamageSnapshot::Partial(_)));
    assert!(!preserved.1.is_empty());

    let mut cells = 0usize;
    terminal.visit_viewport_cells_and_clear_damage(|_, _, _, _, _| {
        cells = cells.saturating_add(1);
    });
    assert_eq!(cells, 8);
    let remaining = terminal.take_render_damage_snapshot();
    assert_eq!(
        (remaining.damage, remaining.scrolls),
        (DamageSnapshot::Partial(Vec::new()), Vec::new())
    );
}

#[test]
fn link_at_expands_same_row_osc8_without_calling_the_classifier() {
    let terminal = Terminal::new_display(
        Size {
            cols: 12,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    terminal.feed_output(b"x\x1b]8;id=docs;https://example.com/docs\x1b\\docs\x1b]8;;\x1b\\ y");

    let link = terminal
        .link_at(0, 2, |_, _| panic!("OSC 8 must bypass classification"))
        .expect("OSC 8 link should resolve");
    assert_eq!(
        link,
        ViewportLink {
            start_row: 0,
            start_col: 1,
            end_row: 0,
            end_col: 4,
            target: "https://example.com/docs".to_string(),
        }
    );
}

#[test]
fn link_at_expands_soft_wrapped_osc8() {
    let terminal = Terminal::new_display(
        Size {
            cols: 5,
            rows: 3,
            ..Size::default()
        },
        Config::default(),
    );
    terminal.feed_output(b"\x1b]8;;https://example.com\x1b\\read-more\x1b]8;;\x1b\\");

    let link = terminal
        .link_at(1, 2, |_, _| panic!("OSC 8 must bypass classification"))
        .expect("wrapped OSC 8 link should resolve");
    assert_eq!((link.start_row, link.start_col), (0, 0));
    assert_eq!((link.end_row, link.end_col), (1, 3));
    assert_eq!(link.target, "https://example.com");
}

#[test]
fn link_at_clips_wrapped_osc8_to_a_scrolled_viewport() {
    let terminal = Terminal::new_display(
        Size {
            cols: 5,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    terminal.feed_output(b"\x1b]8;;https://example.com\x1b\\abcdefghijklmnop\x1b]8;;\x1b\\");
    assert!(terminal.scroll_display(1));

    let link = terminal
        .link_at(0, 2, |_, _| panic!("OSC 8 must bypass classification"))
        .expect("partially visible OSC 8 link should resolve");
    assert_eq!((link.start_row, link.start_col), (0, 0));
    assert_eq!((link.end_row, link.end_col), (1, 4));
    assert_eq!(link.target, "https://example.com");
}

#[test]
fn link_at_runs_the_heuristic_classifier_after_releasing_the_engine_lock() {
    let terminal = Terminal::new_display(
        Size {
            cols: 20,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    terminal.feed_output(b"go example.com! rest");

    let link = terminal
        .link_at(0, 5, |characters, hovered_index| {
            // Calling back into the terminal would deadlock if classification
            // still ran while the engine mutex was held.
            assert_eq!(terminal.size().cols, 20);
            assert_eq!(characters.iter().collect::<String>(), "example.com!");
            assert_eq!(hovered_index, 2);
            Some(LinkMatch {
                start_col: 0,
                end_col: 10,
                target: "http://example.com".to_string(),
            })
        })
        .expect("classified text link should resolve");
    assert_eq!((link.start_row, link.start_col), (0, 3));
    assert_eq!((link.end_row, link.end_col), (0, 13));
    assert_eq!(link.target, "http://example.com");

    assert_eq!(
        terminal.link_at(0, 8, |_, _| Some(LinkMatch {
            start_col: 0,
            end_col: 1,
            target: "invalid".to_string(),
        })),
        None,
        "classifier ranges must contain the hovered cell"
    );
}

#[test]
fn configured_query_colors_are_available_before_the_first_theme_refresh() {
    let colors = QueryColors {
        ansi: [Rgb { r: 1, g: 2, b: 3 }; 16],
        foreground: Rgb { r: 4, g: 5, b: 6 },
        background: Rgb { r: 7, g: 8, b: 9 },
        cursor: Some(Rgb {
            r: 10,
            g: 11,
            b: 12,
        }),
    };
    let config = Config {
        query_colors: colors,
        ..Config::default()
    };
    let mut engine = Engine::new(Size::default(), &config);
    let (_, replies, _) = engine.feed(b"\x1b]4;1;?\x07\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07");

    assert_eq!(
        replies,
        b"\x1b]4;1;rgb:0101/0202/0303\x07\
          \x1b]10;rgb:0404/0505/0606\x07\
          \x1b]11;rgb:0707/0808/0909\x07"
    );
}

#[test]
fn query_replies_match_the_alacritty_engine() {
    let size = Size {
        cols: 12,
        rows: 3,
        cell_width: 9.0,
        cell_height: 18.0,
    };
    let input = b"ab\
                  \x1b[5n\
                  \x1b[6n\
                  \x1b[c\
                  \x1b[>c\
                  \x1bZ\
                  \x1b[?7$p\
                  \x1b[20$p\
                  \x1b[14t\
                  \x1b[18t\
                  \x1b[>1u\x1b[=2;2u\x1b[?u";
    let mut engine = Engine::new(size, &Config::default());
    let (_, tmon_replies, _) = engine.feed(input);
    let alacritty_replies = alacritty_query_replies(input, size);
    let expected = b"\x1b[0n\
                     \x1b[1;3R\
                     \x1b[?6c\
                     \x1b[>0;2600;1c\
                     \x1b[?6c\
                     \x1b[?7;1$y\
                     \x1b[20;2$y\
                     \x1b[4;54;108t\
                     \x1b[8;3;12t\
                     \x1b[?1u";

    assert_eq!(tmon_replies, alacritty_replies);
    assert_eq!(tmon_replies, expected);
}

#[test]
fn display_protocol_reply_api_routes_to_the_drain_queue() {
    let terminal = Terminal::new_display(Size::default(), Config::default());

    terminal.write_protocol_reply_owned(b"clipboard-reply".to_vec());

    assert_eq!(terminal.drain_protocol_replies(), b"clipboard-reply");
    assert!(terminal.drain_protocol_replies().is_empty());
}

#[test]
fn display_protocol_reply_draining_and_discarding_are_bounded() {
    let terminal = Terminal::new_display(Size::default(), Config::default());
    terminal.write_protocol_reply_owned(b"abcdefgh".to_vec());

    assert_eq!(
        terminal.drain_protocol_replies_bounded(3),
        (b"abc".to_vec(), true)
    );
    assert_eq!(terminal.discard_protocol_replies(2), (2, true));
    assert_eq!(
        terminal.drain_protocol_replies_bounded(usize::MAX),
        (b"fgh".to_vec(), false)
    );
    assert_eq!(terminal.discard_protocol_replies(1), (0, false));
}

#[test]
fn pending_event_probe_tracks_bounded_drains() {
    let terminal = Terminal::new_display(Size::default(), Config::default());
    assert!(!terminal.has_pending_events());

    terminal.feed_output(b"\x07");
    assert!(terminal.has_pending_events());
    assert_eq!(terminal.drain_events().0, [Event::Bell, Event::Wakeup]);
    assert!(!terminal.has_pending_events());
}

#[test]
fn live_color_query_replies_match_the_alacritty_engine() {
    let size = Size::default();
    let input = b"\x1b]4;1;#123456\x1b\\\
                  \x1b]10;#010203\x07\
                  \x1b]11;#040506\x1b\\\
                  \x1b]12;#070809\x07\
                  \x1b]4;1;?\x1b\\\
                  \x1b]10;?\x07\
                  \x1b]11;?\x1b\\\
                  \x1b]12;?\x07";
    let mut engine = Engine::new(size, &Config::default());
    let (_, tmon_replies, _) = engine.feed(input);
    let alacritty_replies = alacritty_query_replies(input, size);
    let expected = b"\x1b]4;1;rgb:1212/3434/5656\x1b\\\
                     \x1b]10;rgb:0101/0202/0303\x07\
                     \x1b]11;rgb:0404/0505/0606\x1b\\\
                     \x1b]12;rgb:0707/0808/0909\x07";

    assert_eq!(tmon_replies, alacritty_replies);
    assert_eq!(tmon_replies, expected);
}

#[test]
fn synchronized_query_replies_match_the_alacritty_engine() {
    let size = Size {
        cols: 12,
        rows: 3,
        ..Size::default()
    };
    let input = b"\x1b[?2004;2026hX\x1b[6n\x1b[?2026l";
    let mut engine = Engine::new(size, &Config::default());
    let (_, tmon_replies, active) = engine.feed(input);

    assert!(!active);
    assert_eq!(tmon_replies, alacritty_query_replies(input, size));
    assert_eq!(tmon_replies, b"\x1b[1;2R");
}

#[test]
fn base64_decoder_accepts_padded_and_unpadded_data_but_rejects_junk() {
    assert_eq!(decode_base64(b"dG1vbg=="), Some(b"tmon".to_vec()));
    assert_eq!(decode_base64(b"dG1vbg"), Some(b"tmon".to_vec()));
    assert_eq!(decode_base64(b"dG1v\n bg=="), Some(b"tmon".to_vec()));
    for invalid in [b"A".as_slice(), b"AA=".as_slice(), b"AA==x", b"===="] {
        assert_eq!(decode_base64(invalid), None);
    }
}

#[test]
fn synchronized_updates_wake_after_a_bounded_timeout() {
    let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let notifier_notifications = notifications.clone();
    let terminal = Terminal::new_display_with_wakeup_notifier(
        Size::default(),
        Config::default(),
        WakeupNotifier::new(move || {
            notifier_notifications.fetch_add(1, Ordering::Relaxed);
        }),
    );
    terminal.feed_output(b"\x1b[?2026hframe");
    assert!(terminal.drain_events().0.is_empty());
    assert_eq!(notifications.load(Ordering::Relaxed), 0);
    assert_eq!(terminal.snapshot().cells[0].character, ' ');

    std::thread::sleep(SYNC_WATCHDOG_DELAY + Duration::from_millis(75));
    assert_eq!(notifications.load(Ordering::Relaxed), 1);
    assert_eq!(terminal.drain_events().0, vec![Event::Wakeup]);
    assert_eq!(
        terminal.snapshot().cells[..5]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>(),
        "frame"
    );
}

#[test]
fn synchronized_update_end_notifies_and_exposes_the_committed_frame() {
    let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let notifier_notifications = notifications.clone();
    let terminal = Terminal::new_display_with_wakeup_notifier(
        Size::default(),
        Config::default(),
        WakeupNotifier::new(move || {
            notifier_notifications.fetch_add(1, Ordering::Relaxed);
        }),
    );

    terminal.feed_output(b"\x1b[?2026hframe");
    assert_eq!(notifications.load(Ordering::Relaxed), 0);
    assert_eq!(terminal.snapshot().cells[0].character, ' ');

    terminal.feed_output(b"\x1b[?2026l");
    assert_eq!(notifications.load(Ordering::Relaxed), 1);
    assert_eq!(terminal.drain_events().0, vec![Event::Wakeup]);
    assert_eq!(
        terminal.snapshot().cells[..5]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>(),
        "frame"
    );
}

#[test]
fn output_before_bsu_wakes_without_exposing_the_staged_suffix() {
    let terminal = Terminal::new_display(
        Size {
            cols: 16,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    terminal.feed_output(b"prefix\x1b[?2026hframe");

    assert_eq!(terminal.drain_events().0, vec![Event::Wakeup]);
    assert_eq!(
        terminal.snapshot().cells[..11]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>(),
        "prefix     "
    );
    terminal.feed_output(b"\x1b[?2026l");
    assert_eq!(
        terminal.snapshot().cells[..11]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>(),
        "prefixframe"
    );
}

#[test]
fn synchronized_update_end_invalidates_the_watchdog() {
    let terminal = Terminal::new_display(Size::default(), Config::default());
    terminal.feed_output(b"\x1b[?2026hframe");
    terminal.feed_output(b"\x1b[?2026l");
    assert_eq!(terminal.drain_events().0, vec![Event::Wakeup]);

    std::thread::sleep(SYNC_WATCHDOG_DELAY + Duration::from_millis(75));
    assert!(terminal.drain_events().0.is_empty());
}

#[test]
fn synchronized_query_replies_are_released_on_esu_and_timeout() {
    let ended = Terminal::new_display(Size::default(), Config::default());
    ended.feed_output(b"\x1b[?2026hX\x1b[6n");
    assert!(ended.drain_protocol_replies().is_empty());
    ended.feed_output(b"\x1b[?2026l");
    assert_eq!(ended.drain_protocol_replies(), b"\x1b[1;2R");

    let timed_out = Terminal::new_display(Size::default(), Config::default());
    timed_out.feed_output(b"\x1b[?2026hY\x1b[6n");
    assert!(timed_out.drain_protocol_replies().is_empty());
    std::thread::sleep(SYNC_WATCHDOG_DELAY + Duration::from_millis(75));
    assert_eq!(timed_out.drain_protocol_replies(), b"\x1b[1;2R");
}

#[test]
fn stale_sync_watchdog_cannot_commit_a_refreshed_batch() {
    let engine = Arc::new(Mutex::new(Engine::new(Size::default(), &Config::default())));
    engine
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .feed_detailed(b"\x1b[?2026hX\x1b[6n");
    let events = Arc::new(Mutex::new(VecDeque::new()));
    let wakeup_queued = Arc::new(AtomicBool::new(false));
    let wakeup_enabled = Arc::new(AtomicBool::new(true));
    let sync_batch_active = Arc::new(AtomicBool::new(true));
    let sync_generation = Arc::new(AtomicU64::new(2));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_capture = captured.clone();
    let protocol_reply_sink: ProtocolReplySink = Arc::new(move |reply| {
        sink_capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(reply);
    });
    let task = |generation| SyncWatchdogTask {
        deadline: Instant::now(),
        engine: engine.clone(),
        events: events.clone(),
        wakeup_queued: wakeup_queued.clone(),
        wakeup_enabled: wakeup_enabled.clone(),
        wakeup_notifier: None,
        sync_batch_active: sync_batch_active.clone(),
        sync_generation: sync_generation.clone(),
        protocol_reply_sink: protocol_reply_sink.clone(),
        generation,
    };

    execute_sync_watchdog_task(task(1));
    assert_eq!(
        engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .line(0)
            .unwrap()[0]
            .character,
        ' '
    );
    assert!(captured.lock().unwrap().is_empty());

    execute_sync_watchdog_task(task(2));
    assert_eq!(
        engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grid
            .line(0)
            .unwrap()[0]
            .character,
        'X'
    );
    assert_eq!(*captured.lock().unwrap(), b"\x1b[1;2R");
    assert_eq!(*events.lock().unwrap(), VecDeque::from([Event::Wakeup]));
}

#[test]
fn sync_watchdog_queue_does_not_retain_unbounded_refresh_tasks() {
    let engine = Arc::new(Mutex::new(Engine::new(Size::default(), &Config::default())));
    let events = Arc::new(Mutex::new(VecDeque::new()));
    let wakeup_queued = Arc::new(AtomicBool::new(false));
    let wakeup_enabled = Arc::new(AtomicBool::new(true));
    let sync_batch_active = Arc::new(AtomicBool::new(true));
    let sync_generation = Arc::new(AtomicU64::new(1));
    let protocol_reply_sink: ProtocolReplySink = Arc::new(drop);
    let task = || SyncWatchdogTask {
        deadline: Instant::now(),
        engine: engine.clone(),
        events: events.clone(),
        wakeup_queued: wakeup_queued.clone(),
        wakeup_enabled: wakeup_enabled.clone(),
        wakeup_notifier: None,
        sync_batch_active: sync_batch_active.clone(),
        sync_generation: sync_generation.clone(),
        protocol_reply_sink: protocol_reply_sink.clone(),
        // Keep every task stale so releasing the engine lock only drains
        // queue ownership and cannot commit the synthetic update.
        generation: 0,
    };

    let engine_guard = engine
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let baseline_references = Arc::strong_count(&engine);
    schedule_sync_watchdog(task());
    // Give the worker time to dequeue the first expired task and block on
    // the engine lock held by this test.
    std::thread::sleep(Duration::from_millis(25));
    for _ in 0..512 {
        schedule_sync_watchdog(task());
    }
    let retained_references = Arc::strong_count(&engine).saturating_sub(baseline_references);
    drop(engine_guard);

    assert!(
        retained_references <= 8,
        "same-engine watchdog refreshes retained {retained_references} engine references"
    );
}

#[test]
fn hydration_does_not_leak_incomplete_parser_state_into_live_output() {
    let terminal = |hydrated: &[u8], live: &[u8]| {
        let terminal = Terminal::new_display(
            Size {
                cols: 12,
                rows: 2,
                ..Size::default()
            },
            Config::default(),
        );
        terminal.hydrate_output(hydrated);
        terminal.feed_output(live);
        terminal
    };

    let csi = terminal(b"\x1b[31", b"mX");
    assert_eq!(csi.snapshot().cells[0].character, 'm');
    assert_eq!(csi.snapshot().cells[1].character, 'X');
    assert_eq!(csi.snapshot().cells[1].foreground, Color::Default);

    let osc = terminal(b"\x1b]2;stale", b"live");
    assert_eq!(
        osc.snapshot().cells[..4]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>(),
        "live"
    );

    let dcs = terminal(b"\x1bP$q", b"DCS");
    assert_eq!(
        dcs.snapshot().cells[..3]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>(),
        "DCS"
    );

    let apc = terminal(b"\x1b_Ga=T,f=32,s=1,v=1;AAAA", b"APC");
    assert_eq!(
        apc.snapshot().cells[..3]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>(),
        "APC"
    );

    let utf8 = terminal(&[0xe2, 0x9d], b"UTF8");
    assert_eq!(
        utf8.snapshot().cells[..4]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>(),
        "UTF8"
    );

    let sync = terminal(b"\x1b[?2026hdiscarded", b"SYNC");
    assert_eq!(
        sync.snapshot().cells[..4]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>(),
        "SYNC"
    );

    let repeat = terminal(b"H", b"\x1b[3bX");
    assert_eq!(
        repeat.snapshot().cells[..2]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>(),
        "HX"
    );
}

#[test]
fn hydration_discards_a_completed_multipart_kitty_prefix_before_live_output() {
    let terminal = Terminal::new_display(
        Size {
            cols: 12,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    terminal.hydrate_output(b"\x1b_Ga=t,f=32,s=1,v=1,i=90,m=1,q=1;AQI=\x1b\\");
    assert_eq!(terminal.kitty_graphics_revision(), 0);

    terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=91,C=1;AQID/w==\x1b\\");
    let (revision, placements) = terminal.kitty_graphics_snapshot();

    assert_eq!(revision, 1);
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].image_id, 91);
    assert!(
        terminal
            .drain_protocol_replies()
            .ends_with(b"\x1b_Gi=91;OK\x1b\\")
    );
}

#[test]
fn hydration_keeps_completed_style_charset_title_and_graphics_state() {
    let terminal = Terminal::new_display(
        Size {
            cols: 12,
            rows: 2,
            ..Size::default()
        },
        Config::default(),
    );
    terminal.hydrate_output(
        b"\x1b[31m\x1b)0\x0e\x1b]2;saved\x07\x1b_Ga=T,f=32,s=1,v=1,i=91,C=1;/wAA/w==\x1b\\",
    );
    let hydrated_graphics = terminal.kitty_graphics_snapshot();
    terminal.feed_output(b"q\x1b[22t\x1b]2;temporary\x07\x1b[23t");

    let snapshot = terminal.snapshot();
    assert_eq!(snapshot.cells[0].character, '\u{2500}');
    assert_eq!(snapshot.cells[0].foreground, Color::Indexed(1));
    assert_eq!(terminal.kitty_graphics_snapshot(), hydrated_graphics);
    assert_eq!(hydrated_graphics.1.len(), 1);
    assert!(
        terminal
            .drain_events()
            .0
            .contains(&Event::Title("saved".to_string()))
    );
}

#[test]
fn osc52_paste_queries_are_explicitly_gated_and_formatted() {
    let denied = Terminal::new_display(Size::default(), Config::default());
    denied.feed_output(b"\x1b]52;c;?\x07");
    assert_eq!(denied.drain_events().0, vec![Event::Wakeup]);

    let allowed = Terminal::new_display(
        Size::default(),
        Config {
            osc52: Osc52::CopyPaste,
            ..Config::default()
        },
    );
    allowed.feed_output(b"\x1b]52;s;?\x1b\\");
    let (events, _) = allowed.drain_events();
    let request = events
        .into_iter()
        .find_map(|event| match event {
            Event::ClipboardLoad(request) => Some(request),
            _ => None,
        })
        .expect("OSC 52 load request should reach the host");
    assert_eq!(request.target(), ClipboardTarget::Selection);
    assert_eq!(request.format_reply("tmon"), b"\x1b]52;s;dG1vbg==\x1b\\");
}

#[test]
fn live_options_bound_history_without_rebuilding_the_terminal() {
    let terminal = Terminal::new_display(
        Size {
            cols: 2,
            rows: 1,
            ..Size::default()
        },
        Config {
            scrollback_history: 8,
            ..Config::default()
        },
    );
    terminal.feed_output(b"abcdefgh");
    assert!(terminal.scroll_state().1 > 1);
    terminal.set_options(TerminalOptions {
        scrollback_history: 1,
        ..TerminalOptions::default()
    });
    assert_eq!(terminal.scroll_state().1, 1);
}

#[test]
fn sizes_are_clamped_before_allocating_the_grid() {
    let terminal = Terminal::new_display(
        Size {
            cols: 0,
            rows: 0,
            cell_width: f32::NAN,
            cell_height: 0.0,
        },
        Config::default(),
    );
    assert_eq!(terminal.size().cols, 1);
    assert_eq!(terminal.size().rows, 1);
    assert_eq!(terminal.size().cell_width, 1.0);
    assert_eq!(terminal.size().cell_height, 1.0);
}

#[test]
fn cleared_primary_viewport_stays_clear_across_width_reflow() {
    let terminal = Terminal::new_display(
        Size {
            cols: 40,
            rows: 10,
            ..Size::default()
        },
        Config::default(),
    );
    for line in 0..18 {
        terminal
            .feed_output(format!("HISTORY-{line:02}-abcdefghijklmnopqrstuvwxyz\r\n").as_bytes());
    }
    terminal.feed_output(b"\x1b[H\x1b[2J$ ");

    terminal.resize(Size {
        cols: 20,
        rows: 10,
        ..Size::default()
    });

    let snapshot = terminal.snapshot();
    let visible = snapshot
        .cells
        .chunks(usize::from(snapshot.cols))
        .map(|row| row.iter().map(|cell| cell.character).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!visible.contains("HISTORY-"), "{visible}");
    assert!(snapshot.history_size > 0);
}
