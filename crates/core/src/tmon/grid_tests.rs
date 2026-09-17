use super::*;

fn grid(cols: u16, rows: u16) -> Grid {
    Grid::new(cols, rows, 3, CursorStyle::Block)
}

#[test]
fn cells_remain_compact_with_sparse_combining_storage() {
    assert_eq!(std::mem::size_of::<Color>(), 4);
    assert_eq!(std::mem::size_of::<Option<Color>>(), 4);
    assert_eq!(std::mem::size_of::<Attributes>(), 2);
    assert_eq!(std::mem::size_of::<Cell>(), 24);
}

#[test]
fn compact_history_rows_round_trip_dense_cells() {
    let mut short = vec![Cell::default(); 12];
    for (cell, character) in short.iter_mut().zip("short".chars()) {
        cell.character = character;
    }
    let full = vec![
        Cell {
            character: 'x',
            ..Cell::default()
        };
        17
    ];
    let mut maximum = vec![Cell::default(); usize::from(MAX_GRID_DIMENSION)];
    maximum[0].character = 'a';
    let last = maximum.len() - 1;
    maximum[last].character = 'z';

    for dense in [
        vec![Cell::default(); 8],
        short,
        full,
        vec![Cell {
            character: '1',
            ..Cell::default()
        }],
        maximum,
    ] {
        let history = HistoryRow::from_dense(dense.clone());
        assert_eq!(history.len(), dense.len());
        assert_eq!(history.iter().collect::<Vec<_>>(), dense);
        assert_eq!(
            history.iter().rev().collect::<Vec<_>>(),
            dense.iter().copied().rev().collect::<Vec<_>>()
        );
        if dense.len() >= 2 {
            let mut interleaved = history.iter();
            assert_eq!(interleaved.next(), dense.first().copied());
            assert_eq!(interleaved.next_back(), dense.last().copied());
            assert_eq!(interleaved.collect::<Vec<_>>(), dense[1..dense.len() - 1]);
        }
        assert_eq!(history.into_dense(), dense);
    }
}

#[test]
fn compact_history_rows_preserve_every_cell_field_and_state() {
    let underline_styles = [
        UnderlineStyle::Single,
        UnderlineStyle::Double,
        UnderlineStyle::Curly,
        UnderlineStyle::Dotted,
        UnderlineStyle::Dashed,
    ];
    let mut dense = vec![Cell::default(); underline_styles.len() + 3];
    for (index, style) in underline_styles.into_iter().enumerate() {
        dense[index] = Cell {
            character: char::from(b'a' + index as u8),
            foreground: if index % 2 == 0 {
                Color::Indexed(17 + index as u8)
            } else {
                Color::Rgb {
                    r: index as u8,
                    g: 100 + index as u8,
                    b: 200 + index as u8,
                }
            },
            background: if index % 2 == 0 {
                Color::Rgb {
                    r: 200 + index as u8,
                    g: 100 + index as u8,
                    b: index as u8,
                }
            } else {
                Color::Indexed(37 + index as u8)
            },
            attributes: Attributes::default()
                .with_bold(true)
                .with_dim(true)
                .with_italic(true)
                .with_underline_style(style)
                .with_inverse(true)
                .with_hidden(true)
                .with_strikethrough(true),
            underline_color: Some(if index % 2 == 0 {
                Color::Indexed(57 + index as u8)
            } else {
                Color::Rgb {
                    r: 10 + index as u8,
                    g: 20 + index as u8,
                    b: 30 + index as u8,
                }
            }),
            state: CellState(
                CellState::PROTECTED
                    | CellState::WIDE_SPACER
                    | CellState::LEADING_WIDE_SPACER
                    | CellState::WRAPPED
                    | CellState::HAS_HYPERLINK
                    | CellState::HAS_COMBINING,
            ),
            metadata_id: NonZeroU32::new(POOLED_EXTRA_TAG | (index as u32 + 1)),
        };
    }
    dense[underline_styles.len()] = Cell {
        underline_color: Some(Color::Default),
        ..Cell::default()
    };
    dense[underline_styles.len() + 1].set_protected(true);

    let history = HistoryRow::from_dense(dense.clone());
    assert_eq!(history.stored_len(), dense.len() - 1);
    assert_eq!(history.into_dense(), dense);
}

#[test]
fn compact_history_rows_trim_only_exact_default_tails() {
    let styled_blank = Cell {
        background: Color::Indexed(4),
        ..Cell::default()
    };
    let mut protected_blank = Cell::default();
    protected_blank.set_protected(true);
    let mut linked_blank = Cell::default();
    linked_blank.set_has_hyperlink(true);
    linked_blank.metadata_id = NonZeroU32::new(INLINE_HYPERLINK_TAG | 1);
    let mut combining_blank = Cell::default();
    combining_blank.set_has_combining(true);
    combining_blank.metadata_id = NonZeroU32::new('́' as u32 + 1);

    for meaningful in [styled_blank, protected_blank, linked_blank, combining_blank] {
        let dense = vec![Cell::default(), meaningful, Cell::default()];
        let history = HistoryRow::from_dense(dense.clone());
        assert_eq!(history.stored_len(), 2);
        assert_eq!(history.into_dense(), dense);
    }

    let empty = HistoryRow::from_dense(vec![Cell::default(); 120]);
    assert_eq!(empty.stored_len(), 0);
    assert_eq!(empty.retained_bytes(), 0);
}

#[test]
fn compact_history_preserves_wide_pairs_at_each_row_position() {
    for lead in [0, 3, 6] {
        let mut dense = vec![Cell::default(); 8];
        dense[lead].character = '界';
        dense[lead + 1].set_wide_spacer(true);

        let decoded = HistoryRow::from_dense(dense.clone()).into_dense();
        assert_eq!(decoded, dense, "wide lead at column {lead}");
        assert_eq!(decoded[lead].character, '界');
        assert!(decoded[lead + 1].wide_spacer());
    }
}

