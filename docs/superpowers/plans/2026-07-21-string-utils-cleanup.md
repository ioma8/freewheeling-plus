# String Utils API Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) for tracking.

**Goal:** Remove unnecessary `Option<>` wrappers on `&str` and `&[u8]` parameters in `string_utils.rs`. The C++ API accepted `nullptr` for optional string/buffer parameters; Rust's `&str` already handles the empty/absent distinction through its own length, and every caller passes `Some(value)` at every call site.

**Architecture:** Six functions in `string_utils.rs` take `Option<&str>` or `Option<&[u8]>` where the `None` case is either unreachable in practice or trivially handled by an empty/default value. Drop the `Option`, rename parameters from `take &str` directly. Then update all callers.

**Tech Stack:** Rust 2024 edition.

## Global Constraints

- Every existing test must pass unchanged (behavior preserved).
- All `copy_truncate`/`append_truncate` functions: `None` dst currently means "no-op, return 0". After change: callers that passed `None` dst pass a real buffer, or the call is restructured.
- All `alloc_saveable` functions: `None` name currently omits that segment in the filename. After change: empty string produces an empty segment. The edge case of empty-vs-None filename segment must be verified to not exist in practice.

---

### Task 1: Audit callers — find every `None` argument to these functions

**Files:**
- Read-only: `src/string_utils.rs`
- Check: All files referencing the 6 target functions

**The 6 target functions:**
1. `copy_truncate_bytes(dst: Option<&mut [u8]>, src: Option<&[u8]>)`
2. `copy_truncate(dst: Option<&mut [u8]>, src: Option<&str>)`
3. `append_truncate_bytes(dst: Option<&mut [u8]>, src: Option<&[u8]>)`
4. `append_truncate(dst: Option<&mut [u8]>, src: Option<&str>)`
5. `expand_home_path(dst: Option<&mut [u8]>, src: Option<&str>, home_dir: Option<&str>)`
6. `alloc_saveable_stub(basename: Option<&str>, hashtext: Option<&str>, objname: Option<&str>, ext: Option<&str>)`
7. `alloc_saveable_path(library_path: Option<&str>, basename: Option<&str>, hashtext: Option<&str>, objname: Option<&str>, ext: Option<&str>)`
8. `split_token(src: Option<&str>, delim: u8)`
9. `copy_filename_truncate(dst: Option<&mut [u8]>, src: Option<&str>)`

- [ ] **Step 1: Find every external caller**

```bash
grep -rn 'copy_truncate\|append_truncate\|expand_home_path\|alloc_saveable_stub\|alloc_saveable_path\|split_token\|copy_filename_truncate' src/ --include='*.rs' | grep -v 'string_utils.rs' | grep -v 'fn\|pub fn'
```

For each caller, verify what they pass:
- For `None` dst → determine if the caller genuinely needs a no-op path, or always has a buffer.
- For `None` src → determine if the caller ever passes `None`, or always `Some(...)`.

- [ ] **Step 2: Document findings**

List every external caller and whether they ever pass `None`. If any caller passes `None` to a parameter that would become a plain `&str`, that caller must be updated to pass `""` or restructured.

---

### Task 2: Simplify `split_token` — drop outer `Option<&str>`

**Files:**
- Modify: `src/string_utils.rs`

- [ ] **Step 1: Change signature**

```rust
// Before:
pub fn split_token(src: Option<&str>, delim: u8) -> TokenSpan<'_>

// After:
pub fn split_token(src: &str, delim: u8) -> TokenSpan<'_>
```

- [ ] **Step 2: Update implementation**

The old body:
```rust
pub fn split_token(src: Option<&str>, delim: u8) -> TokenSpan<'_> {
    let Some(src) = src else {
        return TokenSpan { begin: "", len: 0, next: None };
    };
    // ...
}
```

New body — simply remove the None guard:
```rust
pub fn split_token(src: &str, delim: u8) -> TokenSpan<'_> {
    let (begin, rest) = match src.split_at_checked(src.find(delim as char).unwrap_or(src.len())) {
        Some((token, after)) => (token, after.strip_prefix(delim as char).unwrap_or("")),
        None => (src, ""),
    };
    let len = c_string_bytes(begin.as_bytes()).len();
    TokenSpan { begin, len, next: Some(rest) }
    // ... rest of implementation
}
```

