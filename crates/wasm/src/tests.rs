use super::*;

fn text_at(engine: &mut TermyEngine, row: u32) -> String {
    let cols = engine.cols() as usize;
    let cells = engine.read_rows(row, row + 1);
    (0..cols)
        .map(|col| {
            let value = cells[col * CELL_STRIDE];
            if value & cells::STRING_FLAG != 0 {
                engine.read_strings()[(value & !cells::STRING_FLAG) as usize].clone()
            } else {
                char::from_u32(value).unwrap_or(' ').to_string()
            }
        })
        .collect::<String>()
        .trim_end()
        .to_owned()
}

#[test]
fn feeds_text_and_reads_flat_cells() {
    let mut engine = TermyEngine::new(10, 3, 100);
    engine.feed(b"hi\r\n\x1b[1;31mred\x1b[0m");
    assert_eq!(text_at(&mut engine, 0), "hi");
    assert_eq!(text_at(&mut engine, 1), "red");
    let cells = engine.read_rows(1, 2);
    assert_eq!(cells[1], 0x0100_0001, "indexed red foreground");
    assert_eq!(cells[4] & 0xffff, 1, "bold");
    assert_eq!(engine.cursor()[..2], [1, 3]);
}

#[test]
fn combining_text_and_hyperlinks_use_the_string_table() {
    let mut engine = TermyEngine::new(10, 2, 0);
    engine.feed("e\u{301}\x1b]8;;https://termy.sh\x07L\x1b]8;;\x07".as_bytes());
    let cells = engine.read_rows(0, 1);
    assert_ne!(cells[0] & cells::STRING_FLAG, 0);
    let strings = engine.read_strings();
    assert_eq!(
        strings[(cells[0] & !cells::STRING_FLAG) as usize],
        "e\u{301}"
    );
    let link = cells[CELL_STRIDE + 5];
    assert_eq!(strings[link as usize - 1], "https://termy.sh");
}

#[test]
fn a_hyperlink_is_interned_once_per_read() {
    let mut engine = TermyEngine::new(10, 2, 0);
    engine.feed(b"\x1b]8;;https://termy.sh\x07link\x1b]8;;\x07 \x1b]8;;https://termy.sh\x07x");
    let cells = engine.read_rows(0, 1);
    let links: Vec<u32> = (0..6).map(|col| cells[col * CELL_STRIDE + 5]).collect();
    assert_eq!(links, [1, 1, 1, 1, 0, 1]);
    assert_eq!(engine.read_strings(), ["https://termy.sh"]);
}

#[test]
fn device_attribute_queries_produce_replies() {
    let mut engine = TermyEngine::new(10, 2, 0);
    engine.feed(b"\x1b[6n");
    assert_eq!(engine.take_replies(), b"\x1b[1;1R");
    assert!(engine.take_replies().is_empty());
}

#[test]
fn damage_reports_full_then_partial_spans() {
    let mut engine = TermyEngine::new(10, 3, 0);
    let _ = engine.take_damage();
    engine.feed(b"\x1b[2;1Hx");
    let damage = engine.take_damage();
    assert_eq!(damage[0], 0, "partial");
    let spans: Vec<_> = damage[2 + damage[1] as usize * 3..].chunks(3).collect();
    assert!(spans.iter().any(|span| span[0] == 1));
}

#[test]
fn scrollback_lines_are_negative_and_scrollable() {
    let mut engine = TermyEngine::new(5, 2, 10);
    engine.feed(b"one\r\ntwo\r\nthree");
    assert_eq!(engine.history_size(), 1);
    assert_eq!(engine.line_text(-1, true).as_deref(), Some("one"));
    assert!(engine.scroll_display(1));
    assert_eq!(engine.display_offset(), 1);
    assert_eq!(text_at(&mut engine, 0), "one");
    assert!(engine.scroll_to_bottom());
}

#[test]
fn keys_follow_application_cursor_mode() {
    let mut engine = TermyEngine::new(10, 2, 0);
    assert_eq!(
        engine.encode_key("up", None, 0, 0, false),
        Some(b"\x1b[A".to_vec())
    );
    engine.feed(b"\x1b[?1h");
    assert_eq!(
        engine.encode_key("up", None, 0, 0, false),
        Some(b"\x1bOA".to_vec())
    );
    assert_eq!(
        engine.encode_key("c", Some("c".into()), 1, 0, false),
        Some(vec![3])
    );
    assert_eq!(engine.encode_key("up", None, 0, 2, false), None);
}

#[test]
fn mouse_reports_require_tracking_mode() {
    let mut engine = TermyEngine::new(10, 2, 0);
    assert_eq!(engine.encode_mouse(0, 0, 1, 0, 0), None);
    engine.feed(b"\x1b[?1000h\x1b[?1006h");
    assert_eq!(
        engine.encode_mouse(0, 0, 1, 0, 0),
        Some(b"\x1b[<0;2;1M".to_vec())
    );
}

#[test]
fn paste_normalizes_newlines_and_brackets() {
    let mut engine = TermyEngine::new(10, 2, 0);
    assert_eq!(engine.encode_paste("a\nb", true), b"a\rb");
    engine.feed(b"\x1b[?2004h");
    assert_eq!(
        engine.encode_paste("a\x1b[201~b", true),
        b"\x1b[200~a[201~b\x1b[201~"
    );
    assert_ne!(engine.mode_bits() & mode_bits::BRACKETED_PASTE, 0);
}

