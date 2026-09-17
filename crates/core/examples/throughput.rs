use std::{hint::black_box, time::Instant};

use termy_core::tmon::{Config, Size, Terminal};

const TARGET_BYTES: usize = 32 * 1024 * 1024;

fn run(label: &str, chunk: &[u8]) {
    let terminal = Terminal::new_display(
        Size {
            cols: 120,
            rows: 40,
            ..Size::default()
        },
        Config {
            scrollback_history: 10_000,
            ..Config::default()
        },
    );
    let iterations = TARGET_BYTES.div_ceil(chunk.len());
    let started = Instant::now();
    for _ in 0..iterations {
        terminal.feed_output(black_box(chunk));
    }
    let elapsed = started.elapsed();
    let bytes = iterations * chunk.len();
    let mib_per_second = bytes as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
    black_box(terminal.snapshot());
    println!(
        "{label}: {mib_per_second:.1} MiB/s ({bytes} bytes in {:.3}s)",
        elapsed.as_secs_f64()
    );
}

fn main() {
    println!(
        "tmon cell size: {} bytes",
        size_of::<termy_core::tmon::Cell>()
    );
    run(
        "plain scrollback",
        b"the quick brown fox jumps over the lazy dog 0123456789\r\n",
    );
    run(
        "styled TUI updates",
        b"\x1b[H\x1b[38;2;120;180;255;48;5;234;1mstatus\x1b[0m\x1b[2;1H\x1b[Kbuild complete\x1b[3;1Hprogress: 100%",
    );
    run(
        "Unicode and wide cells",
        "ASCII café Ελληνικά 日本語 한글 🙂界\r\n".as_bytes(),
    );
    run(
        "combining character cells",
        "e\u{301} a\u{308} n\u{303} o\u{302} u\u{30a}\r\n".as_bytes(),
    );
    run(
        "stacked combining cells",
        "e\u{301}\u{308} a\u{308}\u{304} n\u{303}\u{327}\r\n".as_bytes(),
    );
}
