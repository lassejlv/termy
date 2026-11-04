use super::super::tests::{command, one_pixel_png, placements_are_consistent, size};
use super::*;

fn sweeps() -> usize {
    ORPHAN_SWEEPS.with(std::cell::Cell::get)
}

fn place(state: &mut KittyGraphicsState, control: &str) -> KittyGraphicsApplyResult {
    state.apply(command(control, &[]), 0, 0, 0, size())
}

fn reply(result: &KittyGraphicsApplyResult) -> &str {
    std::str::from_utf8(result.response.as_deref().unwrap_or_default()).unwrap()
}

fn has(state: &KittyGraphicsState, image_id: u32, placement_id: u32) -> bool {
    state
        .placement_by_key(KittyGraphicsScreen::Primary, image_id, placement_id)
        .is_some()
}

#[test]
fn placing_past_the_cap_evicts_the_oldest_without_sweeping_for_orphans() {
    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    state.apply(command("a=t,f=100,i=1,q=2", &png), 0, 0, 0, size());
    // An id-less image placed once, which the churn evicts.
    state.apply(command("a=T,f=100,q=2", &png), 0, 0, 0, size());
    assert_eq!(state.anonymous_images, 1);

    let before = sweeps();
    // 50k placements cost seconds when each one shifted the list and
    // rebuilt a set of every placement's key.
    for _ in 0..50_000 {
        place(&mut state, "a=p,i=1,C=1,q=2");
    }
    assert_eq!(sweeps(), before, "no placement is relative");
    assert_eq!(state.placements.len(), MAX_PLACEMENTS);
    // The newest placements remain, oldest first.
    let newest = state.next_placement_serial;
    assert!(
        state
            .placements
            .iter()
            .zip(newest - MAX_PLACEMENTS as u64 + 1..)
            .all(|(placement, serial)| placement.placement_serial == serial
                && placement.image_id == 1)
    );
    assert_eq!(state.anonymous_images, 0, "its last placement was evicted");
    assert_eq!(state.images.len(), 1);
    assert!(placements_are_consistent(&state));
}

#[test]
fn evicting_a_parent_at_the_cap_removes_its_children_once() {
    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    for id in 1..=3 {
        let control = format!("a=t,f=100,i={id},q=2");
        state.apply(command(&control, &png), 0, 0, 0, size());
    }
    place(&mut state, "a=p,i=2,p=1,q=2");
    place(&mut state, "a=p,i=3,p=1,P=2,Q=1,q=2");
    // A second child, of whichever placement of image 3 comes first.
    place(&mut state, "a=p,i=3,p=2,P=3,q=2");
    assert_eq!(state.relative_placements, 2);

    let before = sweeps();
    for _ in 0..2 * MAX_PLACEMENTS {
        place(&mut state, "a=p,i=1,C=1,q=2");
    }
    assert!(!has(&state, 2, 1) && !has(&state, 3, 0));
    assert_eq!(state.relative_placements, 0);
    assert_eq!(
        sweeps() - before,
        1,
        "only the parent's eviction orphaned anything"
    );
    assert_eq!(state.placements.len(), MAX_PLACEMENTS);
    assert!(placements_are_consistent(&state));
}

#[test]
fn evicting_one_of_several_placements_keeps_children_of_the_image() {
    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    for id in 1..=3 {
        let control = format!("a=t,f=100,i={id},q=2");
        state.apply(command(&control, &png), 0, 0, 0, size());
    }
    place(&mut state, "a=p,i=2,q=2");
    place(&mut state, "a=p,i=2,p=7,q=2");
    // `Q=0`: any placement of image 2 will do.
    place(&mut state, "a=p,i=3,P=2,q=2");
    while state.placements.len() < MAX_PLACEMENTS {
        place(&mut state, "a=p,i=1,C=1,q=2");
    }
    let first = state.placements[0].placement_serial;
    let before = sweeps();

    place(&mut state, "a=p,i=1,C=1,q=2");
    assert_ne!(state.placements[0].placement_serial, first);
    assert!(has(&state, 2, 7));
    assert_eq!(state.relative_placements, 1);
    assert_eq!(sweeps(), before, "image 2 still has a placement");
    // Evicting `p=7` too leaves the child without a parent.
    place(&mut state, "a=p,i=1,C=1,q=2");
    assert!(!has(&state, 2, 0));
    assert_eq!(state.relative_placements, 0);
    assert_eq!(sweeps() - before, 1);
    assert!(placements_are_consistent(&state));
}

#[test]
fn the_cap_keeps_the_parents_of_the_placement_it_makes_room_for() {
    let png = one_pixel_png();
    let mut state = KittyGraphicsState::default();
    for id in 1..=4 {
        let control = format!("a=t,f=100,i={id},q=2");
        state.apply(command(&control, &png), 0, 0, 0, size());
    }
    // The oldest placements: a chain that a new placement hangs off.
    place(&mut state, "a=p,i=2,p=1,q=2");
    place(&mut state, "a=p,i=3,p=1,P=2,Q=1,q=2");
    while state.placements.len() < MAX_PLACEMENTS {
        place(&mut state, "a=p,i=1,C=1,q=2");
    }
    let third = state.placements[2].placement_serial;

    let result = place(&mut state, "a=p,i=4,p=1,P=3,Q=1");
    assert_eq!(reply(&result), "\x1b_Gi=4,p=1;OK\x1b\\");
    assert!(has(&state, 4, 1), "the new placement outlives its OK");
    assert!(has(&state, 3, 1) && has(&state, 2, 1));
    assert!(
        state
            .placements
            .iter()
            .all(|placement| placement.placement_serial != third),
        "the oldest placement outside the chain made room"
    );
    assert_eq!(state.placements.len(), MAX_PLACEMENTS);
    assert!(placements_are_consistent(&state));
}
