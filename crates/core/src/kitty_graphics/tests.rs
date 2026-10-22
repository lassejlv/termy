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
fn chunks_decode_with_their_own_padding_and_trailing_bits() {
    // chafa's shape: an empty header, then chunks base64-encoded one by one,
    // padded and with the unused bits of their last symbol set.
    let mut state = KittyGraphicsState::default();
    for chunk in [
        &b"a=T,f=32,s=2,v=1,i=9,m=1"[..],
        b"m=1;AQIDBP==",
        b"m=1;BQb=",
        b"m=1;B/8",
        b"m=0",
    ] {
        let result = state.apply(
            KittyGraphicsCommand::parse(chunk.to_vec(), false),
            0,
            0,
            0,
            size(),
        );
        assert!(!reply(&result).contains("EINVAL"), "{}", reply(&result));
    }
    assert_eq!(state.render_placements(0, 0, 24, 80).len(), 1);
}

#[test]
fn single_chunk_payload_ignores_trailing_bits_and_missing_padding() {
    let mut state = KittyGraphicsState::default();
    let upload = state.apply(
        KittyGraphicsCommand::parse(b"a=T,f=32,s=1,v=1,i=9;AQIDBP".to_vec(), false),
        0,
        0,
        0,
        size(),
    );
    assert!(upload.changed, "{}", reply(&upload));
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
fn a_new_transmission_abandons_an_unfinished_chunked_upload() {
    let mut state = KittyGraphicsState::default();
    let first = state.apply(
        command("a=t,f=32,s=1,v=1,i=1,m=1", &[1, 2]),
        0,
        0,
        0,
        size(),
    );
    assert!(first.response.is_none());
    let next = state.apply(
        command("a=T,f=32,s=1,v=1,i=2", &[12, 34, 56, 255]),
        0,
        0,
        0,
        size(),
    );
    assert!(next.changed);
    assert_eq!(state.render_placements(0, 0, 24, 80).len(), 1);
    // The abandoned upload is gone: a continuation now starts nothing.
    assert!(
        !state
            .apply(command("m=0", &[3, 4]), 0, 0, 0, size())
            .changed
    );
}

#[test]
fn zlib_inflation_stops_at_the_declared_raw_size() {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&vec![0; 16 * 1024 * 1024]).unwrap();
    let compressed = encoder.finish().unwrap();
    assert!(decompress_zlib(&compressed, Some(4)).is_err());
    let mut state = KittyGraphicsState::default();
    let result = state.apply(
        command("a=T,f=32,s=1,v=1,o=z,i=12", &compressed),
        0,
        0,
        0,
        size(),
    );
    assert!(!result.changed);
    assert!(
        String::from_utf8_lossy(&result.response.unwrap()).contains("EINVAL:pixel data length")
    );
}

#[test]
fn temporary_file_marker_must_be_in_the_resolved_file_name() {
    let marker_dir = tempfile::Builder::new()
        .prefix("tty-graphics-protocol-")
        .tempdir()
        .unwrap();
    let victim = tempfile::NamedTempFile::new_in(marker_dir.path().parent().unwrap()).unwrap();
    let escape = marker_dir
        .path()
        .join("..")
        .join(victim.path().file_name().unwrap());
    assert!(escape.exists());
    assert!(!temporary_path_can_be_removed(&escape));
    #[cfg(target_os = "linux")]
    assert!(upload::is_pseudo_file(std::path::Path::new(
        "/proc/self/status"
    )));
}

#[cfg(target_os = "linux")]
#[test]
fn file_swapped_for_a_pseudo_file_link_after_open_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("image.png");
    std::fs::write(&path, one_pixel_png()).unwrap();
    // Kept open so no later file can reuse its inode.
    let file = std::fs::File::open(&path).unwrap();
    let opened = file.metadata().unwrap();
    assert!(upload::opened_file_is_allowed(&path, &opened));

    // A link to a pseudo-file, or to any file but the opened one, fails.
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("/proc/self/status", &path).unwrap();
    assert!(!upload::opened_file_is_allowed(&path, &opened));
    let other = dir.path().join("other.png");
    std::fs::write(&other, one_pixel_png()).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&other, &path).unwrap();
    assert!(!upload::opened_file_is_allowed(&path, &opened));
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

