use super::*;

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Print(char),
    Execute(u8),
    Escape(Vec<u8>, u8),
    Csi(Vec<Vec<Option<u16>>>, Option<u8>, Vec<u8>, u8),
    Osc(Vec<u8>),
    Dcs(Vec<u8>),
    Apc(Vec<u8>),
}

#[derive(Default)]
struct Recorder {
    events: Vec<Event>,
    ascii_runs: usize,
}

impl Handler for Recorder {
    fn print(&mut self, character: char) {
        self.events.push(Event::Print(character));
    }

    fn print_ascii(&mut self, bytes: &[u8]) {
        self.ascii_runs += 1;
        assert!(bytes.iter().all(|byte| (0x20..=0x7e).contains(byte)));
        for &byte in bytes {
            self.print(char::from(byte));
        }
    }

    fn execute(&mut self, byte: u8) {
        self.events.push(Event::Execute(byte));
    }

    fn escape(&mut self, intermediates: &[u8], final_byte: u8) {
        self.events
            .push(Event::Escape(intermediates.to_vec(), final_byte));
    }

    fn csi(&mut self, params: &[Param], private: Option<u8>, intermediates: &[u8], final_byte: u8) {
        let params = params
            .iter()
            .map(|param| {
                let mut values = vec![param.value()];
                values.extend_from_slice(param.subparams());
                values
            })
            .collect();
        self.events.push(Event::Csi(
            params,
            private,
            intermediates.to_vec(),
            final_byte,
        ));
    }

    fn osc(&mut self, bytes: &[u8]) {
        self.events.push(Event::Osc(bytes.to_vec()));
    }

    fn dcs(&mut self, bytes: &[u8]) {
        self.events.push(Event::Dcs(bytes.to_vec()));
    }

    fn apc(&mut self, bytes: &mut Vec<u8>) {
        self.events.push(Event::Apc(bytes.clone()));
    }
}

fn parse(bytes: &[u8]) -> Vec<Event> {
    let mut recorder = Recorder::default();
    Parser::default().advance(&mut recorder, bytes);
    recorder.events
}

fn printed(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Print(character) => Some(*character),
            _ => None,
        })
        .collect()
}

#[test]
fn parses_text_controls_and_csi_with_omitted_subparameters() {
    assert_eq!(
        parse(b"a\n\x1b[?1049h\x1b[38:2::255:0:9;1m\x1b[;H\x1b[m\x1b[2 q\x1b(0"),
        vec![
            Event::Print('a'),
            Event::Execute(b'\n'),
            Event::Csi(vec![vec![Some(1049)]], Some(b'?'), vec![], b'h'),
            Event::Csi(
                vec![
                    vec![Some(38), Some(2), None, Some(255), Some(0), Some(9)],
                    vec![Some(1)]
                ],
                None,
                vec![],
                b'm'
            ),
            Event::Csi(vec![vec![None], vec![None]], None, vec![], b'H'),
            Event::Csi(vec![], None, vec![], b'm'),
            Event::Csi(vec![vec![Some(2)]], None, vec![b' '], b'q'),
            Event::Escape(vec![b'('], b'0'),
        ]
    );
}

#[test]
fn every_split_boundary_matches_unfragmented_stream() {
    let bytes = "ASCII é中🙂\n\x1b[?1049h\x1b[38:2::255:2:9m\x1b[1;2 H\x1b(0\x1b]2;标题\x07\x1b]8;;https://example.test\x1b\\\x1bP$qm\x1b\\\x1b_Ga=T;payload\x1b\\end".as_bytes();
    let expected = parse(bytes);
    for split in 0..=bytes.len() {
        let mut parser = Parser::default();
        let mut recorder = Recorder::default();
        parser.advance(&mut recorder, &bytes[..split]);
        parser.advance(&mut recorder, &bytes[split..]);
        assert_eq!(recorder.events, expected, "split at {split}");
    }
    for chunk_size in 1..=17 {
        let mut parser = Parser::default();
        let mut recorder = Recorder::default();
        for chunk in bytes.chunks(chunk_size) {
            parser.advance(&mut recorder, chunk);
        }
        assert_eq!(recorder.events, expected, "chunks of {chunk_size}");
    }
}

