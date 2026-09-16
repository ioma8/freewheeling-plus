//! Interactive rename state used by the native UI.
//!
//! This module deliberately does not perform persistence.  It owns the short-lived
//! edit session and queues a value for the runtime to apply to its browser or
//! snapshot store.

use std::collections::VecDeque;

pub const MAX_NAME_BYTES: usize = 511;

/// SDL1 keysyms used by the rename widget (`src/sdlio.rs` defines the same
/// values for the event mapping).
const KEYCODE_BACKSPACE: i32 = 8;
const KEYCODE_RETURN: i32 = 13;
const KEYCODE_ESCAPE: i32 = 27;
const KEYCODE_KP_ENTER: i32 = 271;

/// Cap on queued results. A consumer that stops draining must not grow this
/// without bound; the oldest result is dropped and counted instead.
pub const MAX_PENDING_RESULTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenameTarget {
    Browser {
        browser: i32,
        item: usize,
    },
    Snapshot {
        slot: i32,
    },
    #[allow(dead_code)] // constructed in native_runtime.rs (not compiled by test binary)
    Loop {
        slot: i32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameResult {
    pub target: RenameTarget,
    /// `None` means the edit was cancelled; `Some` is the committed UTF-8 name.
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameInput<'a> {
    KeyDown { keycode: i32 },
    Text(&'a str),
}

#[derive(Debug, Default)]
pub struct NativeRename {
    target: Option<RenameTarget>,
    name: String,
    results: VecDeque<RenameResult>,
    dropped_results: u64,
}

impl NativeRename {
    pub fn is_active(&self) -> bool {
        self.target.is_some()
    }

    pub fn target(&self) -> Option<RenameTarget> {
        self.target
    }

    pub fn current_name(&self) -> &str {
        &self.name
    }

    /// Starts an edit.  A second begin is rejected so an old result cannot be
    /// accidentally attributed to a new item.
    pub fn begin(&mut self, target: RenameTarget, old_name: Option<&str>) -> bool {
        if self.is_active() {
            return false;
        }
        // `append_text` truncates at the buffer limit. Starting a session from
        // a longer persisted name would commit that truncated name, so such a
        // session is rejected instead.
        if old_name.is_some_and(|name| name.len() > MAX_NAME_BYTES) {
            return false;
        }
        self.target = Some(target);
        self.name.clear();
        if let Some(old_name) = old_name {
            self.append_text(old_name);
        }
        true
    }

    pub fn handle(&mut self, input: RenameInput<'_>) -> bool {
        if !self.is_active() {
            return false;
        }
        match input {
            RenameInput::Text(text) => {
                self.append_text(text);
                true
            }
            RenameInput::KeyDown { keycode } => match keycode {
                KEYCODE_BACKSPACE => {
                    self.name.pop();
                    true
                }
                KEYCODE_RETURN | KEYCODE_KP_ENTER => {
                    self.finish(true);
                    true
                }
                KEYCODE_ESCAPE => {
                    self.finish(false);
                    true
                }
                _ => false,
            },
        }
    }

    pub fn append_text(&mut self, text: &str) {
        if !self.is_active() {
            return;
        }
        for ch in text.chars() {
            // Control characters, plus invisible formatting/separator code
            // points that would spoof the displayed or persisted name
            // (zero-width space, bidi overrides, line separators, BOM).
            if ch.is_control()
                || matches!(
                    ch,
                    '\u{200b}'..='\u{200f}'
                        | '\u{2028}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                        | '\u{feff}'
                )
            {
                continue;
            }
            let size = ch.len_utf8();
            if self.name.len() + size > MAX_NAME_BYTES {
                break;
            }
            self.name.push(ch);
        }
    }

    pub fn commit(&mut self) -> bool {
        self.finish(true)
    }
    pub fn cancel(&mut self) -> bool {
        self.finish(false)
    }

    fn finish(&mut self, commit: bool) -> bool {
        let Some(target) = self.target.take() else {
            return false;
        };
        let name = commit.then(|| std::mem::take(&mut self.name));
        if !commit {
            self.name.clear();
        }
        if self.results.len() >= MAX_PENDING_RESULTS {
            // The consumer has stopped draining; drop the oldest result and
            // count the loss instead of growing without bound.
            self.results.pop_front();
            self.dropped_results = self.dropped_results.saturating_add(1);
        }
        self.results.push_back(RenameResult { target, name });
        true
    }

    pub fn pop_result(&mut self) -> Option<RenameResult> {
        self.results.pop_front()
    }
    pub fn pending_results(&self) -> usize {
        self.results.len()
    }
    /// Results dropped because the queue was full.
    pub fn dropped_results(&self) -> u64 {
        self.dropped_results
    }
}
