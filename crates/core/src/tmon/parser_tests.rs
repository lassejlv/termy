use super::*;
use crate::tmon::grid::{Cell, Color, CursorStyle};

#[derive(Debug, PartialEq, Eq)]
struct ReplayResult {
    snapshot: crate::tmon::Snapshot,
    lines: Vec<Vec<crate::tmon::Cell>>,
    palette: crate::tmon::Palette,
    bracketed_paste: bool,
    alternate_screen: bool,
    mouse: crate::tmon::MouseMode,
    keyboard: crate::tmon::KeyboardMode,
    graphics: Vec<(u32, u32, i32, usize, u32, u32)>,
    events: Vec<ParsedEvent>,
    replies: Vec<u8>,
    synchronized_update_active: bool,
}

fn parse(bytes: &[u8]) -> (Grid, ParseOutput) {
    let mut grid = Grid::new(12, 3, 8, CursorStyle::Block);
    grid.set_cell_metrics(9.0, 18.0);
    let mut parser = Parser::default();
    let output = parser.advance(&mut grid, bytes);
    (grid, output)
}

fn replay_chunks(bytes: &[u8], chunks: &[usize]) -> ReplayResult {
    let mut grid = Grid::new(12, 4, 8, CursorStyle::Block);
    grid.set_cell_metrics(9.0, 18.0);
    let mut parser = Parser::default();
    let mut events = Vec::new();
    let mut replies = Vec::new();
    let mut synchronized_update_active = false;
    let mut start = 0usize;
    for &end in chunks {
        let output = parser.advance(&mut grid, &bytes[start..end]);
        events.extend(output.events);
        replies.extend(output.replies);
        synchronized_update_active = output.synchronized_update_active;
        start = end;
    }
    if start < bytes.len() || chunks.is_empty() {
        let output = parser.advance(&mut grid, &bytes[start..]);
        events.extend(output.events);
        replies.extend(output.replies);
        synchronized_update_active = output.synchronized_update_active;
    }
    let (first, last) = grid.line_bounds();
    let lines = (first..=last)
        .map(|line| grid.line(line).unwrap().to_vec())
        .collect();
    let graphics = parser
        .graphics_placements(&grid)
        .into_iter()
        .map(|placement| {
            (
                placement.image_id,
                placement.placement_id,
                placement.viewport_row,
                placement.col,
                placement.occupied_cols,
                placement.occupied_rows,
            )
        })
        .collect();
    ReplayResult {
        snapshot: grid.snapshot(),
        lines,
        palette: grid.palette(),
        bracketed_paste: grid.bracketed_paste_mode(),
        alternate_screen: grid.alternate_screen_mode(),
        mouse: grid.mouse_mode(),
        keyboard: grid.keyboard_mode(),
        graphics,
        events,
        replies,
        synchronized_update_active,
    }
}

#[test]
fn parses_text_cursor_motion_and_sgr() {
    let (grid, _) = parse(b"abc\x1b[2D\x1b[31;1mX");
    let row = grid.line(0).unwrap();
    assert_eq!(row[0].character, 'a');
    assert_eq!(row[1].character, 'X');
    assert_eq!(row[1].foreground, Color::Indexed(1));
    assert!(row[1].attributes.bold());
}

#[test]
fn bulk_plain_lines_match_row_split_parsing_and_repeat_state() {
    let mut bytes = Vec::new();
    let mut row_splits = Vec::new();
    for row in 0..100 {
        if row == 96 {
            bytes.extend_from_slice(
                b"\x1b_Ga=T,f=32,s=1,v=1,i=77,p=3,c=1,r=1,C=1,q=1;/wAA/w==\x1b\\",
            );
            row_splits.push(bytes.len());
        }
        bytes.extend_from_slice(format!("L{row:03}\r\n").as_bytes());
        row_splits.push(bytes.len());
    }
    bytes.extend_from_slice(b"\x1b[2b");

    let bulk = replay_chunks(&bytes, &[]);
    let row_split = replay_chunks(&bytes, &row_splits);
    let mid_row_split = replay_chunks(&bytes, &[257]);

    assert_eq!(bulk, row_split);
    assert_eq!(bulk, mid_row_split);
}

#[test]
fn bulk_alternate_lines_match_byte_split_parsing() {
    let mut bytes = b"\x1b[?1049h".to_vec();
    for row in 0..100 {
        bytes.extend_from_slice(format!("A{row:03} cafe\u{301} \u{754c}\r\n").as_bytes());
    }
    bytes.extend_from_slice(b"\x1b[2b");

    let bulk = replay_chunks(&bytes, &[]);
    let every_byte = (1..bytes.len()).collect::<Vec<_>>();

    assert!(bulk.alternate_screen);
    assert_eq!(bulk, replay_chunks(&bytes, &every_byte));
}

#[test]
fn complete_default_unicode_lines_match_byte_split_parsing() {
    let mut bytes = Vec::new();
    for row in 0..20 {
        bytes.extend_from_slice(
            format!("U{row:02} cafe\u{301} Ελληνικά 日本語 한글 🙂界\r\n").as_bytes(),
        );
    }
    bytes.extend_from_slice("fallback e\u{301}\u{302}界\r\n\x1b[2b".as_bytes());

    let one_shot = replay_chunks(&bytes, &[]);
    let every_byte = (1..bytes.len()).collect::<Vec<_>>();

    assert_eq!(one_shot, replay_chunks(&bytes, &every_byte));
}

#[test]
fn single_default_line_matches_byte_split_parsing_on_both_screens() {
    for prefix in [b"".as_slice(), b"\x1b[?1049h".as_slice()] {
        let mut fixture = prefix.to_vec();
        fixture.extend_from_slice("ASCII cafe\u{301} \u{754c}\r\n".as_bytes());
        let expected = replay_chunks(&fixture, &[]);
        let every_byte = (1..fixture.len()).collect::<Vec<_>>();

        assert_eq!(replay_chunks(&fixture, &every_byte), expected);
    }
}

#[test]
fn parses_colon_form_truecolor_palette_and_underline_styles() {
    let (grid, _) = parse(b"\x1b[38:2::12:34:56;48:5:200;4:3mX");
    let cell = grid.line(0).unwrap()[0];
    assert_eq!(
        cell.foreground,
        Color::Rgb {
            r: 12,
            g: 34,
            b: 56
        }
    );
    assert_eq!(cell.background, Color::Indexed(200));
    assert_eq!(cell.attributes.underline_style(), UnderlineStyle::Curly);
    assert!(
        !cell.attributes.dim(),
        "underline style must not leak into SGR 2"
    );
}