#[test]
fn utf8_validation_matches_lossy_decoding_including_reconsumption() {
    let samples: &[&[u8]] = &[
        b"\xc0\xaf!",
        b"\xed\xa0\x80!",
        b"\xe0\x80\x80!",
        b"\xf0\x80\x80\x80!",
        b"\xf4\x90\x80\x80!",
        b"\xf5\x80\x80\x80!",
        b"\x80\xbf\xfe\xff!",
        b"\xf0\x9f\x92!",
        b"\xe2\x82!",
        b"\xc2!",
        b"\xe2\xc2\xa3!",
        b"\xf0\x9f\xe2\x82\xac!",
        // U+0080 to U+009F are C1 controls, not text.
        "\u{a0}\u{7ff}\u{800}\u{d7ff}\u{e000}\u{ffff}\u{10000}\u{10ffff}!".as_bytes(),
    ];
    for &bytes in samples {
        let expected = String::from_utf8_lossy(bytes);
        for split in 0..=bytes.len() {
            let mut parser = Parser::default();
            let mut recorder = Recorder::default();
            parser.advance(&mut recorder, &bytes[..split]);
            parser.advance(&mut recorder, &bytes[split..]);
            assert_eq!(
                printed(&recorder.events),
                expected,
                "bytes {bytes:?}, split {split}"
            );
        }
    }
}

#[test]
fn invalid_utf8_reconsumes_escape_and_controls() {
    assert_eq!(
        parse(b"\xe2\x82\x1b[31m\xc2\nX"),
        vec![
            Event::Print(char::REPLACEMENT_CHARACTER),
            Event::Csi(vec![vec![Some(31)]], None, vec![], b'm'),
            Event::Print(char::REPLACEMENT_CHARACTER),
            Event::Execute(b'\n'),
            Event::Print('X'),
        ]
    );
}

#[test]
fn incomplete_utf8_waits_for_another_chunk() {
    let mut parser = Parser::default();
    let mut recorder = Recorder::default();
    parser.advance(&mut recorder, b"\xf0\x9f");
    parser.advance(&mut recorder, b"");
    assert!(recorder.events.is_empty());
    parser.advance(&mut recorder, b"\x99\x82");
    assert_eq!(recorder.events, vec![Event::Print('🙂')]);
}

#[test]
fn complete_utf8_matches_every_scalar_and_rejects_incomplete_prefixes() {
    let mut buffer = [0; 4];
    for character in (128..=0x10ffff).filter_map(char::from_u32) {
        let bytes = character.encode_utf8(&mut buffer).as_bytes();
        assert_eq!(super::complete_utf8(bytes), Some((character, bytes.len())));
        for len in 0..bytes.len() {
            assert_eq!(super::complete_utf8(&bytes[..len]), None);
        }
    }
}

#[test]
fn complete_utf8_rejects_invalid_leads_and_continuation_positions() {
    for lead in 0..=u8::MAX {
        for next in 0..=u8::MAX {
            // Invalid lead/second-byte combinations cover overlong encodings,
            // surrogates, and out-of-range scalars; later bytes test reconsumption.
            for bytes in [
                [lead, next, 0x80, 0x80],
                [lead, 0x80, next, 0x80],
                [lead, 0x80, 0x80, next],
            ] {
                let len = match lead {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => {
                        assert_eq!(super::complete_utf8(&bytes), None);
                        continue;
                    }
                };
                let expected = std::str::from_utf8(&bytes[..len])
                    .ok()
                    .and_then(|text| text.chars().next())
                    .map(|character| (character, len));
                assert_eq!(super::complete_utf8(&bytes), expected, "{bytes:?}");
            }
        }
    }
}

#[test]
fn control_bytes_do_not_reset_escape_or_csi_parameters() {
    assert_eq!(
        parse(b"\x1b\n(\x7f0\x1b[1\t;2\r\x7fH"),
        vec![
            Event::Execute(b'\n'),
            Event::Escape(vec![b'('], b'0'),
            Event::Execute(b'\t'),
            Event::Execute(b'\r'),
            Event::Csi(vec![vec![Some(1)], vec![Some(2)]], None, vec![], b'H'),
        ]
    );
}

