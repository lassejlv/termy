use super::*;

fn scroll(top: usize, bottom: usize, count: usize, down: bool) -> TerminalViewportScroll {
    TerminalViewportScroll {
        top,
        bottom,
        count,
        direction: if down {
            TerminalViewportScrollDirection::Down
        } else {
            TerminalViewportScrollDirection::Up
        },
    }
}

fn seeded_cache(grid: &TerminalGrid) -> TerminalGridPaintCache {
    TerminalGridPaintCache {
        row_ops: (0..grid.rows)
            .map(|row| {
                grid.rebuild_cached_row_ops(
                    row,
                    &grid.cells[row],
                    grid.cursor_color,
                    grid.selection_fg,
                )
            })
            .collect(),
        style_key: Some(grid.paint_style_key()),
        last_cursor_cell: grid.cursor_cell,
        last_cursor_visible: grid.cursor_visible,
        ..Default::default()
    }
}

#[test]
fn scrolling_retains_unmodified_text_allocations_and_repairs_cursor_rows() {
    let mut grid = test_grid_rows(
        "abcde".chars().map(|c| vec![test_cell(0, c)]).collect(),
        None,
    );
    grid.cursor_cell = Some((0, 2));
    grid.cursor_visible = true;
    let mut cache = seeded_cache(&grid);
    let TextDrawOp::Batch(batch) = &cache.row_ops[3].draw_ops[0] else {
        panic!("text batch")
    };
    let text_ptr = batch.text.as_ptr();
    grid.paint_damage = TerminalGridPaintDamage::Scroll {
        scrolls: vec![scroll(1, 4, 1, false)].into(),
        ranges: vec![(4, 0, 0)].into(),
    };
    cache.ensure_row_capacity(grid.rows);
    let (full, style, dirty) = grid.dirty_rows_for_pass(&mut cache);
    assert!(!full && !style);
    assert_eq!(dirty, vec![1, 2, 4]);
    let TextDrawOp::Batch(batch) = &cache.row_ops[2].draw_ops[0] else {
        panic!("text batch")
    };
    assert_eq!(batch.text.as_ptr(), text_ptr);
    assert_eq!(batch.row, 2);
    assert_eq!(batch.text.as_ref(), "d");
}

#[test]
fn scroll_damage_matches_full_rebuild_across_regions_and_batches() {
    let original = test_grid_rows(
        "abcdefg"
            .chars()
            .map(|c| vec![test_cell(0, c), test_cell(1, '\u{2500}')])
            .collect(),
        None,
    );
    for operations in [
        vec![scroll(0, 6, 1, false)],
        vec![scroll(1, 5, 2, true)],
        vec![scroll(2, 4, 3, false)],
        vec![scroll(0, 6, 7, true)],
        vec![scroll(1, 6, 2, false), scroll(0, 5, 1, true)],
        vec![scroll(0, 6, 2, true), scroll(1, 5, 3, false)],
    ] {
        let mut cache = seeded_cache(&original);
        let mut cells: Vec<Vec<CellRenderInfo>> = original
            .cells
            .iter()
            .map(|row| row.as_ref().clone())
            .collect();
        for op in &operations {
            let region = &mut cells[op.top..=op.bottom];
            let exposed = match op.direction {
                TerminalViewportScrollDirection::Up => {
                    region.rotate_left(op.count);
                    op.bottom + 1 - op.count..op.bottom + 1
                }
                TerminalViewportScrollDirection::Down => {
                    region.rotate_right(op.count);
                    op.top..op.top + op.count
                }
            };
            for row in exposed {
                cells[row] = vec![test_cell(0, ' '), test_cell(1, ' ')];
            }
        }
        let mut next = test_grid_rows(cells, None);
        next.paint_damage = TerminalGridPaintDamage::Scroll {
            scrolls: operations.into(),
            ranges: Arc::from([]),
        };
        cache.ensure_row_capacity(next.rows);
        let (full, style, dirty) = next.dirty_rows_for_pass(&mut cache);
        assert!(!full && !style);
        next.rebuild_cached_rows_for_pass(
            &mut cache,
            full,
            style,
            &dirty,
            next.cursor_color,
            next.selection_fg,
        );
        let expected = seeded_cache(&next);
        for (actual, expected) in cache.row_ops.iter().zip(expected.row_ops.iter()) {
            assert!(cached_row_draw_ops_match_without_row(actual, expected));
            assert_eq!(actual.background_spans, expected.background_spans);
        }
    }
}

#[test]
fn invalid_scroll_batch_is_rejected_before_moving_any_cached_row() {
    let grid = test_grid_rows("abc".chars().map(|c| vec![test_cell(0, c)]).collect(), None);
    let mut cache = seeded_cache(&grid);
    let before = cache.row_ops.clone();
    assert!(!cache.scroll_rows(
        &[scroll(0, 2, 1, false), scroll(0, 3, 1, true)],
        &mut vec![]
    ));
    for (actual, expected) in cache.row_ops.iter().zip(before.iter()) {
        assert!(cached_row_draw_ops_match_without_row(actual, expected));
    }
}

