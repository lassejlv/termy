use super::super::{Engine, Options, Size};
use super::{MIN_SNAPSHOT_VERSION, SNAPSHOT_VERSION, SnapshotError, snapshot_version};

fn engine(cols: usize, rows: usize, history: usize) -> Engine {
    Engine::new(
        Size { cols, rows },
        Options {
            scrollback_history: history,
            ..Default::default()
        },
    )
}

fn roundtrip(engine: &mut Engine) -> Engine {
    let bytes = engine.encode_snapshot();
    Engine::decode_snapshot(&bytes).expect("decode snapshot")
}

/// Feed `tail` to `engine` and to a copy decoded from its snapshot, and
/// require identical states (and replies) afterwards.
fn assert_continues_identically(engine: &mut Engine, tail: &[u8], context: &str) {
    let mut decoded = roundtrip(engine);
    assert_eq!(
        decoded.canonical_snapshot(),
        engine.canonical_snapshot(),
        "decoded state differs before the tail ({context})"
    );
    engine.feed(tail);
    decoded.feed(tail);
    // The decoded engine ran no synchronized update timer; commit both.
    engine.stop_synchronized_update();
    decoded.stop_synchronized_update();
    let (mut expected, mut actual) = (Vec::new(), Vec::new());
    engine.drain_replies(&mut expected);
    decoded.drain_replies(&mut actual);
    assert_eq!(actual, expected, "replies differ ({context})");
    assert_eq!(
        decoded.canonical_snapshot(),
        engine.canonical_snapshot(),
        "states differ after the tail ({context})"
    );
    for line in -(engine.history_size() as i32)..engine.size().rows as i32 {
        assert_eq!(
            decoded.line(line),
            engine.line(line),
            "line {line} ({context})"
        );
        assert_eq!(decoded.line_wrapped(line), engine.line_wrapped(line));
    }
    assert_eq!(decoded.cursor(), engine.cursor());
    assert_eq!(decoded.modes(), engine.modes());
    assert_eq!(decoded.title(), engine.title());
    assert_eq!(decoded.working_directory(), engine.working_directory());
}

/// A deterministic stream exercising most of the parser and grid.
fn corpus() -> Vec<u8> {
    let mut out = Vec::new();
    let pieces: &[&[u8]] = &[
        b"plain ascii line\r\n",
        b"\x1b[1;31mbold red\x1b[0m \x1b[4:3;58;2;10;20;30mcurly\x1b[24;59m\r\n",
        "wide \u{4e2d}\u{6587} and e\u{301}\u{302} \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} flag \u{1f1e9}\u{1f1ea}\r\n"
            .as_bytes(),
        b"\x1b]8;id=a;https://example.com\x07link\x1b]8;;\x07 after\r\n",
        b"\x1b]0;window title\x07\x1b[22;0t\x1b]2;second\x07",
        b"\x1b]7;file://host/tmp/some%20dir\x07",
        b"\x1b]4;1;rgb:12/34/56\x07\x1b]10;#abcdef\x07\x1b]11;rgb:01/02/03\x1b\\",
        b"\x1b[?1h\x1b=\x1b[?2004h\x1b[?1004h\x1b[?1002h\x1b[?1006h\x1b[>5u\x1b[?7727h",
        b"\x1b(0lqqk\x1b(B\x0e\x0f",
        b"\x1b[3;10r\x1b[5;1Htext in region\x1bD\x1bM\x1b[r",
        b"\x1b[?69h\x1b[5;40s\x1b[2;3Hmargins\x1b[?69l",
        b"\x1b7\x1b[10;10Hsaved\x1b8",
        b"\x1b[3g\x1b[1;5H\x1bH\x1b[1;13H\x1bH\r\tA\tB\r\n",
        b"\x1b[?1049hALT SCREEN\x1b[5;5Hx\x1b[>1u",
        b"\x1b[?1049l",
        b"\x1b[?6h\x1b[2;2Horigin\x1b[?6l\x1b[4hins\x1b[4l\x1b[20h\x1b[20l",
        b"\x1b[2 q\x1b[?12h\x1b[?25l\x1b[?25h",
        b"\x1b]7501;id=a;state=working;app=test\x1b\\",
        b"\x1b[5ba\x1b[3brepeat\r\n",
        b"\x1b[38;5;200;48;2;1;2;3mcolored\x1b[K\x1b[0m\r\n",
        b"\x1b[2J\x1b[H",
        b"line that is long enough to wrap around the right margin of the screen ok\r\n",
        b"\x1b[6n\x1b[c\x1b[?2004$p\x1b]10;?\x07\x1bP$qm\x1b\\",
        b"\x1b[?2026hsynchronized \x1b[1mtext",
        b"\x1b[?2026l",
        b"\x1b[3Sscrolled\x1b[2T\x1b[2L\x1b[M\x1b[3@\x1b[2P\x1b[4X",
        "\u{1f600}\u{1f600}\u{1f600}\u{1f600}\u{1f600}\u{1f600}\u{1f600}\u{1f600}\r\n".as_bytes(),
        b"\x1b#8\x1b[H\x1b[J",
    ];
    for round in 0..6 {
        for piece in pieces {
            out.extend_from_slice(piece);
        }
        for line in 0..12 {
            out.extend_from_slice(
                format!("round {round} line {line} \x1b[3{}m", line % 8).as_bytes(),
            );
            out.extend_from_slice(b"text\r\n");
        }
    }
    out
}