#[test]
fn cancellation_discards_unfinished_sequences_and_strings() {
    for cancel in [0x18, 0x1a] {
        for prefix in [
            b"\x1b[12".as_slice(),
            b"\x1b(",
            b"\x1b]2;title",
            b"\x1bPdata",
            b"\x1b_data",
        ] {
            let mut bytes = prefix.to_vec();
            bytes.extend_from_slice(&[cancel, b'O', b'K']);
            assert_eq!(parse(&bytes), vec![Event::Print('O'), Event::Print('K')]);
        }
    }
}

#[test]
fn escape_interrupts_unfinished_sequences_and_strings() {
    for prefix in [
        b"\x1b[12".as_slice(),
        b"\x1b(",
        b"\x1b]2;title",
        b"\x1bPdata",
        b"\x1b_data",
    ] {
        let mut bytes = prefix.to_vec();
        bytes.extend_from_slice(b"\x1b[32m!");
        assert_eq!(
            parse(&bytes),
            vec![
                Event::Csi(vec![vec![Some(32)]], None, vec![], b'm'),
                Event::Print('!'),
            ]
        );
    }
}

#[test]
fn numeric_parameters_saturate_without_allocation_or_overflow() {
    let mut bytes = b"\x1b[".to_vec();
    bytes.extend(std::iter::repeat_n(b'9', 100_000));
    bytes.extend_from_slice(b":999999999;m");
    assert_eq!(
        parse(&bytes),
        vec![Event::Csi(
            vec![vec![Some(u16::MAX), Some(u16::MAX)], vec![None]],
            None,
            vec![],
            b'm',
        )]
    );
}

#[test]
fn parameter_and_intermediate_limits_reject_whole_sequence() {
    for parameter_bytes in *b";:" {
        let limit = if parameter_bytes == b';' {
            MAX_PARAMS - 1
        } else {
            MAX_SUBPARAMS
        };
        let mut bytes = b"\x1b[".to_vec();
        bytes.extend(std::iter::repeat_n(parameter_bytes, limit));
        bytes.push(b'm');
        assert!(matches!(parse(&bytes).as_slice(), [Event::Csi(..)]));
        bytes.insert(bytes.len() - 1, parameter_bytes);
        bytes.extend_from_slice(b"OK\x1b[31m");
        assert_eq!(
            parse(&bytes),
            vec![
                Event::Print('O'),
                Event::Print('K'),
                Event::Csi(vec![vec![Some(31)]], None, vec![], b'm'),
            ]
        );
    }
    assert_eq!(
        parse(b"\x1b[  q\x1b  0"),
        vec![
            Event::Csi(vec![], None, vec![b' ', b' '], b'q'),
            Event::Escape(vec![b' ', b' '], b'0'),
        ]
    );
    assert_eq!(
        parse(b"\x1b[   \nmOK\x1b   \t0!"),
        vec![Event::Print('O'), Event::Print('K'), Event::Print('!'),]
    );
}

#[test]
fn enormous_parameter_stream_remains_bounded_and_recovers() {
    let mut parser = Parser::default();
    let mut recorder = Recorder::default();
    parser.advance(&mut recorder, b"\x1b[");
    let chunk = [b';'; 1000];
    for _ in 0..1000 {
        parser.advance(&mut recorder, &chunk);
    }
    assert!(recorder.events.is_empty());
    assert_eq!(parser.param_len, MAX_PARAMS);
    assert_eq!(parser.string.capacity(), 0);
    parser.advance(&mut recorder, b"\nm\x1b[2J");
    assert_eq!(
        recorder.events,
        vec![Event::Csi(vec![vec![Some(2)]], None, vec![], b'J')]
    );
}

#[test]
fn malformed_parameter_order_discards_sequence() {
    for bytes in [
        b"\x1b[1?m!".as_slice(),
        b"\x1b[??1m!",
        b"\x1b[1 2m!",
        b"\x1b[1<2m!",
        b"\x1b[1\xffm!",
    ] {
        assert_eq!(parse(bytes), vec![Event::Print('!')], "{bytes:?}");
    }
}