#[test]
fn preserves_extended_underline_styles_and_vte_fallbacks() {
    let (grid, _) =
        parse(b"\x1b[4mA\x1b[4:2mB\x1b[4:3mC\x1b[4:4mD\x1b[4:5mE\x1b[4:0mF\x1b[4:0:1mG\x1b[24mH");
    let styles = grid
        .line(0)
        .unwrap()
        .iter()
        .take(8)
        .map(|cell| cell.attributes.underline_style())
        .collect::<Vec<_>>();
    assert_eq!(
        styles,
        [
            UnderlineStyle::Single,
            UnderlineStyle::Double,
            UnderlineStyle::Curly,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
            UnderlineStyle::None,
            UnderlineStyle::Single,
            UnderlineStyle::None,
        ]
    );
}

#[test]
fn preserves_extended_underline_colors_and_resets() {
    let (grid, _) =
        parse(b"\x1b[58;2;1;2;3mA\x1b[58;5;201mB\x1b[58:2::4:5:6mC\x1b[59mD\x1b[58:5:7mE\x1b[0mF");
    let colors = grid
        .line(0)
        .unwrap()
        .iter()
        .take(6)
        .map(|cell| cell.underline_color)
        .collect::<Vec<_>>();
    assert_eq!(
        colors,
        [
            Some(Color::Rgb { r: 1, g: 2, b: 3 }),
            Some(Color::Indexed(201)),
            Some(Color::Rgb { r: 4, g: 5, b: 6 }),
            None,
            Some(Color::Indexed(7)),
            None,
        ]
    );
}

#[test]
fn unsupported_sgr_subparameter_groups_are_ignored() {
    let (grid, _) = parse(b"\x1b[1:2mX");
    let cell = grid.line(0).unwrap()[0];

    assert!(!cell.attributes.bold());
    assert!(!cell.attributes.dim());
}

#[test]
fn overflowing_csi_parameters_discards_the_sequence() {
    let mut bytes = b"\x1b[".to_vec();
    for _ in 0..MAX_CSI_PARAMS {
        bytes.extend_from_slice(b"31;");
    }
    bytes.extend_from_slice(b"31mX");

    let (grid, _) = parse(&bytes);
    let cell = grid.line(0).unwrap()[0];
    assert_eq!(cell.character, 'X');
    assert_eq!(cell.foreground, Color::Default);
}

#[test]
fn csi_subparameters_do_not_shift_later_parameters() {
    let (grid, _) = parse(b"\x1b[2:9;4HX");

    assert_eq!(grid.line(1).unwrap()[3].character, 'X');
    assert_eq!(grid.line(1).unwrap()[8].character, ' ');
}

#[test]
fn keeps_utf8_decoder_state_across_reads() {
    let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
    let mut parser = Parser::default();
    parser.advance(&mut grid, &[0xe2, 0x9d]);
    parser.advance(&mut grid, &[0xaf]);
    assert_eq!(grid.line(0).unwrap()[0].character, '❯');
}

#[test]
fn encoded_c1_controls_are_ignored_without_replacing_the_last_printed_character() {
    let (grid, _) = parse("A\u{85}\u{9b}\x1b[2bB".as_bytes());
    let row = grid.line(0).unwrap();

    assert_eq!(
        row.iter()
            .take(4)
            .map(|cell| cell.character)
            .collect::<String>(),
        "AAAB"
    );
    let mut first_combining = None;
    assert!(grid.for_each_line_cell(0, |col, _, combining| {
        if col == 0 {
            first_combining = combining.map(|combining| combining.to_owned_string());
        }
    }));
    assert_eq!(first_combining, None);
}

#[test]
fn ground_text_spans_preserve_invalid_and_split_utf8_behavior() {
    let fixture = b"A\xe2\x28\xa1B\xc0\xafC\xf0\x9f\x99\x82D\xf0\x9f";
    let expected = replay_chunks(fixture, &[]);
    let every_byte = (1..fixture.len()).collect::<Vec<_>>();
    assert_eq!(replay_chunks(fixture, &every_byte), expected);

    let (grid, _) = parse(fixture);
    let rendered = grid
        .line(0)
        .unwrap()
        .iter()
        .take(11)
        .filter(|cell| !cell.wide_spacer())
        .map(|cell| cell.character)
        .collect::<String>();
    assert_eq!(rendered, "A�(�B��C🙂D");
}

#[test]
fn ground_text_spans_consume_long_malformed_runs_in_one_pass() {
    let mut parser = Parser::default();
    let mut grid = Grid::new(80, 24, 0, CursorStyle::Block);
    let mut output = ParseOutput::default();
    let malformed = vec![0x80; 16 * 1024];

    assert_eq!(
        parser.advance_ground_text(&mut grid, &malformed, &mut output),
        malformed.len()
    );
    assert!(!parser.utf8.is_pending());
}

#[test]
fn long_ascii_spans_in_slow_grid_modes_match_chunked_parsing() {
    for prefix in [b"\x1b[4h".as_slice(), b"\x1b[?7l".as_slice()] {
        let mut fixture = prefix.to_vec();
        fixture.extend(std::iter::repeat_n(b'a', 8 * 1024));
        let chunks = (257..fixture.len()).step_by(257).collect::<Vec<_>>();

        assert_eq!(
            replay_chunks(&fixture, &chunks),
            replay_chunks(&fixture, &[])
        );
    }
}

#[test]
fn osc_ignores_embedded_controls_and_dispatches_before_cancel() {
    let (grid, output) = parse(b"\x1b]2;ti\x01tle\x18X");
    assert_eq!(output.events, vec![ParsedEvent::Title("title".to_string())]);
    assert_eq!(grid.line(0).unwrap()[0].character, 'X');
}

#[test]
fn osc52_defaults_an_empty_selector_to_the_clipboard() {
    let (_, output) = parse(b"\x1b]52;;dG1vbg==\x07");
    assert_eq!(
        output.events,
        vec![ParsedEvent::ClipboardStore("tmon".to_string())]
    );
}