/// A small deterministic generator, so failures reproduce.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

fn random_stream(random: &mut Lcg, len: usize) -> Vec<u8> {
    const ALPHABET: &[&[u8]] = &[
        b"a",
        b"Z",
        b" ",
        b"\r",
        b"\n",
        b"\t",
        b"\x08",
        b"\x1b",
        b"[",
        b"]",
        b";",
        b"?",
        b"0",
        b"1",
        b"2",
        b"4",
        b"7",
        b"m",
        b"H",
        b"J",
        b"K",
        b"h",
        b"l",
        b"r",
        b"P",
        b"\\",
        b"\x07",
        b"(",
        b"q",
        b"u",
        b">",
        b"8",
        b":",
        "\u{301}".as_bytes(),
        "\u{4e2d}".as_bytes(),
        "\u{1f600}".as_bytes(),
        b"\xe2",
        b"\x80",
        b"\xf0\x9f",
        b"\x1b[",
        b"\x1b]",
        b"\x1bP",
        b"\x1b_G",
        b"\x1b[?2026h",
        b"\x1b[?1049h",
        b"\x1b[?1049l",
        b"\x1b]8;;http://x\x07",
        b"\x1b]8;;\x07",
        b"\x1b[38;2;1;2;3m",
        b"\x1b[0m",
        b"\x1b[?69h",
        b"\x1b[2;9s",
    ];
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        out.extend_from_slice(ALPHABET[random.below(ALPHABET.len())]);
    }
    out
}

#[test]
fn snapshot_has_versioned_header() {
    let bytes = engine(20, 5, 10).encode_snapshot();
    assert_eq!(&bytes[..8], b"TRMYSNAP");
    assert_eq!(snapshot_version(&bytes), Ok(SNAPSHOT_VERSION));

    let mut future = bytes.clone();
    future[8..12].copy_from_slice(&(SNAPSHOT_VERSION + 1).to_le_bytes());
    assert_eq!(
        Engine::decode_snapshot(&future).err(),
        Some(SnapshotError::UnsupportedVersion(SNAPSHOT_VERSION + 1))
    );
    assert_eq!(
        Engine::decode_snapshot(b"not a snapshot at all").err(),
        Some(SnapshotError::NotASnapshot)
    );
    assert_eq!(
        Engine::decode_snapshot(&bytes[..10]).err(),
        Some(SnapshotError::Truncated)
    );
}