#[test]
fn strings_only_dispatch_when_their_terminator_arrives() {
    let mut parser = Parser::default();
    let mut recorder = Recorder::default();
    parser.advance(&mut recorder, b"\x1b]2;title\x1b");
    assert!(recorder.events.is_empty());
    parser.advance(&mut recorder, b"\\\x1bPq\x07x\x1b\\\x1b_G\x07x\x1b\\");
    assert_eq!(
        recorder.events,
        vec![
            Event::Osc(b"2;title".to_vec()),
            Event::Dcs(b"q\x07x".to_vec()),
            Event::Apc(b"G\x07x".to_vec()),
        ]
    );
}

#[test]
fn osc_preserves_unicode_bytes_and_ignores_embedded_c0() {
    assert_eq!(
        parse("\x1b]2;\u{9c}标题\n\t\x7f!\x07".as_bytes()),
        vec![Event::Osc("2;\u{9c}标题!".as_bytes().to_vec()),]
    );
}

#[test]
fn unsupported_sos_and_privacy_messages_never_leak_text() {
    assert_eq!(
        parse(b"a\x1bXhidden\x07still hidden\x1b\\b\x1b^hidden\x1b\\c"),
        vec![Event::Print('a'), Event::Print('b'), Event::Print('c'),]
    );
}

#[test]
fn string_size_limit_is_exact_and_recovery_is_bounded() {
    for introducer in *b"]P_" {
        for terminator in [b"\x1b\\".as_slice(), b"\x07"] {
            if introducer != b']' && terminator == b"\x07" {
                continue;
            }
            for size in [MAX_STRING_BYTES, MAX_STRING_BYTES + 1] {
                let mut parser = Parser::default();
                let mut recorder = Recorder::default();
                parser.advance(&mut recorder, &[0x1b, introducer]);
                for chunk in vec![b'x'; size].chunks(3000) {
                    parser.advance(&mut recorder, chunk);
                    assert!(parser.string.len() <= MAX_STRING_BYTES);
                    assert!(parser.string.capacity() <= MAX_STRING_BYTES);
                }
                assert!(recorder.events.is_empty());
                parser.advance(&mut recorder, terminator);
                if size == MAX_STRING_BYTES {
                    assert_eq!(recorder.events.len(), 1);
                    match &recorder.events[0] {
                        Event::Osc(bytes) | Event::Dcs(bytes) | Event::Apc(bytes) => {
                            assert_eq!(bytes.len(), size);
                        }
                        _ => panic!("expected a string event"),
                    }
                } else {
                    assert!(recorder.events.is_empty());
                }
                recorder.events.clear();
                parser.advance(&mut recorder, b"OK\x1b]2;next\x07");
                assert_eq!(
                    recorder.events,
                    vec![
                        Event::Print('O'),
                        Event::Print('K'),
                        Event::Osc(b"2;next".to_vec()),
                    ]
                );
            }
        }
    }
}

#[test]
fn ascii_runs_are_batched_and_strings_reuse_allocation() {
    let mut parser = Parser::default();
    let mut recorder = Recorder::default();
    parser.advance(
        &mut recorder,
        b"a continuous printable ASCII run\nsecond run",
    );
    assert_eq!(recorder.ascii_runs, 2);
    assert_eq!(parser.string.capacity(), 0);
    parser.advance(&mut recorder, b"\x1b]2;first title\x07");
    let capacity = parser.string.capacity();
    assert!(capacity > 0);
    let allocation = parser.string.as_ptr();
    parser.advance(&mut recorder, b"\x1b]2;next title\x07");
    assert_eq!(parser.string.as_ptr(), allocation);
    assert_eq!(parser.string.capacity(), capacity);
}

#[test]
fn oversized_strings_discard_embedded_escapes_until_termination() {
    for introducer in *b"]P_" {
        let mut parser = Parser::default();
        let mut recorder = Recorder::default();
        parser.advance(&mut recorder, &[0x1b, introducer]);
        parser.advance(&mut recorder, &vec![b'x'; MAX_STRING_BYTES + 1]);
        parser.advance(
            &mut recorder,
            b"\x1b]hidden\x1bP\x1b\nmore hidden\x1b\x1b\\OK",
        );
        assert_eq!(recorder.events, vec![Event::Print('O'), Event::Print('K')]);
    }
}

