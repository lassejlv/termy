#![allow(clippy::cast_precision_loss)]

use std::{hint::black_box, time::Instant};

use engine::{Terminal, TerminalConfig};

const COLUMNS: usize = 160;
const ROWS: usize = 50;
const HISTORY_LINES: usize = 10_000;
const SUSTAINED_LINES: usize = 50_000;
const SAMPLE_LINES: usize = 5_000;
const SCROLL_STEPS: usize = 2_000;
const RESIZE_STEPS: usize = 80;

fn main() {
    let mut terminal = Terminal::new(TerminalConfig {
        columns: COLUMNS,
        rows: ROWS,
        scrollback_limit: HISTORY_LINES,
    });
    let mut line = vec![b'x'; COLUMNS - 2];
    line.extend_from_slice(b"\r\n");

    for _ in 0..(HISTORY_LINES + ROWS) {
        terminal.feed(&line);
        black_box(terminal.frame_update(false));
    }
    let warm = terminal.memory_stats();
    assert_eq!(warm.scrollback_rows, HISTORY_LINES);
    let capacity_allowance = (ROWS * 2 + 16) * COLUMNS;
    let capacity_ceiling = warm
        .total_cell_capacity()
        .saturating_add(capacity_allowance);

    terminal.reset_metrics();
    let sustained_started = Instant::now();
    let mut peak_cell_capacity = warm.total_cell_capacity();
    for line_index in 0..SUSTAINED_LINES {
        terminal.feed(&line);
        black_box(terminal.frame_update(false));
        if line_index % SAMPLE_LINES == SAMPLE_LINES - 1 {
            let sample = terminal.memory_stats();
            assert_eq!(sample.scrollback_rows, HISTORY_LINES);
            peak_cell_capacity = peak_cell_capacity.max(sample.total_cell_capacity());
            assert!(
                sample.total_cell_capacity() <= capacity_ceiling,
                "retained cell capacity grew after scrollback reached its limit: warm={}, sample={}, ceiling={capacity_ceiling}",
                warm.total_cell_capacity(),
                sample.total_cell_capacity(),
            );
        }
    }
    let sustained_elapsed = sustained_started.elapsed();

    let interaction_started = Instant::now();
    for step in 0..SCROLL_STEPS {
        terminal.scroll_display(if step % 2 == 0 { 1 } else { -1 });
        black_box(terminal.frame_update(true));
    }
    for step in 0..RESIZE_STEPS {
        terminal.resize(if step % 2 == 0 { COLUMNS - 1 } else { COLUMNS }, ROWS);
        black_box(terminal.frame_update(true));
    }
    let interaction_elapsed = interaction_started.elapsed();

    let final_stats = terminal.memory_stats();
    assert_eq!(final_stats.scrollback_rows, HISTORY_LINES);
    assert!(final_stats.total_cell_capacity() <= capacity_ceiling);
    assert!(peak_cell_capacity <= capacity_ceiling);

    println!(
        "stable history: {} retained rows, {:.1} MiB raw cell capacity, peak {} cells (ceiling {})",
        final_stats.scrollback_rows,
        final_stats.cell_capacity_bytes() as f64 / (1024.0 * 1024.0),
        peak_cell_capacity,
        capacity_ceiling,
    );
    println!(
        "sustained output: {:.1} lines/s ({} lines), {:.3} ms/scroll-or-resize step",
        SUSTAINED_LINES as f64 / sustained_elapsed.as_secs_f64(),
        SUSTAINED_LINES,
        interaction_elapsed.as_secs_f64() * 1_000.0 / (SCROLL_STEPS + RESIZE_STEPS) as f64,
    );
    println!("engine metrics: {:?}", terminal.metrics());
}