#[test]
fn mostly_plain_10k_history_does_not_retain_dense_cell_capacity() {
    const COLS: usize = 120;
    const HISTORY_ROWS: usize = 10_000;
    const TEXT_CELLS: usize = 64;

    let mut grid = Grid::new(COLS as u16, 40, HISTORY_ROWS, CursorStyle::Block);
    let mut dense = vec![Cell::default(); COLS];
    for row in 0..HISTORY_ROWS {
        dense.fill(DEFAULT_CELL);
        for (col, cell) in dense.iter_mut().take(TEXT_CELLS).enumerate() {
            cell.character = char::from(b'a' + ((row + col) % 26) as u8);
        }
        dense = grid.push_history(dense, COLS).0;
    }

    let stats = grid.history_storage_stats();
    let old_dense_payload = COLS * HISTORY_ROWS * std::mem::size_of::<Cell>();
    assert_eq!(stats.rows, HISTORY_ROWS);
    assert_eq!(stats.logical_cells, COLS * HISTORY_ROWS);
    assert_eq!(stats.encoded_capacity, TEXT_CELLS * HISTORY_ROWS);
    assert!(
        stats.retained_bytes * 100 <= old_dense_payload * 5,
        "compact rows retained {} bytes versus {old_dense_payload} dense payload bytes",
        stats.retained_bytes
    );
}

#[test]
fn palette_revision_advances_only_for_visible_mutations() {
    let mut grid = grid(4, 2);
    let color = Rgb {
        r: 0x12,
        g: 0x34,
        b: 0x56,
    };
    assert_eq!(grid.palette().revision(), 0);

    grid.set_indexed_color(7, color);
    assert_eq!(grid.palette().revision(), 1);

    grid.set_indexed_color(7, color);
    assert_eq!(grid.palette().revision(), 1);
}

#[test]
fn palette_equality_ignores_mutation_history() {
    let pristine = Palette::default();
    let mut changed_then_reset = grid(4, 2);
    changed_then_reset.set_foreground_color(Some(Rgb { r: 1, g: 2, b: 3 }));
    changed_then_reset.set_foreground_color(None);
    let changed_then_reset = changed_then_reset.palette();

    assert_eq!(changed_then_reset, pristine);
    assert_ne!(changed_then_reset.revision(), pristine.revision());
}

#[test]
fn packed_attribute_and_cell_state_flags_are_independent() {
    let attributes = [
        (Attributes::default().with_bold(true), Attributes::BOLD),
        (Attributes::default().with_dim(true), Attributes::DIM),
        (Attributes::default().with_italic(true), Attributes::ITALIC),
        (
            Attributes::default().with_inverse(true),
            Attributes::INVERSE,
        ),
        (Attributes::default().with_hidden(true), Attributes::HIDDEN),
        (
            Attributes::default().with_strikethrough(true),
            Attributes::STRIKETHROUGH,
        ),
    ];
    for (attributes, expected) in attributes {
        assert_eq!(attributes.0, expected);
    }

    for style in [
        UnderlineStyle::None,
        UnderlineStyle::Single,
        UnderlineStyle::Double,
        UnderlineStyle::Curly,
        UnderlineStyle::Dotted,
        UnderlineStyle::Dashed,
    ] {
        let attributes = Attributes::default().with_underline_style(style);
        assert_eq!(attributes.underline_style(), style);
        assert_eq!(attributes.underline(), style != UnderlineStyle::None);
        assert_eq!(attributes.0 & !Attributes::UNDERLINE_MASK, 0);
    }

    let mut attributes = Attributes::default();
    attributes.set_bold(true);
    attributes.set_dim(true);
    attributes.set_italic(true);
    attributes.set_underline(true);
    attributes.set_inverse(true);
    attributes.set_hidden(true);
    attributes.set_strikethrough(true);
    assert!(attributes.bold());
    assert!(attributes.dim());
    assert!(attributes.italic());
    assert!(attributes.underline());
    assert!(attributes.inverse());
    assert!(attributes.hidden());
    assert!(attributes.strikethrough());
    attributes.set_bold(false);
    assert!(!attributes.bold());
    assert!(attributes.dim());

    type CellStateSetter = fn(&mut Cell);
    let state_flags: [(CellStateSetter, u8); 4] = [
        (
            |cell: &mut Cell| cell.set_protected(true),
            CellState::PROTECTED,
        ),
        (
            |cell: &mut Cell| cell.set_wide_spacer(true),
            CellState::WIDE_SPACER,
        ),
        (
            |cell: &mut Cell| cell.set_leading_wide_spacer(true),
            CellState::LEADING_WIDE_SPACER,
        ),
        (|cell: &mut Cell| cell.set_wrapped(true), CellState::WRAPPED),
    ];
    for (set, expected) in state_flags {
        let mut cell = Cell::default();
        set(&mut cell);
        assert_eq!(cell.state.0, expected);
    }
}

#[test]
fn ascii_runs_match_scalar_writes_across_wrap_and_scrollback() {
    let mut fast = Grid::new(5, 2, 4, CursorStyle::Block);
    let mut scalar = Grid::new(5, 2, 4, CursorStyle::Block);
    assert_eq!(fast.take_damage(), DamageSnapshot::Full);
    assert_eq!(scalar.take_damage(), DamageSnapshot::Full);
    let bytes = b"abcdefghijklmnop";

    let mut offset = 0;
    while offset < bytes.len() {
        let consumed = fast.put_ascii_run(&bytes[offset..]);
        if consumed == 0 {
            fast.put_char(char::from(bytes[offset]));
            offset += 1;
        } else {
            offset += consumed;
        }
    }
    for byte in bytes {
        scalar.put_char(char::from(*byte));
    }

    assert_eq!(fast.primary.cells, scalar.primary.cells);
    assert_eq!(fast.history, scalar.history);
    assert_eq!(fast.cursor_position(), scalar.cursor_position());
    assert_eq!(fast.primary.wrap_pending, scalar.primary.wrap_pending);
    assert_eq!(fast.take_damage(), scalar.take_damage());
}

