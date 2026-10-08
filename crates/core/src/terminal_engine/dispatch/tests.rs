use super::super::{
    Color, CursorShape, Damage, Engine, MouseEncoding, MouseTracking, Options, Size, Style,
    UnderlineStyle,
};

fn engine(cols: usize, rows: usize) -> Engine {
    Engine::new(
        Size { cols, rows },
        Options {
            scrollback_history: 10,
        },
    )
}

fn replies(engine: &mut Engine) -> Vec<u8> {
    let mut bytes = Vec::new();
    engine.drain_replies(&mut bytes);
    bytes
}

fn text(engine: &Engine, row: usize) -> String {
    engine
        .viewport_row(row)
        .unwrap()
        .iter()
        .map(|cell| cell.character)
        .collect()
}

#[test]
fn truecolor_accepts_legacy_and_colon_component_groups() {
    for rendition in [
        "38;2;1;2;3",
        "38:2:1:2:3",
        "38:2::1:2:3",
        "38:2:0:1:2:3",
        "38;2::1:2:3",
        "38;2:1:2:3",
    ] {
        let mut engine = engine(4, 2);
        engine.feed(format!("\x1b[{rendition}mX").as_bytes());
        assert_eq!(
            engine.viewport_row(0).unwrap()[0].style.foreground,
            Color::rgb(1, 2, 3),
            "{rendition}"
        );
    }
}

#[test]
fn invalid_extended_colors_preserve_style_and_do_not_leak_components() {
    for rendition in [
        "38:2::999:2:3",
        "38:2::1::3",
        "38:2::1:2",
        "38:2::1:2:3:4",
        "38:5:256",
        "38:5:3:4",
        "38;2;255:1;0;0",
        "38;2;0;;0",
        "48;5;999",
        "58:2::1:2:999",
        "38;2;0;0",
        "38;5",
    ] {
        let mut engine = engine(4, 2);
        engine.feed(b"\x1b[31;44;58:5:9;4:3mA");
        engine.feed(format!("\x1b[{rendition}mB").as_bytes());
        let row = engine.viewport_row(0).unwrap();
        assert_eq!(row[0].style, row[1].style, "{rendition}");
    }
}

#[test]
fn malformed_color_does_not_swallow_following_independent_rendition() {
    let mut engine = engine(4, 2);
    engine.feed(b"\x1b[31;38;2;999;0;0;1mX");
    let style = engine.viewport_row(0).unwrap()[0].style;
    assert_eq!(style.foreground, Color::indexed(1));
    assert_eq!(style.attributes, Style::BOLD);
}

#[test]
fn unsupported_subparameters_do_not_activate_unrelated_renditions() {
    for rendition in ["1:2", "0:2", ":2", "31:2", "4:99", "4:2:3", "24:3"] {
        let mut engine = engine(4, 2);
        engine.feed(b"\x1b[32;4:3mA");
        engine.feed(format!("\x1b[{rendition}mB").as_bytes());
        let row = engine.viewport_row(0).unwrap();
        assert_eq!(row[0].style, row[1].style, "{rendition}");
    }
}

#[test]
fn underline_subparameters_set_and_reset_known_styles() {
    let mut engine = engine(8, 2);
    engine.feed(b"\x1b[4:0mA\x1b[4:1mB\x1b[4:2mC\x1b[4:3mD\x1b[4:4mE\x1b[4:5mF\x1b[4:mG");
    let row = engine.viewport_row(0).unwrap();
    let styles: Vec<_> = row[..7].iter().map(|cell| cell.style.underline).collect();
    assert_eq!(
        styles,
        [
            UnderlineStyle::None,
            UnderlineStyle::Single,
            UnderlineStyle::Double,
            UnderlineStyle::Curly,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
            UnderlineStyle::Single
        ]
    );
}