#[test]
fn truncated_or_damaged_snapshots_are_rejected_without_panicking() {
    let mut source = engine(30, 6, 50);
    source.feed(&corpus());
    let bytes = source.encode_snapshot();
    for len in (0..bytes.len()).step_by(7) {
        assert!(
            Engine::decode_snapshot(&bytes[..len]).is_err(),
            "length {len}"
        );
    }
    let mut random = Lcg(7);
    for _ in 0..400 {
        let mut damaged = bytes.clone();
        let index = 24 + random.below(damaged.len() - 24);
        damaged[index] ^= 1 << random.below(8);
        // Either rejected or a structurally valid engine that can keep going.
        if let Ok(mut decoded) = Engine::decode_snapshot(&damaged) {
            decoded.feed(b"\x1b[2J\x1b[Hstill alive\r\n\x1b[?1049h\x1b[?1049l");
            decoded.resize(Size { cols: 12, rows: 3 });
            decoded.feed(b"more text after resize\r\n");
        }
    }
}

#[test]
fn corpus_split_at_every_boundary_continues_identically() {
    let corpus = corpus();
    // Every split of the first rounds, then a stride through the rest.
    let splits = (0..corpus.len() / 3).chain((corpus.len() / 3..corpus.len()).step_by(17));
    for split in splits {
        let mut source = engine(40, 8, 64);
        source.feed(&corpus[..split]);
        let mut sink = Vec::new();
        source.drain_replies(&mut sink);
        assert_continues_identically(&mut source, &corpus[split..], &format!("split {split}"));
    }
}

#[test]
fn random_streams_split_at_random_points_continue_identically() {
    let mut random = Lcg(0x5eed);
    for case in 0..300 {
        let cols = 4 + random.below(60);
        let rows = 2 + random.below(14);
        let history = random.below(80);
        let len = 64 + random.below(2048);
        let stream = random_stream(&mut random, len);
        let split = random.below(stream.len() + 1);
        let mut source = engine(cols, rows, history);
        // Feed the prefix in random fragments, as a PTY would.
        let mut offset = 0;
        while offset < split {
            let end = (offset + 1 + random.below(64)).min(split);
            source.feed(&stream[offset..end]);
            offset = end;
        }
        if random.below(4) == 0 {
            source.compact_history();
        }
        let mut sink = Vec::new();
        source.drain_replies(&mut sink);
        assert_continues_identically(
            &mut source,
            &stream[split..],
            &format!("case {case}, {cols}x{rows}, history {history}, split {split}"),
        );
    }
}

#[test]
fn resize_and_scrolling_after_decode_match() {
    let mut source = engine(50, 10, 200);
    source.feed(&corpus());
    source.scroll_display(7);
    let mut decoded = roundtrip(&mut source);
    assert_eq!(decoded.display_offset(), 7);
    for engine in [&mut source, &mut decoded] {
        engine.resize(Size { cols: 23, rows: 7 });
        engine.feed(b"after resize \xe4\xb8\xad\r\n");
        engine.resize(Size { cols: 61, rows: 12 });
    }
    assert_eq!(decoded.canonical_snapshot(), source.canonical_snapshot());
}

#[test]
fn packed_history_keeps_its_representation_and_charge() {
    let mut source = engine(120, 10, 2000);
    for line in 0..1500 {
        source.feed(format!("\x1b[3{}mhistory line {line} \u{e9}\u{301}\r\n", line % 8).as_bytes());
    }
    source.compact_history();
    let decoded = roundtrip(&mut source);
    assert_eq!(
        decoded.state.grid.history_bytes(),
        source.state.grid.history_bytes()
    );
    assert_eq!(decoded.history_size(), source.history_size());
}