#[test]
fn scrolling_with_hovered_links_rebuilds_coordinate_based_decorations() {
    let mut grid = test_grid_rows("abc".chars().map(|c| vec![test_cell(0, c)]).collect(), None);
    let mut cache = seeded_cache(&grid);
    grid.hovered_link_range = Some((0, 0, 0, 0));
    grid.paint_damage = TerminalGridPaintDamage::Scroll {
        scrolls: vec![scroll(0, 2, 1, false)].into(),
        ranges: Arc::from([]),
    };
    cache.ensure_row_capacity(grid.rows);
    assert!(grid.dirty_rows_for_pass(&mut cache).0);
}

#[test]
fn full_damage_reuses_shifted_cells_without_losing_cursor_or_hover_changes() {
    let old = test_grid_rows("abc".chars().map(|c| vec![test_cell(0, c)]).collect(), None);
    let mut cache = seeded_cache(&old);
    let TextDrawOp::Batch(batch) = &cache.row_ops[1].draw_ops[0] else {
        panic!("text batch")
    };
    let retained_ptr = batch.text.as_ptr();
    let mut next = test_grid_rows("bcd".chars().map(|c| vec![test_cell(0, c)]).collect(), None);
    cache.ensure_row_capacity(next.rows);
    next.rebuild_cached_rows_for_pass(
        &mut cache,
        true,
        false,
        &[],
        next.cursor_color,
        next.selection_fg,
    );
    let TextDrawOp::Batch(batch) = &cache.row_ops[0].draw_ops[0] else {
        panic!("text batch")
    };
    assert_eq!(batch.text.as_ptr(), retained_ptr);
    assert_eq!(batch.row, 0);

    next.cursor_cell = Some((0, 0));
    next.cursor_visible = true;
    next.hovered_link_range = Some((1, 0, 1, 0));
    next.rebuild_cached_rows_for_pass(
        &mut cache,
        true,
        false,
        &[],
        next.cursor_color,
        next.selection_fg,
    );
    let expected = seeded_cache(&next);
    for (actual, expected) in cache.row_ops.iter().zip(expected.row_ops.iter()) {
        assert!(cached_row_draw_ops_match_without_row(actual, expected));
    }
}

#[test]
fn matching_text_does_not_reuse_changed_colors_styles_or_combining_marks() {
    let old = test_grid_rows(vec![vec![test_cell(0, 'a'), test_cell(1, 'b')]], None);
    let mut changed = old.cells[0].as_ref().clone();
    changed[0].fg = test_color(0.7, 0.8, 0.9);
    changed[0].bg = test_color(0.2, 0.3, 0.4);
    changed[0].bold = true;
    changed[1].combining = Some("\u{301}".into());
    changed[1].selected = true;
    let next = test_grid_rows(vec![changed], None);
    assert_eq!(
        row_text_signature(&old.cells[0]),
        row_text_signature(&next.cells[0])
    );
    let mut cache = seeded_cache(&old);
    cache.ensure_row_capacity(1);
    next.rebuild_cached_rows_for_pass(
        &mut cache,
        true,
        false,
        &[],
        next.cursor_color,
        next.selection_fg,
    );
    assert!(cached_row_draw_ops_match_without_row(
        &cache.row_ops[0],
        &seeded_cache(&next).row_ops[0]
    ));
}

#[test]
fn repeated_full_damage_scrolling_matches_rebuilding_every_row() {
    const ROWS: usize = 48;
    const COLS: usize = 120;
    const FRAMES: usize = 240;
    fn row(number: usize) -> Vec<CellRenderInfo> {
        (0..COLS)
            .map(|col| {
                let mut cell = test_cell(
                    col,
                    char::from_u32(b'!' as u32 + ((number + col) % 90) as u32).unwrap(),
                );
                cell.fg = test_color((col / 20) as f32 / 10.0, 0.5, 0.5);
                cell
            })
            .collect()
    }
    let mut timings = Vec::new();
    let mut results = Vec::new();
    for reuse_cells in [false, true] {
        let mut grid = test_grid_rows((0..ROWS).map(row).collect(), None);
        let mut cache = seeded_cache(&grid);
        let mut elapsed = std::time::Duration::ZERO;
        for frame in 0..FRAMES {
            let cells = Arc::make_mut(&mut grid.cells);
            cells.rotate_left(1);
            cells[ROWS - 1] = Arc::new(row(ROWS + frame));
            // Full snapshots from the default engine may allocate new rows even
            // when their contents only moved. Exercise value equality too.
            for cells in cells {
                *cells = Arc::new(cells.as_ref().clone());
            }
            if !reuse_cells {
                for row in &mut cache.row_ops {
                    row.source_cells = None;
                }
            }
            cache.ensure_row_capacity(ROWS);
            let start = Instant::now();
            grid.rebuild_cached_rows_for_pass(
                &mut cache,
                true,
                false,
                &[],
                grid.cursor_color,
                grid.selection_fg,
            );
            elapsed += start.elapsed();
        }
        results.push(cache);
        timings.push(elapsed);
    }
    for (actual, expected) in results[0].row_ops.iter().zip(results[1].row_ops.iter()) {
        assert!(cached_row_draw_ops_match_without_row(actual, expected));
    }
    eprintln!(
        "scroll row-cache workload ({FRAMES} frames, {ROWS}x{COLS}): rebuild={:?}, reuse={:?}",
        timings[0], timings[1]
    );
}