#[test]
fn configured_apc_limit_does_not_expand_osc_or_dcs_limits() {
    for introducer in *b"]P_" {
        let mut parser = Parser::with_apc_limit(MAX_STRING_BYTES * 2);
        let mut recorder = Recorder::default();
        parser.advance(&mut recorder, &[0x1b, introducer, b'G']);
        parser.advance(&mut recorder, &vec![b'x'; MAX_STRING_BYTES]);
        parser.advance(&mut recorder, b"\x1b\\");
        if introducer == b'_' {
            assert!(
                matches!(recorder.events.as_slice(), [Event::Apc(bytes)] if bytes.len() == MAX_STRING_BYTES + 1)
            );
        } else {
            assert!(recorder.events.is_empty());
        }
    }
    assert_eq!(Parser::with_apc_limit(usize::MAX).apc_limit, MAX_APC_BYTES);
}

#[test]
fn clipboard_osc_has_its_own_larger_limit() {
    let data = vec![b'Q'; MAX_STRING_BYTES * 2];
    for scanner in [false, true] {
        let mut parser = if scanner {
            Parser::scanner(MAX_STRING_BYTES)
        } else {
            Parser::default()
        };
        let mut recorder = Recorder::default();
        // The selecting prefix may arrive split across reads.
        parser.advance(&mut recorder, b"\x1b]5");
        parser.advance(&mut recorder, b"2;c;");
        for chunk in data.chunks(3000) {
            parser.advance(&mut recorder, chunk);
        }
        assert!(!parser.discarded);
        parser.advance(&mut recorder, b"\x07");
        if !scanner {
            assert!(
                matches!(recorder.events.as_slice(), [Event::Osc(bytes)] if bytes.len() == data.len() + 5)
            );
            assert_eq!(parser.string.capacity(), 0);
        }
    }

    let mut parser = Parser::default();
    let mut recorder = Recorder::default();
    parser.advance(&mut recorder, b"\x1b]52;c;");
    parser.advance(&mut recorder, &vec![b'Q'; MAX_CLIPBOARD_OSC_BYTES]);
    assert!(parser.discarded);
    parser.advance(&mut recorder, b"\x07OK");
    assert_eq!(printed(&recorder.events), "OK");

    // Other OSCs keep the generic bound.
    let mut parser = Parser::default();
    parser.advance(&mut recorder, b"\x1b]2;");
    parser.advance(&mut recorder, &data);
    assert!(parser.discarded);
}

/// Feed `ESC ] head`, split after `split` bytes, then `body` bytes and BEL;
/// the OSC it delivers, if any.
fn osc_with_body(parser: &mut Parser, head: &[u8], split: usize, body: usize) -> Option<usize> {
    let mut recorder = Recorder::default();
    parser.advance(&mut recorder, b"\x1b]");
    parser.advance(&mut recorder, &head[..split]);
    parser.advance(&mut recorder, &head[split..]);
    for chunk in vec![b'A'; body].chunks(3000) {
        parser.advance(&mut recorder, chunk);
    }
    parser.advance(&mut recorder, b"\x07OK");
    assert!(printed(&recorder.events).ends_with("OK"));
    recorder.events.iter().find_map(|event| match event {
        Event::Osc(bytes) => {
            assert!(bytes.starts_with(head));
            Some(bytes.len())
        }
        _ => None,
    })
}

