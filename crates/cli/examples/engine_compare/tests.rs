use super::*;

#[test]
fn workload_targets_match_the_optimization_contract() {
    let targets = WORKLOADS
        .iter()
        .map(|workload| (workload.id, workload.minimum_ratio))
        .collect::<Vec<_>>();

    assert_eq!(
        targets,
        vec![
            ("plain", 1.05),
            ("styled", 1.05),
            ("unicode", 1.00),
            ("edits", 1.00),
        ]
    );
    assert!(
        WORKLOADS
            .iter()
            .all(|workload| !workload.payload.is_empty())
    );
}

#[test]
fn sample_order_is_balanced_for_even_counts() {
    let tmon_first = (0..8)
        .map(|index| sample_order(index, true))
        .collect::<Vec<_>>();
    assert_eq!(tmon_first[0], [Engine::Tmon, Engine::Alacritty]);
    assert_eq!(tmon_first[1], [Engine::Alacritty, Engine::Tmon]);
    assert_eq!(
        tmon_first
            .iter()
            .filter(|order| order[0] == Engine::Tmon)
            .count(),
        4
    );

    let alacritty_first = (0..8)
        .map(|index| sample_order(index, false))
        .collect::<Vec<_>>();
    assert_eq!(alacritty_first[0], [Engine::Alacritty, Engine::Tmon]);
    assert_eq!(
        alacritty_first
            .iter()
            .filter(|order| order[0] == Engine::Alacritty)
            .count(),
        4
    );
}

#[test]
#[should_panic(expected = "TMON_BENCH_SAMPLES must be even")]
fn odd_sample_counts_are_rejected() {
    require_even_sample_count(7);
}

#[test]
fn stats_calculate_even_median_and_range() {
    assert_eq!(
        stats(&[4.0, 1.0, 3.0, 2.0]),
        Stats {
            median: 2.5,
            min: 1.0,
            max: 4.0,
        }
    );
}

#[test]
fn paired_stats_use_sample_ratios_instead_of_independent_medians() {
    let samples = [
        PairedSample {
            first: Engine::Tmon,
            tmon: 10.0,
            alacritty: 1.0,
        },
        PairedSample {
            first: Engine::Alacritty,
            tmon: 20.0,
            alacritty: 100.0,
        },
        PairedSample {
            first: Engine::Tmon,
            tmon: 30.0,
            alacritty: 10.0,
        },
        PairedSample {
            first: Engine::Alacritty,
            tmon: 40.0,
            alacritty: 20.0,
        },
    ];
    let result = paired_stats(&samples);
    assert_eq!(result.ratio.median, 2.5);
    assert_ne!(
        result.ratio.median,
        result.tmon.median / result.alacritty.median
    );
}

#[test]
fn threshold_helpers_reject_non_finite_values_and_apply_the_boundary() {
    assert_eq!(
        validate_target_ratio(f64::NAN),
        Err("target ratio must be finite")
    );
    assert_eq!(
        validate_target_ratio(f64::INFINITY),
        Err("target ratio must be finite")
    );
    assert_eq!(
        validate_target_ratio(f64::NEG_INFINITY),
        Err("target ratio must be finite")
    );
    assert!(meets_target(1.05, 1.05));
    assert!(!meets_target(1.049, 1.05));
    assert!(!meets_target(f64::NAN, 1.05));
}

