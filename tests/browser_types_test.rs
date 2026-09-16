use freewheeling_plus::browser_types::{
    LoopBrowserItem, PatchBank, PatchBrowser, PatchItem, SceneBrowserItem, SnapshotBrowser,
};
use std::time::SystemTime;

#[test]
fn library_items_keep_subclass_type_and_strip_extension() {
    assert_eq!(
        LoopBrowserItem::new(Some(SystemTime::UNIX_EPOCH), "loop", true, Some("a.wav"))
            .item
            .item_type,
        freewheeling_plus::browser::BrowserItemType::Loop
    );
    assert_eq!(
        SceneBrowserItem::new(None, "scene", false, Some("a.xml"))
            .filename
            .as_deref(),
        Some("a")
    );
}

#[test]
fn patch_browser_keeps_banks_distinct() {
    let mut browser = PatchBrowser::new("patches");
    let mut bank = PatchBank::new(1, 42, true);
    let mut patch = PatchItem::new(7, 2, 3, 4, "organ");
    patch.setup_zones(1);
    assert!(patch.is_combi());
    bank.add(patch);
    browser.add_bank(bank);
    assert_eq!(browser.current_bank().unwrap().tag, 42);
    assert_eq!(browser.browser.items[0].name, "organ");
}

#[test]
fn snapshots_have_a_separate_api() {
    let mut snapshots = SnapshotBrowser::new(vec!["one".into(), "two".into()]);
    assert!(snapshots.rename(1, "renamed"));
    snapshots.display_range(1, 4);
    assert_eq!(snapshots.displayed_count, Some(1));
}

#[test]
fn filenames_keep_dotfiles_and_dotted_directories() {
    let item = SceneBrowserItem::new(None, "hidden", true, Some(".gitignore"));
    assert_eq!(item.filename.as_deref(), Some(".gitignore"));
    let item = SceneBrowserItem::new(None, "nested", true, Some("dir.with.dots/loop"));
    assert_eq!(item.filename.as_deref(), Some("dir.with.dots/loop"));
    let item = SceneBrowserItem::new(None, "plain", true, Some("dir/loop.xml"));
    assert_eq!(item.filename.as_deref(), Some("dir/loop"));
}

#[test]
fn bank_switching_keeps_the_browser_cursor_consistent() {
    let mut browser = PatchBrowser::new("patches");
    let mut first = PatchBank::new(1, 1, false);
    first.add(PatchItem::new(1, 0, 0, 0, "organ"));
    first.add(PatchItem::new(2, 0, 1, 0, "piano"));
    browser.add_bank(first);
    let mut second = PatchBank::new(1, 2, false);
    second.add(PatchItem::new(3, 0, 2, 0, "bass"));
    browser.add_bank(second);
    assert_eq!(browser.browser.current_index, Some(0));
    assert!(browser.move_to_bank(1));
    assert_eq!(browser.browser.items[0].name, "bass");
    // The previous bank's selection index must not leak into the new bank.
    assert_eq!(browser.browser.selected_index, None);
    browser.browser.current_index = Some(1);
    assert!(browser.move_to_bank(-1));
    assert_eq!(browser.browser.current_index, Some(0));
    assert_eq!(browser.browser.items[0].name, "organ");
}

#[test]
fn bank_movement_rejects_out_of_range_directions() {
    let mut browser = PatchBrowser::new("patches");
    let mut bank = PatchBank::new(1, 1, false);
    bank.add(PatchItem::new(1, 0, 0, 0, "organ"));
    browser.add_bank(bank);
    assert!(!browser.move_to_bank(i32::MAX));
    assert!(!browser.move_to_bank(i32::MIN));
    assert!(!browser.move_to_bank(1));
}
