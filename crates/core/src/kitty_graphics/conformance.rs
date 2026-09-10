use super::*;

fn size() -> TerminalSize {
    TerminalSize {
        cols: 80,
        rows: 24,
        cell_width: 10.0,
        cell_height: 20.0,
    }
}

fn command(control: &str, bytes: &[u8]) -> KittyGraphicsCommand {
    KittyGraphicsCommand::parse(
        [control.as_bytes(), b";", BASE64.encode(bytes).as_bytes()].concat(),
        false,
    )
}

#[test]
fn final_chunk_uses_current_cursor_and_screen() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=T,f=32,s=1,v=1,i=71,m=1", &[1, 2, 3]),
        2,
        3,
        0,
        size(),
    );
    state.apply_on_screen(
        command("m=0", &[255]),
        9,
        7,
        0,
        size(),
        KittyGraphicsScreen::Alternate,
    );
    let placed = state.render_placements_on_screen(0, 0, 24, 80, KittyGraphicsScreen::Alternate);
    assert_eq!(placed.len(), 1);
    assert_eq!((placed[0].col, placed[0].viewport_row), (9, 7));
}

#[test]
fn utf8_continuation_bytes_never_start_an_apc() {
    let input = "🟢Ghello 🟢Gworld";
    let mut parser = KittyGraphicsInterceptor::default();
    let mut output = Vec::new();
    for byte in input.bytes() {
        for item in parser.process(&[byte]) {
            match item {
                KittyGraphicsItem::Text(bytes) => output.extend(bytes),
                KittyGraphicsItem::Command(_) => panic!("UTF-8 text became a command"),
            }
        }
    }
    assert_eq!(output, input.as_bytes());
    assert!(matches!(
        parser.process(b"\x1b_Ga=d\x1b\\").as_slice(),
        [KittyGraphicsItem::Command(_)]
    ));
}

#[test]
fn failed_replacement_preserves_the_previous_image_and_placement() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=T,i=1,f=32,s=1,v=1,C=1", &[1, 2, 3, 255]),
        2,
        3,
        0,
        size(),
    );
    let generation = state.images[&1].generation;
    let failed = state.apply(
        command("a=T,i=1,f=32,s=1,v=1,w=9,x=9", &[9, 8, 7, 255]),
        5,
        6,
        0,
        size(),
    );
    assert!(!failed.changed);
    let placed = state.render_placements(0, 0, 24, 80);
    assert_eq!((placed[0].col, placed[0].viewport_row), (2, 3));
    assert_eq!(placed[0].image_generation, generation);
    assert_eq!(placed[0].image.rgba(), Some([1, 2, 3, 255].as_slice()));
}

fn placeholder(row: i64, col: usize, image_col: u32) -> KittyGraphicsPlaceholder {
    KittyGraphicsPlaceholder {
        viewport_row: row,
        col,
        image_id_low: 1,
        image_id_high: 0,
        image_id: 1,
        placement_id: 3,
        image_row: 0,
        image_col,
    }
}

#[test]
fn unicode_placeholders_allow_holes_and_multiple_instances() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=T,i=1,p=3,U=1,c=3,r=1,f=32,s=3,v=1", &[255; 12]),
        0,
        0,
        0,
        size(),
    );
    let cells = [
        placeholder(2, 4, 0),
        placeholder(2, 6, 2),
        placeholder(5, 10, 0),
    ];
    let placed = state.render_placements_on_screen_with_placeholders(
        0,
        0,
        24,
        80,
        KittyGraphicsScreen::Primary,
        &cells,
    );
    assert_eq!(placed.len(), 3);
    assert_eq!(
        placed
            .iter()
            .map(|p| (p.viewport_row, p.col, p.virtual_cell))
            .collect::<Vec<_>>(),
        vec![
            (2, 4, Some((0, 0))),
            (2, 6, Some((2, 0))),
            (5, 10, Some((0, 0)))
        ]
    );
    assert!(
        placed
            .iter()
            .all(|p| p.occupied_cols == 1 && p.occupied_rows == 1)
    );
    assert!(Arc::ptr_eq(&placed[0].image, &placed[2].image));
}

