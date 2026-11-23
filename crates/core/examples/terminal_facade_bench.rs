//! Uninstrumented throughput through the public terminal facade.
//!
//! Usage: terminal_facade_bench [MiB] [case] [--consume-damage]
//! Cases: all, plain, styled, unicode, varied-unicode, combining,
//! varied-combining, fragmented.
//! The original six payloads, dimensions, history limit, and warmup match
//! terminal_engine_bench. Varied combining stresses more distinct suffixes.
//! `--consume-damage` drains damage once per payload block, including warmup;
//! this exercises incremental damage tracking without rendering any cells.
//!
//! This source only uses public APIs shared with baseline 64945371, so build
//! the identical file against each revision when comparing: copy it over that
//! revision's crates/core/examples/terminal_facade_bench.rs, then run
//! cargo build --release -p termy_core --example terminal_facade_bench
//! and save target/release/examples/terminal_facade_bench.
//! Select a historical backend in the process environment, outside this helper.
//! Run saved binaries sequentially in alternating order after all builds finish.
//! Use terminal_engine_bench separately for allocation and retained-heap checks.

use std::{hint::black_box, time::Instant};
use termy_core::{Terminal, TerminalRuntimeConfig, TerminalSize};

fn run(case: &str, line: &[u8], target: usize, fragmentation: usize, consume_damage: bool) {
    let terminal = Terminal::new_display(
        TerminalSize {
            cols: 120,
            rows: 40,
            ..Default::default()
        },
        Some(&TerminalRuntimeConfig {
            scrollback_history: 1000,
            ..Default::default()
        }),
    );
    println!("engine: {}", terminal.engine_label());
    let mut payload = Vec::with_capacity(64 * 1024 + line.len());
    while payload.len() < 64 * 1024 {
        payload.extend_from_slice(line);
    }
    for _ in 0..8 {
        terminal.feed_output(&payload);
        if consume_damage {
            black_box(terminal.take_render_damage_snapshot());
        }
    }

    let iterations = target.div_ceil(payload.len());
    let start = Instant::now();
    for _ in 0..iterations {
        for chunk in payload.chunks(fragmentation) {
            terminal.feed_output(black_box(chunk));
        }
        if consume_damage {
            black_box(terminal.take_render_damage_snapshot());
        }
    }
    let elapsed = start.elapsed();
    black_box(terminal.cursor_position());
    let bytes = iterations * payload.len();
    let mib = bytes as f64 / 1_048_576.0;
    println!(
        "{case}: {:.3} MiB/s; {bytes} bytes; {:.6} seconds",
        mib / elapsed.as_secs_f64(),
        elapsed.as_secs_f64(),
    );
}

fn main() {
    let mut positional = Vec::new();
    let mut consume_damage = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--consume-damage" => consume_damage = true,
            "--help" | "-h" => {
                println!(
                    "Usage: terminal_facade_bench [MiB] [case] [--consume-damage]\n\
                     Cases: all, plain, styled, unicode, varied-unicode, combining, varied-combining, fragmented.\n\
                     Damage is drained once per payload block; no cells are rendered."
                );
                return;
            }
            _ if arg.starts_with('-') => panic!("unknown option: {arg}"),
            _ => positional.push(arg),
        }
    }
    assert!(positional.len() <= 2, "expected at most MiB and case");
    let mib = positional.first().map_or(32, |value| {
        value.parse::<usize>().expect("MiB must be an integer")
    });
    assert!((1..=4096).contains(&mib), "MiB must be in 1..=4096");
    let selected = positional.get(1).map_or("all", String::as_str);
    assert!(
        matches!(
            selected,
            "all"
                | "plain"
                | "styled"
                | "unicode"
                | "varied-unicode"
                | "combining"
                | "varied-combining"
                | "fragmented"
        ),
        "unknown case: {selected}"
    );
    println!(
        "terminal_facade: 120x40, 1000 history rows, {mib} MiB per case, consume_damage={consume_damage}"
    );

    let target = mib * 1_048_576;
    let run_case = |case: &str, line: &[u8], fragmentation| {
        if selected == "all" || selected == case {
            run(case, line, target, fragmentation, consume_damage);
        }
    };
    run_case(
        "plain",
        b"the quick brown fox jumps over the lazy dog 0123456789\r\n",
        64 * 1024,
    );
    run_case("styled", b"\x1b[H\x1b[38;2;120;180;255;48;5;234;1mstatus\x1b[0m\x1b[2;1H\x1b[Kbuild complete\x1b[3;1Hprogress: 100%", 64 * 1024);
    run_case(
        "unicode",
        "ASCII café Ελληνικά 日本語 한글 🙂界\r\n".as_bytes(),
        64 * 1024,
    );
    if selected == "all" || selected == "varied-unicode" {
        let mut varied = String::new();
        for scalar in 0x4e00..0x8e00 {
            varied.push(char::from_u32(scalar).unwrap());
            if scalar % 40 == 39 {
                varied.push_str("\r\n");
            }
        }
        run_case("varied-unicode", varied.as_bytes(), 64 * 1024);
    }
    run_case(
        "combining",
        "e\u{301} a\u{308} n\u{303} o\u{302} u\u{30a}\r\n".as_bytes(),
        64 * 1024,
    );
    if selected == "all" || selected == "varied-combining" {
        let mut varied = String::new();
        for (index, scalar) in (0x0300..=0x036f).enumerate() {
            varied.push('e');
            varied.push(char::from_u32(scalar).unwrap());
            varied.push(' ');
            if index % 10 == 9 {
                varied.push_str("\r\n");
            }
        }
        varied.push_str("\r\n");
        run_case("varied-combining", varied.as_bytes(), 64 * 1024);
    }
    run_case(
        "fragmented",
        b"the quick brown fox jumps over the lazy dog 0123456789\r\n",
        1,
    );
}
