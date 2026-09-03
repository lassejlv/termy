#![allow(clippy::cast_precision_loss)]

use std::{hint::black_box, time::Instant};

use engine::{Terminal, TerminalConfig};

const COLUMNS: usize = 120;
const ROWS: usize = 40;
const ITERATIONS: usize = 20_000;

fn main() {
    let mut terminal = Terminal::new(TerminalConfig {
        columns: COLUMNS,
        rows: ROWS,
        scrollback_limit: 10_000,
    });
    black_box(terminal.frame_update(true));
    terminal.reset_metrics();

    let line = b"\r\x1b[2K\x1b[38;2;129;161;193mtmon\x1b[0m: compiling frame 0123456789 abcdefghijklmnopqrstuvwxyz";
    let started = Instant::now();
    let mut bytes = 0_usize;
    for _ in 0..ITERATIONS {
        terminal.feed(line);
        bytes += line.len();
        black_box(terminal.frame_update(false));
    }
    let metrics = terminal.metrics();
    let elapsed = started.elapsed();
    let megabytes = bytes as f64 / 1_000_000.0;
    println!(
        "rewrite: {:.1} MB/s, {:.1} us/update, {} copied cells, {} row updates",
        megabytes / elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1_000_000.0 / ITERATIONS as f64,
        metrics.cells_copied,
        metrics.row_updates,
    );

    let stream = vec![b'x'; 4096];
    let chunks = 2_500;
    terminal.reset_metrics();
    let started = Instant::now();
    for _ in 0..chunks {
        terminal.feed(&stream);
        black_box(terminal.frame_update(false));
    }
    let elapsed = started.elapsed();
    let megabytes = (stream.len() * chunks) as f64 / 1_000_000.0;
    println!(
        "stream: {:.1} MB/s, {:.1} us/chunk, {} damaged frames",
        megabytes / elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1_000_000.0 / chunks as f64,
        terminal.metrics().damaged_frames,
    );
}
