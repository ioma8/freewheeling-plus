# Magic Constants to Enums Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) for tracking.

**Goal:** Replace C++-style `const i32` pseudo-enums with proper Rust `enum` types in `core_dsp.rs`, `core_dsp_root.rs`, and `native_dsp_graph.rs`.

**Architecture:** Three independent groups of constants become enums: the Pulse clock-run states (`SS_NONE`/`SS_START`/`SS_BEAT`/`SS_END`/`SS_ENDED`), the processor chain priorities (`DEFAULT`/`GLOBAL`/`GLOBAL_SECOND_CHAIN`/`HIPRIORITY`/`FINAL`), and the `dsp_profile.rs` pseudo-enums. Each conversion is a safe mechanical change: define `#[repr(i32)]` enum, replace literals with named variants, add `From<i32>`/`Into<i32>` for any cross-boundary conversion.

**Tech Stack:** Rust 2024 edition.

## Global Constraints

- Every existing test must pass unchanged.
- No behavioral change at the i32 representation level (all conversions must preserve exact i32 values).
- Pure Rust files — no C FFI involved for these enums.

---

### Task 1: Replace `SS_NONE`, `SS_START`, `SS_BEAT`, `SS_END`, `SS_ENDED` with a proper enum

**Files:**
- Modify: `src/core_dsp.rs`

**Interfaces:**
- Produces: `enum SyncState` replacing the 5 consts, with exact i32 mapping for any FFI

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod sync_state_tests {
    use super::*;

    #[test]
    fn sync_state_repr_values() {
        assert_eq!(SyncState::None as i32, 0);
        assert_eq!(SyncState::Start as i32, 1);
        assert_eq!(SyncState::Beat as i32, 2);
        assert_eq!(SyncState::End as i32, 3);
        assert_eq!(SyncState::Ended as i32, 4);
    }

    #[test]
    fn sync_state_from_i32() {
        assert_eq!(SyncState::from(0), SyncState::None);
        assert_eq!(SyncState::from(3), SyncState::End);
        assert_eq!(SyncState::from(5), SyncState::None); // default fallback
    }
}
```

Run: `cargo test sync_state -p freewheeling-plus 2>&1`
Expected: COMPILATION ERROR — no `SyncState` enum yet.

- [ ] **Step 2: Define the enum**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SyncState {
    None = 0,
    Start = 1,
    Beat = 2,
    End = 3,
    Ended = 4,
}

impl From<i32> for SyncState {
    fn from(value: i32) -> Self {
        match value {
            0 => SyncState::None,
            1 => SyncState::Start,
            2 => SyncState::Beat,
            3 => SyncState::End,
            4 => SyncState::Ended,
            _ => SyncState::None,
        }
    }
}
```

- [ ] **Step 3: Remove the old consts**

Delete lines 13-17:
```rust
pub const SS_NONE: i32 = 0;
pub const SS_START: i32 = 1;
pub const SS_BEAT: i32 = 2;
pub const SS_END: i32 = 3;
pub const SS_ENDED: i32 = 4;
```

- [ ] **Step 4: Search for all usages of the old consts**

Run: `grep -rn 'SS_NONE\|SS_START\|SS_BEAT\|SS_END\|SS_ENDED' src/`
Expected: Only comments mentioning them (in native_dsp_graph.rs). No code-level usage of the raw consts. (If there are any, update them to `SyncState::Variant`.)

- [ ] **Step 5: Update the `native_dsp_graph.rs` comment reference**

In `src/native_dsp_graph.rs`, update the comment on `ClockRun` and the field `clock_run` to reference `SyncState::None`/`Start`/`Beat` instead of `SS_NONE`/`SS_START`/`SS_BEAT`.

- [ ] **Step 6: Run tests**