#[test]
fn ascii_runs_stop_before_cells_that_need_wide_cleanup() {
    let mut grid = Grid::new(6, 2, 0, CursorStyle::Block);
    grid.set_cursor_position(0, 2);
    grid.put_char('界');
    grid.set_cursor_position(0, 0);

    assert_eq!(grid.put_ascii_run(b"abcdef"), 2);
    assert_eq!(grid.line(0).unwrap()[0].character, 'a');
    assert_eq!(grid.line(0).unwrap()[1].character, 'b');
    assert_eq!(grid.line(0).unwrap()[2].character, '界');
    assert!(grid.line(0).unwrap()[3].wide_spacer());
}

#[test]
fn combining_characters_attach_to_the_previous_cell() {
    let mut grid = grid(4, 2);
    grid.put_char('e');
    grid.put_char('\u{301}');
    grid.put_char('\u{308}');

    let cell = &grid.line(0).unwrap()[0];
    assert_eq!(cell.character, 'e');
    assert_eq!(
        grid.combining_text(cell).map(Combining::to_owned_string),
        Some("\u{301}\u{308}".to_string())
    );
    assert_eq!(grid.cursor_position(), (1, 0));
}

#[test]
fn long_combining_sequences_spill_out_of_inline_storage_without_data_loss() {
    let mut grid = grid(4, 2);
    grid.put_char('e');
    let combining = "\u{301}".repeat(1_024);
    for character in combining.chars() {
        grid.put_char(character);
    }

    let cell = &grid.line(0).unwrap()[0];
    assert_eq!(
        grid.combining_text(cell).map(Combining::to_owned_string),
        Some(combining)
    );
    assert_eq!(
        grid.extras.len(),
        1,
        "one cell's combining run must not retain every previous prefix"
    );
}

#[test]
fn pooled_combining_text_appends_after_cell_moves_without_losing_its_link() {
    let mut grid = grid(8, 2);
    grid.set_hyperlink(None, Some("https://example.com/combined"));
    grid.put_char('e');
    for character in "\u{301}\u{302}\u{303}\u{304}\u{305}".chars() {
        grid.put_char(character);
    }
    grid.set_hyperlink(None, None);

    let metadata_id = grid.line(0).unwrap()[0]
        .metadata_id
        .expect("combined cell metadata");
    assert!(is_pooled_extra_id(metadata_id));
    grid.set_cursor_position(0, 0);
    grid.insert_blank_chars(2);
    assert_eq!(grid.line(0).unwrap()[2].metadata_id, Some(metadata_id));

    grid.set_cursor_position(0, 3);
    grid.put_char('\u{306}');

    let cell = &grid.line(0).unwrap()[2];
    assert_eq!(
        grid.combining_text(cell).map(Combining::to_owned_string),
        Some("\u{301}\u{302}\u{303}\u{304}\u{305}\u{306}".to_string())
    );
    assert_eq!(
        grid.hyperlink_at(0, 2).map(|link| link.target),
        Some("https://example.com/combined".to_string())
    );
    assert_eq!(grid.extras.len(), 1);
}

#[test]
fn combining_characters_attach_to_the_lead_cell_of_wide_text() {
    let mut grid = grid(4, 2);
    grid.put_char('界');
    grid.put_char('\u{301}');

    let line = grid.line(0).unwrap();
    assert_eq!(
        grid.combining_text(&line[0])
            .map(Combining::to_owned_string),
        Some("\u{301}".to_string())
    );
    assert_eq!(grid.combining_text(&line[1]), None);
}

#[test]
fn overwriting_a_cell_drops_its_combining_characters() {
    let mut grid = grid(4, 2);
    grid.put_char('e');
    grid.put_char('\u{301}');
    grid.carriage_return();
    grid.put_char('x');

    let cell = &grid.line(0).unwrap()[0];
    assert_eq!(cell.character, 'x');
    assert_eq!(grid.combining_text(cell), None);
}

#[test]
fn overwriting_a_wide_spacer_drops_combining_text_from_its_lead_cell() {
    let mut grid = grid(4, 2);
    grid.put_char('界');
    grid.put_char('\u{301}');
    grid.backspace();
    grid.put_char('x');

    let line = grid.line(0).unwrap();
    assert_eq!(line[0].character, ' ');
    assert_eq!(grid.combining_text(&line[0]), None);
    assert_eq!(line[1].character, 'x');
}

#[test]
fn live_default_cursor_changes_do_not_override_an_explicit_shape() {
    let mut grid = grid(3, 2);
    grid.set_cursor_style(2);
    grid.set_default_cursor_style(CursorStyle::Line);
    assert_eq!(grid.cursor_state().unwrap().style, CursorStyle::Block);
    assert_eq!(grid.cursor_style_status(), 2);

    grid.set_cursor_style(0);
    grid.set_default_cursor_style(CursorStyle::Block);
    assert_eq!(grid.cursor_state().unwrap().style, CursorStyle::Block);
    assert_eq!(grid.cursor_style_status(), 2);
}

#[test]
fn wrapping_moves_full_rows_into_bounded_history() {
    let mut grid = grid(3, 2);
    for character in "abcdefghijklmnop".chars() {
        grid.put_char(character);
    }
    assert_eq!(grid.history_size(), 3);
    assert_eq!(grid.cursor_position(), (1, 1));
    assert_eq!(grid.line(-3).unwrap().get(0).unwrap().character, 'd');
}

#[test]
fn alternate_screen_does_not_touch_primary_content() {
    let mut grid = grid(6, 2);
    for character in "main".chars() {
        grid.put_char(character);
    }
    grid.sgr(&[32, 44]);
    grid.set_mode(true, 1049, true);
    assert_eq!(grid.cursor_position(), (4, 0));
    assert_eq!(grid.line(0).unwrap()[0].background, Color::Indexed(4));
    grid.put_char('x');
    assert_eq!(grid.line(0).unwrap()[4].character, 'x');
    assert_eq!(grid.line(0).unwrap()[4].foreground, Color::Indexed(2));
    assert_eq!(grid.line(0).unwrap()[4].background, Color::Indexed(4));
    grid.set_mode(true, 1049, false);
    assert_eq!(grid.line(0).unwrap()[0].character, 'm');
    assert_eq!(grid.cursor_position(), (4, 0));
}