#[test]
fn normalized_mixed_frame_matches_and_preserves_combining_text() {
    let tmon = tmon_terminal();
    let alacritty = alacritty_terminal();
    prepare_normalized_tmon(&tmon, 64 * 1024);
    prepare_normalized_alacritty(&alacritty, 64 * 1024);
    let tmon = normalized_tmon_frame(&tmon);
    let alacritty = normalized_alacritty_frame(&alacritty);

    assert_normalized_frames_match(&tmon, &alacritty, "normalized-frame unit fixture");
    assert_eq!(
        tmon.cells.len(),
        usize::from(tmon.cols) * usize::from(tmon.rows)
    );
    assert!(tmon.cells.iter().any(|cell| {
        cell.combining_len != 0 && normalized_combining_text(&tmon, cell) == "\u{301}"
    }));
    assert!(
        tmon.cells
            .iter()
            .any(|cell| cell.flags & NORMALIZED_WIDE_SPACER != 0)
    );
    assert!(
        tmon.cells
            .iter()
            .any(|cell| cell.flags & NORMALIZED_LEADING_WIDE_SPACER != 0)
    );
    assert!(
        tmon.cells
            .iter()
            .any(|cell| cell.flags & NORMALIZED_WRAPPED != 0)
    );
    assert!(
        tmon.cells
            .iter()
            .any(|cell| cell.flags & NORMALIZED_HYPERLINK != 0)
    );

    let underline_color_row = 34 * usize::from(tmon.cols);
    let expected_underline_colors = [
        ('R', Some(RawColor::Rgb(12, 34, 56))),
        ('I', Some(RawColor::Indexed(200))),
        ('C', Some(RawColor::Rgb(65, 43, 21))),
        ('X', Some(RawColor::Indexed(123))),
        ('N', None),
        ('Z', Some(RawColor::Rgb(1, 2, 3))),
        ('D', None),
    ];
    for (col, (character, underline_color)) in expected_underline_colors.into_iter().enumerate() {
        let cell = &tmon.cells[underline_color_row + col];
        assert_eq!(
            cell.character, character,
            "underline-color fixture column {col}"
        );
        assert_eq!(
            cell.underline_color, underline_color,
            "underline-color fixture column {col}"
        );
    }

    let underline_row = 35 * usize::from(tmon.cols);
    let expected_underlines = [
        ('S', NORMALIZED_SINGLE_UNDERLINE),
        ('D', NORMALIZED_DOUBLE_UNDERLINE),
        ('C', NORMALIZED_CURLY_UNDERLINE),
        ('O', NORMALIZED_DOTTED_UNDERLINE),
        ('H', NORMALIZED_DASHED_UNDERLINE),
        ('N', 0),
    ];
    for (col, (character, underline)) in expected_underlines.into_iter().enumerate() {
        let cell = &tmon.cells[underline_row + col];
        assert_eq!(cell.character, character, "underline fixture column {col}");
        assert_eq!(
            cell.flags & NORMALIZED_ALL_UNDERLINES,
            underline,
            "underline fixture column {col}"
        );
    }
}

#[test]
fn normalized_cell_contract_distinguishes_wide_spacer_roles() {
    let mut tmon_trailing = termy_core::tmon::Cell::default();
    tmon_trailing.set_wide_spacer(true);
    let mut tmon_leading = termy_core::tmon::Cell::default();
    tmon_leading.set_leading_wide_spacer(true);
    let tmon_trailing = normalized_tmon_cell(&tmon_trailing, None, &mut String::new());
    let tmon_leading = normalized_tmon_cell(&tmon_leading, None, &mut String::new());

    let alacritty_trailing = TerminalRenderCell {
        wide_character_spacer: true,
        ..TerminalRenderCell::default()
    };
    let alacritty_leading = TerminalRenderCell {
        leading_wide_character_spacer: true,
        ..TerminalRenderCell::default()
    };
    let alacritty_trailing = normalized_alacritty_cell(&alacritty_trailing, &mut String::new());
    let alacritty_leading = normalized_alacritty_cell(&alacritty_leading, &mut String::new());

    assert_eq!(tmon_trailing.flags, alacritty_trailing.flags);
    assert_eq!(tmon_leading.flags, alacritty_leading.flags);
    assert_ne!(tmon_trailing.flags, tmon_leading.flags);
    assert_eq!(tmon_trailing.flags, NORMALIZED_WIDE_SPACER);
    assert_eq!(tmon_leading.flags, NORMALIZED_LEADING_WIDE_SPACER);
}

#[test]
fn renderer_scroll_replay_preserves_effect_order() {
    let mut rows = vec!['a', 'b', 'c', 'd', 'e'];
    let scrolls = [
        ScrollDamage {
            top: 1,
            bottom: 4,
            count: 1,
            direction: ScrollDirection::Down,
        },
        ScrollDamage {
            top: 0,
            bottom: 3,
            count: 2,
            direction: ScrollDirection::Up,
        },
    ];

    assert!(replay_renderer_scrolls(&mut rows, &scrolls));
    assert_eq!(rows, ['b', 'c', 'a', 'e', 'd']);
}

#[test]
fn invalid_renderer_scroll_is_rejected_before_mutation() {
    let mut rows = vec!['a', 'b', 'c'];
    let original = rows.clone();
    let scrolls = [
        ScrollDamage {
            top: 0,
            bottom: 2,
            count: 1,
            direction: ScrollDirection::Up,
        },
        ScrollDamage {
            top: 2,
            bottom: 3,
            count: 1,
            direction: ScrollDirection::Down,
        },
    ];

    assert!(!replay_renderer_scrolls(&mut rows, &scrolls));
    assert_eq!(rows, original);
}