- [ ] **Step 3: Update test to pass `&str` instead of `Some(str)`**

In the test block (`#[cfg(test)]`), change:
```rust
assert_eq!(split_token(Some("abc;def"), b';'), ...);
```
to:
```rust
assert_eq!(split_token("abc;def", b';'), ...);
```

- [ ] **Step 4: Find and update callers**

```bash
grep -rn 'split_token(' src/ --include='*.rs' | grep -v 'string_utils.rs' | grep -v test
```

Replace `split_token(Some(x), ...)` → `split_token(x, ...)` and `split_token(None, ...)` → use empty string or restructure as needed.

- [ ] **Step 5: Run tests**

```bash
cargo test
```

Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/string_utils.rs
git commit -m "refactor(string_utils): drop Option wrapper from split_token"
```

---

### Task 3: Simplify `copy_truncate` / `append_truncate` family — keep `Option<&mut [u8]>` for dst, drop `Option` for src

**Files:**
- Modify: `src/string_utils.rs`

**Rationale:** The dst `Option<&mut [u8]>` is meaningful — it represents "no output buffer available" as a distinct state from "empty buffer". The src `Option<&[u8]>` / `Option<&str>` is never needed: an empty slice already means "nothing to copy".

- [ ] **Step 1: Change signatures**

```rust
// Before:
pub fn copy_truncate_bytes(dst: Option<&mut [u8]>, src: Option<&[u8]>) -> usize
pub fn copy_truncate(dst: Option<&mut [u8]>, src: Option<&str>) -> usize
pub fn append_truncate_bytes(dst: Option<&mut [u8]>, src: Option<&[u8]>) -> usize
pub fn append_truncate(dst: Option<&mut [u8]>, src: Option<&str>) -> usize
pub fn copy_filename_truncate(dst: Option<&mut [u8]>, src: Option<&str>) -> bool

// After:
pub fn copy_truncate_bytes(dst: Option<&mut [u8]>, src: &[u8]) -> usize
pub fn copy_truncate(dst: Option<&mut [u8]>, src: &str) -> usize
pub fn append_truncate_bytes(dst: Option<&mut [u8]>, src: &[u8]) -> usize
pub fn append_truncate(dst: Option<&mut [u8]>, src: &str) -> usize
pub fn copy_filename_truncate(dst: Option<&mut [u8]>, src: &str) -> bool
```

- [ ] **Step 2: Update implementations**

Remove the `let Some(src) = src else { return 0 };` guards and replace with direct usage:

```rust
pub fn copy_truncate_bytes(dst: Option<&mut [u8]>, src: &[u8]) -> usize {
    let Some(dst) = dst else { return 0 };
    if dst.is_empty() {
        return 0;
    }
    // ... rest unchanged (already uses src directly)
}
```

- [ ] **Step 3: Update internal callers**

Inside `string_utils.rs`, `expand_home_path` calls `copy_truncate_bytes(Some(&mut *dst), Some(...))` and `append_truncate_bytes(Some(&mut *dst), Some(...))`. Change to `copy_truncate_bytes(Some(&mut *dst), ...)`.

`copy_filename_truncate` calls `copy_truncate(dst, src)` internally — update both.

- [ ] **Step 4: Update external callers**

```bash
grep -rn 'copy_truncate\|append_truncate\|copy_filename_truncate' src/ --include='*.rs' | grep -v 'string_utils.rs' | grep -v 'test'
```

Replace `copy_truncate(dst, Some(x))` → `copy_truncate(dst, x)`, etc.

- [ ] **Step 5: Run tests**

```bash
cargo test
```

Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/string_utils.rs
git commit -m "refactor(string_utils): drop Option from copy_truncate/append_truncate src params"
```

---

### Task 4: Simplify `expand_home_path` — keep `Option<&mut [u8]>` for dst, drop `Option` for src and home_dir

**Files:**
- Modify: `src/string_utils.rs`

- [ ] **Step 1: Change signature**

```rust
// Before:
pub fn expand_home_path(dst: Option<&mut [u8]>, src: Option<&str>, home_dir: Option<&str>) -> PathExpandResult

// After:
pub fn expand_home_path(dst: Option<&mut [u8]>, src: &str, home_dir: &str) -> PathExpandResult
```

