//! Bounded, renderer-facing native UI state.
//!
//! The runtime owns the mutable audio/browser objects.  This module owns a
//! cheap, coherent snapshot of what the renderer may read.  `sync` is the
//! integration seam: callers pass immutable views and receive one complete
//! frame, never a collection of partially updated fields.

pub const MAX_UI_LOOPS: usize = 128;
pub const MAX_UI_BROWSER_ITEMS: usize = 256;
pub const MAX_UI_SELECTION_SETS: usize = 10;
pub const MAX_UI_SELECTION_ITEMS: usize = 128;
pub const MAX_UI_PATCH_BANKS: usize = 64;
pub const MAX_UI_PATCH_ITEMS: usize = 512;
pub const MAX_UI_SNAPSHOTS: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UiLoop {
    pub id: usize,
    pub status: u8,
    pub frames: u32,
    pub position: u32,
    pub gain: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiBrowserItem {
    pub id: i32,
    pub name: String,
    pub selected: bool,
    pub current: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiPatchBank {
    pub name: String,
    pub cursor: usize,
    pub items: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiSnapshot {
    pub id: i32,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NativeUiState {
    pub sequence: u64,
    pub sample_clock: u64,
    pub pulse_position: u32,
    pub recording_slot: Option<usize>,
    pub loops: Vec<UiLoop>,
    pub browser_cursor: usize,
    pub browser_items: Vec<UiBrowserItem>,
    pub selection_sets: Vec<Vec<usize>>,
    pub patch_bank_cursor: usize,
    pub patch_item_cursor: usize,
    pub patch_banks: Vec<UiPatchBank>,
    pub snapshots: Vec<UiSnapshot>,
    pub streaming: bool,
    pub stream_bytes: u64,
    pub fullscreen: bool,
    pub autosave: bool,
    pub midi_sync: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiRuntimeInput {
    pub sequence: u64,
    pub sample_clock: u64,
    pub pulse_position: u32,
    pub recording_slot: Option<usize>,
    pub loops: Vec<UiLoop>,
    pub browser_cursor: usize,
    pub browser_items: Vec<UiBrowserItem>,
    pub selections: Vec<Vec<usize>>,
    pub patch_bank_cursor: usize,
    pub patch_item_cursor: usize,
    pub patch_banks: Vec<UiPatchBank>,
    pub snapshots: Vec<UiSnapshot>,
    pub streaming: bool,
    pub stream_bytes: u64,
    pub fullscreen: bool,
    pub autosave: bool,
    pub midi_sync: bool,
}

impl NativeUiState {
    /// Replace the published frame, enforcing every renderer-facing bound.
    /// IDs and cursors are clamped, and selection IDs are deduplicated.
    pub fn sync(&mut self, input: UiRuntimeInput) {
        self.sequence = input.sequence;
        self.sample_clock = input.sample_clock;
        self.pulse_position = input.pulse_position;
        self.recording_slot = input.recording_slot.filter(|&n| n < MAX_UI_LOOPS);
        self.loops = input
            .loops
            .into_iter()
            .take(MAX_UI_LOOPS)
            .enumerate()
            .map(|(i, mut v)| {
                v.id = i;
                v
            })
            .collect();
        self.browser_items = input
            .browser_items
            .into_iter()
            .take(MAX_UI_BROWSER_ITEMS)
            .collect();
        self.browser_cursor = clamp(input.browser_cursor, self.browser_items.len());
        self.selection_sets = input
            .selections
            .into_iter()
            .take(MAX_UI_SELECTION_SETS)
            .map(|set| {
                let mut out = Vec::with_capacity(MAX_UI_SELECTION_ITEMS);
                for id in set {
                    if id < self.loops.len() && !out.contains(&id) {
                        if out.len() == MAX_UI_SELECTION_ITEMS {
                            break;
                        }
                        out.push(id);
                    }
                }
                out
            })
            .collect();
        self.patch_banks = input
            .patch_banks
            .into_iter()
            .take(MAX_UI_PATCH_BANKS)
            .map(|mut b| {
                b.items.truncate(MAX_UI_PATCH_ITEMS);
                b.cursor = clamp(b.cursor, b.items.len());
                b
            })
            .collect();
        self.patch_bank_cursor = clamp(input.patch_bank_cursor, self.patch_banks.len());
        self.patch_item_cursor = self
            .patch_banks
            .get(self.patch_bank_cursor)
            .map_or(0, |b| b.cursor);
        self.snapshots = input.snapshots.into_iter().take(MAX_UI_SNAPSHOTS).collect();
        self.streaming = input.streaming;
        self.stream_bytes = input.stream_bytes;
        self.fullscreen = input.fullscreen;
        self.autosave = input.autosave;
        self.midi_sync = input.midi_sync;
    }
}

fn clamp(value: usize, len: usize) -> usize {
    if len == 0 { 0 } else { value.min(len - 1) }
}
