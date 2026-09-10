use super::*;
use std::io::Write as _;

fn size() -> TerminalSize {
    TerminalSize {
        cols: 80,
        rows: 24,
        cell_width: 10.0,
        cell_height: 20.0,
    }
}

fn command(control: &str, payload: &[u8]) -> KittyGraphicsCommand {
    KittyGraphicsCommand::parse(
        [control.as_bytes(), b";", BASE64.encode(payload).as_bytes()].concat(),
        false,
    )
}

fn one_pixel_png() -> Vec<u8> {
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[255, 0, 0, 255])
            .unwrap();
    }
    png
}

#[test]
fn interceptor_removes_kitty_apc_and_preserves_text() {
    let png = one_pixel_png();
    let sequence = format!(
        "before\x1b_Ga=T,f=100,i=7;{}\x1b\\after",
        BASE64.encode(png)
    );
    let mut interceptor = KittyGraphicsInterceptor::default();
    let items = interceptor.process(sequence.as_bytes());
    assert!(matches!(&items[0], KittyGraphicsItem::Text(text) if text == b"before"));
    assert!(matches!(&items[1], KittyGraphicsItem::Command(_)));
    assert!(matches!(&items[2], KittyGraphicsItem::Text(text) if text == b"after"));
}

#[test]
fn interceptor_handles_sequence_split_across_reads() {
    let mut interceptor = KittyGraphicsInterceptor::default();
    assert!(
        matches!(interceptor.process(b"x\x1b_").as_slice(), [KittyGraphicsItem::Text(text)] if text == b"x")
    );
    assert!(
        interceptor
            .process(b"Ga=T,f=32,s=1,v=1,i=9;//AA")
            .is_empty()
    );
    let items = interceptor.process(b"/w==\x1b\\y");
    assert!(matches!(&items[0], KittyGraphicsItem::Command(_)));
    assert!(matches!(&items[1], KittyGraphicsItem::Text(text) if text == b"y"));
}

#[test]
fn interceptor_preserves_non_kitty_apc_sequences() {
    let mut interceptor = KittyGraphicsInterceptor::default();
    let input = b"a\x1b_not-kitty\x1b\\b";
    let items = interceptor.process(input);
    assert!(matches!(items.as_slice(), [KittyGraphicsItem::Text(text)] if text == input));
}

#[test]
fn uploads_raw_rgba_and_places_at_cursor() {
    let mut state = KittyGraphicsState::default();
    let result = state.apply(
        command("a=T,f=32,s=1,v=1,i=42,c=2,r=3", &[1, 2, 3, 255]),
        4,
        5,
        10,
        size(),
    );
    assert!(result.changed);
    assert_eq!(result.cursor_advance, Some((2, 3)));
    assert_eq!(result.response.unwrap(), b"\x1b_Gi=42;OK\x1b\\");
    let placements = state.render_placements(10, 0, 24, 80);
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].placement_serial, 1);
    assert_eq!(placements[0].viewport_row, 5);
    assert_eq!(placements[0].col, 4);
    assert_eq!(placements[0].occupied_cols, 2);
    assert_eq!(placements[0].occupied_rows, 3);
    assert!(placements[0].image.png().starts_with(b"\x89PNG"));
}

#[test]
fn natural_size_uses_cell_metrics_for_occupancy() {
    // 100×40 px image, 10×20 cell → 10 cols × 2 rows at 1:1.
    let pixels = vec![0u8; 100 * 40 * 4];
    let mut state = KittyGraphicsState::default();
    let result = state.apply(
        command("a=T,f=32,s=100,v=40,i=50,q=1,C=0", &pixels),
        0,
        0,
        0,
        size(),
    );
    assert!(result.changed);
    assert_eq!(result.cursor_advance, Some((10, 2)));
    let placements = state.render_placements(0, 0, 24, 80);
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].occupied_cols, 10);
    assert_eq!(placements[0].occupied_rows, 2);
    assert_eq!(placements[0].display_cols, None);
    assert_eq!(placements[0].display_rows, None);
    assert_eq!(placements[0].source_width, 100);
    assert_eq!(placements[0].source_height, 40);
}