```bash
cargo test
```

Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/core_dsp.rs src/native_dsp_graph.rs
git commit -m "refactor(dsp): replace SS_NONE/SS_START/SS_BEAT magic constants with SyncState enum"
```

---

### Task 2: Replace `DEFAULT`, `GLOBAL`, `GLOBAL_SECOND_CHAIN`, `HIPRIORITY`, `FINAL` with a proper enum

**Files:**
- Modify: `src/core_dsp_root.rs`

**Interfaces:**
- Produces: `enum ProcessorPriority` replacing the 5 consts

- [ ] **Step 1: Write the failing test**

Insert in `core_dsp_root.rs`:

```rust
#[cfg(test)]
mod priority_tests {
    use super::*;

    #[test]
    fn priority_repr_values() {
        assert_eq!(ProcessorPriority::Default as i32, 0);
        assert_eq!(ProcessorPriority::Global as i32, 1);
        assert_eq!(ProcessorPriority::GlobalSecondChain as i32, 2);
        assert_eq!(ProcessorPriority::HiPriority as i32, 3);
        assert_eq!(ProcessorPriority::Final as i32, 4);
    }
}
```

Run: `cargo test priority -p freewheeling-plus 2>&1`
Expected: COMPILATION ERROR.

- [ ] **Step 2: Define the enum and `add_child` update**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ProcessorPriority {
    Default = 0,
    Global = 1,
    GlobalSecondChain = 2,
    HiPriority = 3,
    Final = 4,
}
```

Update the `RootProcessor::add_child` signature from `(name: i32)` to `(priority: ProcessorPriority)`:

```rust
// In impl<A: RootApp, Q: CommandQueue> RootProcessor<A, Q>:
pub fn add_child(
    &mut self,
    processor: Box<dyn Processor>,
    priority: ProcessorPriority,
    start: bool,
) -> bool {
```

The function body uses `priority as usize` for array indexing — this still works with `#[repr(i32)]`.

- [ ] **Step 3: Remove the old consts**

Delete lines 11-15:
```rust
pub const DEFAULT: i32 = 0;
pub const GLOBAL: i32 = 1;
pub const GLOBAL_SECOND_CHAIN: i32 = 2;
pub const HIPRIORITY: i32 = 3;
pub const FINAL: i32 = 4;
```

- [ ] **Step 4: Replace all usages inside `core_dsp_root.rs`**

Replace every `DEFAULT` → `ProcessorPriority::Default`, `GLOBAL` → `ProcessorPriority::Global`, etc. There are ~15 usages in the file body and tests.

- [ ] **Step 5: Check for external callers**

The consts are only used within `core_dsp_root.rs` (verified via grep earlier). No external files use `core_dsp_root::DEFAULT`. If imported via `use crate::core_dsp_root::DEFAULT`, update those imports.

- [ ] **Step 6: Run tests**

```bash
cargo test
```

Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/core_dsp_root.rs
git commit -m "refactor(dsp): replace DEFAULT/GLOBAL/HIPRIORITY magic constants with ProcessorPriority enum"
```

---

### Task 3: Check `dsp_profile.rs` for similar patterns

**Files:**
- Check: `src/dsp_profile.rs`

- [ ] **Step 1: Scan for const pseudo-enums**

```bash
grep -n 'pub const.*: i32' src/dsp_profile.rs
```

If any const groups represent mutually exclusive states, convert to enum. If they're just independent configuration constants, leave them.

- [ ] **Step 2: Apply same pattern if applicable**

Follow the same `#[repr(i32)] enum` pattern from Tasks 1-2.

- [ ] **Step 3: Commit**

```bash
git add src/dsp_profile.rs
git commit -m "refactor(dsp): convert dsp_profile constants to enum"
```

---

## Self-Review

**1. Spec coverage:** All three const groups covered (SS_*, processor priorities, dsp_profile). Trivial, safe, isolated.

**2. Placeholder scan:** No TBDs or TODOs.

**3. Type consistency:** All enums are `#[repr(i32)]`, all `as i32` casts preserve exact values, all `From<i32>` impls have a default fallback for safety.