#[test]
fn sgr_reset_preserves_character_protection_for_selective_erase() {
    let mut engine = engine(5, 2);
    engine.feed(b"\x1b[1\"q\x1b[31mA\x1b[0mB\x1b[99\"qC\x1b[0\"qD\x1b[H\x1b[?2K");
    assert_eq!(text(&engine, 0), "ABC  ");
    let row = engine.viewport_row(0).unwrap();
    assert_eq!(row[0].style.foreground, Color::indexed(1));
    assert_eq!(row[1].style.foreground, Color::DEFAULT);
    assert_ne!(row[1].style.attributes & Style::PROTECTED, 0);
}

#[test]
fn origin_mode_cursor_reports_are_relative_to_scrolling_region() {
    let mut engine = engine(20, 8);
    engine.feed(b"\x1b[3;7r\x1b[?6h\x1b[2;4H\x1b[6n\x1b[?6n");
    assert_eq!((engine.cursor().row, engine.cursor().col), (3, 3));
    assert_eq!(replies(&mut engine), b"\x1b[2;4R\x1b[?2;4R");
    engine.feed(b"\x1b[?6l\x1b[4;4H\x1b[6n");
    assert_eq!(replies(&mut engine), b"\x1b[4;4R");
}

#[test]
fn explicit_zero_margins_restore_full_scrolling_region() {
    let mut engine = engine(8, 6);
    engine.feed(b"\x1b[3;4r\x1b[0;0r\x1b[?6h\x1b[6;1H\x1b[6n");
    assert_eq!(engine.cursor().row, 5);
    assert_eq!(replies(&mut engine), b"\x1b[6;1R");
}

#[test]
fn counted_tabs_stop_at_screen_edges_without_unbounded_iteration() {
    let mut engine = engine(40, 2);
    engine.feed(b"\t\x1b[2I");
    assert_eq!(engine.cursor().col, 24);
    engine.feed(b"\x1b[2Z");
    assert_eq!(engine.cursor().col, 8);
    engine.feed(b"\x1b[65535I");
    assert_eq!(engine.cursor().col, 39);
    engine.feed(b"\x1b[65535Z");
    assert_eq!(engine.cursor().col, 0);
}

#[test]
fn index_ignores_newline_mode_but_line_feed_obeys_it() {
    let mut engine = engine(10, 4);
    engine.feed(b"\x1b[20hab\x1bD");
    assert_eq!((engine.cursor().row, engine.cursor().col), (1, 2));
    engine.feed(b"\n");
    assert_eq!((engine.cursor().row, engine.cursor().col), (2, 0));
}

#[test]
fn cursor_visibility_shape_and_blinking_changes_emit_damage() {
    let mut engine = engine(10, 4);
    engine.take_damage();
    for sequence in [
        b"\x1b[?25l".as_slice(),
        b"\x1b[?25h",
        b"\x1b[5 q",
        b"\x1b[?12l",
    ] {
        engine.feed(sequence);
        assert!(
            matches!(engine.take_damage(), Damage::Partial(spans) if spans.iter().any(|span| span.row == 0 && span.start == 0 && span.end >= 1))
        );
    }
    assert_eq!(engine.cursor().shape, CursorShape::Beam);
    assert!(!engine.cursor().blinking);
    engine.feed(b"\x1b[?12l");
    assert_eq!(engine.take_damage(), Damage::Partial(Vec::new()));
}

#[test]
fn palette_mutations_invalidate_screen_but_queries_do_not() {
    let mut engine = engine(10, 4);
    engine.take_damage();
    for sequence in [
        b"\x1b]4;1;#123456\x07".as_slice(),
        b"\x1b]10;#abcdef\x07",
        b"\x1b]104;1\x07",
        b"\x1b]110\x07",
    ] {
        engine.feed(sequence);
        assert_eq!(engine.take_damage(), Damage::Full);
    }
    engine.feed(b"\x1b]4;1;?\x07\x1b]10;?\x07\x1b]4;1;invalid\x07");
    assert_eq!(engine.take_damage(), Damage::Partial(Vec::new()));
}