#[test]
fn natural_size_truncates_to_available_width_not_scale() {
    // 500×20 px image at col 70 on an 80-col grid with 10px cells:
    // only 10 cols / 100px remain → truncate source width to 100, occupy 10.
    let pixels = vec![0u8; 500 * 20 * 4];
    let mut state = KittyGraphicsState::default();
    let result = state.apply(
        command("a=T,f=32,s=500,v=20,i=51,q=1,C=0", &pixels),
        70,
        0,
        0,
        size(),
    );
    assert!(result.changed);
    assert_eq!(result.cursor_advance, Some((10, 1)));
    let placements = state.render_placements(0, 0, 24, 80);
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].col, 70);
    assert_eq!(placements[0].occupied_cols, 10);
    assert_eq!(placements[0].occupied_rows, 1);
    assert_eq!(
        placements[0].source_width, 100,
        "right-edge truncation must crop source width, not scale the whole image"
    );
    assert_eq!(placements[0].source_height, 20);
}

#[test]
fn explicit_cell_size_is_not_truncated_to_available_width() {
    // Client-requested c/r is authoritative (Grok fit_image_to_cells path).
    let pixels = vec![0u8; 500 * 20 * 4];
    let mut state = KittyGraphicsState::default();
    let result = state.apply(
        command("a=T,f=32,s=500,v=20,i=52,c=40,r=4,q=1,C=1", &pixels),
        70,
        0,
        0,
        size(),
    );
    assert!(result.changed);
    assert_eq!(result.cursor_advance, None);
    let placements = state.render_placements(0, 0, 24, 80);
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].occupied_cols, 40);
    assert_eq!(placements[0].occupied_rows, 4);
    assert_eq!(placements[0].source_width, 500);
}

#[test]
fn place_with_only_cols_derives_rows_from_aspect_ratio() {
    let pixels = vec![0u8; 100 * 40 * 4];
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=t,f=32,s=100,v=40,i=53,q=1", &pixels),
        0,
        0,
        0,
        size(),
    );
    let result = state.apply(command("a=p,i=53,c=20,q=1,C=1", &[]), 0, 0, 0, size());
    assert!(result.changed);
    let placements = state.render_placements(0, 0, 24, 80);
    assert_eq!(placements.len(), 1);
    // width 20 cols = 200px; height keeps aspect → 80px → 4 rows.
    assert_eq!(placements[0].occupied_cols, 20);
    assert_eq!(placements[0].occupied_rows, 4);
}

#[test]
fn each_placement_gets_a_stable_monotonic_serial() {
    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    state.apply(command("a=t,f=100,i=3,q=1", &png), 0, 0, 0, size());
    state.apply(command("a=p,i=3,q=1", &[]), 0, 0, 0, size());
    state.apply(command("a=p,i=3,q=1", &[]), 2, 0, 0, size());

    let placements = state.render_placements(0, 0, 24, 80);
    assert_eq!(placements.len(), 2);
    assert_eq!(placements[0].placement_serial, 1);
    assert_eq!(placements[1].placement_serial, 2);
}

#[test]
fn grok_virtual_placement_is_accepted_without_moving_the_cursor() {
    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    state.apply(command("a=t,f=100,i=42,q=1", &png), 0, 0, 0, size());

    let result = state.apply(
        command("a=p,U=1,i=42,p=7,c=1,r=1,q=1", &[]),
        60,
        20,
        0,
        size(),
    );

    assert!(result.changed, "the virtual placement must be registered");
    assert_eq!(result.cursor_advance, None);
    assert_eq!(state.placements.len(), 1);
}

#[test]
fn grok_relative_placement_uses_parent_origin_and_signed_offset() {
    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    state.apply(command("a=t,f=100,i=41,q=1", &png), 0, 0, 0, size());
    state.apply(command("a=t,f=100,i=42,q=1", &png), 0, 0, 0, size());
    state.apply(
        command("a=p,i=41,p=7,c=1,r=1,C=1,q=1", &[]),
        4,
        5,
        0,
        size(),
    );

    let result = state.apply(
        command("a=p,i=42,p=8,P=41,Q=7,H=3,V=-2,c=2,r=2,q=1", &[]),
        70,
        20,
        0,
        size(),
    );

    assert!(result.changed);
    assert_eq!(
        result.cursor_advance, None,
        "relative placements never move the cursor"
    );
    let placement = state
        .render_placements(0, 0, 24, 80)
        .into_iter()
        .find(|placement| placement.image_id == 42)
        .expect("relative child must be visible");
    assert_eq!((placement.viewport_row, placement.col), (3, 7));
}