#[test]
fn only_osc_1337_file_transfers_use_the_apc_limit() {
    let body = MAX_STRING_BYTES * 2;
    let cases: [(&[u8], bool); 9] = [
        (b"1337;File=inline=1:", true),
        (b"1337;FilePart=", true),
        (b"1337;MultipartFile=inline=1", true),
        // Shell integration marks and other OSC 1337 commands keep the
        // generic limit, though they share the first bytes.
        (b"133;A;", false),
        (b"133;D;0;", false),
        (b"1338;", false),
        (b"1337;SetUserVar=name=", false),
        (b"1337;FileEnd", false),
        (b"1337;Fil", false),
    ];
    for (head, large) in cases {
        for split in 0..=head.len() {
            let mut parser = Parser::with_apc_limit(MAX_APC_BYTES);
            let delivered = osc_with_body(&mut parser, head, split, body);
            assert_eq!(
                delivered,
                large.then_some(head.len() + body),
                "{:?} split at {split}",
                String::from_utf8_lossy(head)
            );
            // Within the generic limit every one is delivered.
            let small = MAX_STRING_BYTES - head.len();
            let delivered = osc_with_body(&mut parser, head, split, small);
            assert_eq!(delivered, Some(MAX_STRING_BYTES));
        }
    }

    // File transfers are still bounded by the APC limit.
    let mut parser = Parser::with_apc_limit(MAX_STRING_BYTES * 2);
    assert_eq!(
        osc_with_body(&mut parser, b"1337;File=:", 4, MAX_STRING_BYTES * 2),
        None
    );
    assert_eq!(
        osc_with_body(&mut parser, b"1337;File=:", 4, MAX_STRING_BYTES),
        Some(MAX_STRING_BYTES + 11)
    );
}

#[test]
fn a_configured_string_limit_bounds_osc_133_but_not_file_transfers() {
    let mut parser = Parser::with_apc_limit(MAX_APC_BYTES);
    parser.set_string_limits(16, 16);
    // A stale prefix from an earlier string selects nothing.
    assert_eq!(
        osc_with_body(&mut parser, b"1337;File=:", 0, 100),
        Some(111)
    );
    for split in 0..=6 {
        assert_eq!(osc_with_body(&mut parser, b"133;A;", split, 100), None);
        assert_eq!(osc_with_body(&mut parser, b"133;A;", split, 10), Some(16));
    }
    // A prefix of a file transfer that ends early is held to the generic
    // limit, though it was provisionally allowed more.
    let mut parser = Parser::with_apc_limit(MAX_APC_BYTES);
    parser.set_string_limits(4, 4);
    assert_eq!(osc_with_body(&mut parser, b"1337;Fil", 0, 0), None);
    for split in 0..=10 {
        assert_eq!(
            osc_with_body(&mut parser, b"1337;File=", split, 100),
            Some(110)
        );
    }
}

#[test]
fn scanner_tracks_file_transfer_limits_without_allocating_payloads() {
    for (head, discarded) in [(b"1337;File=:".as_slice(), false), (b"133;A;", true)] {
        let mut parser = Parser::scanner(MAX_APC_BYTES);
        let mut recorder = Recorder::default();
        parser.advance(&mut recorder, b"\x1b]");
        parser.advance(&mut recorder, &head[..3]);
        parser.advance(&mut recorder, &head[3..]);
        parser.advance(&mut recorder, &vec![b'A'; MAX_STRING_BYTES * 2]);
        assert_eq!(parser.discarded, discarded);
        assert_eq!(parser.string.capacity(), 0);
    }
}

#[test]
fn large_apc_storage_is_released_after_completion_or_interruption() {
    for ending in [b"\x1b\\".as_slice(), b"\x18", b"\x1a", b"\x1b[2J", b"\x1bc"] {
        let mut parser = Parser::with_apc_limit(MAX_STRING_BYTES * 2);
        let mut recorder = Recorder::default();
        parser.advance(&mut recorder, b"\x1b_G");
        parser.advance(&mut recorder, &vec![b'x'; MAX_STRING_BYTES]);
        assert!(parser.string.capacity() > MAX_STRING_BYTES);
        parser.advance(&mut recorder, ending);
        assert_eq!(parser.string.capacity(), 0, "ending {ending:?}");
        parser.advance(&mut recorder, b"OK");
        assert_eq!(printed(&recorder.events), "OK");
    }
}