#[test]
fn renderer_cache_replay_and_patch_matches_fresh_rebuild() {
    let cols = 12;
    let rows = 5;
    let terminal = TmonTerminal::new_display(
        TmonSize {
            cols,
            rows,
            ..TmonSize::default()
        },
        TmonConfig {
            scrollback_history: 32,
            ..TmonConfig::default()
        },
    );
    terminal.feed_output(
        concat!(
            "\x1b[2J\x1b[1;1Hrow-one",
            "\x1b[2;1Hrow-two",
            "\x1b[3;1Hrow-three",
            "\x1b[4;1Hrow-four",
            "\x1b[5;1Hrow-five",
        )
        .as_bytes(),
    );
    let before = fresh_renderer_cache_frame(&terminal, cols, rows);
    assert!(matches!(
        terminal.take_render_damage_snapshot().damage,
        DamageSnapshot::Full
    ));

    terminal.feed_output(
        concat!(
            "\x1b[5;1Htail-A\r\n",
            "tail-B",
            "\x1b[2;4r\x1b[2;1H\x1bM",
            "\x1b[4;1H\n",
            "\x1b[r",
            "\x1b[3;2HZ cafe\u{301}",
        )
        .as_bytes(),
    );
    let update = terminal.take_render_damage_snapshot();
    assert!(matches!(update.damage, DamageSnapshot::Partial(_)));
    assert_eq!(
        update
            .scrolls
            .iter()
            .map(|scroll| scroll.direction)
            .collect::<Vec<_>>(),
        [
            ScrollDirection::Up,
            ScrollDirection::Down,
            ScrollDirection::Up,
        ]
    );

    let expected = fresh_renderer_cache_frame(&terminal, cols, rows);
    let mut actual = before.clone();
    assert!(Arc::ptr_eq(&actual.cell_rows, &before.cell_rows));
    assert!(apply_renderer_cache_update(&mut actual, &terminal, &update,));
    assert!(
        !Arc::ptr_eq(&actual.cell_rows, &before.cell_rows),
        "the cache update must copy-on-write the shared outer row table"
    );
    assert_eq!(actual, expected);
}

#[cfg(feature = "benchmark-allocations")]
fn allocation_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(feature = "benchmark-allocations")]
#[test]
fn allocation_counter_calculates_signed_requested_byte_change() {
    let positive = allocation_counter::AllocationSnapshot {
        alloc_calls: 2,
        alloc_bytes: 100,
        dealloc_calls: 1,
        dealloc_bytes: 40,
        realloc_calls: 1,
        realloc_old_bytes: 20,
        realloc_new_bytes: 70,
    };
    assert_eq!(positive.net_requested_bytes(), 110);

    let negative = allocation_counter::AllocationSnapshot {
        alloc_calls: 1,
        alloc_bytes: 10,
        dealloc_calls: 2,
        dealloc_bytes: 80,
        realloc_calls: 1,
        realloc_old_bytes: 30,
        realloc_new_bytes: 20,
    };
    assert_eq!(negative.net_requested_bytes(), -80);
}

#[cfg(feature = "benchmark-allocations")]
#[test]
fn allocation_measurement_rejects_nested_guards() {
    let _test_lock = allocation_test_lock();
    let outer = allocation_counter::AllocationMeasurement::begin()
        .expect("the first allocation measurement should start");
    assert!(
        allocation_counter::AllocationMeasurement::begin().is_err(),
        "a nested allocation measurement must be rejected"
    );
    drop(outer);

    let after_drop = allocation_counter::AllocationMeasurement::begin()
        .expect("dropping the outer guard should release the measurement state");
    drop(after_drop);
}

#[cfg(feature = "benchmark-allocations")]
#[test]
fn allocation_measurement_disables_during_panic_unwind() {
    let _test_lock = allocation_test_lock();
    let unwind = std::panic::catch_unwind(|| {
        let _measurement = allocation_counter::AllocationMeasurement::begin()
            .expect("allocation measurement should start before the test panic");
        panic!("intentional allocation-guard unwind");
    });
    assert!(unwind.is_err());

    let after_unwind = allocation_counter::AllocationMeasurement::begin()
        .expect("the allocation guard should disable itself while unwinding");
    drop(after_unwind);
}

