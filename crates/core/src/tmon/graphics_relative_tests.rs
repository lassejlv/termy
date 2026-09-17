#[test]
fn grok_virtual_placement_is_accepted_without_moving_the_cursor() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    upload_one_pixel(&mut state, &mut grid, 42);
    grid.set_cursor_position(20, 60);
    let cursor = grid.cursor_position();

    let result = state.apply(
        command("a=p,U=1,i=42,p=7,c=1,r=1,q=1", &[]),
        &mut grid,
        test_size(),
    );

    assert!(result.changed, "the virtual placement must be registered");
    assert_eq!(grid.cursor_position(), cursor);
    assert_eq!(state.placements.len(), 1);
}

#[test]
fn grok_relative_placement_uses_parent_origin_and_signed_offset() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    upload_one_pixel(&mut state, &mut grid, 41);
    upload_one_pixel(&mut state, &mut grid, 42);
    grid.set_cursor_position(5, 4);
    state.apply(
        command("a=p,i=41,p=7,c=1,r=1,C=1,q=1", &[]),
        &mut grid,
        test_size(),
    );
    grid.set_cursor_position(20, 70);
    let cursor = grid.cursor_position();

    let result = state.apply(
        command("a=p,i=42,p=8,P=41,Q=7,H=3,V=-2,c=2,r=2,q=1", &[]),
        &mut grid,
        test_size(),
    );

    assert!(result.changed);
    assert_eq!(
        grid.cursor_position(),
        cursor,
        "relative placements never move the cursor"
    );
    let placement = state
        .render_placements(&grid)
        .into_iter()
        .find(|placement| placement.image_id == 42)
        .expect("relative child must be visible");
    assert_eq!((placement.viewport_row, placement.col), (3, 7));
}

#[test]
fn grok_relative_placement_tracks_and_clears_with_unicode_placeholder() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    upload_one_pixel(&mut state, &mut grid, 41);
    upload_one_pixel(&mut state, &mut grid, 42);
    state.apply(
        command("a=p,U=1,i=41,p=7,c=1,r=1,C=1,q=1", &[]),
        &mut grid,
        test_size(),
    );
    state.apply(
        command("a=p,i=42,p=8,P=41,Q=7,H=3,V=-2,c=2,r=2,q=1", &[]),
        &mut grid,
        test_size(),
    );
    grid.sgr(&[38, 5, 41, 58, 5, 7]);
    grid.set_cursor_position(6, 4);
    grid.put_char(PLACEHOLDER);
    grid.put_char('\u{0305}');
    grid.put_char('\u{0305}');

    let child = state
        .render_placements(&grid)
        .into_iter()
        .find(|placement| placement.image_id == 42)
        .expect("the child should follow the virtual placeholder");
    assert_eq!((child.viewport_row, child.col), (4, 7));

    grid.set_cursor_position(6, 4);
    grid.put_char('x');
    assert!(
        state.render_placements(&grid).is_empty(),
        "overwriting the placeholder must clear its image and relative children"
    );
}

#[test]
fn grok_deleting_parent_removes_relative_descendants() {
    let mut state = GraphicsState::default();
    let mut grid = Grid::new(80, 24, 100, CursorStyle::Block);
    upload_one_pixel(&mut state, &mut grid, 41);
    upload_one_pixel(&mut state, &mut grid, 42);
    grid.set_cursor_position(5, 4);
    state.apply(
        command("a=p,i=41,p=7,c=1,r=1,C=1,q=1", &[]),
        &mut grid,
        test_size(),
    );
    grid.set_cursor_position(20, 70);
    state.apply(
        command("a=p,i=42,p=8,P=41,Q=7,c=2,r=2,q=1", &[]),
        &mut grid,
        test_size(),
    );

    state.apply(command("a=d,d=i,i=41,p=7,q=1", &[]), &mut grid, test_size());

    assert!(
        state
            .placements
            .iter()
            .all(|placement| placement.image_id != 42),
        "relative descendants must share their parent's lifetime"
    );
}