#[test]
fn cursor_save_restore_preserves_designated_and_active_character_sets() {
    for (save, restore) in [
        ("\x1b7", "\x1b8"),
        ("\x1b[s", "\x1b[u"),
        ("\x1b[?1048h", "\x1b[?1048l"),
    ] {
        let mut engine = engine(5, 2);
        engine.feed(format!("\x1b)0\x0e{save}\x0f\x1b)B{restore}q").as_bytes());
        assert_eq!(text(&engine, 0), "─    ");
    }
}

#[test]
fn alternate_screen_1049_restores_primary_character_set() {
    let mut engine = engine(5, 2);
    engine.feed(b"\x1b(0\x1b[?1049h\x1b(B\x1b7\x1b[?1049lq");
    assert_eq!(text(&engine, 0), "─    ");
}

#[test]
fn alternate_screen_1047_preserves_on_entry_and_clears_on_exit() {
    let mut engine = engine(10, 2);
    engine.feed(b"\x1b[?47hretained\x1b[?47l\x1b[?1047h");
    assert_eq!(text(&engine, 0), "retained  ");
    engine.feed(b"\x1b[?1047l\x1b[?47h");
    assert_eq!(text(&engine, 0), "          ");
}

#[test]
fn unrelated_mouse_mode_resets_preserve_active_tracking_and_encoding() {
    let mut engine = engine(5, 2);
    engine.feed(b"\x1b[?1003h\x1b[?1000l\x1b[?1016h\x1b[?1006l");
    assert_eq!(engine.modes().mouse_tracking, MouseTracking::Motion);
    assert_eq!(engine.modes().mouse_encoding, MouseEncoding::SgrPixels);
    engine.feed(b"\x1b[?1003l\x1b[?1016l");
    assert_eq!(engine.modes().mouse_tracking, MouseTracking::None);
    assert_eq!(engine.modes().mouse_encoding, MouseEncoding::Default);
}

#[test]
fn kitty_flags_mask_unknown_bits_instead_of_enabling_every_flag() {
    let mut engine = engine(5, 2);
    engine.feed(b"\x1b[>32u");
    assert_eq!(engine.modes().kitty_keyboard, 0);
    engine.feed(b"\x1b[>37u\x1b[=34;2u\x1b[=33;3u\x1b[?u");
    assert_eq!(replies(&mut engine), b"\x1b[?6u");
    engine.feed(b"\x1b[<0u");
    assert_eq!(engine.modes().kitty_keyboard, 6);
    engine.feed(b"\x1b[<65535u");
    assert_eq!(engine.modes().kitty_keyboard, 0);
}

#[test]
fn full_reset_clears_saved_character_sets_keyboard_flags_and_styles() {
    let mut engine = engine(10, 4);
    engine.feed(b"\x1b(0\x1b7\x1b[>7u\x1b[31m\x1b[?25l\x1b[5 q\x1bc\x1b8q");
    assert_eq!(text(&engine, 0), "q         ");
    assert_eq!(engine.modes().kitty_keyboard, 0);
    assert!(engine.cursor().visible);
    assert_eq!(engine.cursor().shape, CursorShape::Block);
    assert_eq!(engine.viewport_row(0).unwrap()[0].style, Style::default());
}