#[cfg(feature = "benchmark-allocations")]
#[test]
fn allocation_measurement_waits_for_registered_hooks() {
    let _test_lock = allocation_test_lock();
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let finished_by_finisher = std::sync::Arc::clone(&finished);

    let measurement = allocation_counter::AllocationMeasurement::begin()
        .expect("allocation measurement should start");
    assert!(allocation_counter::register_hook_for_test());
    let finisher = std::thread::spawn(move || {
        black_box(measurement.finish());
        finished_by_finisher.store(true, std::sync::atomic::Ordering::Release);
    });

    while !allocation_counter::stopping_for_test() {
        std::hint::spin_loop();
    }
    assert!(
        !finished.load(std::sync::atomic::Ordering::Acquire),
        "finish must wait while an allocator hook remains registered"
    );
    allocation_counter::unregister_hook_for_test();
    finisher.join().expect("the finisher thread should exit");
    assert!(finished.load(std::sync::atomic::Ordering::Acquire));
}

#[cfg(feature = "benchmark-allocations")]
#[test]
fn stale_hook_registration_does_not_cross_measurement_generations() {
    let _test_lock = allocation_test_lock();
    let first = allocation_counter::AllocationMeasurement::begin()
        .expect("the first allocation measurement should start");
    let stale_gate = allocation_counter::gate_for_test();
    drop(first);

    let second = allocation_counter::AllocationMeasurement::begin()
        .expect("the second allocation measurement should start");
    assert!(
        !allocation_counter::register_hook_from_for_test(stale_gate),
        "a hook that observed the prior generation must remain uncounted"
    );
    drop(second);
}

#[cfg(feature = "benchmark-allocations")]
#[test]
fn allocation_measurement_counts_zeroed_allocations_and_deallocations() {
    let _test_lock = allocation_test_lock();
    let layout = std::alloc::Layout::from_size_align(4096, 64)
        .expect("the test allocation layout should be valid");
    let measurement = allocation_counter::AllocationMeasurement::begin()
        .expect("allocation measurement should start");
    // SAFETY: `layout` has non-zero size and a valid power-of-two alignment.
    // The returned pointer is checked before it is deallocated exactly once
    // with the same layout.
    let pointer = black_box(unsafe { std::alloc::alloc_zeroed(layout) });
    assert!(!pointer.is_null(), "the test allocation should succeed");
    // SAFETY: `pointer` is non-null and points to at least 4096 initialized
    // bytes. The volatile read makes the zeroed allocation observable to
    // the release optimizer before the allocation is freed.
    assert_eq!(unsafe { pointer.read_volatile() }, 0);
    // SAFETY: `pointer` came from `alloc_zeroed(layout)`, is non-null, and
    // has not been deallocated or reallocated.
    unsafe { std::alloc::dealloc(black_box(pointer), layout) };
    let snapshot = measurement.finish();

    assert!(snapshot.alloc_calls > 0);
    assert!(snapshot.alloc_bytes >= 4096);
    assert!(snapshot.dealloc_calls > 0);
    assert!(snapshot.dealloc_bytes >= 4096);
}

#[cfg(feature = "benchmark-allocations")]
#[test]
fn allocation_measurement_counts_successful_reallocations() {
    let _test_lock = allocation_test_lock();
    let old_layout = std::alloc::Layout::from_size_align(1024, 64)
        .expect("the old test allocation layout should be valid");
    // SAFETY: `old_layout` has non-zero size and a valid power-of-two
    // alignment. The pointer is checked and later passed exactly once to
    // `realloc` with its original layout.
    let pointer = black_box(unsafe { std::alloc::alloc(old_layout) });
    assert!(
        !pointer.is_null(),
        "the initial test allocation should succeed"
    );

    let measurement = allocation_counter::AllocationMeasurement::begin()
        .expect("allocation measurement should start");
    // SAFETY: `pointer` is a live allocation created with `old_layout`, and
    // 4096 is a valid non-zero new size for the same alignment.
    let new_pointer =
        black_box(unsafe { std::alloc::realloc(black_box(pointer), old_layout, 4096) });
    let snapshot = measurement.finish();
    if new_pointer.is_null() {
        // SAFETY: A failed `realloc` leaves the original allocation live,
        // so it must be released with its original layout.
        unsafe { std::alloc::dealloc(pointer, old_layout) };
        panic!("the test reallocation should succeed");
    }
    let new_layout = std::alloc::Layout::from_size_align(4096, 64)
        .expect("the new test allocation layout should be valid");
    // SAFETY: `new_pointer` is the successful result of reallocating to
    // 4096 bytes with alignment 64 and has not otherwise been freed.
    unsafe { std::alloc::dealloc(new_pointer, new_layout) };

    assert!(snapshot.realloc_calls > 0);
    assert!(snapshot.realloc_old_bytes >= 1024);
    assert!(snapshot.realloc_new_bytes >= 4096);
}