#[test]
fn damage_is_scoped_after_initial_full_frame() {
    let mut grid = grid(8, 2);
    assert_eq!(grid.take_damage(), DamageSnapshot::Full);
    grid.put_char('x');
    assert_eq!(
        grid.take_damage(),
        DamageSnapshot::Partial(vec![DirtySpan {
            row: 0,
            left_col: 0,
            right_col: 0,
        }])
    );
}

#[test]
fn full_damage_discards_stale_partial_spans_before_the_next_frame() {
    let mut damage = Damage::new(2);
    assert_eq!(damage.take(0), DamageSnapshot::Full);
    damage.mark(1, 2, 4);
    damage.mark_full();
    assert_eq!(damage.take(0), DamageSnapshot::Full);
    assert_eq!(damage.take(0), DamageSnapshot::Partial(Vec::new()));
}

#[test]
fn renderer_damage_transforms_spans_through_scrolls() {
    let mut damage = Damage::new(4);
    assert_eq!(damage.take_render(0), (DamageSnapshot::Full, Vec::new()));

    damage.mark(0, 1, 1);
    damage.mark(1, 2, 4);
    damage.scroll_up(0, 3, 1, 8);
    damage.mark(2, 6, 6);

    assert_eq!(
        damage.take_render(0),
        (
            DamageSnapshot::Partial(vec![
                DirtySpan {
                    row: 0,
                    left_col: 2,
                    right_col: 4,
                },
                DirtySpan {
                    row: 2,
                    left_col: 6,
                    right_col: 6,
                },
                DirtySpan {
                    row: 3,
                    left_col: 0,
                    right_col: 7,
                },
            ]),
            vec![ScrollDamage {
                top: 0,
                bottom: 3,
                count: 1,
                direction: ScrollDirection::Up,
            }],
        )
    );
}

#[test]
fn renderer_damage_preserves_order_for_opposing_partial_scrolls() {
    let mut damage = Damage::new(5);
    let _ = damage.take_render(0);
    damage.mark(1, 2, 2);
    damage.scroll_down(1, 4, 2, 6);
    damage.scroll_up(0, 3, 1, 6);

    let (snapshot, scrolls) = damage.take_render(0);
    assert_eq!(
        scrolls,
        vec![
            ScrollDamage {
                top: 1,
                bottom: 4,
                count: 2,
                direction: ScrollDirection::Down,
            },
            ScrollDamage {
                top: 0,
                bottom: 3,
                count: 1,
                direction: ScrollDirection::Up,
            },
        ]
    );
    assert_eq!(
        snapshot,
        DamageSnapshot::Partial(vec![
            DirtySpan {
                row: 0,
                left_col: 0,
                right_col: 5,
            },
            DirtySpan {
                row: 1,
                left_col: 0,
                right_col: 5,
            },
            DirtySpan {
                row: 2,
                left_col: 2,
                right_col: 2,
            },
            DirtySpan {
                row: 3,
                left_col: 0,
                right_col: 5,
            },
        ])
    );
}

#[test]
fn renderer_damage_coalesces_adjacent_scrolls_and_bounds_the_queue() {
    let mut damage = Damage::new(3);
    let _ = damage.take_render(0);
    damage.scroll_up(0, 2, 1, 4);
    damage.scroll_up(0, 2, 1, 4);
    let (_, scrolls) = damage.take_render(0);
    assert_eq!(
        scrolls,
        vec![ScrollDamage {
            top: 0,
            bottom: 2,
            count: 2,
            direction: ScrollDirection::Up,
        }]
    );

    for index in 0..=MAX_SCROLL_DAMAGE_OPS {
        let direction = if index % 2 == 0 {
            ScrollDirection::Up
        } else {
            ScrollDirection::Down
        };
        damage.scroll(0, 2, 1, 4, direction);
    }
    assert_eq!(damage.take_render(0), (DamageSnapshot::Full, Vec::new()));
}

#[test]
fn legacy_damage_stays_conservative_for_scrolls() {
    let mut damage = Damage::new(3);
    let _ = damage.take(0);
    damage.scroll_up(0, 2, 1, 4);
    assert_eq!(damage.take(0), DamageSnapshot::Full);
}

#[test]
fn damage_maps_live_screen_rows_into_a_scrolled_viewport() {
    let mut damage = Damage::new(4);
    let _ = damage.take(0);
    damage.mark(0, 1, 2);
    damage.mark(3, 4, 5);
    assert_eq!(
        damage.take(1),
        DamageSnapshot::Partial(vec![DirtySpan {
            row: 1,
            left_col: 1,
            right_col: 2,
        }])
    );
}

#[test]
fn bottom_margin_line_feed_emits_a_replayable_scroll() {
    let mut grid = Grid::new(4, 3, 8, CursorStyle::Block);
    grid.set_cursor_position(2, 0);
    let _ = grid.take_render_damage();

    grid.line_feed();

    assert_eq!(
        grid.take_render_damage(),
        (
            DamageSnapshot::Partial(vec![DirtySpan {
                row: 2,
                left_col: 0,
                right_col: 3,
            }]),
            vec![ScrollDamage {
                top: 0,
                bottom: 2,
                count: 1,
                direction: ScrollDirection::Up,
            }],
        )
    );
}

#[test]
fn full_frame_barriers_discard_pending_renderer_scrolls() {
    let mut resized = Grid::new(4, 3, 8, CursorStyle::Block);
    resized.set_cursor_position(2, 0);
    let _ = resized.take_render_damage();
    resized.line_feed();
    resized.resize(5, 4);
    assert_eq!(
        resized.take_render_damage(),
        (DamageSnapshot::Full, Vec::new())
    );

    let mut alternate = Grid::new(4, 3, 8, CursorStyle::Block);
    alternate.set_cursor_position(2, 0);
    let _ = alternate.take_render_damage();
    alternate.line_feed();
    alternate.set_mode(true, 1049, true);
    assert_eq!(
        alternate.take_render_damage(),
        (DamageSnapshot::Full, Vec::new())
    );

    let mut reset = Grid::new(4, 3, 8, CursorStyle::Block);
    reset.set_cursor_position(2, 0);
    let _ = reset.take_render_damage();
    reset.line_feed();
    reset.reset();
    assert_eq!(
        reset.take_render_damage(),
        (DamageSnapshot::Full, Vec::new())
    );
}