#[test]
fn grok_relative_placement_tracks_and_clears_with_unicode_placeholder() {
    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    state.apply(command("a=t,f=100,i=41,q=1", &png), 0, 0, 0, size());
    state.apply(command("a=t,f=100,i=42,q=1", &png), 0, 0, 0, size());
    state.apply(
        command("a=p,U=1,i=41,p=7,c=1,r=1,C=1,q=1", &[]),
        70,
        20,
        0,
        size(),
    );
    state.apply(
        command("a=p,i=42,p=8,P=41,Q=7,H=3,V=-2,c=2,r=2,q=1", &[]),
        70,
        20,
        0,
        size(),
    );
    let placeholders = [KittyGraphicsPlaceholder {
        viewport_row: 6,
        col: 4,
        image_id_low: 41,
        image_id_high: 0,
        image_id: 41,
        placement_id: 7,
        image_row: 0,
        image_col: 0,
    }];

    let child = state
        .render_placements_on_screen_with_placeholders(
            0,
            0,
            24,
            80,
            KittyGraphicsScreen::Primary,
            &placeholders,
        )
        .into_iter()
        .find(|placement| placement.image_id == 42)
        .expect("the child should follow the virtual placeholder");
    assert_eq!((child.viewport_row, child.col), (4, 7));
    assert!(
        state
            .render_placements_on_screen_with_placeholders(
                0,
                0,
                24,
                80,
                KittyGraphicsScreen::Primary,
                &[],
            )
            .is_empty(),
        "overwriting the placeholder must clear its image and relative children"
    );
}

#[test]
fn grok_deleting_parent_removes_relative_descendants() {
    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    state.apply(command("a=t,f=100,i=41,q=1", &png), 0, 0, 0, size());
    state.apply(command("a=t,f=100,i=42,q=1", &png), 0, 0, 0, size());
    state.apply(
        command("a=p,i=41,p=7,c=1,r=1,C=1,q=1", &[]),
        4,
        5,
        0,
        size(),
    );
    state.apply(
        command("a=p,i=42,p=8,P=41,Q=7,c=2,r=2,q=1", &[]),
        70,
        20,
        0,
        size(),
    );

    state.apply(command("a=d,d=i,i=41,p=7,q=1", &[]), 0, 0, 0, size());

    assert!(
        state
            .placements
            .iter()
            .all(|placement| placement.image_id != 42),
        "relative descendants must share their parent's lifetime"
    );
}

#[test]
fn grok_anonymous_placement_churn_has_a_hard_bound() {
    const EXPECTED_MAX_PLACEMENTS: usize = 4_096;

    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    state.apply(command("a=t,f=100,i=42,q=1", &png), 0, 0, 0, size());
    for _ in 0..=EXPECTED_MAX_PLACEMENTS {
        state.apply(command("a=p,i=42,c=1,r=1,C=1,q=1", &[]), 0, 0, 0, size());
    }

    let mut samples = Vec::with_capacity(50);
    let mut rendered_count = 0;
    for _ in 0..50 {
        let started = std::time::Instant::now();
        rendered_count = state.render_placements(0, 0, 24, 80).len();
        samples.push(started.elapsed().as_micros());
    }
    samples.sort_unstable();
    eprintln!(
        "grok churn baseline: stored={} rendered={} snapshot_p50={}us snapshot_p95={}us",
        state.placements.len(),
        rendered_count,
        samples[25],
        samples[47],
    );

    assert!(
        state.placements.len() <= EXPECTED_MAX_PLACEMENTS,
        "a redraw loop must not grow placement work without bound; stored {} placements",
        state.placements.len(),
    );
}

#[test]
fn clearing_a_viewport_preserves_scrollback_and_other_screens() {
    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=T,f=100,i=3,c=2,r=2,C=1,q=1", &png),
        0,
        0,
        0,
        size(),
    );
    state.apply(command("a=p,i=3,c=2,r=2,C=1,q=1", &[]), 0, 1, 10, size());
    state.apply_on_screen(
        command("a=p,i=3,c=2,r=2,C=1,q=1", &[]),
        0,
        0,
        0,
        size(),
        KittyGraphicsScreen::Alternate,
    );

    assert!(state.clear_viewport_on_screen(KittyGraphicsScreen::Primary, 10, 24, 80,));
    assert!(
        state
            .render_placements_on_screen(10, 0, 24, 80, KittyGraphicsScreen::Primary)
            .is_empty()
    );
    assert_eq!(
        state
            .render_placements_on_screen(10, 10, 24, 80, KittyGraphicsScreen::Primary)
            .len(),
        1,
        "placements already in scrollback must survive ED2"
    );
    assert_eq!(
        state
            .render_placements_on_screen(0, 0, 24, 80, KittyGraphicsScreen::Alternate)
            .len(),
        1,
        "clearing the primary viewport must not affect the alternate screen"
    );
}

