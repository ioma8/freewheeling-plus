#[path = "../src/native_ui_state.rs"]
mod native_ui_state;
use native_ui_state::*;

#[test]
fn sync_publishes_a_bounded_coherent_frame() {
    let mut state = NativeUiState::default();
    let mut input = UiRuntimeInput {
        browser_cursor: 999,
        patch_bank_cursor: 99,
        selections: vec![vec![0, 0, 7]],
        loops: vec![UiLoop::default()],
        ..Default::default()
    };
    input.browser_items = (0..MAX_UI_BROWSER_ITEMS + 4)
        .map(|i| UiBrowserItem {
            id: i as i32,
            ..Default::default()
        })
        .collect();
    input.patch_banks = vec![UiPatchBank {
        items: vec!["a".into()],
        ..Default::default()
    }];
    state.sync(input);
    assert_eq!(state.browser_items.len(), MAX_UI_BROWSER_ITEMS);
    assert_eq!(state.browser_cursor, MAX_UI_BROWSER_ITEMS - 1);
    assert_eq!(state.selection_sets, vec![vec![0]]);
    assert_eq!(state.patch_bank_cursor, 0);
}

#[test]
fn sync_carries_transport_and_control_flags() {
    let mut s = NativeUiState::default();
    s.sync(UiRuntimeInput {
        sequence: 4,
        sample_clock: 12,
        pulse_position: 3,
        recording_slot: Some(2),
        streaming: true,
        stream_bytes: 42,
        fullscreen: true,
        autosave: true,
        midi_sync: true,
        ..Default::default()
    });
    assert_eq!(
        (
            s.sequence,
            s.sample_clock,
            s.pulse_position,
            s.recording_slot
        ),
        (4, 12, 3, Some(2))
    );
    assert!(s.streaming && s.fullscreen && s.autosave && s.midi_sync && s.stream_bytes == 42);
}

#[test]
fn sync_replaces_live_frame_without_leaking_old_browser_or_patch_state() {
    let mut state = NativeUiState::default();
    state.sync(UiRuntimeInput {
        browser_items: vec![UiBrowserItem {
            name: "old".into(),
            ..Default::default()
        }],
        patch_banks: vec![UiPatchBank {
            items: vec!["old patch".into()],
            ..Default::default()
        }],
        ..Default::default()
    });
    state.sync(UiRuntimeInput {
        sequence: 2,
        browser_items: vec![UiBrowserItem {
            name: "renamed".into(),
            selected: true,
            ..Default::default()
        }],
        browser_cursor: 0,
        patch_banks: vec![UiPatchBank {
            items: vec!["new patch".into()],
            cursor: 0,
            ..Default::default()
        }],
        patch_bank_cursor: 0,
        ..Default::default()
    });

    assert_eq!(state.sequence, 2);
    assert_eq!(state.browser_items[0].name, "renamed");
    assert!(state.browser_items[0].selected);
    assert_eq!(state.patch_banks[0].items, vec!["new patch"]);
}