#[test]
fn large_apc_storage_is_released_on_overflow_and_parser_reset() {
    for overflow in [false, true] {
        let mut parser = Parser::with_apc_limit(MAX_STRING_BYTES * 2);
        let mut recorder = Recorder::default();
        parser.advance(&mut recorder, b"\x1b_G");
        parser.advance(&mut recorder, &vec![b'x'; MAX_STRING_BYTES]);
        assert!(parser.string.capacity() > MAX_STRING_BYTES);
        if overflow {
            parser.advance(&mut recorder, &vec![b'x'; MAX_STRING_BYTES]);
            assert!(parser.discarded);
            assert_eq!(parser.string.capacity(), 0);
            parser.advance(&mut recorder, b"\x1b\\");
        } else {
            parser.reset();
        }
        assert_eq!(parser.string.capacity(), 0);
        parser.advance(&mut recorder, b"OK");
        assert_eq!(recorder.events, vec![Event::Print('O'), Event::Print('K')]);
    }
}

#[test]
fn scanner_tracks_large_string_limits_without_allocating_payloads() {
    for introducer in *b"]P_" {
        let mut parser = Parser::scanner(MAX_STRING_BYTES * 2);
        let mut recorder = Recorder::default();
        parser.advance(&mut recorder, &[0x1b, introducer, b'G']);
        parser.advance(&mut recorder, &vec![b'x'; MAX_STRING_BYTES]);
        assert_eq!(parser.string.capacity(), 0);
        // The APC fits the larger limit; oversized OSC/DCS recover at CSI.
        parser.advance(&mut recorder, b"\x1b[?2026lEND");
        assert_eq!(printed(&recorder.events), "END");
    }
}

#[test]
fn only_graphics_apcs_use_the_larger_apc_limit() {
    for (introducer, delivered) in [(b'G', true), (b'Z', false)] {
        let mut parser = Parser::with_apc_limit(MAX_APC_BYTES);
        let mut recorder = Recorder::default();
        parser.advance(&mut recorder, &[0x1b, b'_', introducer]);
        parser.advance(&mut recorder, &vec![b'x'; MAX_STRING_BYTES]);
        assert_eq!(parser.string.capacity() > MAX_STRING_BYTES, delivered);
        parser.advance(&mut recorder, b"\x1b\\");
        assert_eq!(
            matches!(recorder.events.as_slice(), [Event::Apc(bytes)] if bytes.len() == MAX_STRING_BYTES + 1),
            delivered
        );
    }
}

#[test]
fn escape_followed_by_non_ascii_text_prints_the_text() {
    for input in ["A\x1bé!BC", "A\x1b(é!BC"] {
        assert_eq!(printed(&parse(input.as_bytes())), "Aé!BC", "{input:?}");
        // Fragmented input reaches the same state.
        let mut parser = Parser::default();
        let mut recorder = Recorder::default();
        for byte in input.as_bytes() {
            parser.advance(&mut recorder, std::slice::from_ref(byte));
        }
        assert_eq!(printed(&recorder.events), "Aé!BC", "{input:?}");
    }
}

#[test]
fn csi_recovers_from_an_oversized_string() {
    for introducer in *b"]P_" {
        let mut parser = Parser::default();
        let mut recorder = Recorder::default();
        parser.advance(&mut recorder, &[0x1b, introducer]);
        parser.advance(&mut recorder, &vec![b'x'; MAX_STRING_BYTES + 1]);
        parser.advance(&mut recorder, b"hidden\x1b[mOK");
        assert_eq!(recorder.events[1..], [Event::Print('O'), Event::Print('K')]);
        assert!(matches!(recorder.events[0], Event::Csi(_, None, _, b'm')));
    }
}

#[test]
fn full_reset_recovers_from_an_oversized_string() {
    for introducer in *b"]P_" {
        let mut parser = Parser::default();
        let mut recorder = Recorder::default();
        parser.advance(&mut recorder, &[0x1b, introducer]);
        parser.advance(&mut recorder, &vec![b'x'; MAX_STRING_BYTES + 1]);
        parser.advance(&mut recorder, b"\x1b]hidden\x1bcOK");
        assert_eq!(
            recorder.events,
            vec![
                Event::Escape(Vec::new(), b'c'),
                Event::Print('O'),
                Event::Print('K')
            ]
        );
    }
}

