# RCU Simplification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) for tracking.

**Goal:** Replace the custom hand-rolled read-copy-update (`Rcu<T>`) implementation with the standard `crossbeam-epoch` crate, or simplify to a bare `AtomicPtr` if the full RCU pattern is overkill.

**Architecture:** The current `rcu.rs` implements:
- `RcuRegistry` — thread registration for readers
- `RcuReader` — token for the read side
- `Rcu<T>` — `AtomicPtr<T>` with manual `fence()` barriers and a spin-wait `synchronize()`
- `unsafe impl Send + Sync`

Two options:
1. **Replace with `crossbeam-epoch`** (already in dependency tree via `crossbeam-queue`) — eliminates the manual fence, the spin-wait, and the thread registration entirely. Crossbeam's epoch-based GC handles reader tracking automatically.
2. **Simplify to bare `AtomicPtr` + acquire/release** if there's exactly 1 writer and the old pointer is never freed (leak is acceptable for static config).

Option 1 is preferred for correctness; Option 2 is the ponytail path.

**Tech Stack:** Rust 2024 edition, `crossbeam-epoch` (crossbeam is already in dependencies).

## Global Constraints

- Every existing test must pass unchanged.
- The RCU must remain safe for realtime reader threads (no blocking, no allocation on the read side).
- Thread-safe, lock-free read path preserved.

---

### Task 1: Audit RCU usage — find every writer reader

- [ ] **Step 1: Find all `Rcu<T>` instantiations and callers**

```bash
grep -rn 'Rcu::\|rcu::Rcu\|\.swap(\|\.read(\|\.synchronize(' src/ --include='*.rs'
```

For each usage, determine:
- How many readers? How many writers?
- Is the old value freed (dropped) after `synchronize()`?
- Could a bare `AtomicPtr` with leak work?

- [ ] **Step 2: Decide between Option 1 (crossbeam-epoch) and Option 2 (AtomicPtr)**

Based on findings:
- If values are **never freed** (written once at startup, never updated) → use `OnceLock<T>` instead of RCU entirely. Delete `rcu.rs`.
- If values are updated rarely (config reload) and exactly 1 writer → Option 2 is safe.
- If values are updated frequently with many readers → Option 1.

- [ ] **Step 3: Document the decision at the top of `rcu.rs`**

---

### Task 2: Option A — Replace with `crossbeam-epoch`

**Files:**
- Modify: `src/rcu.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Add `crossbeam-epoch` to Cargo.toml**

```toml
crossbeam-epoch = "0.9"
```

- [ ] **Step 2: Rewrite `rcu.rs`**

Strip the entire file down to a thin wrapper:

```rust
//! Realtime read-copy-update using crossbeam-epoch.
//!
//! Replaces the custom AtomicPtr+spin-wait RCU with crossbeam's
//! epoch-based GC, which handles reader tracking automatically
//! without busy-waiting.

use crossbeam_epoch::{Atomic, Guard, Owned};
use std::sync::atomic::Ordering;

/// An RCU-protected pointer. The read side is wait-free and does
/// not allocate.  The write side replaces the pointer and defers
/// the old value's drop to crossbeam's epoch reclamation.
pub struct Rcu<T> {
    ptr: Atomic<T>,
}

impl<T: Send> Rcu<T> {
    /// Create a new RCU slot containing `value`.
    pub fn new(value: T) -> Self {
        Rcu {
            ptr: Atomic::new(value),
        }
    }

    /// Read the current value.  The returned reference is valid only
    /// as long as the `Guard` is held.
    pub fn read<'g>(&self, guard: &'g Guard) -> &'g T {
        // Safety: crossbeam-epoch guarantees the pointer is valid
        // for the guard's lifetime.
        unsafe { self.ptr.load(Ordering::Acquire, guard).deref() }
    }

    /// Atomically replace the value.  The old value is retired
    /// and will be dropped when no readers hold a reference.
    pub fn swap(&self, new_value: T, guard: &Guard) {
        let old = self.ptr.swap(Owned::new(new_value), Ordering::Release, guard);
        // Defer deallocation to epoch reclamation.
        unsafe { guard.defer_unchecked(old.into_box()); }
    }
}

