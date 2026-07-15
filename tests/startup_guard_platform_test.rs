use freewheeling_plus::startup_guard_platform::{OwnedResource, PlatformStartupGuard};
use std::sync::{Arc, Mutex};

#[test]
fn rollback_preserves_tags_and_lifo_order() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut guard = PlatformStartupGuard::new();
    for tag in [4, 8, 15] {
        let seen = Arc::clone(&seen);
        guard.push(tag, move |tag| seen.lock().unwrap().push(tag));
    }
    guard.rollback();
    guard.rollback();
    assert_eq!(*seen.lock().unwrap(), vec![15, 8, 4]);
    assert!(guard.is_released());
}

#[test]
fn capacity_release_and_owned_handle_cleanup_are_exact() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut guard = PlatformStartupGuard::new();
    for tag in 0..PlatformStartupGuard::MAX_ENTRIES as i32 + 1 {
        guard.push(tag, |_| {});
    }
    assert_eq!(guard.count(), PlatformStartupGuard::MAX_ENTRIES);

    let cleaned = Arc::clone(&events);
    guard.release();
    guard.push_resource(
        7,
        OwnedResource::new(String::from("handle"), move |handle, tag| {
            cleaned.lock().unwrap().push((handle, tag));
        }),
    );
    guard.rollback();
    assert!(events.lock().unwrap().is_empty());

    let cleaned = Arc::clone(&events);
    let mut guard = PlatformStartupGuard::new();
    guard.push_resource(
        9,
        OwnedResource::new(String::from("owned"), move |handle, tag| {
            cleaned.lock().unwrap().push((handle, tag));
        }),
    );
    guard.rollback();
    guard.rollback();
    assert_eq!(*events.lock().unwrap(), vec![(String::from("owned"), 9)]);
}