- [ ] **Step 2: Update implementation**

The function checks `src.strip_prefix('~')` — with `&str` it can't be None, so the check becomes unconditional:

```rust
pub fn expand_home_path(dst: Option<&mut [u8]>, src: &str, home_dir: &str) -> PathExpandResult {
    let Some(dst) = dst else { return PathExpandResult::Truncated };
    if dst.is_empty() {
        return PathExpandResult::Truncated;
    }
    if let Some(src_tail) = src.strip_prefix('~') {
        // expand ~ to home_dir
        let copied = copy_truncate_bytes(Some(&mut *dst), home_dir.as_bytes());
        let expanded = append_truncate_bytes(Some(&mut *dst), src_tail.as_bytes());
        // ...
    } else {
        // no ~ prefix, treat as absolute path
        let copied = copy_truncate_bytes(Some(&mut *dst), src.as_bytes());
        // ...
    }
}
```

- [ ] **Step 3: Update test**

```rust
assert_eq!(expand_home_path(Some(&mut path), "~/x", "/home/a"), PathExpandResult::Ok);
```

- [ ] **Step 4: Find and update callers**

```bash
grep -rn 'expand_home_path' src/ --include='*.rs' | grep -v test
```

Replace `expand_home_path(buf, Some(x), Some(y))` → `expand_home_path(buf, x, y)`.

- [ ] **Step 5: Run tests**

```bash
cargo test
```

- [ ] **Step 6: Commit**

```bash
git add src/string_utils.rs
git commit -m "refactor(string_utils): drop Option from expand_home_path src/home_dir params"
```

---

### Task 5: Simplify `alloc_saveable_stub` / `alloc_saveable_path` — drop `Option` for all string params

**Files:**
- Modify: `src/string_utils.rs`

- [ ] **Step 1: Change signatures**

```rust
// Before:
pub fn alloc_saveable_stub(basename: Option<&str>, hashtext: Option<&str>, objname: Option<&str>, ext: Option<&str>) -> String
pub fn alloc_saveable_path(library_path: Option<&str>, basename: Option<&str>, hashtext: Option<&str>, objname: Option<&str>, ext: Option<&str>) -> String

// After:
pub fn alloc_saveable_stub(basename: &str, hashtext: &str, objname: &str, ext: &str) -> String
pub fn alloc_saveable_path(library_path: &str, basename: &str, hashtext: &str, objname: &str, ext: &str) -> String
```

- [ ] **Step 2: Update implementation**

The old code used `basename.unwrap_or("")` etc. With `&str` those become the direct reference:

```rust
pub fn alloc_saveable_stub(basename: &str, hashtext: &str, objname: &str, ext: &str) -> String {
    format!("{}-{}{}",
        basename,
        hashtext,
        if objname.is_empty() { String::new() } else { format!("-{objname}") }
    ) + ext
}
```

Keeping the exact same format logic as before, but `""` instead of `None`.

- [ ] **Step 3: Update tests**

```rust
assert_eq!(alloc_saveable_stub("loop", "hash", "name", ".wav"), "loop-hash-name.wav");
assert_eq!(alloc_saveable_path("", "loop", "hash", "", ".wav"), "/loop-hash.wav");
```

- [ ] **Step 4: Find and update callers**

```bash
grep -rn 'alloc_saveable_stub\|alloc_saveable_path' src/ --include='*.rs' | grep -v 'string_utils.rs' | grep -v test
```

Rarely used — `core_persistence.rs` has `saveable_stub` and `saveable_path` which delegate to these. Update those callers.

- [ ] **Step 5: Run tests**

```bash
cargo test
```

- [ ] **Step 6: Commit**

```bash
git add src/string_utils.rs
git commit -m "refactor(string_utils): drop Option from alloc_saveable_stub/path params"
```

---

## Self-Review

**1. Spec coverage:** All 6 functions identified in the analysis are covered across Tasks 2-5. Each task has caller audit, signature change, implementation update, and test update.

**2. Placeholder scan:** No TBDs or TODOs. Actual code in every step.

**3. Type consistency:** `copy_truncate(dst, &str)` — dst stays `Option<&mut [u8]>` because "no buffer available" is a real state; src drops `Option`. All callers updated consistently.