#[test]
fn negative_relative_origins_are_clipped_without_moving_the_image() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=T,i=1,p=1,f=32,s=1,v=1,C=1", &[255; 4]),
        0,
        2,
        0,
        size(),
    );
    state.apply(
        command("a=T,i=2,p=1,P=1,Q=1,H=-1,c=3,r=1,f=32,s=3,v=1", &[255; 12]),
        8,
        8,
        0,
        size(),
    );
    let placed = state.render_placements(0, 0, 24, 80);
    let child = placed.iter().find(|p| p.image_id == 2).unwrap();
    assert_eq!(
        (child.col, child.col_offset, child.viewport_row),
        (0, -1, 2)
    );
}

#[test]
fn animation_upload_control_composition_and_deletion_share_pixels() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command(
            "a=T,i=1,f=32,s=2,v=1,C=1",
            &[0, 0, 255, 255, 0, 0, 255, 255],
        ),
        0,
        0,
        0,
        size(),
    );
    let frame = state.apply(
        command("a=f,i=1,f=32,s=1,v=1,c=1,x=1,z=40", &[255, 0, 0, 128]),
        0,
        0,
        0,
        size(),
    );
    assert!(frame.changed);
    assert!(
        state
            .apply(command("a=a,i=1,c=2", &[]), 0, 0, 0, size())
            .changed
    );
    assert_eq!(
        state.images[&1].image.rgba().unwrap(),
        &[0, 0, 255, 255, 128, 0, 127, 255]
    );
    assert!(
        state
            .apply(
                command("a=c,i=1,r=2,c=1,X=1,x=0,w=1,h=1,C=1", &[]),
                0,
                0,
                0,
                size()
            )
            .changed
    );
    state.apply(command("a=a,i=1,c=1", &[]), 0, 0, 0, size());
    assert_eq!(
        state.images[&1].image.rgba().unwrap(),
        &[128, 0, 127, 255, 0, 0, 255, 255]
    );
    let before = state.stored_bytes;
    assert!(
        state
            .apply(command("a=d,d=f,i=1,r=2", &[]), 0, 0, 0, size())
            .changed
    );
    assert_eq!(state.stored_bytes, before / 2);
    assert_eq!(state.images[&1].animation.as_ref().unwrap().frames.len(), 1);
}

#[test]
fn frame_uploads_can_be_chunked_and_overlapping_composition_is_rejected() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=T,i=1,f=32,s=1,v=1,C=1", &[255; 4]),
        0,
        0,
        0,
        size(),
    );
    state.apply(
        command("a=f,i=1,f=32,s=1,v=1,m=1", &[1, 2, 3]),
        0,
        0,
        0,
        size(),
    );
    assert!(
        state
            .apply(command("a=f,m=0", &[255]), 0, 0, 0, size())
            .changed
    );
    let result = state.apply(command("a=c,i=1,r=1,c=1", &[]), 0, 0, 0, size());
    assert!(!result.changed);
    assert!(
        String::from_utf8(result.response.unwrap())
            .unwrap()
            .contains("EINVAL")
    );
}

#[test]
fn scrolling_margins_clip_images_and_leave_crossing_images_fixed() {
    let mut state = KittyGraphicsState::default();
    for (id, row, height) in [(1, 2, 3), (2, 1, 3), (3, 7, 1)] {
        state.apply(
            command(
                &format!("a=T,i={id},f=32,s=1,v=1,c=1,r={height},C=1"),
                &[255; 4],
            ),
            0,
            row,
            0,
            size(),
        );
    }
    assert!(state.scroll_region_on_screen(KittyGraphicsScreen::Primary, 2, 6, 1, 0));
    let placed = state.render_placements(0, 0, 24, 80);
    assert_eq!((placed[0].viewport_row, placed[0].clip_top_rows), (1, 1));
    assert_eq!(placed[1].viewport_row, 1);
    assert_eq!(placed[2].viewport_row, 7);
    assert!(state.scroll_region_on_screen(KittyGraphicsScreen::Primary, 2, 6, -1, 0));
    assert_eq!(state.render_placements(0, 0, 24, 80)[0].clip_top_rows, 1);
}