#[test]
fn printable_ascii_scan_stops_at_every_other_byte_in_every_position() {
    for byte in 0..=u8::MAX {
        let printable = (0x20..=0x7e).contains(&byte);
        for len in 0..=40 {
            for position in 0..len {
                let mut bytes = vec![b'a'; len];
                bytes[position] = byte;
                let expected = if printable { len } else { position };
                assert_eq!(
                    printable_ascii_len(&bytes),
                    expected,
                    "byte {byte:#04x} at {position} of {len}"
                );
            }
        }
    }
}

// The text fast path decodes with `complete_utf8`, so it must accept exactly
// the scalars `str::from_utf8` accepts as a prefix, for every lead byte.
#[test]
fn complete_utf8_matches_std_for_every_two_and_three_byte_sequence() {
    let expect = |bytes: &[u8]| {
        let valid = match std::str::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap(),
        };
        valid
            .chars()
            .next()
            .filter(|character| !character.is_ascii())
            .map(|character| (character, character.len_utf8()))
    };
    for first in 0x80..=0xffu8 {
        for second in 0..=0xffu8 {
            assert_eq!(complete_utf8(&[first, second]), expect(&[first, second]));
            for third in 0..=0xffu8 {
                let bytes = [first, second, third];
                assert_eq!(complete_utf8(&bytes), expect(&bytes), "{bytes:02x?}");
            }
        }
    }
    for first in 0xf0..=0xffu8 {
        for second in 0..=0xffu8 {
            for fourth in [0x00, 0x7f, 0x80, 0xbf, 0xc0] {
                for third in [0x00, 0x7f, 0x80, 0xa5, 0xbf, 0xc0] {
                    let bytes = [first, second, third, fourth];
                    assert_eq!(complete_utf8(&bytes), expect(&bytes), "{bytes:02x?}");
                }
            }
        }
    }
}

// Line endings execute their line feed together with the carriage return
// when both are in one chunk; parsed whole or a byte at a time, text with
// line endings, lone controls and escapes yields the same events.
#[test]
fn line_endings_parse_alike_whole_and_bytewise() {
    let pieces: [&[u8]; 8] = [
        b"\r\n",
        b"\r",
        b"\n",
        b"ab",
        b"\x1b[2J",
        b"\t",
        "é".as_bytes(),
        b"\r\r\n",
    ];
    let mut seed = 0x9e37_79b9_u32;
    let mut next = |bound: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as usize % bound
    };
    for _ in 0..500 {
        let mut bytes = Vec::new();
        for _ in 0..next(12) {
            bytes.extend_from_slice(pieces[next(pieces.len())]);
        }
        let mut bytewise = Recorder::default();
        let mut parser = Parser::default();
        for byte in &bytes {
            parser.advance(&mut bytewise, std::slice::from_ref(byte));
        }
        assert_eq!(parse(&bytes), bytewise.events, "{bytes:02x?}");
    }
}

#[test]
fn utf8_encoded_c1_controls_act_as_their_seven_bit_forms() {
    // U+009B is CSI, U+009D OSC (ended by BEL), U+0084 IND and U+0090 DCS.
    let stream = "a\u{9b}31mb\u{9d}0;title\x07\u{84}\u{90}q\x1b\\c".as_bytes();
    let expected = vec![
        Event::Print('a'),
        Event::Csi(vec![vec![Some(31)]], None, vec![], b'm'),
        Event::Print('b'),
        Event::Osc(b"0;title".to_vec()),
        Event::Escape(vec![], b'D'),
        Event::Dcs(b"q".to_vec()),
        Event::Print('c'),
    ];
    assert_eq!(parse(stream), expected);
    // The same, one byte at a time through the streaming decoder.
    let mut recorder = Recorder::default();
    let mut parser = Parser::default();
    for byte in stream {
        parser.advance(&mut recorder, std::slice::from_ref(byte));
    }
    assert_eq!(recorder.events, expected);
    // Raw 8-bit C1 bytes are not UTF-8.
    assert_eq!(
        parse(b"\x9b31m"),
        vec![
            Event::Print(char::REPLACEMENT_CHARACTER),
            Event::Print('3'),
            Event::Print('1'),
            Event::Print('m'),
        ]
    );
}