#[test]
fn assembles_chunked_upload_before_displaying() {
    let png = one_pixel_png();
    let split = png.len() / 2;
    let first = command("a=T,f=100,i=8,m=1", &png[..split]);
    let second = command("m=0", &png[split..]);
    let mut state = KittyGraphicsState::default();
    assert!(!state.apply(first, 0, 0, 0, size()).changed);
    assert!(state.render_placements(0, 0, 24, 80).is_empty());
    assert!(state.apply(second, 0, 0, 0, size()).changed);
    assert_eq!(state.render_placements(0, 0, 24, 80).len(), 1);
}

#[test]
fn chunked_upload_places_on_the_screen_and_cursor_at_completion() {
    let png = one_pixel_png();
    let split = png.len() / 2;
    let mut state = KittyGraphicsState::default();
    state.apply_on_screen(
        command("a=T,f=100,i=88,c=2,r=3,m=1", &png[..split]),
        4,
        5,
        10,
        size(),
        KittyGraphicsScreen::Primary,
    );
    let result = state.apply_on_screen(
        command("m=0", &png[split..]),
        0,
        0,
        0,
        size(),
        KittyGraphicsScreen::Alternate,
    );
    assert!(result.changed);
    assert_eq!(result.cursor_advance, Some((2, 3)));
    assert_eq!(
        result.cursor_advance_screen,
        Some(KittyGraphicsScreen::Alternate)
    );
    assert!(
        state
            .render_placements_on_screen(10, 0, 24, 80, KittyGraphicsScreen::Primary)
            .is_empty()
    );
    let alternate = state.render_placements_on_screen(0, 0, 24, 80, KittyGraphicsScreen::Alternate);
    assert_eq!(alternate.len(), 1);
    assert_eq!((alternate[0].col, alternate[0].viewport_row), (0, 0));
}

#[test]
fn rejects_truncated_png_after_valid_header() {
    let mut png = one_pixel_png();
    png.truncate(png.len() - 16);
    let mut state = KittyGraphicsState::default();

    let result = state.apply(command("a=T,f=100,i=9,q=1", &png), 0, 0, 0, size());

    assert!(!result.changed);
    assert_eq!(
        result.response.unwrap(),
        b"\x1b_Gi=9;EINVAL:invalid PNG image\x1b\\"
    );
    assert!(state.render_placements(0, 0, 24, 80).is_empty());
}

#[test]
fn delete_by_image_and_placement_id_is_precise() {
    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    state.apply(command("a=t,f=100,i=3,q=1", &png), 0, 0, 0, size());
    state.apply(command("a=p,i=3,p=1,q=1", &[]), 0, 0, 0, size());
    state.apply(command("a=p,i=3,p=2,q=1", &[]), 2, 0, 0, size());
    state.apply(command("a=d,d=i,i=3,p=1,q=1", &[]), 0, 0, 0, size());
    let placements = state.render_placements(0, 0, 24, 80);
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].placement_id, 2);
}

#[test]
fn quiet_mode_suppresses_success_but_not_errors_at_level_one() {
    let mut state = KittyGraphicsState::default();
    let success = state.apply(
        command("a=T,f=32,s=1,v=1,i=1,q=1", &[0, 0, 0, 0]),
        0,
        0,
        0,
        size(),
    );
    assert!(success.response.is_none());
    let failure = state.apply(command("a=p,i=999,q=1", &[]), 0, 0, 0, size());
    assert!(failure.response.is_some());
}

#[test]
fn accepts_zlib_compressed_raw_pixels() {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&[12, 34, 56, 255]).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut state = KittyGraphicsState::default();

    let result = state.apply(
        command("a=T,f=32,s=1,v=1,o=z,i=12,q=1", &compressed),
        0,
        0,
        0,
        size(),
    );

    assert!(result.changed);
    assert_eq!(state.render_placements(0, 0, 24, 80).len(), 1);
}

#[test]
fn reads_and_removes_safe_temporary_file_transfers() {
    let png = one_pixel_png();
    let mut file = tempfile::Builder::new()
        .prefix("tty-graphics-protocol-")
        .tempfile()
        .unwrap();
    file.write_all(&png).unwrap();
    file.flush().unwrap();
    let path = file.path().to_path_buf();
    let mut state = KittyGraphicsState::default();

    let result = state.apply(
        command("a=T,f=100,t=t,i=13,q=1", path.to_string_lossy().as_bytes()),
        0,
        0,
        0,
        size(),
    );

    assert!(result.changed);
    assert!(!path.exists());
    assert_eq!(state.render_placements(0, 0, 24, 80).len(), 1);
}