#[test]
fn large_scrollback_encodes_quickly() {
    // About 16 MiB of dense scrollback: 20,000 rows of 100 cells.
    let mut source = engine(100, 50, 20_000);
    let line: Vec<u8> = (0..99).map(|index| b'!' + (index % 90) as u8).collect();
    for row in 0..20_050 {
        if row % 50 == 0 {
            source.feed(b"\x1b[1;32m");
        }
        source.feed(&line);
        source.feed(b"\r\n");
    }
    let started = std::time::Instant::now();
    let bytes = source.encode_snapshot();
    let encode = started.elapsed();
    let started = std::time::Instant::now();
    let decoded = Engine::decode_snapshot(&bytes).expect("decode");
    let decode = started.elapsed();
    assert_eq!(decoded.history_size(), source.history_size());
    eprintln!(
        "snapshot of {} history rows: {} bytes, encode {encode:?}, decode {decode:?}",
        source.history_size(),
        bytes.len()
    );
    // Generous for unoptimized test builds; release builds are far faster.
    assert!(encode.as_secs() < 5, "{encode:?}");
}

#[test]
fn history_byte_budget_evicts_identically_after_decode() {
    // 300 dense columns cross the 16 MiB history budget well before the
    // row limit, so eviction depends on how every row is charged.
    let mut source = engine(300, 8, 20_000);
    let line: Vec<u8> = (0..299).map(|index| b'#' + (index % 80) as u8).collect();
    let feed = |engine: &mut Engine, rows: std::ops::Range<usize>| {
        for row in rows {
            engine.feed(format!("\x1b[3{}m", row % 8).as_bytes());
            engine.feed(&line);
            engine.feed(b"\r\n");
        }
    };
    feed(&mut source, 0..3000);
    source.compact_history();
    feed(&mut source, 3000..3500);
    let mut decoded = roundtrip(&mut source);
    for engine in [&mut source, &mut decoded] {
        feed(engine, 3500..9000);
        engine.compact_history();
        feed(engine, 9000..14_000);
    }
    // The budget, not the row limit, bounded history.
    assert!(
        source.history_size() < 14_000 - 8,
        "{}",
        source.history_size()
    );
    assert_eq!(decoded.history_size(), source.history_size());
    assert_eq!(
        decoded.state.grid.history_bytes(),
        source.state.grid.history_bytes()
    );
    assert_eq!(decoded.canonical_snapshot(), source.canonical_snapshot());
}

/// Lines mixing OSC 8 links (some too long for the composed-cell cache) with
/// combining marks on linked and unlinked cells.
fn linked_marks(engine: &mut Engine, lines: std::ops::Range<usize>) {
    let long = "x".repeat(1200);
    for line in lines {
        let uri = if line % 7 == 0 {
            format!("https://example.com/{long}/{}", line % 3)
        } else {
            format!("https://example.com/{}", line % 5)
        };
        engine.feed(format!("\x1b]8;id=l{};{uri}\x07", line % 4).as_bytes());
        for col in 0..30 {
            let mark = char::from_u32(0x300 + ((line * 31 + col) % 7) as u32).unwrap();
            engine.feed(format!("e{mark}").as_bytes());
            if col % 9 == 0 {
                engine.feed("\u{302}".as_bytes());
            }
        }
        engine.feed(b"\x1b]8;;\x07 ");
        engine.feed("o\u{308}u\u{308} \u{1f44d}\u{1f3fd}\r\n".as_bytes());
    }
}

#[test]
fn links_with_combining_marks_evict_identically_after_decode() {
    // A small byte limit makes every few lines evict, so a charge differing
    // by one unit would show as a different history.
    let mut source = Engine::new(
        Size { cols: 80, rows: 6 },
        Options {
            scrollback_history: usize::MAX,
            scrollback_bytes: 96 * 1024,
            ..Default::default()
        },
    );
    linked_marks(&mut source, 0..400);
    let mut decoded = roundtrip(&mut source);
    // Eviction converges again once differently charged rows leave, so
    // compare after every line.
    for line in 400..1800 {
        if line == 1500 {
            source.resize(Size { cols: 57, rows: 9 });
            decoded.resize(Size { cols: 57, rows: 9 });
        }
        linked_marks(&mut source, line..line + 1);
        linked_marks(&mut decoded, line..line + 1);
        assert_eq!(
            (decoded.history_size(), decoded.state.grid.history_bytes()),
            (source.history_size(), source.state.grid.history_bytes()),
            "line {line}"
        );
    }
    assert!(source.history_size() < 1800, "{}", source.history_size());
    assert_eq!(decoded.history_size(), source.history_size());
    assert_eq!(
        decoded.state.grid.history_bytes(),
        source.state.grid.history_bytes()
    );
    assert_eq!(decoded.canonical_snapshot(), source.canonical_snapshot());
}

