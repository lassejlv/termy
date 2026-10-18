use super::super::{
    Color, CursorShape, Damage, Engine, MouseEncoding, MouseTracking, Options, Size, Style,
    UnderlineStyle,
};

fn engine(cols: usize, rows: usize) -> Engine {
    Engine::new(
        Size { cols, rows },
        Options {
            scrollback_history: 10,
            ..Default::default()
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
        assert_eq!(
            row.underline_color(0),
            row.underline_color(1),
            "{rendition}"
        );
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
        assert_eq!(
            row.underline_color(0),
            row.underline_color(1),
            "{rendition}"
        );
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
fn resetting_any_mouse_tracking_mode_turns_tracking_off() {
    for reset in [9, 1000, 1002, 1003] {
        let mut engine = engine(5, 2);
        engine.feed(format!("\x1b[?1003h\x1b[?{reset}l").as_bytes());
        assert_eq!(
            engine.modes().mouse_tracking,
            MouseTracking::None,
            "{reset}"
        );
    }
}

#[test]
fn unrelated_mouse_encoding_reset_preserves_active_encoding() {
    let mut engine = engine(5, 2);
    engine.feed(b"\x1b[?1006h\x1b[?1005l");
    assert_eq!(engine.modes().mouse_encoding, MouseEncoding::Sgr);
    engine.feed(b"\x1b[?1006l");
    assert_eq!(engine.modes().mouse_encoding, MouseEncoding::Default);
}

#[test]
fn unsupported_urxvt_and_sgr_pixel_mouse_modes_are_rejected() {
    let mut engine = engine(5, 2);
    engine.feed(b"\x1b[?1006h\x1b[?1015h\x1b[?1016h");
    assert_eq!(engine.modes().mouse_encoding, MouseEncoding::Sgr);
    engine.feed(b"\x1b[?1015l\x1b[?1016l\x1b[?1015$p\x1b[?1016$p");
    assert_eq!(engine.modes().mouse_encoding, MouseEncoding::Sgr);
    assert_eq!(replies(&mut engine), b"\x1b[?1015;0$y\x1b[?1016;0$y");
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
        .map(|row| engine.viewport_row(row).unwrap().snapshot())
        .collect();
    let history = engine.line(-1).unwrap().snapshot();
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
    assert_eq!(engine.line(-1).unwrap().snapshot(), history);
    for (row, expected) in screen.iter().enumerate() {
        assert_eq!(&engine.viewport_row(row).unwrap().snapshot(), expected);
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

#[test]
fn clamped_repeat_matches_writing_every_repetition() {
    for (cols, rows) in [(7, 3), (8, 4), (1, 2)] {
        for prefix in ["", "ab", "\x1b[2;3r\x1b[3;1H"] {
            for (character, count) in [('x', 65_535), ('中', 4_099), ('é', 1_001), ('x', 23)] {
                let mut repeated = engine(cols, rows);
                repeated.feed(format!("{prefix}{character}\x1b[{count}bZ").as_bytes());
                let mut written = engine(cols, rows);
                let text = character.to_string().repeat(count + 1);
                written.feed(format!("{prefix}{text}Z").as_bytes());
                for row in 0..rows {
                    assert_eq!(
                        repeated.viewport_row(row),
                        written.viewport_row(row),
                        "{cols}x{rows} {prefix:?} {character} x{count} row {row}"
                    );
                }
                assert_eq!(repeated.cursor(), written.cursor());
            }
        }
    }
}

#[test]
fn repeating_grapheme_extenders_is_bounded() {
    let mut engine = engine(80, 24);
    // Unclamped, this repeats 13M non-ordinary scalars: far over the bound
    // even in release builds. Clamped, it stays well under it in debug CI.
    let input = "🇩\x1b[65535b".repeat(200);
    let started = std::time::Instant::now();
    engine.feed(input.as_bytes());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn insert_and_delete_lines_return_the_cursor_to_the_left_margin() {
    for sequence in ["\x1b[L", "\x1b[M"] {
        let mut engine = engine(12, 4);
        engine.feed(format!("\x1b[2;10H{sequence}").as_bytes());
        assert_eq!((engine.cursor().row, engine.cursor().col), (1, 0));
    }
}

#[test]
fn osc7_file_urls_are_percent_decoded_and_other_schemes_ignored() {
    for (input, expected) in [
        ("file://host/Users/me/My%20Dir", Some("/Users/me/My Dir")),
        ("file://host/tmp/%C3%A6%C3%B8", Some("/tmp/æø")),
        ("file:///plain", Some("/plain")),
        ("FILE://host/a%20b", Some("/a b")),
        // A literal `%` that is not a valid escape keeps the raw path.
        ("file://host/bad%zz", Some("/bad%zz")),
        ("file://host/100%", Some("/100%")),
        ("file://host/%FF", Some("/%FF")),
        ("file://host", None),
        ("https://example.com/tmp", None),
        ("kitty-shell-cwd://host/tmp", None),
        ("/raw/path", Some("/raw/path")),
        ("C:\\Users\\me", Some("C:\\Users\\me")),
    ] {
        let mut engine = engine(8, 2);
        engine.feed(format!("\x1b]7;{input}\x07").as_bytes());
        let mut cwd = None;
        while let Some(event) = engine.pop_event() {
            if let super::super::Event::WorkingDirectory(path) = event {
                cwd = Some(path);
            }
        }
        assert_eq!(cwd.as_deref(), expected, "{input}");
    }
}

#[test]
fn windows_file_url_paths_map_drives_and_unc_shares() {
    for (input, expected) in [
        ("/C:/Users/me", r"C:\Users\me"),
        ("/c:/", r"c:\"),
        ("/D:", r"D:\"),
        ("//server/share/dir", r"\\server\share\dir"),
        ("///server/share", r"\\server\share"),
        // A plain path is kept even when another host sent it.
        ("/home/me", "/home/me"),
        ("//", "//"),
        ("/C:dir", "/C:dir"),
    ] {
        assert_eq!(
            super::windows_file_url_path(input.to_owned()),
            expected,
            "{input}"
        );
    }
}

#[cfg(windows)]
#[test]
fn osc7_windows_file_urls_become_windows_paths() {
    for (input, expected) in [
        ("file://host/C:/Users/me/My%20Dir", r"C:\Users\me\My Dir"),
        ("file:///C:/", r"C:\"),
        ("file://host//server/share/dir", r"\\server\share\dir"),
        ("file://host/home/me", "/home/me"),
    ] {
        let mut engine = engine(8, 2);
        engine.feed(format!("\x1b]7;{input}\x07").as_bytes());
        let mut cwd = None;
        while let Some(event) = engine.pop_event() {
            if let super::super::Event::WorkingDirectory(path) = event {
                cwd = Some(path);
            }
        }
        assert_eq!(cwd.as_deref(), Some(expected), "{input}");
    }
}

#[test]
fn save_restore_and_screen_switches_keep_cursor_visibility_shape_and_blink() {
    for (save, restore) in [
        ("\x1b7", "\x1b8"),
        ("\x1b[s", "\x1b[u"),
        ("\x1b[?1048h", "\x1b[?1048l"),
        ("\x1b[?1049h", "\x1b[?1049l"),
        ("\x1b[?47h", "\x1b[?47l"),
        ("\x1b[?1047h", "\x1b[?1047l"),
    ] {
        let mut engine = engine(10, 4);
        engine.feed(format!("{save}\x1b[?25l\x1b[6 q{restore}").as_bytes());
        let cursor = engine.cursor();
        assert!(!cursor.visible, "{save:?}");
        assert_eq!(cursor.shape, CursorShape::Beam, "{save:?}");
        assert!(!cursor.blinking, "{save:?}");
    }
}

#[test]
fn restore_and_alternate_screens_keep_the_configured_default_cursor_shape() {
    for sequence in [
        "\x1b8",
        "\x1b[?47h",
        "\x1b[?1049h",
        "\x1b[?1049h\x1b[?1049l",
    ] {
        let mut engine = engine(10, 4);
        engine.set_default_cursor_shape(CursorShape::Beam);
        engine.feed(sequence.as_bytes());
        assert_eq!(engine.cursor().shape, CursorShape::Beam, "{sequence:?}");
        // Nothing marked the shape overridden, so it follows the default.
        engine.set_default_cursor_shape(CursorShape::Underline);
        assert_eq!(
            engine.cursor().shape,
            CursorShape::Underline,
            "{sequence:?}"
        );
    }
}

#[test]
fn restore_cursor_keeps_the_current_hyperlink_and_restores_underline_color() {
    for (save, restore) in [("\x1b7", "\x1b8"), ("\x1b[?1049h", "\x1b[?1049l")] {
        let mut open = engine(10, 2);
        open.feed(
            format!(
                "\x1b]8;;https://old\x1b\\\x1b[58;5;1m{save}\x1b]8;;https://new\x1b\\\x1b[59m{restore}x"
            )
            .as_bytes(),
        );
        let row = open.viewport_row(0).unwrap();
        assert_eq!(row.hyperlink(0).unwrap().uri, "https://new", "{save:?}");
        assert_eq!(row.underline_color(0), Color::indexed(1), "{save:?}");

        // A link closed before the restore stays closed.
        let mut closed = engine(10, 2);
        closed.feed(format!("\x1b]8;;https://old\x1b\\{save}\x1b]8;;\x1b\\{restore}x").as_bytes());
        assert!(
            closed.viewport_row(0).unwrap().extra(0).is_none(),
            "{save:?}"
        );
    }
}

#[test]
fn other_charset_designations_select_ascii_and_dec_graphics_underscore_is_blank() {
    let mut engine = engine(6, 2);
    engine.feed(b"\x1b(0a_q\x1b(Aq\x1b)0\x0eq\x1b)<q");
    assert_eq!(text(&engine, 0), "▒ ─q─q");
}

#[test]
fn cursor_color_query_falls_back_to_foreground() {
    let mut engine = engine(5, 2);
    engine.feed(b"\x1b]12;?\x1b\\\x1b]10;#123456\x07\x1b]12;?\x1b\\");
    engine.feed(b"\x1b]12;#abcdef\x07\x1b]12;?\x1b\\\x1b]112\x07\x1b]110\x07\x1b]12;?\x1b\\");
    assert_eq!(
        replies(&mut engine),
        b"\x1b]12;rgb:e5e5/e5e5/e5e5\x1b\\\x1b]12;rgb:1212/3434/5656\x1b\\\
          \x1b]12;rgb:abab/cdcd/efef\x1b\\\x1b]12;rgb:e5e5/e5e5/e5e5\x1b\\"
    );
}

#[test]
fn column_and_save_cursor_modes_are_reported() {
    let mut engine = engine(10, 4);
    engine.feed(b"\x1b[?3$p\x1b[?1048$p\x1b[?3h\x1b[?3$p\x1b[?1048h\x1b[?1048$p");
    // Each screen has its own saved cursor, like xterm.
    engine.feed(b"\x1b[?1049h\x1b[?1048$p\x1b7\x1b[?1048$p\x1b[?1049l\x1b[?3l\x1b[?3$p");
    assert_eq!(
        replies(&mut engine),
        b"\x1b[?3;2$y\x1b[?1048;2$y\x1b[?3;1$y\x1b[?1048;1$y\
          \x1b[?1048;2$y\x1b[?1048;1$y\x1b[?3;2$y"
    );
    engine.feed(b"\x1b7\x1b[!p\x1b[?1048$p\x1b[?3h\x1bc\x1b[?3$p");
    assert_eq!(replies(&mut engine), b"\x1b[?1048;2$y\x1b[?3;2$y");
}

#[test]
fn xtversion_reports_the_engine_name_and_version() {
    let mut engine = engine(5, 2);
    engine.feed(b"\x1b[>q\x1b[>0q\x1b[>1q");
    let expected = format!("\x1bP>|Termy {}\x1b\\", env!("CARGO_PKG_VERSION"));
    assert_eq!(replies(&mut engine), expected.repeat(2).as_bytes());
}

#[test]
fn grapheme_clustering_mode_is_permanently_set() {
    let mut engine = engine(5, 2);
    engine.feed(b"\x1b[?2027$p\x1b[?2027l\x1b[?2027$p\x1b[2027$p");
    assert_eq!(
        replies(&mut engine),
        b"\x1b[?2027;3$y\x1b[?2027;3$y\x1b[2027;0$y"
    );
}

#[test]
fn color_queries_echo_the_query_terminator() {
    let mut engine = engine(5, 2);
    engine.feed(b"\x1b]10;#123456\x07\x1b]4;1;?\x07\x1b]4;1;?\x1b\\\x1b]10;?;?\x07\x1b]12;?\x1b\\");
    assert_eq!(
        replies(&mut engine),
        b"\x1b]4;1;rgb:cdcd/0000/0000\x07\x1b]4;1;rgb:cdcd/0000/0000\x1b\\\
          \x1b]10;rgb:1212/3434/5656\x07\x1b]11;rgb:1e1e/1e1e/1e1e\x07\
          \x1b]12;rgb:1212/3434/5656\x1b\\"
    );
}

#[test]
fn reverse_video_mode_is_tracked_reported_and_repaints() {
    let mut engine = engine(5, 2);
    engine.take_damage();
    let revision = engine.palette_revision();
    engine.feed(b"\x1b[?5h");
    assert!(engine.modes().reverse_video);
    assert_eq!(engine.take_damage(), Damage::Full);
    assert_ne!(engine.palette_revision(), revision);
    // Setting it again changes nothing.
    let revision = engine.palette_revision();
    engine.feed(b"\x1b[?5h");
    assert_eq!(engine.take_damage(), Damage::Partial(Vec::new()));
    assert_eq!(engine.palette_revision(), revision);
    engine.feed(b"\x1b[?5$p\x1b[?5s\x1b[?5l\x1b[?5$p\x1b[?5r");
    assert_eq!(replies(&mut engine), b"\x1b[?5;1$y\x1b[?5;2$y");
    assert!(engine.modes().reverse_video);
    engine.feed(b"\x1bc");
    assert!(!engine.modes().reverse_video);
}

#[test]
fn screen_alignment_fills_with_e_resets_margins_and_homes() {
    let mut engine = engine(4, 3);
    engine.feed(
        "\x1b[31;1;4m界x\r\n\x1b]8;;https://x\x1b\\link\x1b[2;3r\x1b[?6h\x1b[2;2H".as_bytes(),
    );
    engine.take_damage();
    engine.feed(b"\x1b#8");
    for row in 0..3 {
        assert_eq!(text(&engine, row), "EEEE");
        let line = engine.viewport_row(row).unwrap();
        assert!(line.extras().next().is_none());
        for cell in line {
            assert_eq!(cell.flags, 0);
            assert_eq!(cell.style.foreground, Color::indexed(1));
            assert_eq!(cell.style.attributes, 0);
            assert_eq!(cell.style.underline, UnderlineStyle::None);
        }
    }
    assert_eq!((engine.cursor().row, engine.cursor().col), (0, 0));
    assert_eq!(engine.take_damage(), Damage::Full);
    // The margins span the screen again; origin mode is kept, like xterm.
    engine.feed(b"\x1b[3;1H\x1b[6n");
    assert_eq!(replies(&mut engine), b"\x1b[3;1R");
}

#[test]
fn cursor_style_zero_restores_the_configured_default_without_blinking() {
    let mut engine = engine(5, 2);
    engine.set_default_cursor_shape(CursorShape::Beam);
    engine.feed(b"\x1b[3 q");
    assert_eq!(engine.cursor().shape, CursorShape::Underline);
    assert!(engine.cursor().blinking);
    engine.feed(b"\x1b[0 q");
    assert_eq!(engine.cursor().shape, CursorShape::Beam);
    assert!(!engine.cursor().blinking);
    engine.feed(b"\x1b[ q\x1bP$q q\x1b\\");
    assert_eq!(replies(&mut engine), b"\x1bP1$r6 q\x1b\\");
    // The default still follows later configuration changes.
    engine.set_default_cursor_shape(CursorShape::Underline);
    assert_eq!(engine.cursor().shape, CursorShape::Underline);
}

#[test]
fn entering_1049_from_another_alternate_mode_saves_the_cursor_and_clears() {
    let mut engine = engine(6, 3);
    engine.feed(b"primary\x1b[2;3H\x1b7\x1b[3;5H\x1b[?47hold\x1b[3;2H\x1b[?1049h");
    assert!(engine.alternate_screen());
    assert!((0..3).all(|row| text(&engine, row) == "      "));
    // The cursor stays put and is saved on the alternate screen.
    assert_eq!((engine.cursor().row, engine.cursor().col), (2, 1));
    engine.feed(b"\x1b[H\x1b8");
    assert_eq!((engine.cursor().row, engine.cursor().col), (2, 1));
    // Leaving 1049 restores the primary screen's saved cursor.
    engine.feed(b"\x1b[?1049l");
    assert!(!engine.alternate_screen());
    assert_eq!(text(&engine, 0), "primar");
    assert_eq!((engine.cursor().row, engine.cursor().col), (1, 2));
}

#[test]
fn utf8_encoded_c1_controls_are_executed_not_stored_as_marks() {
    let mut engine = engine(20, 3);
    // U+009B is CSI: `1m` sets bold rather than printing.
    engine.feed("x\u{9b}1my\u{85}z".as_bytes());
    assert_eq!(text(&engine, 0).trim_end(), "xy");
    let row = engine.viewport_row(0).unwrap();
    assert_eq!(row.combining(0), "");
    assert_eq!(row[1].style.attributes, Style::BOLD);
    // U+0085 is NEL.
    assert_eq!(text(&engine, 1).trim_end(), "z");
}
