use freewheeling_plus::native_rename::{
    MAX_NAME_BYTES, MAX_PENDING_RESULTS, NativeRename, RenameInput, RenameResult, RenameTarget,
};

fn browser() -> RenameTarget {
    RenameTarget::Browser {
        browser: 2,
        item: 7,
    }
}

#[test]
fn begins_each_supported_target_and_queues_commit() {
    let mut rename = NativeRename::default();
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
    let mut rename = NativeRename::default();
    assert!(rename.begin(RenameTarget::Snapshot { slot: 1 }, Some("café🙂")));
    assert!(rename.handle(RenameInput::KeyDown { keycode: 8 }));
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
    let mut rename = NativeRename::default();
    assert!(rename.begin(browser(), None));
    rename.handle(RenameInput::Text(&"é".repeat(300)));
    let name = rename.current_name();
    // The bound is tight: the buffer stops at the last character that still
    // fits, so the next character would exceed it.
    assert_eq!(name.len(), MAX_NAME_BYTES - 1);
    assert!(name.len() + 'é'.len_utf8() > MAX_NAME_BYTES);
    assert!(name.chars().all(|ch| ch == 'é'));
    assert!(rename.is_active());
}

#[test]
fn a_persisted_name_longer_than_the_buffer_is_rejected() {
    let mut rename = NativeRename::default();
    let long = "a".repeat(MAX_NAME_BYTES + 1);
    assert!(!rename.begin(browser(), Some(&long)));
    assert!(!rename.is_active());
    assert!(rename.begin(browser(), Some(&"a".repeat(MAX_NAME_BYTES))));
}

#[test]
fn enter_variants_commit_escape_cancels_and_inactive_input_is_ignored() {
    let mut rename = NativeRename::default();
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
fn control_and_invisible_text_is_not_inserted() {
    let mut rename = NativeRename::default();
    assert!(rename.begin(browser(), None));
    rename.handle(RenameInput::Text("a\n\0\t\r\u{1}\u{7f}b"));
    // Control characters must also not count toward the byte budget.
    assert_eq!(rename.current_name(), "ab");
    rename.handle(RenameInput::Text("\u{200b}\u{202e}\u{2028}\u{feff}c"));
    assert_eq!(rename.current_name(), "abc");
}

#[test]
fn queued_results_are_bounded_and_report_the_loss() {
    let mut rename = NativeRename::default();
    for _ in 0..MAX_PENDING_RESULTS + 3 {
        assert!(rename.begin(browser(), None));
        assert!(rename.commit());
    }
    assert_eq!(rename.pending_results(), MAX_PENDING_RESULTS);
    assert_eq!(rename.dropped_results(), 3);
}