#[test]
fn rows_beyond_twenty_thousand_survive_a_snapshot() {
    // A flood packs rows with room for a whole line, so give them a budget
    // that holds every one.
    let mut source = Engine::new(
        Size { cols: 80, rows: 10 },
        Options {
            scrollback_history: usize::MAX,
            scrollback_bytes: 64 * 1024 * 1024,
            ..Default::default()
        },
    );
    for line in 0..50_000 {
        source.feed(format!("short line {line}\r\n").as_bytes());
    }
    assert_eq!(source.history_size(), 50_000 - 9);
    let mut decoded = roundtrip(&mut source);
    assert_eq!(decoded.scrollback_limit(), usize::MAX);
    assert_eq!(decoded.history_size(), source.history_size());
    assert_eq!(decoded.canonical_snapshot(), source.canonical_snapshot());
}

/// The stream `snapshot/v1.snap` was written from, by the release that
/// introduced snapshots (40x6, 500 rows of history, replies drained).
fn v1_fixture_stream() -> Vec<u8> {
    let mut out = Vec::new();
    for line in 0..120 {
        out.extend_from_slice(format!("\x1b[3{}mline {line} ", line % 8).as_bytes());
        out.extend_from_slice("e\u{301}\u{302} \u{4e2d}\u{6587} ".as_bytes());
        out.extend_from_slice(
            format!(
                "\x1b]8;id=l{};https://example.com/{}\x07link\x1b]8;;\x07",
                line % 3,
                line % 5
            )
            .as_bytes(),
        );
        out.extend_from_slice("a\u{308}\x1b[4:3;58;5;1mcurly\x1b[0m\r\n".as_bytes());
    }
    out.extend_from_slice(
        b"\x1b]0;fixture title\x07\x1b]7;file://host/tmp\x07\x1b[?2004h\x1b[?1049hALT\x1b[?1049l",
    );
    out.extend_from_slice(b"\x1b[2;4r\x1b[?6h\x1b[?6l\x1b[r\x1b]2;partial ti");
    out
}

#[test]
fn snapshots_from_the_previous_release_decode() {
    let fixture = include_bytes!("v1.snap");
    assert_eq!(snapshot_version(fixture), Ok(1));
    let mut decoded = Engine::decode_snapshot(fixture).expect("decode a version 1 snapshot");
    let mut expected = engine(40, 6, 500);
    expected.feed(&v1_fixture_stream());
    let mut sink = Vec::new();
    expected.drain_replies(&mut sink);
    assert_eq!(decoded.canonical_snapshot(), expected.canonical_snapshot());
    assert_eq!(
        decoded.scrollback_byte_limit(),
        super::super::DEFAULT_SCROLLBACK_BYTES
    );
    // The in-progress OSC 2 completes after decode.
    decoded.feed(b"tle\x07");
    assert_eq!(decoded.title(), "partial title");
    assert_continues_identically(&mut decoded, &corpus(), "after a version 1 decode");
}

