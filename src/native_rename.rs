//! Interactive rename state used by the native UI.
//!
//! This module deliberately does not perform persistence.  It owns the short-lived
//! edit session and queues a value for the runtime to apply to its browser or
//! snapshot store.

use std::collections::VecDeque;

pub const MAX_NAME_BYTES: usize = 511;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenameTarget {
    Browser { browser: i32, item: usize },
    Snapshot { slot: i32 },
    #[allow(dead_code)] // constructed in native_runtime.rs (not compiled by test binary)
    Loop { slot: i32 },
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
}

impl NativeRename {
    pub const MAX_NAME_BYTES: usize = MAX_NAME_BYTES;

    pub fn new() -> Self {
        Self::default()
    }

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
                8 => {
                    self.name.pop();
                    true
                }
                13 | 271 => {
                    self.finish(true);
                    true
                }
                27 => {
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
            if ch.is_control() {
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
        self.results.push_back(RenameResult { target, name });
        true
    }

    pub fn pop_result(&mut self) -> Option<RenameResult> {
        self.results.pop_front()
    }
    pub fn pending_results(&self) -> usize {
        self.results.len()
    }
}
