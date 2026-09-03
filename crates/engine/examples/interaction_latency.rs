#![allow(clippy::cast_precision_loss)]

use std::{hint::black_box, time::Instant};

use engine::{Terminal, TerminalConfig};

const COLUMNS: usize = 160;
const ROWS: usize = 50;
const HISTORY_LINES: usize = 10_000;
const RESIZE_STEPS: usize = 80;
const SCROLL_STEPS: usize = 2_000;

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
    }
    black_box(terminal.frame_update(true));

    let started = Instant::now();
    for step in 0..RESIZE_STEPS {
        let columns = if step % 2 == 0 { COLUMNS - 1 } else { COLUMNS };
        terminal.resize(columns, ROWS);
        black_box(terminal.frame_update(true));
    }
    let resize_elapsed = started.elapsed();

    let started = Instant::now();
    for step in 0..SCROLL_STEPS {
        terminal.scroll_display(if step % 2 == 0 { 1 } else { -1 });
        black_box(terminal.frame_update(true));
    }
    let scroll_elapsed = started.elapsed();

    println!(
        "resize: {:.2} ms/step ({RESIZE_STEPS} steps, {HISTORY_LINES} history rows)",
        resize_elapsed.as_secs_f64() * 1_000.0 / RESIZE_STEPS as f64,
    );
    println!(
        "scroll: {:.3} ms/step ({SCROLL_STEPS} steps, {ROWS} visible rows)",
        scroll_elapsed.as_secs_f64() * 1_000.0 / SCROLL_STEPS as f64,
    );
}