#[test]
fn older_versions_are_written_for_older_readers() {
    assert_eq!(MIN_SNAPSHOT_VERSION, 1);
    let mut source = engine(40, 6, 500);
    source.feed(&v1_fixture_stream());
    let v1 = source.encode_snapshot_version(1).expect("encode version 1");
    assert_eq!(snapshot_version(&v1), Ok(1));
    let mut decoded = Engine::decode_snapshot(&v1).expect("decode version 1");
    assert_eq!(decoded.canonical_snapshot(), source.canonical_snapshot());
    assert_eq!(
        source.encode_snapshot_version(SNAPSHOT_VERSION + 1).err(),
        Some(SnapshotError::UnsupportedVersion(SNAPSHOT_VERSION + 1))
    );
    assert_eq!(
        source.encode_snapshot_version(0).err(),
        Some(SnapshotError::UnsupportedVersion(0))
    );

    // Version 1 holds at most 20,000 rows: the newest are kept, and newer
    // limits fall back to the defaults.
    let mut large = Engine::new(
        Size { cols: 30, rows: 4 },
        Options {
            scrollback_history: usize::MAX,
            scrollback_bytes: 8 * 1024 * 1024,
            max_string_bytes: 1024,
            ..Default::default()
        },
    );
    for line in 0..25_000 {
        large.feed(format!("row {line}\r\n").as_bytes());
    }
    large.scroll_display(24_000);
    let mut decoded = Engine::decode_snapshot(&large.encode_snapshot_version(1).unwrap()).unwrap();
    assert_eq!(decoded.history_size(), 20_000);
    assert_eq!(decoded.scrollback_limit(), 20_000);
    assert_eq!(decoded.display_offset(), 20_000);
    assert_eq!(
        decoded.options(),
        Options {
            scrollback_history: 20_000,
            ..Default::default()
        }
    );
    assert_eq!(decoded.line(-1), large.line(-1));
    assert_eq!(
        decoded.line(-20_000),
        large.line(-20_000),
        "the oldest row kept"
    );
    decoded.feed(b"more\r\n");
    assert_eq!(decoded.history_size(), 20_000);
}

/// Mutated snapshots that decoded and then panicked on the next wide glyph,
/// found by the TERA-14 audit's snapshot-mutation fuzzer: each turns a
/// margin byte into a left/right margin region one column wide.
#[test]
fn fuzzed_snapshots_that_used_to_panic_are_rejected() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/terminal_engine/snapshot/fuzz_crashes");
    let mut inputs: Vec<_> = std::fs::read_dir(&dir)
        .expect("fuzz crash fixtures")
        .map(|entry| entry.expect("fixture").path())
        .collect();
    inputs.sort();
    assert_eq!(inputs.len(), 22);
    for path in inputs {
        let bytes = std::fs::read(&path).expect("read fixture");
        match Engine::decode_snapshot(&bytes) {
            Err(error) => assert_eq!(
                error,
                SnapshotError::Corrupt("margins"),
                "{}",
                path.display()
            ),
            Ok(mut engine) => {
                // Whatever still decodes must survive what used to panic.
                engine.feed("\x1b[H界界界\u{301}\x1b[?69h\x1b[2;5s\x1b[3@界".as_bytes());
                engine.resize(Size { cols: 2, rows: 3 });
                engine.feed("界界".as_bytes());
            }
        }
    }
}

#[test]
fn color_scheme_reports_survive_a_snapshot() {
    let mut source = engine(20, 4, 10);
    source.feed(b"\x1b[?2031h");
    let mut decoded = roundtrip(&mut source);
    assert!(decoded.modes().color_scheme_reports);
    decoded.feed(b"\x1b[?2031$p");
    let mut reply = Vec::new();
    decoded.drain_replies(&mut reply);
    assert_eq!(reply, b"\x1b[?2031;1$y");
    assert_continues_identically(&mut source, b"\x1b[?2031l", "after ?2031");

    // Version 2 has no room for the mode: it decodes as off, and every
    // other field still decodes.
    let mut source = engine(20, 4, 10);
    source.feed(b"\x1b[?2031h\x1b[?2004hv2 text");
    let v2 = source.encode_snapshot_version(2).expect("encode version 2");
    assert_eq!(snapshot_version(&v2), Ok(2));
    let decoded = Engine::decode_snapshot(&v2).expect("decode version 2");
    assert!(!decoded.modes().color_scheme_reports);
    assert!(decoded.modes().bracketed_paste);
    assert_eq!(decoded.line(0), source.line(0));
}

