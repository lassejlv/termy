use super::*;

fn size() -> TerminalSize {
    TerminalSize {
        cols: 80,
        rows: 24,
        cell_width: 10.0,
        cell_height: 20.0,
    }
}

/// Kitty's `c` and `r` for `args` and an image of `width`×`height` pixels.
fn cells(args: &str, width: u32, height: u32, cursor_col: usize) -> (Option<u32>, Option<u32>) {
    let command = KittyGraphicsCommand::inline_image(args.as_bytes(), Vec::new()).unwrap();
    command
        .inline
        .unwrap()
        .display_cells(width, height, size(), cursor_col)
}

#[test]
fn only_inline_files_become_quiet_anonymous_uploads() {
    assert!(KittyGraphicsCommand::inline_image(b"name=eA==;size=3", Vec::new()).is_none());
    assert!(KittyGraphicsCommand::inline_image(b"inline=0", Vec::new()).is_none());
    let command = KittyGraphicsCommand::inline_image(b"inline=1", b"AQID".to_vec()).unwrap();
    assert!(command.validate().is_ok());
    assert_eq!(command.char_value('a'), Some('T'));
    assert_eq!(command.u32_value('q'), Some(2));
    assert_eq!(command.u32_value('C'), Some(0));
    assert_eq!(command.value('i'), None);
    assert_eq!(command.payload, b"AQID");
    let command =
        KittyGraphicsCommand::inline_image(b"inline=1;doNotMoveCursor=1", Vec::new()).unwrap();
    assert_eq!(command.u32_value('C'), Some(1));
}

#[test]
fn auto_sizes_keep_natural_size_or_shrink_to_fit() {
    assert_eq!(cells("inline=1", 40, 40, 0), (None, None));
    assert_eq!(
        cells("inline=1;width=auto;height=auto", 800, 480, 0),
        (None, None)
    );
    // Wider than the screen: the full width, height from the aspect ratio.
    assert_eq!(cells("inline=1", 1600, 100, 0), (Some(80), None));
    // Only the space right of the cursor counts.
    assert_eq!(cells("inline=1", 1600, 100, 60), (Some(20), None));
    // Taller than the screen: the full height.
    assert_eq!(cells("inline=1", 100, 960, 0), (None, Some(24)));
}

#[test]
fn one_dimension_derives_the_other_from_the_aspect_ratio() {
    assert_eq!(cells("inline=1;width=12", 100, 50, 0), (Some(12), None));
    assert_eq!(cells("inline=1;width=40px", 100, 50, 0), (Some(4), None));
    assert_eq!(cells("inline=1;width=50%", 100, 50, 0), (Some(40), None));
    assert_eq!(cells("inline=1;height=3", 100, 50, 0), (Some(12), None));
    // A tall image names its rows, which span more cells.
    assert_eq!(cells("inline=1;height=10", 50, 400, 0), (None, Some(10)));
    assert_eq!(cells("inline=1;height=25%", 50, 400, 0), (None, Some(6)));
}

#[test]
fn both_dimensions_fit_the_image_unless_asked_to_stretch() {
    // A wide image fits a 20x20-cell box by its width.
    assert_eq!(
        cells("inline=1;width=20;height=20", 200, 20, 0),
        (Some(20), None)
    );
    assert_eq!(
        cells(
            "inline=1;width=20;height=20;preserveAspectRatio=0",
            200,
            20,
            0
        ),
        (Some(20), Some(20))
    );
    // A stretched dimension left `auto` keeps its natural size.
    assert_eq!(
        cells("inline=1;width=20;preserveAspectRatio=0", 200, 40, 0),
        (Some(20), Some(2))
    );
}

#[test]
fn invalid_dimensions_are_auto() {
    for value in ["0", "-3", "x", "0px", "%", "1.5"] {
        assert_eq!(Dimension::parse(value), Dimension::Auto, "{value}");
    }
    assert_eq!(Dimension::parse("150%").pixels(10.0, 800.0), Some(800.0));
}

#[test]
fn multipart_files_join_parts_split_anywhere() {
    let mut upload = InlineMultipart::new(b"inline=1;width=2");
    for part in [&b"AQ"[..], b"I", b"D/w="] {
        assert!(upload.push(part));
    }
    let command = upload.finish().unwrap();
    assert_eq!(command.payload, b"AQID/w=");
    assert!(command.inline.is_some());
}