#[test]
fn virtual_placement_presence_follows_screen_replacement_deletion_and_reset() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=t,f=32,s=1,v=1,i=1,q=2", &[1, 2, 3, 255]),
        0,
        0,
        0,
        size(),
    );
    let primary = KittyGraphicsScreen::Primary;
    let alternate = KittyGraphicsScreen::Alternate;
    let assert_presence = |state: &KittyGraphicsState, primary_present, alternate_present| {
        assert_eq!(
            state.has_virtual_placements_on_screen(primary),
            primary_present
        );
        assert_eq!(
            state.has_virtual_placements_on_screen(alternate),
            alternate_present
        );
        assert_eq!(
            state.has_virtual_placements(),
            primary_present || alternate_present
        );
    };
    for screen in [primary, alternate] {
        state.apply_on_screen(command("a=p,i=1,p=1,U=1,q=2", &[]), 0, 0, 0, size(), screen);
    }
    assert_presence(&state, true, true);
    // Replacing the same ID on one screen must not affect the other screen.
    state.apply(command("a=p,i=1,p=1,C=1,q=2", &[]), 0, 0, 0, size());
    assert_presence(&state, false, true);
    state.apply(command("a=p,i=1,p=2,U=1,q=2", &[]), 0, 0, 0, size());
    state.apply(command("a=p,i=1,p=3,U=1,q=2", &[]), 0, 0, 0, size());
    state.apply(command("a=d,d=i,i=1,p=2,q=2", &[]), 0, 0, 0, size());
    assert_presence(&state, true, true);
    // Viewport clears and scrolling leave virtual prototypes available.
    state.clear_visible_on_screen(primary);
    state.scroll_up_without_history_on_screen(100, primary);
    assert_presence(&state, true, true);
    state.apply(command("a=d,d=I,i=1,q=2", &[]), 0, 0, 0, size());
    assert_presence(&state, false, false);
    state.apply(
        command("a=T,f=32,s=1,v=1,i=2,U=1,q=2", &[1, 2, 3, 255]),
        0,
        0,
        0,
        size(),
    );
    assert_presence(&state, true, false);
    state.reset();
    assert_presence(&state, false, false);
}

#[test]
fn evicted_virtual_placements_stop_requiring_placeholder_scans() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=T,f=32,s=1,v=1,i=1,U=1,q=2", &[1, 2, 3, 255]),
        0,
        0,
        0,
        size(),
    );
    assert!(state.has_virtual_placements());
    for _ in 0..MAX_PLACEMENTS {
        state.apply(command("a=p,i=1,C=1,q=2", &[]), 0, 0, 0, size());
    }
    assert!(state.has_placements());
    assert!(!state.has_virtual_placements());
    // Exercise image eviction too, as that uses a different removal path.
    state.apply(command("a=p,i=1,p=1,U=1,q=2", &[]), 0, 0, 0, size());
    assert!(state.has_virtual_placements());
    state.remove_image(1);
    assert!(!state.has_virtual_placements());
    assert!(!state.has_placements());
}

#[test]
fn id_less_images_are_freed_with_their_last_placement() {
    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    state.apply(command("a=T,f=100,c=1,r=1,q=1", &png), 0, 0, 0, size());
    state.apply(command("a=T,f=100,i=5,c=1,r=1,q=1", &png), 0, 1, 0, size());
    state.apply(command("a=T,f=100,I=6,c=1,r=1,q=1", &png), 0, 2, 0, size());
    assert_eq!((state.images.len(), state.anonymous_images), (3, 1));
    assert!(state.clear_visible());
    // Images a later command can still name survive their placements.
    assert_eq!(state.images.len(), 2);
    assert!(state.images.values().all(|image| !image.anonymous));
    assert_eq!(state.anonymous_images, 0);
    let expected: usize = state.images.values().map(StoredImage::byte_len).sum();
    assert_eq!(state.stored_bytes, expected);

    state.apply(command("a=T,f=100,c=1,r=1,q=1", &png), 0, 0, 0, size());
    assert_eq!(state.anonymous_images, 1);
    state.reset();
    assert_eq!((state.images.len(), state.anonymous_images), (2, 0));
    assert_eq!(state.insertion_order.len(), 2);
}

fn reply(result: &KittyGraphicsApplyResult) -> String {
    String::from_utf8(result.response.clone().unwrap_or_default()).unwrap()
}

#[test]
fn a_restarting_upload_is_checked_and_answered_as_itself() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=t,f=32,s=1,v=1,i=1,m=1", &[1, 2]),
        0,
        0,
        0,
        size(),
    );
    // The restart decodes as a fresh single-chunk upload: its own error,
    // reported for its own id, not the abandoned upload's.
    let invalid = KittyGraphicsCommand::parse(b"a=t,f=32,s=1,v=1,i=2;!!!!".to_vec(), false);
    let result = state.apply(invalid, 0, 0, 0, size());
    assert_eq!(
        reply(&result),
        "\x1b_Gi=2;EINVAL:invalid base64 payload\x1b\\"
    );
    assert!(state.pending.is_none());
}

