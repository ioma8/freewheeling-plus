#[path = "../src/native_loop_selection.rs"]
mod native_loop_selection;

use native_loop_selection::{NUM_SELECTION_SETS, NativeLoopSelection, SelectionError};

#[test]
fn all_sets_are_independent_and_toggle_is_idempotent() {
    let mut s = NativeLoopSelection::new(8);
    assert_eq!(s.toggle(0, 4), Ok(true));
    assert_eq!(s.toggle(0, 4), Ok(false));
    assert_eq!(s.selected(0, 4), Ok(false));
    assert_eq!(s.selected(1, 4), Ok(false));
    assert_eq!(s.count(NUM_SELECTION_SETS - 1), Ok(0));
}

#[test]
fn bulk_operations_and_playing_filter_match_events() {
    let mut s = NativeLoopSelection::new(8);
    s.select_all(0, &[0, 1, 2, 3]).unwrap();
    s.select_only_playing(0, &[0, 1, 2, 3], |id| id % 2 == 0)
        .unwrap();
    assert_eq!(s.selected_ids(0).unwrap(), &[0, 2]);
    s.invert(0, &[0, 1, 2, 3]).unwrap();
    assert_eq!(s.selected_ids(0).unwrap(), &[1, 3]);
    s.clear(0).unwrap();
    assert_eq!(s.count(0), Ok(0));
}

#[test]
fn erase_and_import_update_every_set() {
    let mut s = NativeLoopSelection::new(8);
    s.select_all(0, &[1, 2, 3]).unwrap();
    s.select_all(1, &[2, 4]).unwrap();
    assert_eq!(s.erase_selected(0).unwrap(), vec![1, 2, 3]);
    assert_eq!(s.selected_ids(1).unwrap(), &[4]);
    s.toggle(2, 4).unwrap();
    s.update_after_import(&[4]);
    assert_eq!(s.selected_ids(1).unwrap(), &[4]);
    assert_eq!(s.selected_ids(2).unwrap(), &[4]);
}

#[test]
fn move_and_bounds_are_explicit() {
    let mut s = NativeLoopSelection::new(1);
    assert_eq!(s.capacity(), 1);
    s.toggle(0, 7).unwrap();
    s.update_after_move(7, 9);
    assert_eq!(s.selected(0, 9), Ok(true));
    assert_eq!(s.toggle(0, 10), Err(SelectionError::CapacityExceeded));
    assert_eq!(s.clear(NUM_SELECTION_SETS), Err(SelectionError::InvalidSet));
}