#[test]
fn every_input_split_matches_one_shot_parsing() {
    let mut fixture = "ab界".as_bytes().to_vec();
    fixture.extend_from_slice(
        b"\x1b[31;1mR\x1b[0m\x1b]2;split-safe\x1b\\\
          \x1b]4;3;#123456\x07\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\\
          \r\nline-2\r\nline-3\r\nline-4\r\nline-5\
          \x1b[2;2H\x1b_Ga=T,f=32,s=1,v=1,i=77,p=3,c=2,r=1,C=1,q=1;/wAA/w==\x1b\\Z\
          \x1bP+q544e;5463\x1b\\\x1b[?2004h\x1b[?1006h\
          \x1b[?2026hbatched\x1b[?2026l\x1b[6n",
    );
    let expected = replay_chunks(&fixture, &[]);
    for split in 0..=fixture.len() {
        assert_eq!(
            replay_chunks(&fixture, &[split]),
            expected,
            "parser state diverged at byte split {split}"
        );
    }
}

#[test]
fn complete_csi_batches_match_byte_split_parsing() {
    let fixture = b"\x1b[2;3H\x1b[31;1mred\x1b[0m\x1b[2K\
        \x1b[3A\x1b[4:3;58:2::12:34:56mU\x1b[?25lX\x1b[?25h\x1b[6n";
    let expected = replay_chunks(fixture, &[]);
    let every_byte = (1..fixture.len()).collect::<Vec<_>>();

    assert_eq!(replay_chunks(fixture, &every_byte), expected);
}

#[test]
fn kitty_apc_terminator_is_chunk_safe() {
    let mut grid = Grid::new(12, 4, 8, CursorStyle::Block);
    let mut parser = Parser::default();

    let pending = parser.advance(
        &mut grid,
        b"\x1b_Ga=T,f=32,s=1,v=1,i=77,p=3,c=2,r=1,C=1;/wAA/w==\x1b",
    );
    assert!(pending.replies.is_empty());
    assert!(parser.graphics_placements(&grid).is_empty());
    assert_eq!(parser.state, State::KittyEscape);

    let finished = parser.advance(&mut grid, b"\\");
    assert_eq!(finished.replies, b"\x1b_Gi=77,p=3;OK\x1b\\");
    assert_eq!(parser.graphics_placements(&grid).len(), 1);
    assert_eq!(parser.state, State::Ground);
    assert!(!parser.kitty_payload_started);
}

#[test]
fn kitty_apc_caps_unterminated_control_across_chunks() {
    let mut grid = Grid::new(12, 4, 8, CursorStyle::Block);
    let mut parser = Parser::default();
    let mut control = b"i=79,".to_vec();
    control.resize(MAX_CONTROL_BYTES, b'x');
    let mut first_chunk = b"\x1b_G".to_vec();
    first_chunk.extend_from_slice(&control);

    assert!(parser.advance(&mut grid, &first_chunk).replies.is_empty());
    assert_eq!(parser.state, State::Kitty);
    assert_eq!(parser.kitty.len(), MAX_CONTROL_BYTES);
    assert!(!parser.kitty_oversized);
    assert!(!parser.kitty_payload_started);

    assert!(parser.advance(&mut grid, b"x").replies.is_empty());
    assert!(parser.kitty_oversized);
    assert_eq!(parser.kitty.len(), MAX_CONTROL_BYTES);
    assert!(
        parser
            .advance(&mut grid, &vec![b'y'; 8 * 1024])
            .replies
            .is_empty()
    );
    assert_eq!(parser.kitty.len(), MAX_CONTROL_BYTES);

    let finished = parser.advance(&mut grid, b"\x1b\\");
    assert_eq!(
        finished.replies,
        b"\x1b_Gi=79;EFBIG:image command exceeds storage limit\x1b\\"
    );
    assert_eq!(parser.state, State::Ground);
    assert!(!parser.kitty_oversized);
    assert!(!parser.kitty_payload_started);
}

#[test]
fn kitty_apc_accepts_separator_at_the_control_limit() {
    let mut grid = Grid::new(12, 4, 8, CursorStyle::Block);
    let mut parser = Parser::default();
    let mut control = b"a=T,f=32,s=1,v=1,i=80,C=1,".to_vec();
    control.resize(MAX_CONTROL_BYTES, b'x');
    let mut first_chunk = b"\x1b_G".to_vec();
    first_chunk.extend_from_slice(&control);

    parser.advance(&mut grid, &first_chunk);
    parser.advance(&mut grid, b";");
    assert!(!parser.kitty_oversized);
    assert!(parser.kitty_payload_started);
    assert_eq!(parser.kitty.len(), MAX_CONTROL_BYTES + 1);

    let finished = parser.advance(&mut grid, b"/wAA/w==\x1b\\");
    assert_eq!(finished.replies, b"\x1b_Gi=80;OK\x1b\\");
    assert_eq!(parser.graphics_placements(&grid).len(), 1);
    assert!(!parser.kitty_payload_started);
}

#[test]
fn kitty_control_tracking_resets_on_cancel_start_and_hydration() {
    let mut grid = Grid::new(12, 4, 8, CursorStyle::Block);
    let mut parser = Parser::default();

    parser.advance(&mut grid, b"\x1b_Ga=t;AAAA");
    assert!(parser.kitty_payload_started);
    parser.advance(&mut grid, &[0x18]);
    assert_eq!(parser.state, State::Ground);
    assert!(parser.kitty.is_empty());
    assert!(!parser.kitty_oversized);
    assert!(!parser.kitty_payload_started);

    parser.kitty.extend_from_slice(b"stale");
    parser.kitty_oversized = true;
    parser.kitty_payload_started = true;
    parser.advance(&mut grid, b"\x1b_G");
    assert_eq!(parser.state, State::Kitty);
    assert!(parser.kitty.is_empty());
    assert!(!parser.kitty_oversized);
    assert!(!parser.kitty_payload_started);

    parser.push_kitty(b';');
    assert!(parser.kitty_payload_started);
    parser.reset_transient_state_after_hydration();
    assert_eq!(parser.state, State::Ground);
    assert!(parser.kitty.is_empty());
    assert!(!parser.kitty_oversized);
    assert!(!parser.kitty_payload_started);
}

#[test]
fn kitty_apc_accepts_the_native_engines_eight_bit_introducer() {
    let mut parser = Parser::default();
    let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
    let output = parser.advance(&mut grid, b"\x9fGa=T,f=32,s=1,v=1,i=7;/wAA/w==\x9cZ");

    assert_eq!(grid.line(1).unwrap()[1].character, 'Z');
    assert_eq!(parser.graphics_placements(&grid).len(), 1);
    assert!(output.replies.starts_with(b"\x1b_Gi=7;"));
}

#[test]
fn non_kitty_eight_bit_apc_is_forwarded_as_native_text() {
    let (grid, _) = parse(b"\x9fXpayload\x9cZ");
    let rendered = grid
        .line(0)
        .unwrap()
        .iter()
        .take(9)
        .map(|cell| cell.character)
        .collect::<String>();

    assert_eq!(rendered, "XpayloadZ");
}

#[test]
fn c1_st_does_not_end_osc_or_ignored_strings_in_utf8_mode() {
    let mut parser = Parser::default();
    let mut grid = Grid::new(12, 2, 0, CursorStyle::Block);

    let ignored = parser.advance(&mut grid, b"\x1b_Xignored\x9cZ");
    assert!(ignored.events.is_empty());
    assert_eq!(parser.state, State::IgnoreString);
    assert_eq!(grid.line(0).unwrap()[0].character, ' ');
    parser.advance(&mut grid, b"\x1b\\Q");
    assert_eq!(grid.line(0).unwrap()[0].character, 'Q');

    let mut parser = Parser::default();
    let mut grid = Grid::new(12, 2, 0, CursorStyle::Block);
    let osc = parser.advance(&mut grid, b"\x1b]2;A\x9cZ");
    assert!(osc.events.is_empty());
    assert_eq!(parser.state, State::Osc);
    assert_eq!(grid.line(0).unwrap()[0].character, ' ');
    parser.advance(&mut grid, b"\x07Q");
    assert_eq!(grid.line(0).unwrap()[0].character, 'Q');
}

#[test]
fn kitty_apc_preserves_non_terminating_escape_bytes_across_chunks() {
    let mut grid = Grid::new(12, 4, 8, CursorStyle::Block);
    let mut parser = Parser::default();

    let first = parser.advance(
        &mut grid,
        b"\x1b_Ga=T,f=32,s=1,v=1,i=78,C=1,q=1;/wAA/w==\x1b",
    );
    assert!(first.replies.is_empty());
    assert_eq!(parser.state, State::KittyEscape);

    let middle = parser.advance(&mut grid, b"x\x1b");
    assert!(middle.replies.is_empty());
    assert!(parser.graphics_placements(&grid).is_empty());
    assert_eq!(parser.state, State::KittyEscape);

    let finished = parser.advance(&mut grid, b"\\Z");
    assert_eq!(
        finished.replies,
        b"\x1b_Gi=78;EINVAL:invalid base64 payload\x1b\\"
    );
    assert!(parser.graphics_placements(&grid).is_empty());
    assert_eq!(grid.line(0).unwrap()[0].character, 'Z');
    assert_eq!(parser.state, State::Ground);
}

#[test]
fn malformed_random_streams_keep_grid_and_graphics_invariants() {
    let mut grid = Grid::new(31, 7, 16, CursorStyle::Block);
    let mut parser = Parser::default();
    let mut seed = 0x6a09_e667_f3bc_c909u64;
    let mut bytes = Vec::with_capacity(128 * 1024);
    for index in 0..128 * 1024 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        bytes.push(seed as u8);
        if index % 257 == 0 {
            bytes.extend_from_slice("界\x1b[31mR\x1b[0m".as_bytes());
        }
    }

    for chunk in bytes.chunks(113) {
        parser.advance(&mut grid, chunk);
        let (col, row) = grid.cursor_position();
        assert!(col < grid.cols());
        assert!(row < grid.rows());
        assert!(grid.history_size() <= 16);
        let (first, last) = grid.line_bounds();
        for line_index in first..=last {
            let line = grid.line(line_index).unwrap();
            assert_eq!(line.len(), grid.cols());
            for (index, cell) in line.iter().enumerate() {
                if cell.wide_spacer() {
                    assert!(index > 0);
                    assert!(!line.get(index - 1).unwrap().wide_spacer());
                }
            }
        }
        assert!(parser.graphics_placements(&grid).len() <= 4096);
    }
}

#[test]
fn parses_alt_screen_and_bracketed_paste_modes() {
    let (grid, _) = parse(b"main\x1b[?1049hX\x1b[?2004h");
    assert!(grid.alternate_screen_mode());
    assert!(grid.bracketed_paste_mode());
    assert_eq!(grid.line(0).unwrap()[4].character, 'X');
}

#[test]
fn emits_title_shell_and_clipboard_events() {
    let (_, output) = parse(b"\x1b]2;hello\x07\x1b]133;C\x07\x1b]52;c;dGVybXk=\x07");
    assert_eq!(
        output.events,
        vec![
            ParsedEvent::Title("hello".to_string()),
            ParsedEvent::ShellCommandExecuting,
            ParsedEvent::ClipboardStore("termy".to_string()),
        ]
    );
}

#[test]
fn custom_osc_events_preserve_semicolons_and_reject_malformed_progress() {
    let (_, output) = parse(
        b"\x1b]7;file://host/tmp/a%20b;c\x07\
          \x1b]9;9;\"/tmp/d;e\"\x07\
          \x1b]9;4;invalid;50\x07\
          \x1b]9;4;1;150\x07\
          \x1b]133;D;7;ignored\x07",
    );
    assert_eq!(
        output.events,
        vec![
            ParsedEvent::WorkingDirectory("/tmp/a%20b;c".to_string()),
            ParsedEvent::WorkingDirectory("/tmp/d;e".to_string()),
            ParsedEvent::Progress(Progress::InProgress(100)),
            ParsedEvent::ShellCommandFinished(None),
        ]
    );
}

#[test]
fn custom_osc_events_match_native_utf8_and_prefix_rules() {
    let (_, output) = parse(
        b"\x1b]7\x07\
          \x1b]7;file:///tmp/A\xffB\x07\
          \x1b]9;4;1;50;\xff\x07\
          \x1b]133;A;\xff\x07\
          \x1b]133;Aextra\x07\
          \x1b]133;Dextra\x07",
    );
    assert_eq!(
        output.events,
        vec![
            ParsedEvent::ShellPromptStart,
            ParsedEvent::ShellCommandFinished(None),
        ]
    );
}

#[test]
fn title_osc_trims_text_and_ignores_a_missing_parameter() {
    let (_, output) = parse(b"\x1b]2\x07\x1b]2;  hello world  \x07");
    assert_eq!(
        output.events,
        vec![ParsedEvent::Title("hello world".to_string())]
    );
}

#[test]
fn title_osc_skips_invalid_utf8_fields_like_vte() {
    let (_, output) = parse(
        b"\x1b]2;A\xffB\x07\
          \x1b]2;left;\xff;right\x07",
    );
    assert_eq!(
        output.events,
        vec![
            ParsedEvent::Title(String::new()),
            ParsedEvent::Title("left;right".to_string()),
        ]
    );
}

#[test]
fn title_osc_matches_vte_parameter_limit() {
    let mut bytes = b"\x1b]2".to_vec();
    for index in 0..20 {
        bytes.extend_from_slice(format!(";{index}").as_bytes());
    }
    bytes.push(0x07);

    let (_, output) = parse(&bytes);
    assert_eq!(
        output.events,
        vec![ParsedEvent::Title(
            "0;1;2;3;4;5;6;7;8;9;10;11;12;13;14".to_string()
        )]
    );
}

#[test]
fn osc_cursor_shape_updates_the_rendered_cursor() {
    let (grid, _) = parse(b"\x1b]50;CursorShape=1\x07");
    assert_eq!(grid.cursor_state().unwrap().style, CursorStyle::Line);

    let (grid, _) = parse(b"\x1b]50;CursorShape=0\x1b\\");
    assert_eq!(grid.cursor_state().unwrap().style, CursorStyle::Block);
}

#[test]
fn osc_cursor_shape_matches_vte_first_byte_compatibility() {
    let (grid, _) = parse(b"\x1b]50;CursorShape=10\x07");
    assert_eq!(grid.cursor_style_status(), 6);

    let (grid, _) = parse(b"\x1b]50;CursorShape=20\x07");
    assert_eq!(grid.cursor_style_status(), 4);

    let (grid, _) = parse(b"\x1b]50;CursorShape=9\x07");
    assert_eq!(grid.cursor_state().unwrap().style, CursorStyle::Block);
}

#[test]
fn cursor_shape_queries_preserve_blinking_and_underlying_shape() {
    let (grid, output) = parse(
        b"\x1b[3 q\x1bP$q q\x1b\\\
          \x1b[?12l\x1bP$q q\x1b\\\
          \x1b]50;CursorShape=1\x07\x1bP$q q\x1b\\\
          \x1b[?12h\x1b[99 q\x1bP$q q\x1b\\",
    );
    assert_eq!(grid.cursor_state().unwrap().style, CursorStyle::Line);
    assert_eq!(
        output.replies,
        b"\x1bP1$r3 q\x1b\\\x1bP1$r4 q\x1b\\\x1bP1$r6 q\x1b\\\x1bP1$r5 q\x1b\\"
    );
}

#[test]
fn saves_and_restores_window_titles() {
    let (_, output) = parse(b"\x1b]2;A\x07\x1b[22;1t\x1b]2;B\x07\x1b[23;1t");
    assert_eq!(
        output.events,
        vec![
            ParsedEvent::Title("A".to_string()),
            ParsedEvent::Title("B".to_string()),
            ParsedEvent::Title("A".to_string()),
        ]
    );

    let (_, output) = parse(b"\x1b]2;one\x07\x1b[22;2t\x1b]2;two\x07\x1b[23;2t\x1b[23;2t");
    assert_eq!(
        output.events,
        vec![
            ParsedEvent::Title("one".to_string()),
            ParsedEvent::Title("two".to_string()),
            ParsedEvent::Title("one".to_string()),
        ]
    );
}

#[test]
fn window_title_stack_matches_alacritty_depth_limit() {
    let mut grid = Grid::new(12, 3, 8, CursorStyle::Block);
    let mut parser = Parser::default();

    for index in 0..=TITLE_STACK_MAX_DEPTH {
        parser.title = Some(Arc::from(index.to_string()));
        parser.advance(&mut grid, b"\x1b[22;1t");
    }

    assert_eq!(parser.title_stack.len(), TITLE_STACK_MAX_DEPTH);
    assert_eq!(
        parser.title_stack.front().and_then(Option::as_deref),
        Some("1")
    );
    assert_eq!(
        parser.title_stack.back().and_then(Option::as_deref),
        Some("4096")
    );
}

#[test]
fn window_title_stack_shares_saved_title_storage() {
    let mut grid = Grid::new(12, 3, 8, CursorStyle::Block);
    let mut parser = Parser {
        title: Some(Arc::from("a reasonably large saved title")),
        ..Parser::default()
    };
    let title = parser.title.as_ref().unwrap().clone();

    parser.advance(&mut grid, b"\x1b[22t");

    let saved = parser.title_stack.back().unwrap().as_ref().unwrap();
    assert!(Arc::ptr_eq(&title, saved));
}

#[test]
fn answers_cursor_position_queries() {
    let (_, output) = parse(b"ab\x1b[6n");
    assert_eq!(output.replies, b"\x1b[1;3R");
}

#[test]
fn dec_special_graphics_render_tui_box_drawing() {
    let (grid, _) = parse(b"\x1b(0_`lqk\x1b(B");
    let rendered = grid
        .line(0)
        .unwrap()
        .iter()
        .take(5)
        .map(|cell| cell.character)
        .collect::<String>();
    assert_eq!(rendered, " ◆┌─┐");
}

#[test]
fn alternate_screen_charset_designation_does_not_leak_to_primary() {
    let (grid, _) = parse(b"\x1b[?1049h\x1b(0x\x1b[?1049lx");
    assert_eq!(grid.line(0).unwrap()[0].character, 'x');

    let (grid, _) = parse(b"\x1b7\x1b(0\x1b[?1049htext\x1b[?1049l\x1b8_x");
    assert_eq!(grid.line(0).unwrap()[0].character, ' ');
    assert_eq!(grid.line(0).unwrap()[1].character, '│');
}

#[test]
fn supports_alignment_test_g2_g3_and_single_shift_charsets() {
    let (grid, _) = parse(b"\x1b#8\x1b[H\x1b*A\x1bN#\x1b+0\x1bOq");
    let row = grid.line(0).unwrap();
    assert_eq!(row[0].character, '£');
    assert_eq!(row[1].character, '─');
    assert!(row.iter().skip(2).all(|cell| cell.character == 'E'));
    assert!(
        grid.line(1)
            .unwrap()
            .iter()
            .all(|cell| cell.character == 'E')
    );
}

#[test]
fn full_erase_clears_alignment_test_cells() {
    let (grid, _) = parse(b"\x1b#8\x1b[2J");

    let snapshot = grid.snapshot();
    assert!(snapshot.cells.iter().all(|cell| cell.character == ' '));
}

#[test]
fn saved_cursor_and_alternate_screen_preserve_rendition_state() {
    let (grid, _) = parse(b"\x1b[31m\x1b7\x1b[32mG\x1b8R\x1b[?1049hA\x1b[34mB\x1b[?1049lP");
    let primary = grid.line(0).unwrap();
    assert_eq!(primary[0].character, 'R');
    assert_eq!(primary[0].foreground, Color::Indexed(1));
    assert_eq!(primary[1].character, 'P');
    assert_eq!(primary[1].foreground, Color::Indexed(1));
}

#[test]
fn erasures_use_the_current_background_and_keep_wide_pairs_valid() {
    let (grid, _) = parse("\x1b[44mabc\x1b[2K\x1b[49m界\x1b[2DX".as_bytes());
    let row = grid.line(0).unwrap();
    assert!(
        row.iter()
            .take(3)
            .all(|cell| cell.background == Color::Indexed(4))
    );
    assert_eq!(row[3].character, 'X');
    assert!(!row[4].wide_spacer());
}

#[test]
fn selective_erase_preserves_protected_narrow_and_wide_cells() {
    let mut grid = Grid::new(12, 3, 0, CursorStyle::Block);
    let mut parser = Parser::default();
    let output = parser.advance(
        &mut grid,
        "\x1b[1\"q\x1b[31mA\x1b[0m\x1b[0\"qB\x1b[1\"q界\x1b[0\"qC\
         \x1b[1;1H\x1b[?2K\x1b[1\"q\x1bP$q\"q\x1b\\"
            .as_bytes(),
    );

    let row = grid.line(0).unwrap();
    assert_eq!(row[0].character, 'A');
    assert!(row[0].protected());
    assert_eq!(row[1].character, ' ');
    assert_eq!(row[2].character, '界');
    assert!(row[2].protected());
    assert!(row[3].wide_spacer());
    assert!(row[3].protected());
    assert_eq!(row[4].character, ' ');
    assert_eq!(output.replies, b"\x1bP1$r1\"q\x1b\\");

    parser.advance(&mut grid, b"\x1b[2K");
    assert!(
        grid.line(0)
            .unwrap()
            .iter()
            .all(|cell| cell == crate::tmon::Cell::default())
    );
}

#[test]
fn erase_to_end_of_line_respects_pending_wrap() {
    let (grid, _) = parse(b"abcdefghijkl\x1b[K");
    assert_eq!(
        grid.line(0)
            .unwrap()
            .iter()
            .map(|cell| cell.character)
            .collect::<String>(),
        "abcdefghijkl"
    );
}

#[test]
fn osc8_hyperlinks_attach_to_cells_and_close_cleanly() {
    let (grid, _) =
        parse(b"\x1b]8;id=docs;https://example.com/reference\x1b\\read\x1b]8;;\x1b\\ plain");

    let link = grid.hyperlink_at(0, 2).expect("OSC 8 link should exist");
    assert_eq!((link.start_col, link.end_col), (0, 3));
    assert_eq!(link.target, "https://example.com/reference");
    assert_eq!(grid.hyperlink_at(0, 5), None);
}

#[test]
fn osc8_rejects_invalid_utf8_and_ignores_a_missing_uri() {
    let (grid, _) = parse(
        b"\x1b]8;id=first;https://example.com\x1b\\A\
          \x1b]8;id=ignored\x1b\\B\
          \x1b]8;;https://exa\xffmple\x1b\\C",
    );

    assert!(grid.hyperlink_at(0, 0).is_some());
    assert!(grid.hyperlink_at(0, 1).is_some());
    assert!(grid.hyperlink_at(0, 2).is_none());
}

#[test]
fn hyperlink_combining_text_and_underline_color_share_compact_cell_metadata() {
    let combining = "\u{301}\u{302}\u{303}\u{304}\u{305}\u{306}\u{307}\u{308}\u{309}";
    let bytes = format!(
        "\x1b]8;id=meta;https://example.com/meta\x1b\\\x1b[4:3;58:2::12:34:56me{combining}\x1b[0mB\x1b]8;;\x1b\\C"
    );
    let (grid, _) = parse(bytes.as_bytes());
    let line = grid.line(0).unwrap();

    assert!(line[0].has_hyperlink());
    assert!(line[0].has_combining());
    let mut resolved_combining = None;
    assert!(grid.for_each_line_cell(0, |col, _, value| {
        if col == 0 {
            resolved_combining = value.map(|value| value.to_owned_string());
        }
    }));
    assert_eq!(resolved_combining, Some(combining.to_string()));
    assert_eq!(
        line[0].underline_color,
        Some(Color::Rgb {
            r: 12,
            g: 34,
            b: 56
        })
    );
    assert_eq!(line[0].attributes.underline_style(), UnderlineStyle::Curly);

    assert!(line[1].has_hyperlink(), "SGR 0 must not close OSC 8");
    assert!(!line[1].has_combining());
    assert_eq!(line[1].underline_color, None);
    assert_eq!(line[1].attributes.underline_style(), UnderlineStyle::None);
    assert!(!line[2].has_hyperlink());

    let link = grid
        .hyperlink_at(0, 0)
        .expect("combined metadata keeps its link");
    assert_eq!((link.start_col, link.end_col), (0, 1));
}

#[test]
fn osc8_ranges_use_the_protocol_id_and_uri_identity() {
    let uri = "https://example.com/same";
    let bytes = format!(
        "\x1b]8;foo=bar:id=one;{uri}\x1b\\A\
         \x1b]8;id=two;{uri}\x1b\\B\
         \x1b]8;;{uri}\x1b\\C\
         \x1b]8;;{uri}\x1b\\D\
         \x1b]8;id=one;{uri}\x1b\\E"
    );
    let (grid, _) = parse(bytes.as_bytes());

    for col in 0..5 {
        let link = grid
            .hyperlink_at(0, col)
            .expect("cell should carry an OSC 8 link");
        assert_eq!((link.start_col, link.end_col), (col, col));
        assert_eq!(link.target, uri);
    }

    let bytes = format!(
        "\x1b]8;id=shared;{uri}\x1b\\A\x1b]8;;\x1b\\\
         \x1b]8;id=shared;{uri}\x1b\\B"
    );
    let (grid, _) = parse(bytes.as_bytes());
    let link = grid.hyperlink_at(0, 0).expect("reopened link should exist");
    assert_eq!((link.start_col, link.end_col), (0, 1));
}

#[test]
fn answers_and_updates_indexed_and_dynamic_color_queries() {
    let (grid, output) = parse(
        b"\x1b]4;1;?\x07\x1b]4;1;#123456\x1b\\\x1b]4;1;?\x1b\\\
          \x1b]10;?\x07\x1b]11;rgb:f/8/0\x07\x1b]11;?\x07\
          \x1b]12;?\x07\x1b]12;#abcdef\x07\x1b]12;?\x07",
    );

    assert_eq!(
        grid.palette().indexed(1),
        Some(Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56
        })
    );
    assert_eq!(
        grid.palette().background(),
        Some(Rgb {
            r: 0xff,
            g: 0x88,
            b: 0x00
        })
    );
    assert_eq!(
        grid.palette().cursor(),
        Some(Rgb {
            r: 0xab,
            g: 0xcd,
            b: 0xef
        })
    );
    assert_eq!(
        output.replies,
        b"\x1b]4;1;rgb:cdcd/0000/0000\x07\
          \x1b]4;1;rgb:1212/3434/5656\x1b\\\
          \x1b]10;rgb:e5e5/e5e5/e5e5\x07\
          \x1b]11;rgb:ffff/8888/0000\x07\
          \x1b]12;rgb:abab/cdcd/efef\x07"
    );
}

#[test]
fn color_resets_restore_query_fallbacks() {
    let (grid, output) = parse(
        b"\x1b]4;2;#abcdef\x07\x1b]104;2\x07\x1b]4;2;?\x07\
          \x1b]10;#010203\x07\x1b]110\x07\x1b]10;?\x07",
    );

    assert_eq!(grid.palette().indexed(2), None);
    assert_eq!(grid.palette().foreground(), None);
    assert_eq!(
        output.replies,
        b"\x1b]4;2;rgb:0000/cdcd/0000\x07\x1b]10;rgb:e5e5/e5e5/e5e5\x07"
    );
}

#[test]
fn xparsecolor_expands_short_hex_components() {
    assert_eq!(
        parse_x_color("#abc"),
        Some(Rgb {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        })
    );
    let (grid, _) = parse(b"\x1b[4:1mA\x1b[4:0mB");
    assert!(grid.line(0).unwrap()[0].attributes.underline());
    assert!(!grid.line(0).unwrap()[1].attributes.underline());
}

#[test]
fn reports_modes_keyboard_flags_and_text_area_size() {
    let (_, output) = parse(
        b"\x1b[?2004h\x1b[?2004$p\x1b[4$p\x1b[=5u\x1b[?u\
          \x1b[14t\x1b[18t",
    );

    assert_eq!(
        output.replies,
        b"\x1b[?2004;1$y\x1b[4;2$y\x1b[?0u\x1b[4;54;108t\x1b[8;3;12t"
    );
}

#[test]
fn keyboard_mode_query_reports_the_stack_top_like_alacritty() {
    let (grid, output) = parse(b"\x1b[>1u\x1b[=2;2u\x1b[?u");

    let keyboard = grid.keyboard_mode();
    assert!(keyboard.disambiguate_escape_codes);
    assert!(keyboard.report_event_types);
    assert_eq!(output.replies, b"\x1b[?1u");
}

#[test]
fn reports_alacritty_compatible_default_private_modes() {
    let (_, output) = parse(b"\x1b[?7$p\x1b[?25$p\x1b[?1007$p\x1b[?1042$p");

    assert_eq!(
        output.replies,
        b"\x1b[?7;1$y\x1b[?25;1$y\x1b[?1007;1$y\x1b[?1042;1$y"
    );
}

#[test]
fn custom_tab_stops_drive_forward_and_backward_tabulation() {
    let (grid, _) = parse(b"\x1b[3g\x1b[5G\x1bH\r\tX\x1b[2IY\x1b[1ZZ");
    assert_eq!(grid.line(0).unwrap()[4].character, 'X');
    assert_eq!(grid.line(0).unwrap()[11].character, 'Y');
    assert_eq!(grid.line(1).unwrap()[0].character, 'Z');

    let (grid, _) = parse(b"\x1b[3g\x1b[6G\x1b[Z");
    assert_eq!(grid.cursor_position(), (5, 0));
}

#[test]
fn tabs_leave_reflow_markers_and_preserve_pending_wrap_for_csi_motion() {
    let mut parser = Parser::default();
    let mut grid = Grid::new(12, 3, 0, CursorStyle::Block);
    parser.advance(&mut grid, b"a\tb\x1b[3g\r\x1b[5G\x1bH\r\tX\x1b[2IY\x1b[1Zz");

    assert_eq!(grid.line(0).unwrap()[1].character, '\t');
    assert_eq!(grid.line(0).unwrap()[4].character, 'X');
    assert!(grid.line(0).unwrap()[4].wrapped());
    assert_eq!(grid.line(1).unwrap()[0].character, 'z');
}

#[test]
fn tab_does_not_wrap_when_autowrap_is_disabled() {
    let mut parser = Parser::default();
    let mut grid = Grid::new(4, 2, 0, CursorStyle::Block);
    parser.advance(&mut grid, b"\x1b[?7l\x1b[4GX\t");

    assert_eq!(grid.cursor_position(), (3, 0));
    assert_eq!(grid.line(0).unwrap()[3].character, 'X');
    assert!(!grid.line(0).unwrap()[3].wrapped());
}

#[test]
fn newline_mode_is_reported_but_raw_line_feed_preserves_the_column() {
    let mut parser = Parser::default();
    let mut grid = Grid::new(8, 3, 0, CursorStyle::Block);
    let output = parser.advance(&mut grid, b"abc\x1b[20h\nX\x1b[20$p");

    assert_eq!(grid.cursor_position(), (4, 1));
    assert_eq!(grid.line(1).unwrap()[3].character, 'X');
    assert_eq!(output.replies, b"\x1b[20;1$y");
}

#[test]
fn standalone_c1_bytes_are_ignored_in_utf8_mode() {
    let mut parser = Parser::default();
    let mut grid = Grid::new(12, 2, 0, CursorStyle::Block);
    parser.advance(&mut grid, b"abc\x85x\x9b2;3H@\x84y\x8dz");

    assert_eq!(
        grid.line(0)
            .unwrap()
            .iter()
            .take(11)
            .map(|cell| cell.character)
            .collect::<String>(),
        "abcx2;3H@yz"
    );
}

#[test]
fn repeat_without_a_preceding_character_is_a_noop() {
    let mut parser = Parser::default();
    let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
    parser.advance(&mut grid, b"\x1b[2C\x1b[3b");

    assert_eq!(grid.cursor_position(), (2, 0));
    assert!(
        grid.line(0)
            .unwrap()
            .iter()
            .all(|cell| cell == Cell::default())
    );
}

#[test]
fn repeat_uses_the_last_character_from_an_ascii_span() {
    let (grid, _) = parse(b"abc\x1b[3b");

    assert_eq!(
        grid.line(0)
            .unwrap()
            .iter()
            .take(6)
            .map(|cell| cell.character)
            .collect::<String>(),
        "abcccc"
    );
}

#[test]
fn disabling_origin_mode_keeps_the_active_cursor_position() {
    let mut parser = Parser::default();
    let mut grid = Grid::new(8, 4, 0, CursorStyle::Block);
    parser.advance(&mut grid, b"\x1b[2;4r\x1b[?6h\x1b[2;3H\x1b[?6l");

    assert_eq!(grid.cursor_position(), (2, 2));
}

#[test]
fn scroll_regions_clamp_the_bottom_and_home_the_cursor() {
    let mut parser = Parser::default();
    let mut grid = Grid::new(8, 4, 0, CursorStyle::Block);
    parser.advance(&mut grid, b"\x1b[4;5H\x1b[2;99r");
    assert_eq!(grid.scroll_region_status(), (2, 4));
    assert_eq!(grid.cursor_position(), (0, 0));

    parser.advance(&mut grid, b"\x1b[3;4H\x1b[r");
    assert_eq!(grid.scroll_region_status(), (1, 4));
    assert_eq!(grid.cursor_position(), (0, 0));
}

#[test]
fn synchronized_updates_expose_batch_state_until_esu() {
    let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
    let mut parser = Parser::default();

    assert!(
        parser
            .advance(&mut grid, b"\x1b[?2026hframe")
            .synchronized_update_active
    );
    assert!(
        parser
            .advance(&mut grid, b"-body")
            .synchronized_update_active
    );
    assert!(
        !parser
            .advance(&mut grid, b"\x1b[?2026l")
            .synchronized_update_active
    );
}

#[test]
fn synchronized_updates_stage_grid_events_and_replies_atomically() {
    let mut grid = Grid::new(12, 2, 0, CursorStyle::Block);
    let mut parser = Parser::default();

    let staged = parser.advance(&mut grid, b"\x1b[?2026hframe\x07\x1b]2;batched\x07\x1b[6n");
    assert!(staged.synchronized_update_active);
    assert!(staged.events.is_empty());
    assert!(staged.replies.is_empty());
    assert_eq!(grid.line(0).unwrap()[0].character, ' ');

    let committed = parser.advance(&mut grid, ESU_CSI);
    assert!(!committed.synchronized_update_active);
    assert_eq!(
        grid.line(0)
            .unwrap()
            .iter()
            .take(5)
            .map(|cell| cell.character)
            .collect::<String>(),
        "frame"
    );
    assert_eq!(
        committed.events,
        vec![ParsedEvent::Bell, ParsedEvent::Title("batched".to_string())]
    );
    assert_eq!(committed.replies, b"\x1b[1;6R");
}

#[test]
fn synchronized_update_initial_bsu_can_share_a_private_mode_sequence() {
    let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
    let mut parser = Parser::default();

    let staged = parser.advance(&mut grid, b"\x1b[?2004;2026hX");
    assert!(staged.synchronized_update_active);
    assert_eq!(grid.line(0).unwrap()[0].character, ' ');
    assert!(grid.bracketed_paste_mode());

    parser.advance(&mut grid, ESU_CSI);
    assert_eq!(grid.line(0).unwrap()[0].character, 'X');
}

#[test]
fn exact_synchronized_update_end_is_chunk_safe_at_every_split() {
    for split in 0..=ESU_CSI.len() {
        let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
        let mut parser = Parser::default();
        parser.advance(&mut grid, b"\x1b[?2026hX");

        let pending = parser.advance(&mut grid, &ESU_CSI[..split]);
        if split < ESU_CSI.len() {
            assert!(pending.synchronized_update_active, "split {split}");
            assert_eq!(grid.line(0).unwrap()[0].character, ' ', "split {split}");
        }
        let committed = parser.advance(&mut grid, &ESU_CSI[split..]);
        assert!(!committed.synchronized_update_active, "split {split}");
        assert_eq!(grid.line(0).unwrap()[0].character, 'X', "split {split}");
    }
}

#[test]
fn synchronized_scanner_requires_exact_raw_end_and_retains_later_bsu() {
    let mut grid = Grid::new(12, 2, 0, CursorStyle::Block);
    let mut parser = Parser::default();

    let pending = parser.advance(&mut grid, b"\x1b[?2026hA\x1b[?2026;25lB");
    assert!(pending.synchronized_update_active);
    assert_eq!(grid.line(0).unwrap()[0].character, ' ');

    let extended = parser.advance(&mut grid, b"\x1b[?2026l\x1b[?2026hC");
    assert!(extended.synchronized_update_active);
    assert_eq!(
        grid.line(0)
            .unwrap()
            .iter()
            .take(2)
            .map(|cell| cell.character)
            .collect::<String>(),
        "AB"
    );

    parser.advance(&mut grid, ESU_CSI);
    assert_eq!(
        grid.line(0)
            .unwrap()
            .iter()
            .take(3)
            .map(|cell| cell.character)
            .collect::<String>(),
        "ABC"
    );
}

#[test]
fn explicit_sync_stop_commits_buffered_protocol_replies() {
    let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
    let mut parser = Parser::default();
    parser.advance(&mut grid, b"\x1b[?2026hX\x1b[6n");

    let committed = parser.stop_synchronized_update(&mut grid);
    assert!(!committed.synchronized_update_active);
    assert_eq!(grid.line(0).unwrap()[0].character, 'X');
    assert_eq!(committed.replies, b"\x1b[1;2R");
}

#[test]
fn synchronized_update_capacity_flushes_before_exceeding_two_mibibytes() {
    let mut grid = Grid::new(8, 2, 0, CursorStyle::Block);
    let mut parser = Parser::default();
    parser.advance(&mut grid, BSU_CSI);
    let oversized = vec![b'X'; MAX_SYNC_BYTES - 1];

    let flushed = parser.advance(&mut grid, &oversized);
    assert!(!flushed.synchronized_update_active);
    assert!(flushed.synchronized_update_committed);
    assert_eq!(grid.line(0).unwrap()[0].character, 'X');
}

#[test]
fn cancel_and_substitute_abort_incomplete_control_sequences() {
    let (grid, _) = parse(b"\x1b[31\x18mX\x1b[32\x1aY");
    let row = grid.line(0).unwrap();
    assert_eq!(row[0].character, 'm');
    assert_eq!(row[1].character, 'X');
    assert_eq!(row[2].character, 'Y');
    assert!(
        row.iter()
            .take(3)
            .all(|cell| cell.foreground == Color::Default)
    );
}

#[test]
fn oversized_osc_and_dcs_payloads_are_discarded() {
    let mut bytes = b"\x1b]2;".to_vec();
    bytes.resize(MAX_OSC_BYTES + 32, b'x');
    bytes.push(0x07);
    bytes.extend_from_slice(b"\x1bP$q");
    bytes.resize(bytes.len() + MAX_DCS_BYTES + 1, b'm');
    bytes.extend_from_slice(b"\x1b\\");

    let (_, output) = parse(&bytes);
    assert!(output.events.is_empty());
    assert!(output.replies.is_empty());
}

#[test]
fn c1_st_only_terminates_dcs_after_the_final_byte() {
    let (grid, output) = parse(b"\x1bP\x9cABC\x1b\\Z");
    assert!(output.replies.is_empty());
    assert_eq!(grid.line(0).unwrap()[0].character, 'Z');

    let (grid, output) = parse(b"\x1bP$qm\x9cQ");
    assert_eq!(output.replies, b"\x1bP1$r0m\x1b\\");
    assert_eq!(grid.line(0).unwrap()[0].character, 'Q');
}

#[test]
fn answers_decrqss_and_xtgettcap_queries() {
    let (_, output) = parse(
        b"\x1b[31;1m\x1b[2;3r\
          \x1bP$qm\x1b\\\x1bP$qr\x1b\\\x1bP$q q\x1b\\\
          \x1bP+q544e;436f;5463;5a5a\x1b\\",
    );

    assert_eq!(
        output.replies,
        b"\x1bP1$r1;31m\x1b\\\x1bP1$r2;3r\x1b\\\x1bP1$r2 q\x1b\\\
          \x1bP1+r544e=787465726d2d323536636f6c6f72\x1b\\\
          \x1bP1+r436f=323536\x1b\\\x1bP1+r5463\x1b\\\x1bP0+r5a5a\x1b\\"
    );
}

#[test]
fn decrqss_reports_extended_underline_style_and_color() {
    let (_, output) = parse(b"\x1b[4:3;58:2::12:34:56m\x1bP$qm\x1b\\");
    assert_eq!(output.replies, b"\x1bP1$r4:3;58;2;12;34;56m\x1b\\");
}

#[test]
fn kitty_clipboard_packets_modes_queries_and_reset_are_supported() {
    let mut grid = Grid::new(12, 3, 8, CursorStyle::Block);
    let mut parser = Parser::default();
    let output = parser.advance(
        &mut grid,
        b"\x1b[?5522h\x1b[?5522$p\x1b]5522;type=read:id=one;Lg==\x07\x1bc",
    );

    assert_eq!(output.replies, b"\x1b[?5522;1$y");
    assert!(!parser.kitty_clipboard_paste_events_mode());
    assert!(matches!(
        output.events.as_slice(),
        [
            ParsedEvent::KittyClipboardMode(true),
            ParsedEvent::KittyClipboard(packet),
            ParsedEvent::ResetTitle,
            ParsedEvent::KittyClipboardReset,
        ] if packet.body() == b"type=read:id=one;Lg==" && packet.bell_terminated()
    ));
}