#[test]
fn alternate_screen_storage_is_lazy_and_reset_releases_it() {
    let mut grid = Grid::new(4, 3, 8, CursorStyle::Block);
    assert!(grid.alternate.is_none());

    grid.resize(6, 4);
    assert!(grid.alternate.is_none());

    grid.set_mode(true, 1049, true);
    let alternate = grid
        .alternate
        .as_ref()
        .expect("entering the alternate screen should allocate it");
    assert_eq!((alternate.cols, alternate.rows), (6, 4));

    grid.set_mode(true, 1049, false);
    assert!(grid.alternate.is_some());
    grid.reset();
    assert!(grid.alternate.is_none());
}

#[test]
fn live_scrolling_while_viewing_history_falls_back_to_full_damage() {
    let mut grid = Grid::new(4, 3, 8, CursorStyle::Block);
    grid.set_cursor_position(2, 0);
    grid.line_feed();
    let _ = grid.take_render_damage();
    assert!(grid.scroll_display(1));
    let _ = grid.take_render_damage();

    grid.set_cursor_position(2, 0);
    grid.line_feed();

    assert_eq!(
        grid.take_render_damage(),
        (DamageSnapshot::Full, Vec::new())
    );
}

#[test]
fn width_resize_reflows_soft_wrapped_content_and_cursor() {
    let mut grid = Grid::new(5, 3, 16, CursorStyle::Block);
    for character in "abcdefghij".chars() {
        grid.put_char(character);
    }

    grid.resize(3, 4);

    let rendered = (0..4)
        .map(|line| {
            grid.line(line)
                .unwrap()
                .iter()
                .map(|cell| cell.character)
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert_eq!(rendered, ["abc", "def", "ghi", "j  "]);
    assert_eq!(grid.cursor_position(), (1, 3));
    assert!(grid.line(0).unwrap()[2].wrapped());
    assert!(grid.line(1).unwrap()[2].wrapped());
    assert!(grid.line(2).unwrap()[2].wrapped());
    assert!(!grid.line(3).unwrap()[2].wrapped());
}

#[test]
fn width_resize_does_not_emit_a_destructive_graphics_effect() {
    let mut grid = Grid::new(8, 4, 16, CursorStyle::Block);

    grid.resize(4, 4);

    assert_eq!(grid.pop_effect(), None);
}

#[test]
fn width_resize_keeps_wide_character_and_spacer_together() {
    let mut grid = Grid::new(5, 3, 16, CursorStyle::Block);
    for character in "abc界".chars() {
        grid.put_char(character);
    }

    grid.resize(4, 3);

    assert_eq!(
        grid.line(0)
            .unwrap()
            .iter()
            .take(3)
            .map(|cell| cell.character)
            .collect::<String>(),
        "abc"
    );
    assert!(grid.line(0).unwrap()[3].leading_wide_spacer());
    assert!(grid.line(0).unwrap()[3].wrapped());
    assert_eq!(grid.line(1).unwrap()[0].character, '界');
    assert!(grid.line(1).unwrap()[1].wide_spacer());

    grid.resize(8, 3);
    assert_eq!(grid.line(0).unwrap()[3].character, '界');
    assert!(grid.line(0).unwrap()[4].wide_spacer());
}

#[test]
fn scrolled_resize_to_one_column_keeps_wide_cells_and_terminates() {
    let mut grid = Grid::new(3, 2, 16, CursorStyle::Block);
    for character in "a界bcdefghijkl".chars() {
        grid.put_char(character);
    }
    assert!(!grid.history.is_empty());
    assert!(grid.scroll_display(1));

    grid.resize(1, 2);

    assert_eq!(grid.cols(), 1);
    assert!(grid.display_offset <= grid.history.len());
    let one_column = grid
        .history
        .iter()
        .flat_map(HistoryRow::iter)
        .chain(grid.primary.cells.iter().flatten().copied())
        .collect::<Vec<_>>();
    let wide = one_column
        .iter()
        .position(|cell| cell.character == '界')
        .expect("wide character survives one-column reflow");
    assert!(
        one_column
            .get(wide + 1)
            .is_some_and(|cell| cell.wide_spacer())
    );

    grid.resize(3, 2);

    let widened = grid
        .history
        .iter()
        .flat_map(HistoryRow::iter)
        .chain(grid.primary.cells.iter().flatten().copied())
        .collect::<Vec<_>>();
    let wide = widened
        .iter()
        .position(|cell| cell.character == '界')
        .expect("wide character survives widening again");
    assert!(widened.get(wide + 1).is_some_and(|cell| cell.wide_spacer()));
}

#[test]
fn rendered_cursor_uses_the_leading_cell_of_a_wide_character() {
    let mut grid = Grid::new(4, 2, 0, CursorStyle::Block);
    grid.set_cursor_col(2);
    grid.put_char('界');

    assert_eq!(grid.cursor_position(), (3, 0));
    assert_eq!(grid.render_cursor_position(), (2, 0));
    assert_eq!(grid.cursor_state().unwrap().col, 2);
}

#[test]
fn wide_character_at_the_margin_styles_the_wrapped_placeholder() {
    let mut grid = Grid::new(4, 2, 0, CursorStyle::Block);
    grid.sgr(&[31, 44]);
    grid.set_cursor_col(3);
    grid.put_char('界');

    let placeholder = grid.line(0).unwrap()[3];
    assert_eq!(placeholder.character, ' ');
    assert_eq!(placeholder.foreground, Color::Indexed(1));
    assert_eq!(placeholder.background, Color::Indexed(4));
    assert!(placeholder.leading_wide_spacer());
    assert!(placeholder.wrapped());
    assert_eq!(grid.line(1).unwrap()[0].character, '界');
    assert!(grid.line(1).unwrap()[1].wide_spacer());
}

#[test]
fn wide_spacer_repair_mutates_the_newest_compact_history_row() {
    let mut grid = Grid::new(4, 2, 8, CursorStyle::Block);
    grid.set_cursor_position(0, 3);
    grid.put_char('界');
    assert!(grid.line(0).unwrap()[3].leading_wide_spacer());
    assert_eq!(grid.line(1).unwrap()[0].character, '界');

    grid.set_cursor_position(1, 0);
    grid.line_feed();
    assert!(grid.line(-1).unwrap().get(3).unwrap().leading_wide_spacer());

    grid.set_cursor_position(0, 0);
    grid.put_char('x');
    assert!(!grid.line(-1).unwrap().get(3).unwrap().leading_wide_spacer());
}

#[test]
fn insert_mode_rotates_and_clears_a_displaced_wide_spacer_like_alacritty() {
    let mut grid = Grid::new(12, 2, 0, CursorStyle::Block);
    grid.set_cursor_col(9);
    grid.put_char('界');
    grid.put_char('q');
    grid.set_cursor_col(0);
    grid.set_mode(false, 4, true);

    grid.put_char('a');
    grid.put_char('b');

    assert_eq!(grid.line(0).unwrap()[0].character, ' ');
    assert_eq!(grid.line(0).unwrap()[1].character, 'b');
}

#[test]
fn deleting_past_the_margin_clears_the_requested_tail_width() {
    let mut grid = Grid::new(12, 2, 0, CursorStyle::Block);
    grid.set_cursor_col(8);
    for character in "abc".chars() {
        grid.put_char(character);
    }

    grid.delete_chars(3);

    assert_eq!(
        grid.line(0)
            .unwrap()
            .iter()
            .skip(8)
            .take(4)
            .map(|cell| cell.character)
            .collect::<String>(),
        "a   "
    );
}

#[test]
fn saved_cursor_restores_pending_wrap() {
    let mut grid = Grid::new(3, 2, 0, CursorStyle::Block);
    for character in "abc".chars() {
        grid.put_char(character);
    }
    grid.save_cursor();
    grid.put_char('d');
    grid.restore_cursor();
    grid.put_char('e');

    assert_eq!(grid.line(0).unwrap()[2].character, 'c');
    assert_eq!(grid.line(1).unwrap()[0].character, 'e');
}

#[test]
fn explicit_line_feed_keeps_pending_wrap() {
    let mut grid = Grid::new(3, 3, 0, CursorStyle::Block);
    for character in "abc".chars() {
        grid.put_char(character);
    }

    grid.line_feed();
    assert_eq!(grid.cursor_position(), (2, 1));
    grid.put_char('d');

    assert_eq!(grid.line(0).unwrap()[2].character, 'c');
    assert_eq!(grid.line(2).unwrap()[0].character, 'd');
}

#[test]
fn backspace_in_a_single_column_keeps_pending_wrap() {
    let mut grid = Grid::new(1, 2, 0, CursorStyle::Block);
    grid.put_char('a');

    grid.backspace();
    grid.put_char('b');

    assert_eq!(grid.line(0).unwrap()[0].character, 'a');
    assert_eq!(grid.line(1).unwrap()[0].character, 'b');
}

#[test]
fn clearing_the_primary_viewport_moves_visible_rows_into_history() {
    let mut grid = Grid::new(4, 3, 8, CursorStyle::Block);
    grid.put_char('a');
    grid.set_cursor_position(2, 0);
    grid.put_char('b');

    grid.erase_display(2, false);

    assert_eq!(grid.history_size(), 3);
    assert_eq!(grid.line(-3).unwrap().get(0).unwrap().character, 'a');
    assert_eq!(grid.line(-1).unwrap().get(0).unwrap().character, 'b');
    assert!(
        grid.line(0)
            .unwrap()
            .iter()
            .all(|cell| cell.character == ' ')
    );
}

#[test]
fn clearing_an_empty_primary_viewport_seeds_scrollback_once() {
    let mut grid = Grid::new(4, 3, 8, CursorStyle::Block);

    grid.erase_display(2, false);
    assert_eq!(grid.history_size(), 1);

    grid.erase_display(2, false);
    assert_eq!(grid.history_size(), 1);
}

#[test]
fn scroll_region_survives_alternate_screen_entry() {
    let mut grid = Grid::new(4, 4, 0, CursorStyle::Block);
    grid.set_scroll_region(1, 2);

    grid.set_mode(true, 1049, true);

    assert_eq!(grid.scroll_region_status(), (2, 3));
}

#[test]
fn clearing_history_on_the_alternate_screen_keeps_primary_scrollback() {
    let mut grid = Grid::new(4, 2, 8, CursorStyle::Block);
    grid.put_char('a');
    grid.next_line();
    grid.next_line();
    assert_eq!(grid.history_size(), 1);

    grid.set_mode(true, 1049, true);
    grid.erase_display(3, false);
    grid.set_mode(true, 1049, false);

    assert_eq!(grid.history_size(), 1);
    assert_eq!(grid.line(-1).unwrap().get(0).unwrap().character, 'a');
}

#[test]
fn alternate_saved_cursor_survives_screen_reentry() {
    let mut grid = Grid::new(4, 4, 0, CursorStyle::Block);
    grid.set_mode(true, 1049, true);
    grid.set_cursor_position(1, 2);
    grid.save_cursor();
    grid.set_mode(true, 1049, false);

    grid.set_cursor_position(3, 3);
    grid.set_mode(true, 1049, true);
    grid.restore_cursor();

    assert_eq!(grid.cursor_position(), (2, 1));
}

#[test]
fn row_growth_pulls_recent_history_before_adding_blank_rows() {
    let mut grid = Grid::new(8, 2, 8, CursorStyle::Block);
    for line in ["one", "two", "three"] {
        for character in line.chars() {
            grid.put_char(character);
        }
        grid.next_line();
    }
    assert_eq!(grid.history_size(), 2);

    grid.resize(8, 4);

    assert_eq!(grid.history_size(), 0);
    assert_eq!(grid.cursor_position(), (0, 3));
    let rendered = (0..4)
        .map(|line| {
            grid.line(line)
                .unwrap()
                .iter()
                .map(|cell| cell.character)
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert_eq!(rendered, ["one     ", "two     ", "three   ", "        "]);
}

#[test]
fn row_shrink_scrolls_only_enough_to_keep_the_cursor_visible() {
    let mut grid = Grid::new(4, 4, 2, CursorStyle::Block);
    for (row, character) in ['a', 'b', 'c', 'd'].into_iter().enumerate() {
        grid.set_cursor_position(row, 0);
        grid.put_char(character);
    }
    grid.set_cursor_position(2, 1);

    grid.resize(4, 2);

    assert_eq!(grid.history_size(), 1);
    assert_eq!(grid.line(-1).unwrap().get(0).unwrap().character, 'a');
    assert_eq!(grid.line(0).unwrap()[0].character, 'b');
    assert_eq!(grid.line(1).unwrap()[0].character, 'c');
    assert_eq!(grid.cursor_position(), (1, 1));
}

#[test]
fn row_shrink_without_scrollback_discards_scrolled_rows() {
    let mut grid = Grid::new(4, 4, 0, CursorStyle::Block);
    for (row, character) in ['a', 'b', 'c', 'd'].into_iter().enumerate() {
        grid.set_cursor_position(row, 0);
        grid.put_char(character);
    }
    grid.set_cursor_position(3, 0);

    grid.resize(4, 2);

    assert_eq!(grid.history_size(), 0);
    assert_eq!(grid.line(0).unwrap()[0].character, 'c');
    assert_eq!(grid.line(1).unwrap()[0].character, 'd');
    assert_eq!(grid.cursor_position(), (0, 1));
}

#[test]
fn deleting_lines_from_the_top_records_primary_scrollback() {
    let mut grid = Grid::new(4, 2, 4, CursorStyle::Block);
    grid.put_char('a');
    grid.set_cursor_position(1, 0);
    grid.put_char('b');
    grid.set_cursor_position(0, 0);

    grid.delete_lines(1);

    assert_eq!(grid.history_size(), 1);
    assert_eq!(grid.line(-1).unwrap().get(0).unwrap().character, 'a');
    assert_eq!(grid.line(0).unwrap()[0].character, 'b');
}

#[test]
fn multi_row_shifts_preserve_order_through_history_rollover() {
    let mut grid = Grid::new(2, 4, 3, CursorStyle::Block);
    for (row, character) in ['a', 'b', 'c', 'd'].into_iter().enumerate() {
        grid.primary.row_mut_through(row, 1)[0].character = character;
    }

    grid.scroll_up(2);
    assert_eq!(grid.history_size(), 2);
    assert_eq!(grid.line(-2).unwrap().get(0).unwrap().character, 'a');
    assert_eq!(grid.line(-1).unwrap().get(0).unwrap().character, 'b');
    assert_eq!(grid.line(0).unwrap()[0].character, 'c');
    assert_eq!(grid.line(1).unwrap()[0].character, 'd');

    grid.primary.row_mut_through(2, 1)[0].character = 'e';
    grid.primary.row_mut_through(3, 1)[0].character = 'f';
    grid.scroll_up(2);
    assert_eq!(grid.history_size(), 3);
    assert_eq!(grid.line(-3).unwrap().get(0).unwrap().character, 'b');
    assert_eq!(grid.line(-2).unwrap().get(0).unwrap().character, 'c');
    assert_eq!(grid.line(-1).unwrap().get(0).unwrap().character, 'd');
    assert_eq!(grid.line(0).unwrap()[0].character, 'e');
    assert_eq!(grid.line(1).unwrap()[0].character, 'f');
    assert!(
        grid.line(2)
            .unwrap()
            .iter()
            .all(|cell| cell == Cell::default())
    );
    assert!(
        grid.line(3)
            .unwrap()
            .iter()
            .all(|cell| cell == Cell::default())
    );
}

#[test]
fn hyperlink_pruning_is_amortized_when_the_grid_retains_many_live_links() {
    let mut grid = Grid::new(HYPERLINK_PRUNE_MIN_LEN as u16, 1, 0, CursorStyle::Block);
    for id in 1..=HYPERLINK_PRUNE_MIN_LEN as u32 {
        grid.hyperlinks.insert(
            id,
            HyperlinkEntry {
                protocol_id: None,
                target: format!("https://example.com/{id}"),
            },
        );
        let hyperlink_id = NonZeroU32::new(id).expect("test hyperlink IDs are nonzero");
        let cell = &mut grid.primary.cells[0][id as usize - 1];
        cell.metadata_id = Some(inline_hyperlink_metadata_id(hyperlink_id));
        cell.set_has_hyperlink(true);
    }
    grid.next_hyperlink_id = HYPERLINK_PRUNE_MIN_LEN as u32 + 1;

    grid.set_hyperlink(None, Some("https://example.com/first-new"));
    assert_eq!(
        grid.next_hyperlink_prune_len,
        HYPERLINK_PRUNE_MIN_LEN + HYPERLINK_PRUNE_INTERVAL
    );

    for index in 1..HYPERLINK_PRUNE_INTERVAL {
        grid.set_hyperlink(None, Some(&format!("https://example.com/new/{index}")));
    }
    assert_eq!(
        grid.hyperlinks.len(),
        HYPERLINK_PRUNE_MIN_LEN + HYPERLINK_PRUNE_INTERVAL
    );
    assert_eq!(
        grid.next_hyperlink_prune_len,
        HYPERLINK_PRUNE_MIN_LEN + HYPERLINK_PRUNE_INTERVAL
    );

    grid.set_hyperlink(None, Some("https://example.com/prune-again"));
    assert_eq!(
        grid.hyperlinks.len(),
        HYPERLINK_PRUNE_MIN_LEN + 2,
        "the next amortized scan should retain the grid links and active pen only"
    );
    assert_eq!(
        grid.next_hyperlink_prune_len,
        HYPERLINK_PRUNE_MIN_LEN + HYPERLINK_PRUNE_INTERVAL + 1
    );
}

#[test]
fn pooled_metadata_and_hyperlinks_retain_live_history_roots() {
    let mut grid = grid(4, 2);
    grid.set_hyperlink(Some("live"), Some("https://example.com/live"));
    let live_hyperlink = grid.pen().hyperlink_id.expect("link is active");
    grid.put_char('e');
    grid.put_char('\u{301}');
    grid.put_char('\u{302}');
    let live_metadata = grid.primary.cells[0][0]
        .metadata_id
        .expect("combined metadata is pooled");
    assert!(is_pooled_extra_id(live_metadata));
    assert_eq!(
        grid.extras.len(),
        1,
        "appending mutates the cell's uniquely owned pooled value"
    );

    grid.set_hyperlink(None, None);
    grid.hyperlinks.insert(
        METADATA_VALUE_MASK,
        HyperlinkEntry {
            protocol_id: None,
            target: "https://example.com/dead".to_string(),
        },
    );
    grid.scroll_up(1);
    assert_eq!(grid.history.len(), 1);

    grid.prune_extras();
    assert_eq!(grid.extras.len(), 1);
    assert!(grid.extras.contains_key(&live_metadata));
    let history_cell = grid.history[0].get(0).expect("history cell");
    assert_eq!(grid.cell_hyperlink_id(&history_cell), Some(live_hyperlink));

    grid.prune_hyperlinks();
    assert!(grid.hyperlinks.contains_key(&live_hyperlink.get()));
    assert!(!grid.hyperlinks.contains_key(&METADATA_VALUE_MASK));
}

#[test]
fn compact_history_preserves_long_combining_text_with_and_without_links() {
    let mut grid = Grid::new(8, 2, 8, CursorStyle::Block);
    let marks = "\u{301}\u{302}\u{303}\u{304}\u{305}";
    grid.put_char('e');
    for mark in marks.chars() {
        grid.put_char(mark);
    }
    grid.set_hyperlink(None, Some("https://example.com/combined"));
    grid.put_char('a');
    for mark in marks.chars() {
        grid.put_char(mark);
    }
    let hyperlink = grid.pen().hyperlink_id.expect("link is active");
    grid.set_hyperlink(None, None);

    grid.scroll_up(1);
    let history = grid.line(-1).expect("combined cells moved into history");
    let unlinked = history.get(0).expect("unlinked combined cell");
    let linked = history.get(1).expect("linked combined cell");
    assert_eq!(
        grid.combining_text(&unlinked)
            .map(Combining::to_owned_string),
        Some(marks.to_string())
    );
    assert_eq!(grid.cell_hyperlink_id(&unlinked), None);
    assert_eq!(
        grid.combining_text(&linked).map(Combining::to_owned_string),
        Some(marks.to_string())
    );
    assert_eq!(grid.cell_hyperlink_id(&linked), Some(hyperlink));
}

#[test]
fn history_eviction_releases_uniquely_owned_pooled_metadata() {
    let mut grid = Grid::new(4, 2, 1, CursorStyle::Block);
    grid.set_hyperlink(None, Some("https://example.com/evicted"));
    grid.put_char('e');
    for mark in "\u{301}\u{302}\u{303}\u{304}\u{305}".chars() {
        grid.put_char(mark);
    }
    grid.set_hyperlink(None, None);

    grid.scroll_up(1);
    assert_eq!(grid.history.len(), 1);
    assert_eq!(grid.extras.len(), 1);

    grid.scroll_up(1);
    assert_eq!(grid.history.len(), 1);
    assert!(grid.extras.is_empty());
}

#[test]
fn shrinking_and_clearing_history_release_rows_and_rare_metadata() {
    let mut grid = Grid::new(8, 2, 8, CursorStyle::Block);

    for action in [
        Grid::set_history_limit as fn(&mut Grid, usize),
        |grid, _| {
            assert!(grid.clear_history());
        },
    ] {
        grid.set_history_limit(8);
        grid.set_hyperlink(Some("release"), Some("https://example.com/release"));
        grid.put_char('e');
        for mark in "\u{301}\u{302}\u{303}\u{304}\u{305}".chars() {
            grid.put_char(mark);
        }
        grid.set_hyperlink(None, None);
        grid.scroll_up(1);
        assert!(!grid.history.is_empty());
        assert!(!grid.extras.is_empty());
        assert!(!grid.hyperlinks.is_empty());

        action(&mut grid, 0);
        assert!(grid.history.is_empty());
        assert_eq!(grid.history.capacity(), 0);
        assert!(grid.extras.is_empty());
        assert_eq!(grid.extras.capacity(), 0);
        assert!(grid.hyperlinks.is_empty());
        assert_eq!(grid.hyperlinks.capacity(), 0);
        assert!(grid.hyperlink_identities.is_empty());
        assert_eq!(grid.hyperlink_identities.capacity(), 0);

        grid.set_cursor_position(0, 0);
        grid.put_char('x');
        assert_eq!(grid.line(0).unwrap()[0].character, 'x');
        grid.erase_display(2, false);
    }
}

#[test]
fn explicit_hyperlink_identity_index_reopens_without_duplication() {
    let mut grid = grid(4, 2);

    grid.set_hyperlink(Some("shared"), Some("https://example.com/one"));
    let first = grid.pen().hyperlink_id;
    grid.set_hyperlink(None, None);
    grid.set_hyperlink(Some("shared"), Some("https://example.com/one"));

    assert_eq!(grid.pen().hyperlink_id, first);
    assert_eq!(grid.hyperlinks.len(), 1);
    assert_eq!(grid.hyperlink_identities.len(), 1);

    grid.set_hyperlink(Some("shared"), Some("https://example.com/two"));
    assert_ne!(grid.pen().hyperlink_id, first);
    assert_eq!(grid.hyperlinks.len(), 2);
    assert_eq!(grid.hyperlink_identities.len(), 2);
}
