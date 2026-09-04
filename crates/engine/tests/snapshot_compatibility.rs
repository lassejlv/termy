use std::fs;

use engine::{TERMINAL_SNAPSHOT_VERSION, Terminal, TerminalConfig};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Corpus {
    schema_version: u16,
    fixtures: Vec<Fixture>,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    name: String,
    snapshot_version: u16,
    encoding: String,
    expected_current_result: String,
    hex: String,
}

#[test]
fn versioned_snapshot_fixtures_lock_acceptance_and_rejection() {
    let source = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/protocol/snapshots.json"
    ))
    .expect("read snapshot fixture corpus");
    let corpus: Corpus = serde_json::from_str(&source).expect("parse snapshot fixture corpus");
    assert_eq!(corpus.schema_version, 1);
    assert_eq!(corpus.fixtures.len(), 2);

    let legacy = fixture(&corpus, "terminal-snapshot-v2-bincode");
    assert_eq!(legacy.snapshot_version, 2);
    assert_eq!(legacy.encoding, "bincode-2-standard");
    assert_eq!(legacy.expected_current_result, "reject");
    let error = Terminal::from_snapshot(&decode_hex(&legacy.hex))
        .expect_err("v2 bincode snapshot must not enter the v3 postcard decoder");
    assert!(
        error
            .to_string()
            .contains("unsupported terminal snapshot version 2")
            || error
                .to_string()
                .contains("deserializing terminal snapshot"),
        "unexpected v2 rejection: {error:#}"
    );

    let current = fixture(&corpus, "terminal-snapshot-v3-postcard");
    assert_eq!(current.snapshot_version, TERMINAL_SNAPSHOT_VERSION);
    assert_eq!(current.encoding, "postcard-1");
    assert_eq!(current.expected_current_result, "accept");
    let bytes = decode_hex(&current.hex);
    let mut restored = Terminal::from_snapshot(&bytes).expect("restore current golden snapshot");
    assert_eq!(restored.dimensions(), (8, 2));
    assert_eq!(restored.encode_paste("paste"), b"\x1b[200~paste\x1b[201~");

    let mut regenerated = fixture_terminal();
    assert_eq!(
        regenerated.snapshot().expect("regenerate current snapshot"),
        bytes,
        "snapshot bytes changed without a deliberate fixture/version update"
    );
    assert_eq!(
        restored.frame_update(true),
        regenerated.frame_update(true),
        "golden snapshot must restore the intended terminal cells and metadata"
    );
}

fn fixture<'a>(corpus: &'a Corpus, name: &str) -> &'a Fixture {
    corpus
        .fixtures
        .iter()
        .find(|fixture| fixture.name == name)
        .expect("named snapshot fixture exists")
}

fn fixture_terminal() -> Terminal {
    let mut terminal = Terminal::new(TerminalConfig {
        columns: 8,
        rows: 2,
        scrollback_limit: 3,
    });
    terminal.feed(b"alpha\r\n\x1b[31mred\x1b[0m\x1b[?2004h");
    terminal.set_pixel_size(80, 40);
    terminal
}

fn decode_hex(hex: &str) -> Vec<u8> {
    assert!(hex.len().is_multiple_of(2));
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digits = std::str::from_utf8(pair).expect("fixture hex is UTF-8");
            u8::from_str_radix(digits, 16).expect("fixture contains hexadecimal bytes")
        })
        .collect()
}