#[test]
fn soft_reset_restores_vt_modes_without_erasing_or_moving_content() {
    let mut engine = engine(10, 4);
    engine.set_default_cursor_shape(CursorShape::Beam);
    engine.feed(b"history\r\nfirst\r\nsecond\r\nthird\r\nfourth");
    engine.feed(b"\x1b[?25l\x1b[2 q\x1b[?1h\x1b=\x1b[4h\x1b[?7l\x1b[2;3r\x1b[?6h\x1b[31;1;4m\x1b[1\"q\x1b(0\x1b7\x1b[2;5H");
    let screen: Vec<_> = (0..4)
        .map(|row| engine.viewport_row(row).unwrap().to_vec())
        .collect();
    let history = engine.line(-1).unwrap().to_vec();
    let position = (engine.cursor().row, engine.cursor().col);
    engine.take_damage();
    // Exercise the parser's fragmented intermediate/final handling too.
    for byte in b"\x1b[!p" {
        engine.feed(&[*byte]);
    }
    assert_eq!((engine.cursor().row, engine.cursor().col), position);
    assert!(engine.cursor().visible);
    assert_eq!(engine.cursor().shape, CursorShape::Beam);
    assert!(!engine.cursor().blinking);
    assert!(!engine.modes().application_keypad);
    assert_eq!(engine.history_size(), 1);
    assert_eq!(engine.line(-1).unwrap(), history);
    for (row, expected) in screen.iter().enumerate() {
        assert_eq!(engine.viewport_row(row).unwrap(), expected);
    }
    assert!(matches!(engine.take_damage(), Damage::Partial(spans) if !spans.is_empty()));
    engine.feed(b"\x1b[?25;6;1;7$p\x1b[4$p\x1bP$qr\x1b\\\x1bP$qm\x1b\\\x1bP$q\"q\x1b\\");
    assert_eq!(replies(&mut engine), b"\x1b[?25;1$y\x1b[?6;2$y\x1b[?1;2$y\x1b[?7;1$y\x1b[4;2$y\x1bP1$r1;4r\x1b\\\x1bP1$r0m\x1b\\\x1bP1$r0\"q\x1b\\");
    engine.feed(b"q");
    assert_eq!(
        engine.viewport_row(position.0).unwrap()[position.1].character,
        'q'
    );
    engine.feed(b"\x1b8q");
    assert_eq!(engine.viewport_row(0).unwrap()[0].character, 'q');
    assert_eq!(engine.viewport_row(0).unwrap()[0].style, Style::default());
}

#[test]
fn soft_reset_preserves_alternate_screen_and_extended_session_modes() {
    let mut engine = engine(10, 4);
    engine.feed(b"primary\x1b[?1049h\x1b[?2004;1003;1006;1004;5522h\x1b[>7u\x1b]4;1;#123456\x07alternate\x1b[!p");
    assert!(engine.alternate_screen());
    assert_eq!(text(&engine, 0), "alternate ");
    assert!(engine.modes().bracketed_paste);
    assert!(engine.modes().focus_events);
    assert!(engine.modes().clipboard_paste_events);
    assert_eq!(engine.modes().mouse_tracking, MouseTracking::Motion);
    assert_eq!(engine.modes().mouse_encoding, MouseEncoding::Sgr);
    assert_eq!(engine.modes().kitty_keyboard, 7);
    assert_eq!(engine.palette()[1], Some(Color::rgb(0x12, 0x34, 0x56)));
    engine.feed(b"\x1b[?1049l");
    assert_eq!(text(&engine, 0), "primary   ");
}

#[test]
fn private_mode_restore_changes_only_modes_previously_saved() {
    let mut engine = engine(10, 4);
    for byte in b"\x1b[?1;25;2004s\x1b[?1;2004;1003;1006h\x1b[?25l\x1b[?1;25r" {
        engine.feed(&[*byte]);
    }
    assert!(engine.cursor().visible);
    assert!(!engine.modes().application_cursor);
    assert!(engine.modes().bracketed_paste);
    assert_eq!(engine.modes().mouse_tracking, MouseTracking::Motion);
    assert_eq!(engine.modes().mouse_encoding, MouseEncoding::Sgr);
    engine.feed(b"\x1b[?2004;1003;1006;9999r");
    assert!(!engine.modes().bracketed_paste);
    assert_eq!(engine.modes().mouse_tracking, MouseTracking::Motion);
    assert_eq!(engine.modes().mouse_encoding, MouseEncoding::Sgr);
}