#[test]
fn payload_limits_survive_a_snapshot() {
    let mut source = Engine::new(
        Size { cols: 40, rows: 4 },
        Options {
            max_string_bytes: 16,
            max_clipboard_bytes: 32,
            ..Default::default()
        },
    );
    let mut decoded = roundtrip(&mut source);
    assert_eq!(decoded.options(), source.options());
    for engine in [&mut source, &mut decoded] {
        engine.feed(b"\x1b]2;short\x07");
        assert_eq!(engine.title(), "short");
        engine.feed(b"\x1b]2;a title past sixteen bytes\x07");
        assert_eq!(engine.title(), "short");
    }
}

fn graphics_summary(engine: &mut Engine) -> Vec<String> {
    engine
        .graphics_placements()
        .into_iter()
        .map(|placement| {
            format!(
                "{} {} {}x{} gen {} at {},{}+{} z {} rgba {:?}",
                placement.image_id,
                placement.placement_id,
                placement.image_width,
                placement.image_height,
                placement.image_generation,
                placement.viewport_row,
                placement.col,
                placement.col_offset,
                placement.z_index,
                placement.image.rgba(),
            )
        })
        .collect()
}

#[test]
fn kitty_graphics_survive_a_snapshot() {
    let mut source = engine(20, 6, 50);
    // A direct placement, a virtual placement shown by placeholder cells, a
    // relative placement, an animation frame and an upload still arriving.
    source.feed(b"text\r\n\x1b_Ga=T,f=32,s=2,v=1,i=1,p=3,c=2,r=1,z=-1,q=2;AQID/wQFBv8=\x1b\\");
    source.feed(b"\r\n\x1b_Ga=T,f=24,s=1,v=1,i=2,p=1,c=1,r=1,U=1,q=2;CQoL\x1b\\");
    source.feed("\x1b[38;5;2m\u{10eeee}\u{0305}\u{0305}\x1b[0m\r\n".as_bytes());
    source.feed(b"\x1b_Ga=p,i=1,p=4,P=1,Q=3,H=1,V=1,q=2\x1b\\");
    source.feed(b"\x1b_Ga=f,i=1,f=32,s=1,v=1,x=1,q=2;/wAA/w==\x1b\\");
    source.feed(b"\x1b_Ga=a,i=1,s=3,v=2,z=100,q=2\x1b\\");
    source.feed(b"\x1b_Ga=t,f=32,s=1,v=1,i=9,m=1,q=2;AQID\x1b\\");
    assert!(source.graphics_placements().len() >= 3);
    let mut decoded = roundtrip(&mut source);
    assert_eq!(decoded.canonical_snapshot(), source.canonical_snapshot());
    assert_eq!(
        graphics_summary(&mut decoded),
        graphics_summary(&mut source)
    );
    assert_eq!(decoded.graphics_revision(), source.graphics_revision());

    for engine in [&mut source, &mut decoded] {
        // Finish the pending upload and place it, then scroll and delete.
        engine.feed(b"\x1b_Gm=0;/w==\x1b\\\x1b_Ga=p,i=9,q=2\x1b\\");
        engine.feed(b"\r\n\r\n\r\nscrolled\r\n");
        engine.feed(b"\x1b_Ga=d,d=i,i=2,q=2\x1b\\");
        engine.feed(b"\x1b_Ga=T,f=32,s=1,v=1,c=1,r=1,q=2;CAgI/w==\x1b\\");
    }
    assert_eq!(
        graphics_summary(&mut decoded),
        graphics_summary(&mut source)
    );
    assert_eq!(decoded.canonical_snapshot(), source.canonical_snapshot());
    assert_eq!(decoded.graphics_revision(), source.graphics_revision());
}

#[test]
fn version_1_snapshots_leave_graphics_out() {
    let mut source = engine(20, 4, 10);
    source.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=1,c=1,r=1,q=2;AQID/w==\x1b\\");
    assert_eq!(source.graphics_placements().len(), 1);
    let v1 = source.encode_snapshot_version(1).unwrap();
    let mut decoded = Engine::decode_snapshot(&v1).unwrap();
    assert!(decoded.graphics_placements().is_empty());
    assert_eq!(source.graphics_placements().len(), 1);
}