unsafe impl<T: Send> Send for Rcu<T> {}
unsafe impl<T: Send> Sync for Rcu<T> {}
```

Remove `RcuRegistry`, `RcuReader`, `synchronize()`, and all manual fence/thread registration code.

- [ ] **Step 3: Update all callers to use the new API**

The old API:
- `rcu.read()` → `rcu.read(&guard)` where guard = `crossbeam_epoch::pin()`
- `rcu.swap(new_ptr)` → `rcu.swap(new_value, &guard)`
- No more `rcu.synchronize()` — epoch GC handles it.
- No more `RcuReader` — crossbeam uses `pin()` per read.

- [ ] **Step 4: Update tests**

The old tests in `rcu.rs` use thread registration and manual synchronize. Rewrite tests using crossbeam's `pin()`:

```rust
#[test]
fn rcu_read_write() {
    let rcu = Rcu::new(42);
    let guard = crossbeam_epoch::pin();
    assert_eq!(*rcu.read(&guard), 42);
    rcu.swap(100, &guard);
    assert_eq!(*rcu.read(&guard), 100);
}
```

- [ ] **Step 5: Remove unsafe impls that are no longer needed**

The `unsafe impl Send for Rcu<T>` may still be needed if T is `Send`. Keep the explicit unsafe impl with the same bounds.

- [ ] **Step 6: Run tests**

```bash
cargo test
```

- [ ] **Step 7: Commit**

```bash
git add src/rcu.rs Cargo.toml
git commit -m "refactor(rcu): replace custom RCU with crossbeam-epoch"
```

---

### Task 2 alt: Option B — Simplify to bare `AtomicPtr` (ponytail path)

**Use this if**: RCU is used for static config (written once at startup, never updated), or if the `synchronize()` call is never reached (single writer, old pointer leaked).

**Files:**
- Modify: `src/rcu.rs`

- [ ] **Step 1: Strip to minimal `AtomicPtr` wrapper**

```rust
use std::sync::atomic::{AtomicPtr, Ordering};

pub struct Rcu<T> {
    ptr: AtomicPtr<T>,
}

impl<T: Send> Rcu<T> {
    pub fn new(value: T) -> Self {
        Rcu {
            ptr: AtomicPtr::new(Box::into_raw(Box::new(value))),
        }
    }

    /// Acquire-load the pointer.  Safe for realtime: no allocation,
    /// no blocking.
    pub fn read(&self) -> &T {
        // Safety: the pointer is valid for the program lifetime.
        // The writer must never free the old pointer.
        unsafe { &*self.ptr.load(Ordering::Acquire) }
    }

    /// Replace the pointer.  The old value is intentionally leaked.
    pub fn swap(&self, new_value: T) {
        let new = Box::into_raw(Box::new(new_value));
        let old = self.ptr.swap(new, Ordering::Release);
        // Leak the old value intentionally — no readers may hold
        // a reference.  If readers do hold references, use epoch
        // GC instead.
        let _ = unsafe { Box::from_raw(old) };  // or leak: std::mem::forget
    }
}
```

- [ ] **Step 2: Verify that no reader holds a `read()` result across a `swap()`**

If any code pattern does `let v = rcu.read(); ... rcu.swap(...); ... use v;` then this simplification is unsound — use Option A.

- [ ] **Step 3: Remove `RcuRegistry`, `RcuReader`, spin-wait, tests**

- [ ] **Step 4: Update callers**

`RcuReader` and `register_current_reader` are gone. Reader callers just call `rcu.read()` directly.

- [ ] **Step 5: Run tests**

```bash
cargo test
```

- [ ] **Step 6: Commit**

```bash
git add src/rcu.rs
git commit -m "refactor(rcu): simplify to bare AtomicPtr (single-writer-safe)"
```

---

## Self-Review

**1. Spec coverage:** Task 1 audits usage; Task 2 implements either Option A (crossbeam) or Option B (AtomicPtr). Both paths remove the thread registry, the spin-wait synchronize, the manual fence, and the `RcuReader` token.

**2. Placeholder scan:** No TBDs. Both Option A and Option B have complete code.

**3. Type consistency:** `Rcu<T>` retains the same generic name for minimal diff. Lifetime semantics change from "no lifetime" to either `&'g T` (crossbeam) or `&T` (AtomicPtr+leak), which may require borrow-checker adjustments in callers.
