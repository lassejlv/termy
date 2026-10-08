//! Text-feed cost with direct image placements parked on the inactive screen.
//! Uploads, snapshot validation, screen switching and warmup precede timing.
//! Compile identical source against baseline and candidate release libraries.

use std::{hint::black_box, time::Instant};
use termy_core::terminal_engine::{Engine, Options, Size};
fn main() {
    for round in 0..5 {
        let counts = if round % 2 == 0 {
            vec![0, 64, 512, 4096]
        } else {
            vec![4096, 512, 64, 0]
        };
        for count in counts {
            let mut e = Engine::new(
                Size {
                    cols: 120,
                    rows: 40,
                },
                Options::default(),
            );
            if count > 0 {
                e.feed(b"\x1b_Ga=t,f=32,s=1,v=1,i=1,q=2;AQID/w==\x1b\\");
                for p in 1..=count {
                    e.feed(format!("\x1b_Ga=p,i=1,p={p},c=1,r=1,C=1,q=2;\x1b\\").as_bytes());
                }
                assert_eq!(e.graphics_placements().len(), count);
            }
            // Measure alternate-screen text with every image parked on the inactive primary screen.
            e.feed(b"\x1b[?47h");
            for _ in 0..1000 {
                e.feed(b"\rx");
            }
            let start = Instant::now();
            for _ in 0..100_000 {
                e.feed(black_box(b"\rx"));
            }
            println!(
                "round={round} inactive_direct_placements={count} elapsed_us={}",
                start.elapsed().as_micros()
            );
            black_box(e.viewport_row(0));
        }
    }
}
