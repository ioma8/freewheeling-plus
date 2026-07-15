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
    assert_eq!(snapshots.displayed, Some(1));
}
