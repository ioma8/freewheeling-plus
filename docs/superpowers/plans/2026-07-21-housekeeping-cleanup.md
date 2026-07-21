# Housekeeping Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) for tracking.

**Goal:** Apply five small, independent, mechanical cleanups across the codebase: remove delegating wrapper functions, rename misnamed types, delete compatibility aliases, drop a dead macro pattern, and remove C-buffer wrappers.

**Architecture:** Five isolated tasks, each touching at most 2 files. No behavioral changes. Every task is a safe, search-and-replace level refactor.

**Tech Stack:** Rust 2024 edition.

## Global Constraints

- Every existing test must pass unchanged.
- No behavioral change.

---

### Task 1: Remove `get_core_data_type` wrapper function in `datatypes.rs`

**Files:**
- Modify: `src/datatypes.rs`

**The problem:** There's a public function that does nothing but delegate:
```rust
pub fn get_core_data_type(name: &str) -> CoreDataType {
    CoreDataType::from_name(name)
}
```

- [ ] **Step 1: Find all callers**

```bash
grep -rn 'get_core_data_type' src/ --include='*.rs'
```

- [ ] **Step 2: Replace all callers**

Replace `crate::datatypes::get_core_data_type(x)` → `CoreDataType::from_name(x)` or `datatypes::CoreDataType::from_name(x)`.

- [ ] **Step 3: Delete the function**

Remove lines 92-94 from `datatypes.rs`.

- [ ] **Step 4: Run tests**

```bash
cargo test
```

Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git commit -m "chore(datatypes): remove delegating get_core_data_type wrapper"
```

---

### Task 2: Rename `RTDataStruct_Updater` to `RtDataStructUpdater`

**Files:**
- Modify: `src/datatypes.rs`

- [ ] **Step 1: Rename the trait**

```rust
// Before (line 37-39):
#[allow(non_camel_case_types)]
pub trait RTDataStruct_Updater {
    fn update_num_rw_threads(&mut self, new_num_writers: i32);
}

// After:
pub trait RtDataStructUpdater {
    fn update_num_rw_threads(&mut self, new_num_writers: i32);
}
```

Remove the `#[allow(non_camel_case_types)]` attribute.

- [ ] **Step 2: Find and update all references**

```bash
grep -rn 'RTDataStruct_Updater' src/ --include='*.rs'
```

Replace each with `RtDataStructUpdater`.

- [ ] **Step 3: Run tests**

```bash
cargo test
```

- [ ] **Step 4: Commit**

```bash
git commit -m "chore(datatypes): rename RTDataStruct_Updater to RtDataStructUpdater"
```

---

### Task 3: Remove C-compatibility `pub use` aliases in `stacktrace.rs`

**Files:**
- Modify: `src/stacktrace.rs`

- [ ] **Step 1: Find references to C-name aliases**

```bash
grep -rn 'StackTrace\b\|StackTraceInit\b\|StackTraceFromSafeContext\b' src/ --include='*.rs'
```

Check if any file imports or uses these aliases.

- [ ] **Step 2: Remove the aliases**

Delete lines 209-212:
```rust
pub use stack_trace as StackTrace;
pub use stack_trace_from_safe_context as StackTraceFromSafeContext;
pub use stack_trace_init as StackTraceInit;
```

- [ ] **Step 3: Update any callers**

Replace `StackTrace(...)` → `stack_trace(...)`, `StackTraceInit(...)` → `stack_trace_init(...)`, `StackTraceFromSafeContext(...)` → `stack_trace_from_safe_context(...)`.

- [ ] **Step 4: Run tests**

```bash
cargo test
```

- [ ] **Step 5: Commit**

```bash
git commit -m "chore(stacktrace): remove unused C-compatibility aliases"
```

---

### Task 4: Remove C-buffer wrapper functions in `stacktrace.rs`

**Files:**
- Modify: `src/stacktrace.rs`

- [ ] **Step 1: Find references to the wrapper functions**

The wrappers are:
```rust
pub fn stacktrace_build_nm_command(dst: &mut [u8], use_gnu_nm: bool, progname: &str) -> i32
pub fn stacktrace_build_debugger_command(dst: &mut [u8], progname: &str, command_file: &str) -> i32
pub fn stacktrace_parse_nm_symbol_line(line: &str, addr: &mut u64, typ: &mut char, name: &mut [u8]) -> i32
pub fn stacktrace_copy_symbol_name(dst: &mut [u8], src: &str) -> i32
pub fn stacktrace_format_symbol_entry(dst: &mut [u8], ...) -> i32
```

Check usage:
```bash
grep -rn 'stacktrace_build_nm_command\|stacktrace_build_debugger_command\|stacktrace_parse_nm_symbol_line\|stacktrace_copy_symbol_name\|stacktrace_format_symbol_entry' src/ --include='*.rs'
```

- [ ] **Step 2: Remove if unused**

If no external calls exist, delete the wrapper functions. Keep the Rust-native counterparts (`build_nm_command`, `build_debugger_command`, `parse_nm_symbol_line`, `copy_symbol_name`, `format_symbol_entry`) — those are the real implementations.

- [ ] **Step 3: Run tests**

```bash
cargo test
```

- [ ] **Step 4: Commit**

```bash
git commit -m "chore(stacktrace): remove unused C-buffer wrappers"
```

---

### Task 5: Update sound volume constants in `core_dsp.rs`

**Already handled in Plan 2 (Magic Constants → Enums). Skip here.**

(If Plan 2 is executed first, this is redundant.)

- [ ] **Step 1: Verify these are covered by Plan 2**

Check that `SS_NONE`/`SS_START`/`SS_BEAT`/`SS_END`/`SS_ENDED` were converted. If Plan 2 hasn't run yet, document that this is covered there.

---

### Task 6: Import cleanup — remove `use crate::core_dsp_root::DEFAULT` etc from any importer

**Files:**
- Check: all files importing the old consts

- [ ] **Step 1: Find stale imports**

```bash
grep -rn 'use.*DEFAULT\|use.*GLOBAL\|use.*HIPRIORITY\|use.*FINAL' src/ --include='*.rs'
```

If any exist after Plan 2, remove and replace with `ProcessorPriority::Variant`.

- [ ] **Step 2: Run tests**

```bash
cargo test
```

- [ ] **Step 3: Commit (if changes found)**

```bash
git commit -m "chore: remove stale imports after const-to-enum conversion"
```

---

## Self-Review

**1. Spec coverage:**
- Task 1: Remove `get_core_data_type` wrapper ✓
- Task 2: Rename `RTDataStruct_Updater` ✓
- Task 3: Remove C-name aliases ✓
- Task 4: Remove C-buffer wrappers ✓
- Task 5: Already covered by Plan 2 ✓
- Task 6: Import cleanup ✓

**2. Placeholder scan:** No TBDs or TODOs. Each step has the exact code to change.

**3. Type consistency:** No cross-task type dependencies — each task is fully independent.
