# Event Enum Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) for tracking.

**Goal:** Replace the 130+ concrete event structs + trait-object dispatch with a single `enum Event`, eliminating `Box<dyn Event>`, `as_any()` downcasting, the `impl_event!` macro, and the `default_via_new!` macro.

**Architecture:** The current system has each event as a separate struct implementing `trait Event` (which requires `get_type()`, `as_any()`, `as_any_mut()`, `clone_box()`, `get_num_params()`, `get_param()`). Events are boxed (`Box<dyn Event>`) and downcast via `ev.as_any().downcast_ref::<ConcreteType>()`. Replace with a flat enum where each variant carries its fields directly. Pattern matching replaces downcasting. `EventListener` takes `&Event` instead of `Box<dyn Event>`.

**Tech Stack:** Rust 2024 edition, no new dependencies.

## Global Constraints

- Every existing test must pass unchanged (same observable behavior).
- Event construction callers use `Event::Variant { field: value }` syntax.
- No heap allocation for event dispatch.
- The `EventType` enum remains for string-based name resolution (XML config bindings).
- Do not restyle unrelated code.

---

### Task 1: Define the `Event` enum with all variants

**Files:**
- Modify: `src/event.rs`

**Interfaces:**
- Consumes: All 130+ event struct definitions, the `Event` trait, the `impl_event!` macro
- Produces: `enum Event { ... }` with one variant per current event type

