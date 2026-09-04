#![no_main]

use engine::Terminal;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = Terminal::from_snapshot(data);
});