#[test]
fn paste_cannot_escape_the_bracket() {
    let mut engine = TermyEngine::new(10, 2, 0);
    engine.feed(b"\x1b[?2004h");
    // A single-pass marker strip would collapse this into a real end marker.
    let pasted = engine.encode_paste("x\x1b[20\x1b[201~1~rm -rf ~\n", true);
    assert_eq!(pasted, b"\x1b[200~x[20[201~1~rm -rf ~\r\x1b[201~");
    let body = &pasted[6..pasted.len() - 6];
    assert!(!body.contains(&0x1b));
    // C1 CSI, DEL and other C0 controls are dropped too; tabs survive.
    assert_eq!(
        engine.encode_paste("a\u{9b}201~\x7f\x03\tb\r\nc", true),
        b"\x1b[200~a201~\tb\rc\x1b[201~"
    );
    // Hosts that ignore bracketed paste still get the filter.
    assert_eq!(engine.encode_paste("x\x1b[201~\x03", false), b"x[201~");
    engine.feed(b"\x1b[?2004l");
    assert_eq!(engine.encode_paste("\x1b[201~ls\x04", true), b"[201~ls");
}

#[test]
fn builtin_themes_flatten_to_nineteen_colors() {
    let ids = theme_ids();
    assert!(ids.iter().any(|id| id == "termy"));
    let colors = theme_colors("termy").unwrap();
    assert_eq!(colors.len(), 19);
    assert!(theme_colors("not-a-theme").is_none());
}

#[test]
fn osc_palette_overrides_are_reported_raw() {
    let mut engine = TermyEngine::new(10, 2, 0);
    engine.feed(b"\x1b]4;1;rgb:ff/00/00\x07");
    assert_eq!(engine.palette_overrides()[1], 0x02ff_0000);
    assert_eq!(engine.palette_overrides()[2], 0);
}

#[test]
fn box_drawing_uses_geometry_and_letters_do_not() {
    let plan = glyph_plan('─', [None; 4], [9.0, 18.0, 14.0]).unwrap();
    assert_eq!(plan[0], 1.0, "box drawing kind");
    assert!(plan[1] + plan[2] > 0.0);
    assert!(glyph_plan('a', [None; 4], [9.0, 18.0, 14.0]).is_none());
}

#[test]
fn kitty_keyboard_flags_change_key_encoding() {
    let mut engine = TermyEngine::new(10, 2, 0);
    engine.feed(b"\x1b[>1u");
    assert_eq!(engine.keyboard_flags(), 1);
    assert_eq!(
        engine.encode_key("escape", None, 0, 0, false),
        Some(b"\x1b[27u".to_vec())
    );
}

#[test]
fn kitty_graphics_rgba_transmission_creates_a_placement() {
    let mut engine = TermyEngine::new(10, 4, 0);
    engine.set_cell_pixels(10.0, 20.0);
    // 1x1 RGBA pixel "/wAA/w==" = ff 00 00 ff, transmitted and displayed.
    engine.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=7;/wAA/w==\x1b\\");
    let placements = engine.read_graphics(10.0, 20.0);
    assert_eq!(placements.len(), PLACEMENT_STRIDE);
    assert_eq!(placements[0], 7.0, "image id");
    // Natural 1x1 size at the origin.
    assert_eq!(placements[22..30], [0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0]);
    assert!(!engine.graphics_image_is_png(0));
    assert_eq!(engine.graphics_image(0).unwrap(), vec![0xff, 0, 0, 0xff]);
}

#[test]
fn invalid_cell_and_font_sizes_produce_no_geometry() {
    let mut engine = TermyEngine::new(10, 4, 0);
    engine.set_cell_pixels(10.0, 20.0);
    engine.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=7;/wAA/w==\x1b\\");
    assert_eq!(engine.read_graphics(10.0, 20.0).len(), PLACEMENT_STRIDE);
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -10.0] {
        assert!(
            engine.read_graphics(bad, 20.0).is_empty(),
            "cell width {bad}"
        );
        assert!(
            engine.read_graphics(10.0, bad).is_empty(),
            "cell height {bad}"
        );
        assert!(engine.graphics_image(0).is_none());
        assert!(glyph_plan('─', [None; 4], [bad, 18.0, 14.0]).is_none());
        assert!(glyph_plan('─', [None; 4], [9.0, bad, 14.0]).is_none());
        assert!(glyph_plan('─', [None; 4], [9.0, 18.0, bad]).is_none());
    }
}

#[test]
fn line_text_range_slices_by_column() {
    let mut engine = TermyEngine::new(10, 2, 0);
    engine.feed("a界b".as_bytes());
    assert_eq!(engine.line_text_range(0, 1, 3, true).as_deref(), Some("界"));
    assert_eq!(engine.line_text_range(0, 3, 10, true).as_deref(), Some("b"));
}

#[test]
fn read_line_reaches_scrollback() {
    let mut engine = TermyEngine::new(4, 1, 10);
    engine.feed(b"ab\r\ncd");
    let cells = engine.read_line(-1);
    assert_eq!(cells[0], u32::from('a'));
    assert_eq!(cells[CELL_STRIDE], u32::from('b'));
}