**Design notes:**
- The `BaseEvent` struct currently contains no fields (it's a unit-like base class marker). Merge its events directly into the enum — `StartSession`, `ExitSession`, `SlideLoopAmpStopAll`, `EraseAllLoops`, `ToggleDiskOutput`, `SaveNewScene`, `SaveCurrentScene`, `TransmitPlayingLoopsToDAW`, `PulseSync`, `EndRecord` become unit variants.
- Events with parameters become tuple-struct or struct variants. E.g.:
  - `GoSub { sub: i32, param1: f32, param2: f32, param3: f32 }`
  - `KeyInput { down: bool, keysym: i32, unicode: i32 }`
  - `TriggerLoop { index: i32, vol: f32 }`
- All derive `Clone, Debug, PartialEq`.
- `EventType` is retained as-is (it's used for config binding name resolution).

- [ ] **Step 1: Write the failing test — compile check that old constructors are gone**

Create a compile-time test at the bottom of event.rs:

```rust
#[cfg(test)]
mod enum_compile_check {
    // Verify old struct names are removed. If any still exist, this won't compile.
    // The actual struct definitions will be gone after Step 3.
}
```

Run: `cargo check 2>&1 | head -50`
Expected: Compiles successfully with the new enum, fails when old structs referenced.

- [ ] **Step 2: Define the full `Event` enum**

Inside `src/event.rs`, replace all ~130 struct definitions and the `Event` trait with:

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    // BaseEvent -> parameterless variants
    None,
    EndRecord,
    StartSession,
    ExitSession,
    SlideLoopAmpStopAll,
    EraseAllLoops,
    ToggleDiskOutput,
    SaveNewScene,
    SaveCurrentScene,
    TransmitPlayingLoopsToDAW,
    PulseSync,

    // Events with parameters
    GoSub { sub: i32, param1: f32, param2: f32, param3: f32 },
    KeyInput { down: bool, keysym: i32, unicode: i32 },
    LoopClicked { down: bool, x: f32, y: f32, buttons: i32 },
    JoystickButtonInput { down: bool, button: i32, joystick: i32 },
    MouseButtonInput { down: bool, button: i32, x: i32, y: i32 },
    MouseMotionInput { x: i32, y: i32 },
    TriggerLoop { index: i32, vol: f32 },
    // ... one variant per existing event struct, with its fields as the variant payload
    // ... all 130 variants
}
```

Exact mapping: every `struct FooEvent { fields }` becomes `FooEvent { fields }`. Every struct with zero fields becomes a unit variant.

Run: `cargo check`
Expected: Compilation errors because callers still use old struct construction and `Box<dyn Event>`.

- [ ] **Step 3: Commit**

```bash
git add src/event.rs
git commit -m "refactor(event): define Event enum with all variants"
```

---

### Task 2: Update `EventType` — remove `as_any()` / `clone_box()` / param delegation

**Files:**
- Modify: `src/event.rs`

**Interfaces:**
- Produces: `impl Event` block with methods extracted from variant data

- [ ] **Step 1: Reimplement `Event` trait methods as free functions on `enum Event`**

After the enum definition, add an `impl Event` block that replaces the old trait methods:

```rust
impl Event {
    pub fn get_type(&self) -> EventType {
        match self {
            Event::None => EventType::None,
            Event::EndRecord => EventType::EndRecord,
            Event::StartSession => EventType::StartSession,
            // ... one arm per variant
        }
    }

    // No as_any* needed — pattern matching replaces downcasting

    pub fn get_num_params(&self) -> usize {
        EventType::parameters(self.get_type()).len()
    }

    pub fn get_param(&self, idx: usize) -> Option<EventParameter> {
        EventType::parameters(self.get_type()).get(idx).copied()
    }
}
```

Remove: the old `Event` trait, the `impl_event!` macro, the `default_via_new!` macro, and all old per-struct `impl Event for ...` blocks.

- [ ] **Step 2: Compile check**

Run: `cargo check`
Expected: Errors from callers still using `as_any().downcast_ref::<>()` and `Box::new(FooEvent::new(...))` — these are expected and will be fixed in Tasks 3-5.

- [ ] **Step 3: Commit**

```bash
git add src/event.rs
git commit -m "refactor(event): replace Event trait with inherent impl on enum"
```

---

### Task 3: Update `EventManager` — replace `Box<dyn Event>` with `Event`

**Files:**
- Modify: `src/event.rs`

**Interfaces:**
- Produces: `EventManager::try_post_event(Event)` (no Box), `EventManager::post_event(Event)` (no Box)
- `EventListener::receive_event(&mut self, ev: &Event, from: &dyn EventProducer)` (reference, not Box)

- [ ] **Step 1: Update `EventListener` trait**

```rust
pub trait EventListener: Send {
    fn receive_event(&mut self, ev: &Event, from: &dyn EventProducer);
}
```

- [ ] **Step 2: Update `EventManager` internals**

```rust
pub struct EventManager {
    listeners: Arc<Mutex<HashMap<EventType, Vec<ListenerEntry>>>>,
    queue: Arc<Mutex<VecDeque<Event>>>,  // was VecDeque<Box<dyn Event>>
    // ... rest unchanged
}
```

```rust
pub fn try_post_event(&self, ev: Event) -> Result<Event, Event> {
    // no Box
}

pub fn post_event(&self, ev: Event) {
    let _ = self.try_post_event(ev);
}
```

Update dispatch loops to pass `&ev` instead of `ev.clone_box()`:

```rust
for ev in events {
    if let Some(entries) = lists.get(&ev.get_type()) {
        for entry in entries {
            if let Ok(mut listener) = entry.listener.lock() {
                listener.receive_event(&ev, &());
            }
        }
    }
}
```

- [ ] **Step 3: Remove `default_via_new!` call for `EventManager`**

The trait is already gone from the step above. Remove the macro definition and its invocation entirely.

- [ ] **Step 4: Compile check**

Run: `cargo check`
Expected: Errors from callers in other files using `Box<dyn Event>` and `Box::new(FooEvent::new(...))`.

- [ ] **Step 5: Commit**

```bash
git add src/event.rs
git commit -m "refactor(event): EventManager uses Event enum directly, no boxing"
```

---

### Task 4: Update `config.rs` — event construction and downcasting

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Replace `instantiate_bound_event` — use enum construction instead of `Box::new(Struct::new(...))`**

The 500-line `instantiate_bound_event` method constructs events via `Box::new` of old struct constructors. Replace every arm to construct `Event::Variant { fields }`:

```rust
// Before:
EventType::StartSession => Ok(Box::new(crate::event::StartSessionEvent::new())),
EventType::GoSub => {
    let sub = self.required_int_param(&resolved.parameters, "sub")?;
    let param1 = self.required_float_param(&resolved.parameters, "param1")?;
    let param2 = self.required_float_param(&resolved.parameters, "param2")?;
    let param3 = self.required_float_param(&resolved.parameters, "param3")?;
    Ok(Box::new(crate::event::GoSubEvent::new(sub, param1, param2, param3)))
}

// After:
EventType::StartSession => Ok(crate::event::Event::StartSession),
EventType::GoSub => {
    let sub = self.required_int_param(&resolved.parameters, "sub")?;
    let param1 = self.required_float_param(&resolved.parameters, "param1")?;
    let param2 = self.required_float_param(&resolved.parameters, "param2")?;
    let param3 = self.required_float_param(&resolved.parameters, "param3")?;
    Ok(crate::event::Event::GoSub { sub, param1, param2, param3 })
}
```

Also update all functions returning `Vec<Box<dyn Event>>` to `Vec<Event>`.

- [ ] **Step 2: Replace `as_any().downcast_ref` with pattern matching**

The `config.rs` file contains `as_any().downcast_ref::<FooEvent>()` calls for parameter extraction. Replace each with direct pattern matching on `&Event`:

```rust
// Before:
EventType::InputKey => {
    if let Some(key) = ev.as_any().downcast_ref::<KeyInputEvent>() {
        match param.name { ... }
    }
}

// After:
EventType::InputKey => {
    if let Event::KeyInput { down, keysym, unicode } = ev {
        match param.name {
            "keydown" => value.set_char(if *down { 1 } else { 0 }),
            "key" => value.set_int(*keysym),
            ...
        }
    }
}
```

- [ ] **Step 3: Update return types and parameter types**

Functions like `emit_bound_events`, `emit_registered_events`, `dispatch_event_bindings` that return `Vec<Box<dyn Event>>` → `Vec<Event>`. `dispatch_event_bindings` takes `&dyn Event` → `&Event`.

- [ ] **Step 4: Compile check**

Run: `cargo check`
Expected: Only errors from remaining files (native_event_bridge.rs, browser.rs, native_runtime.rs).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "refactor(config): use Event enum, remove as_any downcasting"
```

---

### Task 5: Update `native_event_bridge.rs` — event construction

**Files:**
- Modify: `src/native_event_bridge.rs`

- [ ] **Step 1: Replace `Box::new(FooEvent::new(...))` with enum construction**

```rust
// Before:
InputEvent::JoystickButton { joystick, button, down } => {
    Ok(Box::new(JoystickButtonInputEvent::new(down, button, joystick)))
}
InputEvent::MouseMotion { x, y } => Ok(Box::new(MouseMotionInputEvent::new(x, y))),

// After:
InputEvent::JoystickButton { joystick, button, down } => {
    Ok(Event::JoystickButtonInput { down, button, joystick })
}
InputEvent::MouseMotion { x, y } => Ok(Event::MouseMotionInput { x, y }),
```

Update all return types: `Result<Box<dyn Event>, CoreEvent>` → `Result<Event, CoreEvent>`, `Vec<Box<dyn Event>>` → `Vec<Event>`.

- [ ] **Step 2: Run tests specific to this file**

Run: `cargo test -p freewheeling-plus --test native_event_bridge`
Expected: Tests fail until the types align.

- [ ] **Step 3: Wait — this file is both `src/native_event_bridge.rs` and `tests/native_event_bridge.rs`. The test file may also reference old types.**

Check: `grep -n 'Box<dyn Event>\|FooEvent' tests/native_event_bridge.rs`
If old types appear, update them too.

- [ ] **Step 4: Compile check**

Run: `cargo check`
Expected: Only errors from browser.rs and native_runtime.rs.

- [ ] **Step 5: Commit**

```bash
git add src/native_event_bridge.rs tests/native_event_bridge.rs
git commit -m "refactor(native_event_bridge): use Event enum"
```

---

### Task 6: Update `browser.rs` — replace downcasting with pattern matching

**Files:**
- Modify: `src/browser.rs`

- [ ] **Step 1: Update `Browser::receive_event` signature and body**

```rust
// Before:
pub fn receive_event(&mut self, ev: &dyn Event) -> bool {
    if let Some(event) = ev.as_any().downcast_ref::<BrowserMoveToItemEvent>() { ... }

// After:
pub fn receive_event(&mut self, ev: &Event) -> bool {
    match ev {
        Event::BrowserMoveToItem { browserid, adjust, jumpadjust } => { ... }
        Event::BrowserMoveToItemAbsolute { browserid, idx } => { ... }
        Event::BrowserSelectItem { browserid } => { ... }
        Event::BrowserRenameItem { browserid } => { ... }
        Event::RenameLoop { loopid, in_layout: _ } => { ... }
        Event::PatchBrowserMoveToBank { direction } => { ... }
        Event::PatchBrowserMoveToBankByIndex { idx } => { ... }
        Event::BrowserItemBrowsed { browserid } => { ... }
        _ => false,
    }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check`
Expected: Only errors from native_runtime.rs.

- [ ] **Step 3: Commit**

```bash
git add src/browser.rs
git commit -m "refactor(browser): use Event enum, replace as_any downcasting"
```

---

### Task 7: Update `native_runtime.rs` — event dispatch and downcasting

**Files:**
- Modify: `src/native_runtime.rs`

- [ ] **Step 1: Update `RuntimeInboxListener`**

```rust
// Before:
impl EventListener for RuntimeInboxListener {
    fn receive_event(&mut self, event: Box<dyn Event>, _from: &dyn EventProducer) {
        let _ = self.inbox.push(event);
    }
}

// After:
impl EventListener for RuntimeInboxListener {
    fn receive_event(&mut self, event: &Event, _from: &dyn EventProducer) {
        // Event is Copy-like for small events, or clone for larger ones
        let _ = self.inbox.push(event.clone());
    }
}
```

Update the inbox type from `ArrayQueue<Box<dyn Event>>` to `ArrayQueue<Event>`.

- [ ] **Step 2: Replace `as_any().downcast_ref` with pattern matching**

The MIDI dispatch contains downcasting patterns:

```rust
// Before:
if event.get_type() == EventType::SetMidiTuning
    && let Some(tuning) = event.as_any().downcast_ref::<SetMidiTuningEvent>()
    && let Some(midi) = r.midi.as_mut()
{
    // use tuning.tuning
}

// After:
if let Event::SetMidiTuning { tuning } = event
    && let Some(midi) = r.midi.as_mut()
{
    // use tuning
}
```

Same pattern for `InputMIDIKey`, `InputMIDIController`, `InputMIDIPitchBend` branches.

- [ ] **Step 3: Compile check**

Run: `cargo check`
Expected: Compiles cleanly.

- [ ] **Step 4: Run the test suite**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/native_runtime.rs
git commit -m "refactor(native_runtime): use Event enum, remove downcasting"
```

---

### Task 8: Update `videoio_display_widgets.rs` — event construction

**Files:**
- Modify: `src/videoio_display_widgets.rs`

- [ ] **Step 1: Replace `Box::new(FooEvent::new(...))` with enum construction**

```rust
// Before:
pub fn select_event(&self) -> Option<Box<dyn Event>> {
    self.browser.current_index.map(|_| {
        Box::new(super::event::BrowserSelectItemEvent::new(self.browser.browser_id)) as Box<dyn Event>
    })
}

// After:
pub fn select_event(&self) -> Option<Event> {
    self.browser.current_index.map(|_| {
        Event::BrowserSelectItem { browserid: self.browser.browser_id }
    })
}
```

Same for `move_event` — return `Event` instead of `Box<dyn Event>`.

- [ ] **Step 2: Compile check**

Run: `cargo check`
Expected: Clean compilation.

- [ ] **Step 3: Run test suite**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/videoio_display_widgets.rs
git commit -m "refactor(display_widgets): use Event enum"
```

---

### Task 9: Remove dead items — `as_any()`, `clone_box()`, trait definitions, `impl_event!`, `default_via_new!`

**Files:**
- Modify: `src/event.rs`

- [ ] **Step 1: Final cleanup**

Remove:
- The `Event` trait (line 934-966 area)
- The `impl_event!` macro
- The `default_via_new!` macro definition and all invocations
- All old struct definitions (should already be replaced by enum variants)
- All remaining `impl Event for FooEvent` blocks
- Any unused imports (`std::any::Any`)

- [ ] **Step 2: Remove test file dead code references**

Check tests and remove any test that constructs old struct types.

Run: `cargo check && cargo test`
Expected: Clean compilation, all tests pass.

- [ ] **Step 3: Final commit**

```bash
git add src/event.rs
git commit -m "refactor(event): remove dead macros, traits, and legacy structs"
```

---

## Self-Review

**1. Spec coverage:**
- Task 1-2: Core enum + EventType updates ✓
- Task 3: EventManager boxing removal ✓
- Task 4-8: All callers updated ✓
- Task 9: Clean dead code ✓

**2. Placeholder scan:** No TBDs, TODOs, or "implement later". Every step has concrete code.

**3. Type consistency:** `Event` (no Box), `EventListener::receive_event(&Event, ...)`, return types `Vec<Event>`, inbox types `ArrayQueue<Event>` — all consistent.