#[test]
fn private_mode_saves_overwrite_independently_and_ris_forgets_them() {
    let mut engine = engine(10, 4);
    engine.feed(b"\x1b[?25;2004s\x1b[?25l\x1b[?25s\x1b[?25;2004h\x1b[?25;2004r");
    assert!(!engine.cursor().visible);
    assert!(!engine.modes().bracketed_paste);
    // Unsupported subparameters must neither overwrite nor restore a mode.
    engine.feed(b"\x1b[?25h\x1b[?25:1s\x1b[?25r\x1b[?25:1r");
    assert!(!engine.cursor().visible);
    engine.feed(b"\x1bc\x1b[?25r");
    assert!(engine.cursor().visible);
}

#[test]
fn restoring_private_modes_applies_screen_and_clipboard_side_effects() {
    let mut engine = engine(10, 4);
    engine.feed(b"primary\x1b[?1049;5522s\x1b[?1049;5522h\x1b[?1049;5522r");
    assert!(!engine.alternate_screen());
    assert_eq!(text(&engine, 0), "primary   ");
    assert!(!engine.modes().clipboard_paste_events);
    assert_eq!(
        engine.pop_event(),
        Some(super::super::Event::KittyClipboardControl(
            crate::KittyClipboardControl::Set(true)
        ))
    );
    assert_eq!(
        engine.pop_event(),
        Some(super::super::Event::KittyClipboardControl(
            crate::KittyClipboardControl::Set(false)
        ))
    );
}

#[test]
fn osc_7501_fragmented_reports_detection_and_screen_lifetime() {
    let input = b"\x1b]7501;?\x07\x1b[c\x1b]7501;state=blocked:app=deploy:kind=auth:progress=42:msg=SGk\x1b\\";
    for split in 0..=input.len() {
        let mut engine = engine(8, 2);
        engine.feed(&input[..split]);
        engine.feed(&input[split..]);
        assert_eq!(replies(&mut engine), b"\x1b]7501;?\x1b\\\x1b[?62;22c");
        let records = engine.take_program_status().unwrap();
        assert_eq!(records[0].state, crate::ProgramState::Blocked);
        assert_eq!(records[0].msg.as_deref(), Some("Hi"));
        assert_eq!(records[0].progress, Some(42));
        engine.feed(b"\x1b[?1049h\x1b[!p\x1b[?1049l");
        assert_eq!(engine.program_status(), records);
        assert_eq!(engine.take_program_status(), None);
        engine.feed(b"\x1b]7501;state=done:id=child\x07\x1b]133;A;click_events=1\x07");
        let records = engine.take_program_status().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state, crate::ProgramState::Done);
        engine.feed(b"\x1b]7501;state=working\x07");
        engine.process_exited();
        assert_eq!(engine.take_program_status(), Some(records));
        engine.feed(b"\x1bc");
        assert_eq!(engine.take_program_status(), Some(vec![]));
        assert_eq!(text(&engine, 0), "        ");
    }
}

#[test]
fn osc_7501_snapshot_survives_event_overflow_and_osc9_is_independent() {
    let mut engine = engine(8, 2);
    engine.feed(b"\x1b]7501;state=blocked:kind=question:msg=SGk=\x07\x1b]9;4;1;20\x07");
    engine.feed(&vec![7; 2048]);
    assert_eq!(
        engine.take_program_status().unwrap()[0].kind,
        Some(crate::ProgramStatusKind::Question)
    );
    engine.feed(b"\x1b]7501;state=clear\x1b\\");
    assert_eq!(engine.take_program_status(), Some(vec![]));
}

#[test]
fn osc_7501_sequence_limit_includes_ignored_control_bytes() {
    let mut engine = engine(8, 2);
    engine.feed(b"\x1b]7501;state=done");
    engine.feed(&vec![0; 4096]);
    engine.feed(b"\x07ok");
    assert!(engine.program_status().is_empty());
    assert_eq!(text(&engine, 0), "ok      ");
    engine.feed(b"\x1b]7501;state=done\x07");
    assert_eq!(engine.program_status().len(), 1);
}