#[test]
fn font_resize_recomputes_natural_image_occupancy() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=T,i=1,f=32,s=13,v=21,C=1", &[255; 13 * 21 * 4]),
        0,
        0,
        0,
        size(),
    );
    assert_eq!(state.render_placements(0, 0, 24, 80)[0].occupied_rows, 2);
    state.resize(TerminalSize {
        cell_width: 5.0,
        cell_height: 7.0,
        ..size()
    });
    let placed = state.render_placements(0, 0, 24, 80);
    assert_eq!((placed[0].occupied_cols, placed[0].occupied_rows), (3, 3));
    assert_eq!(
        (placed[0].display_cols, placed[0].display_rows),
        (None, None)
    );
}

#[test]
fn image_number_order_is_not_changed_by_editing_an_older_animation() {
    let mut state = KittyGraphicsState::default();
    state.apply(command("a=t,I=7,f=32,s=1,v=1", &[255; 4]), 0, 0, 0, size());
    let older = state.resolve_image_id(&command("I=7", &[])).unwrap();
    state.apply(command("a=t,I=7,f=32,s=1,v=1", &[0; 4]), 0, 0, 0, size());
    let newer = state.resolve_image_id(&command("I=7", &[])).unwrap();
    state.apply(
        command(&format!("a=f,i={older},f=32,s=1,v=1"), &[3; 4]),
        0,
        0,
        0,
        size(),
    );
    assert_eq!(state.resolve_image_id(&command("I=7", &[])), Some(newer));
}

#[test]
fn image_numbers_allocate_distinct_ids_and_put_the_newest() {
    let mut state = KittyGraphicsState::default();
    let a = state.apply(
        command("a=t,f=32,s=1,v=1,I=77", &[1, 2, 3, 255]),
        0,
        0,
        0,
        size(),
    );
    let b = state.apply(
        command("a=t,f=32,s=1,v=1,I=77", &[3, 2, 1, 255]),
        0,
        0,
        0,
        size(),
    );
    assert_ne!(a.response, b.response);
    let result = state.apply(command("a=p,I=77", &[]), 4, 5, 0, size());
    assert!(result.changed);
    assert_eq!(state.render_placements(0, 0, 24, 80).len(), 1);
}

#[test]
fn deleting_one_image_does_not_free_other_unplaced_uploads() {
    let mut state = KittyGraphicsState::default();
    for id in [1, 2] {
        state.apply(
            command(&format!("a=t,f=32,s=1,v=1,i={id}"), &[1, 2, 3, 255]),
            0,
            0,
            0,
            size(),
        );
    }
    state.apply(command("a=d,d=I,i=1", &[]), 0, 0, 0, size());
    assert!(
        state
            .apply(command("a=p,i=2", &[]), 0, 0, 0, size())
            .changed
    );
}

#[test]
fn measure_raw_upload_cost() {
    let mut state = KittyGraphicsState::default();
    let bytes: Vec<_> = (0..1024 * 1024 * 4)
        .map(|i| ((i * 73 + i / 1024) % 256) as u8)
        .collect();
    let commands: Vec<_> = (0..8)
        .map(|_| command("a=T,f=32,s=1024,v=1024,i=1,C=1,q=2", &bytes))
        .collect();
    let start = std::time::Instant::now();
    for command in commands {
        assert!(state.apply(command, 0, 0, 0, size()).changed);
        std::hint::black_box(state.render_placements(0, 0, 24, 80));
    }
    eprintln!(
        "kitty upload benchmark: 8 x 1024x1024 RGBA: {:?}",
        start.elapsed()
    );
}
