#[path = "../src/native_rename.rs"]
mod native_rename;

use native_rename::{NativeRename, RenameInput, RenameResult, RenameTarget};

fn browser() -> RenameTarget {
    RenameTarget::Browser {
        browser: 2,
        item: 7,
    }
}

#[test]
fn begins_each_supported_target_and_queues_commit() {
    let mut rename = NativeRename::new();
    assert!(rename.begin(browser(), Some("old")));
    assert_eq!(rename.target(), Some(browser()));
    assert!(!rename.begin(RenameTarget::Snapshot { slot: 3 }, Some("ignored")));
    assert!(rename.handle(RenameInput::Text(" name")));
    assert!(rename.handle(RenameInput::KeyDown { keycode: 13 }));
    assert_eq!(
        rename.pop_result(),
        Some(RenameResult {
            target: browser(),
            name: Some("old name".into())
        })
    );
    assert_eq!(rename.pop_result(), None);
    assert!(rename.begin(RenameTarget::Snapshot { slot: 3 }, None));
}

#[test]
fn unicode_and_utf8_safe_backspace_are_supported() {
    let mut rename = NativeRename::new();
    rename.begin(RenameTarget::Snapshot { slot: 1 }, Some("café🙂"));
    rename.handle(RenameInput::KeyDown { keycode: 8 });
    assert_eq!(rename.current_name(), "café");
    rename.handle(RenameInput::Text(" 日本語"));
    assert!(rename.commit());
    assert_eq!(
        rename.pop_result().unwrap().name.as_deref(),
        Some("café 日本語")
    );
}

#[test]
fn byte_bound_is_exact_and_does_not_split_characters() {
    let mut rename = NativeRename::new();
    rename.begin(browser(), None);
    rename.handle(RenameInput::Text(&"é".repeat(300)));
    assert!(rename.current_name().len() <= NativeRename::MAX_NAME_BYTES);
    assert_eq!(rename.current_name().len() % 2, 0);
    assert!(rename.current_name().chars().count() <= 255);
}

#[test]
fn enter_variants_commit_escape_cancels_and_inactive_input_is_ignored() {
    let mut rename = NativeRename::new();
    assert!(!rename.handle(RenameInput::Text("ignored")));
    rename.begin(browser(), Some("x"));
    assert!(rename.handle(RenameInput::KeyDown { keycode: 271 }));
    rename.begin(RenameTarget::Snapshot { slot: 0 }, Some("y"));
    assert!(rename.cancel());
    assert_eq!(rename.pending_results(), 2);
    assert_eq!(rename.pop_result().unwrap().name, Some("x".into()));
    assert_eq!(rename.pop_result().unwrap().name, None);
}

#[test]
fn control_text_is_not_inserted() {
    let mut rename = NativeRename::new();
    rename.begin(browser(), None);
    rename.handle(RenameInput::Text("a\n\0\tb"));
    assert_eq!(rename.current_name(), "ab");
}
