use termy_core::{Terminal, TerminalSize};

fn terminal() -> Terminal {
    Terminal::new_display(
        TerminalSize {
            cols: 80,
            rows: 24,
            cell_width: 10.0,
            cell_height: 20.0,
        },
        None,
    )
}

#[test]
fn chunked_image_uses_the_final_cursor_through_the_public_runtime() {
    let terminal = terminal();
    terminal.feed_output(b"\x1b[2;3H\x1b_Ga=T,i=7,f=32,s=1,v=1,m=1,C=1;AQID\x1b\\");
    terminal.feed_output(b"\x1b[8;10H\x1b_Gm=0;/w==\x1b\\");
    let placements = terminal.kitty_graphics_placements();
    assert_eq!(placements.len(), 1);
    assert_eq!((placements[0].col, placements[0].viewport_row), (9, 7));
}

#[test]
fn synchronized_grok_preview_places_image_inside_the_panel() {
    for chunk_size in [usize::MAX, 7, 1] {
        check_synchronized_grok_preview(chunk_size);
    }
}

fn check_synchronized_grok_preview(chunk_size: usize) {
    let terminal = terminal();
    // Grok Build 1.0.25 paints the panel, moves to its image origin, uploads
    // chunks, and restores the prompt cursor inside one synchronized update.
    terminal.feed_output(b"\x1b[?1049h\x1b[24;80H");
    let frame = b"\x1b[?2026h\x1b[5;30H\x1b_Ga=T,f=32,s=1,v=1,t=d,q=2,C=1,z=1,i=1,p=1,c=22,r=11,m=1;AQID\x1b\\\x1b_Gq=2,m=0;/w==\x1b\\\x1b[20;18H\x1b[?2026l";
    for chunk in frame.chunks(chunk_size) {
        terminal.feed_output(chunk);
    }
    let placements = terminal.kitty_graphics_placements();
    assert_eq!(placements.len(), 1);
    assert_eq!((placements[0].col, placements[0].viewport_row), (29, 4));
    assert_eq!(
        (placements[0].display_cols, placements[0].display_rows),
        (Some(22), Some(11))
    );
    assert_eq!(terminal.cursor_position(), (17, 19));
}

#[test]
fn synchronized_graphics_follow_screen_clear_scroll_and_cursor_changes_in_order() {
    let terminal = terminal();
    // Existing graphics must be cleared before the replacement is placed.
    terminal.feed_output(b"\x1b_Ga=T,i=1,f=32,s=1,v=1,C=1;AQID/w==\x1b\\");
    terminal
        .feed_output(b"\x1b[?2026h\x1b[?1049h\x1b[2J\x1b[4;5H\x1b_Ga=p,i=1,p=1,c=2,r=2,C=1\x1b\\");
    // Scroll the first placement before placing a second at the new cursor.
    terminal.feed_output(b"\x1b[24;1H\n\x1b[8;10H\x1b_Ga=p,i=1,p=2,c=2,r=1\x1b\\\x1b[?2026l");
    let placements = terminal.kitty_graphics_placements();
    assert_eq!(placements.len(), 2);
    let first = placements.iter().find(|p| p.placement_id == 1).unwrap();
    let second = placements.iter().find(|p| p.placement_id == 2).unwrap();
    assert_eq!((first.col, first.viewport_row), (4, 2));
    assert_eq!((second.col, second.viewport_row), (9, 7));
    assert_eq!(terminal.cursor_position(), (11, 8));
}

#[test]
fn margin_scrolling_clips_the_image_and_keeps_the_footer_fixed() {
    let terminal = terminal();
    terminal
        .feed_output(b"\x1b[?1049h\x1b[5;5H\x1b_Ga=T,i=1,f=32,s=1,v=1,c=2,r=3,C=1;AQID/w==\x1b\\");
    terminal.feed_output(b"\x1b[15;5H\x1b_Ga=p,i=1,p=2,c=2,r=2,C=1\x1b\\");
    terminal.feed_output(b"\x1b[5;10r\x1b[10;1H\n");
    let placements = terminal.kitty_graphics_placements();
    assert_eq!(placements.len(), 2);
    let image = placements.iter().find(|p| p.placement_id == 0).unwrap();
    assert_eq!((image.viewport_row, image.clip_top_rows), (3, 1));
    assert_eq!(
        placements
            .iter()
            .find(|p| p.placement_id == 2)
            .unwrap()
            .viewport_row,
        14
    );
    terminal.feed_output(b"\x1b[5;1H\x1bM");
    let image = terminal
        .kitty_graphics_placements()
        .into_iter()
        .find(|p| p.placement_id == 0)
        .unwrap();
    assert_eq!((image.viewport_row, image.clip_top_rows), (4, 1));
}

#[test]
fn deleting_at_a_virtual_parents_position_removes_its_relative_child_only() {
    let terminal = terminal();
    terminal.feed_output(b"\x1b_Ga=T,i=1,p=3,U=1,c=2,r=1,f=32,s=1,v=1;AQID/w==\x1b\\");
    terminal.feed_output(b"\x1b_Ga=T,i=2,p=4,P=1,Q=3,H=1,c=2,r=1,f=32,s=1,v=1;AQID/w==\x1b\\");
    terminal.feed_output("\x1b[4;5H\x1b[38;2;0;0;1m\u{10eeee}\u{305}\u{305}\x1b[0m".as_bytes());
    let placements = terminal.kitty_graphics_placements();
    let child = placements.iter().find(|p| p.image_id == 2).unwrap();
    assert_eq!((child.viewport_row, child.col), (3, 5));
    terminal.feed_output(b"\x1b_Ga=d,d=p,x=6,y=4\x1b\\");
    let placements = terminal.kitty_graphics_placements();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].image_id, 1);
}