#[test]
fn orphaned_relative_placements_keep_their_named_images() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=T,i=1,p=1,f=32,s=1,v=1", &[255; 4]),
        0,
        0,
        0,
        size(),
    );
    state.apply(
        command("a=T,i=2,P=1,Q=1,f=32,s=1,v=1", &[255; 4]),
        0,
        0,
        0,
        size(),
    );
    state.apply(command("a=d,d=i,i=1", &[]), 0, 0, 0, size());
    assert!(!state.has_placements());
    let result = state.apply(command("a=p,i=2", &[]), 0, 0, 0, size());
    assert_eq!(reply(&result), "\x1b_Gi=2;OK\x1b\\");
    assert_eq!(state.render_placements(0, 0, 24, 80).len(), 1);
}

#[test]
fn resending_an_image_relative_to_its_own_placement_has_no_parent() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=T,i=1,p=1,f=32,s=1,v=1", &[255; 4]),
        0,
        0,
        0,
        size(),
    );
    // Replacing image 1 removes its placements, including the parent named
    // here. kitty frees the old placements first and answers ENOPARENT.
    let result = state.apply(
        command("a=T,i=1,p=2,P=1,Q=1,f=32,s=1,v=1", &[0; 4]),
        0,
        0,
        0,
        size(),
    );
    assert_eq!(
        reply(&result),
        "\x1b_Gi=1,p=2;ENOPARENT:relative placement parent not found\x1b\\"
    );
    assert!(result.changed, "the old placement is gone");
    assert!(!state.has_placements());
    assert_eq!(state.images[&1].image.rgba().unwrap(), &[0; 4]);
}

#[test]
fn replies_report_image_numbers_as_numbers() {
    let mut state = KittyGraphicsState::default();
    let result = state.apply(command("a=p,I=5", &[]), 0, 0, 0, size());
    assert_eq!(reply(&result), "\x1b_GI=5;ENOENT:image id not found\x1b\\");
    let result = state.apply(command("a=t,I=5,f=32,s=1,v=1", &[255; 4]), 0, 0, 0, size());
    let id = state.resolve_image_id(&command("I=5", &[])).unwrap();
    assert_eq!(reply(&result), format!("\x1b_Gi={id},I=5;OK\x1b\\"));
    let result = state.apply(command("a=p,I=5,p=3", &[]), 0, 0, 0, size());
    assert_eq!(reply(&result), format!("\x1b_Gi={id},I=5,p=3;OK\x1b\\"));
}

#[test]
fn positional_deletes_resolve_a_shared_virtual_parent_once() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=t,i=1,f=32,s=1,v=1,q=2", &[255; 4]),
        0,
        0,
        0,
        size(),
    );
    state.apply(command("a=p,U=1,i=1,p=1,c=4,r=4,q=2", &[]), 0, 0, 0, size());
    state.apply(
        command("a=t,i=2,f=32,s=1,v=1,q=2", &[255; 4]),
        0,
        0,
        0,
        size(),
    );
    for placement in 1..=2000 {
        state.apply(
            command(&format!("a=p,i=2,p={placement},P=1,Q=1,c=1,r=1,q=2"), &[]),
            0,
            0,
            0,
            size(),
        );
    }
    let placeholders: Vec<_> = (0..4)
        .flat_map(|row| {
            (0..4).map(move |col| KittyGraphicsPlaceholder {
                viewport_row: row,
                col: col as usize,
                image_id_low: 1,
                image_id_high: 0,
                image_id: 1,
                placement_id: 1,
                image_row: row as u32,
                image_col: col,
            })
        })
        .collect();
    let scans = || placement::VIRTUAL_ORIGIN_SCANS.with(std::cell::Cell::get);
    let before = scans();
    let result = state.apply_on_screen_with_placeholders(
        command("a=d,d=q,x=1,y=1,z=999,q=2", &[]),
        (0, 0),
        0,
        size(),
        KittyGraphicsScreen::Primary,
        &placeholders,
    );
    assert!(!result.changed);
    assert_eq!(scans() - before, 1);
    let rendered = state.render_placements_on_screen_with_placeholders(
        0,
        0,
        24,
        80,
        KittyGraphicsScreen::Primary,
        &placeholders,
    );
    assert_eq!(
        rendered.len(),
        16 + 2000,
        "each placeholder cell, then each child"
    );
    assert_eq!(scans() - before, 2);

    state.apply_on_screen_with_placeholders(
        command("a=d,d=p,x=1,y=1,q=2", &[]),
        (0, 0),
        0,
        size(),
        KittyGraphicsScreen::Primary,
        &placeholders,
    );
    assert_eq!(state.placements.len(), 1, "every child sat on cell (0, 0)");
}