#[test]
fn damaged_graphics_snapshots_are_rejected_without_panicking() {
    let mut source = engine(20, 6, 20);
    source.feed(b"\x1b_Ga=T,f=32,s=2,v=1,i=1,c=2,r=1,q=2;AQID/wQFBv8=\x1b\\");
    source.feed(b"\x1b_Ga=f,i=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\");
    source.feed(b"\x1b_Ga=t,f=32,s=1,v=1,i=9,m=1,q=2;AQID\x1b\\");
    let bytes = source.encode_snapshot();
    let mut random = Lcg(11);
    for _ in 0..600 {
        let mut damaged = bytes.clone();
        let index = 24 + random.below(damaged.len() - 24);
        damaged[index] ^= 1 << random.below(8);
        if let Ok(mut decoded) = Engine::decode_snapshot(&damaged) {
            decoded.feed(b"\x1b_Gm=0;/w==\x1b\\\x1b_Ga=f,i=1,f=32,s=1,v=1,q=2;AQID/w==\x1b\\");
            decoded.feed(b"\x1b_Ga=a,i=1,s=3,q=2\x1b\\\r\n\r\n\x1b_Ga=d,q=2\x1b\\");
            let _ = decoded.graphics_placements();
        }
    }
}

/// Lines of varying length and color, some with links and marks.
fn budget_stream(lines: std::ops::Range<usize>) -> Vec<u8> {
    let mut out = Vec::new();
    for line in lines {
        out.extend_from_slice(format!("\x1b[3{}m", line % 8).as_bytes());
        if line % 5 == 0 {
            out.extend_from_slice(
                format!("\x1b]8;;https://example.com/{}\x07", line % 3).as_bytes(),
            );
        }
        out.extend_from_slice(format!("line {line} ").as_bytes());
        out.extend_from_slice("x".repeat(line * 13 % 90).as_bytes());
        if line % 7 == 0 {
            out.extend_from_slice("e\u{301}".as_bytes());
        }
        out.extend_from_slice(b"\x1b]8;;\x07\x1b[0m\r\n");
    }
    out
}

#[test]
fn byte_budget_eviction_ignores_feed_splits_and_compaction_timing() {
    let new = || {
        Engine::new(
            Size { cols: 80, rows: 6 },
            Options {
                scrollback_history: usize::MAX,
                scrollback_bytes: 128 * 1024,
                ..Default::default()
            },
        )
    };
    let stream = budget_stream(0..6000);
    // A host fed whole PTY reads and compacting after each, a replica fed
    // small fragments with idle and forced steps in between, and an engine
    // that never compacts. Each sees the same bytes.
    let mut host = new();
    let mut replica = new();
    let mut never = new();
    let mut random = Lcg(0xb0d6e7);
    for (index, chunk) in stream.chunks(5000).enumerate() {
        host.feed(chunk);
        if index % 3 == 0 {
            host.compact_history();
        }
        let mut offset = 0;
        while offset < chunk.len() {
            let end = (offset + 1 + random.below(40)).min(chunk.len());
            replica.feed(&chunk[offset..end]);
            if random.below(6) == 0 {
                replica.compact_history_step(1 + random.below(300), random.below(2) == 0);
            }
            offset = end;
        }
        never.feed(chunk);
        assert_eq!(
            (replica.history_size(), replica.state.grid.history_bytes()),
            (host.history_size(), host.state.grid.history_bytes()),
            "chunk {index}"
        );
        assert_eq!(never.history_size(), host.history_size(), "chunk {index}");
    }
    assert!(host.history_size() < 6000 - 6, "{}", host.history_size());
    // The same rows survive (compaction state itself differs).
    for line in -(host.history_size() as i32)..host.size().rows as i32 {
        assert_eq!(replica.line(line), host.line(line), "line {line}");
        assert_eq!(never.line(line), host.line(line), "line {line}");
    }
}
