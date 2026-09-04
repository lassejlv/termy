#![no_main]

use engine::{Terminal, TerminalConfig};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut terminal = Terminal::new(TerminalConfig {
        columns: 80,
        rows: 24,
        scrollback_limit: 256,
    });
    for chunk in data.chunks(257) {
        terminal.feed(chunk);
        let _ = terminal.drain_events();
    }
    let _ = terminal.frame_update(false);
});