#[test]
fn stored_images_hold_no_more_memory_than_they_are_charged() {
    let (width, height) = (1025u32, 1024u32);
    let pixels: Vec<u8> = (0..width * height * 4).map(|i| (i % 251) as u8).collect();
    let control = format!("a=t,i=1,f=32,s={width},v={height},q=2");

    let mut chunked = KittyGraphicsState::default();
    let chunks: Vec<_> = pixels.chunks(3 * 4096).collect();
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        let control = if index == 0 {
            format!("{control},m={more}")
        } else {
            format!("m={more}")
        };
        chunked.apply(command(&control, chunk), 0, 0, 0, size());
    }

    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&pixels).unwrap();
    let mut compressed = KittyGraphicsState::default();
    compressed.apply(
        command(&format!("{control},o=z"), &encoder.finish().unwrap()),
        0,
        0,
        0,
        size(),
    );

    for state in [chunked, compressed] {
        let image = &state.images[&1].image;
        assert_eq!(image.rgba().unwrap(), pixels.as_slice());
        assert_eq!(image.rgba_capacity(), image.byte_len());
        assert_eq!(state.stored_bytes, image.byte_len());
    }
}

#[test]
fn compositions_write_into_unshared_frames_in_place() {
    let mut state = KittyGraphicsState::default();
    state.apply(
        command("a=t,i=1,f=32,s=2,v=1,q=2", &[9; 8]),
        0,
        0,
        0,
        size(),
    );
    // An opaque red frame 2 on a background, then make frame 2 current.
    state.apply(
        command("a=f,i=1,f=32,s=1,v=1,Y=4278190335,q=2", &[255, 0, 0, 255]),
        0,
        0,
        0,
        size(),
    );
    let frames = |state: &KittyGraphicsState| {
        state.images[&1]
            .animation
            .as_ref()
            .unwrap()
            .frames
            .iter()
            .map(|frame| frame.image.rgba().unwrap().to_vec())
            .collect::<Vec<_>>()
    };
    assert_eq!(frames(&state)[1], [255, 0, 0, 255, 255, 0, 0, 255]);
    state.apply(command("a=a,i=1,c=2,q=2", &[]), 0, 0, 0, size());

    let pointer = |state: &KittyGraphicsState| {
        state.images[&1].animation.as_ref().unwrap().frames[1]
            .image
            .rgba()
            .unwrap()
            .as_ptr()
    };
    let before = pointer(&state);
    let bytes = state.stored_bytes;
    let result = state.apply(command("a=c,i=1,r=1,c=2,w=1,h=1,C=1", &[]), 0, 0, 0, size());
    assert_eq!(reply(&result), "\x1b_Gi=1;OK\x1b\\");
    assert_eq!(
        pointer(&state),
        before,
        "the current frame is edited in place"
    );
    assert_eq!(frames(&state)[1], [9, 9, 9, 9, 255, 0, 0, 255]);
    assert_eq!(state.images[&1].image.rgba().unwrap(), &frames(&state)[1]);
    assert_eq!(state.stored_bytes, bytes);

    // A frame a snapshot still shows is copied, never changed under it.
    state.apply(command("a=p,i=1,q=2", &[]), 0, 0, 0, size());
    let snapshot = state.render_placements(0, 0, 24, 80);
    state.apply(
        command("a=c,i=1,r=2,c=2,X=1,x=0,w=1,h=1,C=1", &[]),
        0,
        0,
        0,
        size(),
    );
    assert_eq!(
        snapshot[0].image.rgba().unwrap(),
        [9, 9, 9, 9, 255, 0, 0, 255]
    );
    assert_eq!(frames(&state)[1], [255, 0, 0, 255, 255, 0, 0, 255]);
    assert_ne!(pointer(&state), before);

    // A failed composition leaves a still image as it was.
    state.apply(
        command("a=t,i=2,f=32,s=1,v=1,q=2", &[1; 4]),
        0,
        0,
        0,
        size(),
    );
    let result = state.apply(command("a=c,i=2,r=1,c=1", &[]), 0, 0, 0, size());
    assert!(reply(&result).contains("EINVAL"));
    assert!(state.images[&2].animation.is_none());
    assert_eq!(state.images[&2].image.rgba().unwrap(), &[1; 4]);
}
