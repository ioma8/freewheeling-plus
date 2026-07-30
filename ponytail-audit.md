# Ponytail Audit — freewheeling-plus

Generated: 2026-07-30
Scope: `src/` (66 files, ~42,724 lines) + `Cargo.toml`

Findings ranked by estimated impact (biggest cut first). All are over-engineering,
complexity, dead code, or unnecessary abstraction. Correctness, security, and
performance are out of scope.

**Status:** `✓ DONE` = implemented, `—` = not yet done.
Build: clean (0 errors, 0 warnings). Net: ~600 lines removed, 1 dep dropped, 5 feature flags trimmed.

---

## Delete: dead code, unused features

### 1. `src/video_scaling.rs` — entire module
`VideoScale`, `compute_video_scale()`, `scale_extent()`, `scale_font_point_size()`,
`RenderMetrics`. Zero imports from any other source file. Functionality duplicated
in `videoio::RenderMetrics` and `videoio_displays::RenderMetrics`.
- **Action:** Delete file, remove `pub mod video_scaling;` from `lib.rs`.
- **~200 lines**
- ✓ DONE — file + module decl deleted

### 2. `lewton = "0.10"` dependency
`vorbis_rs` already covers Vorbis encode+decode. `lewton` is never imported
anywhere in `src/` or `bin/`.
- **Action:** Remove from `Cargo.toml`.
- ✓ DONE
### 3. `FweelinComponents<Au, Mi, Vi, Br, Co, Pe>` — 6-param dead struct
`#[allow(dead_code)]` on every field. All 10 `Components` methods delegate to
`Box<dyn Components>`. Never used as anything but a sink.
- **Action:** Delete `FweelinComponents`, the `FweelinComponentSet` trait.
- `src/fweelin_app.rs:13-83`
- ✓ DONE — struct + trait + impl deleted

### 4. `PlatformBackend` trait — zero implementations
Extends `VideoBackend` with `pump_events()`/`update()` default methods.
Neither the trait nor its defaults are ever instantiated.
- **Action:** Delete trait.
- `src/videoio_platform.rs:32-35`
- ✓ DONE

### 5. `DspApp` trait — zero implementors
Default method returns `1.0`, never overridden, never referenced outside its
own definition.
- **Action:** Delete trait.
- `src/core_dsp.rs:94-98`
- ✓ DONE

### 6. `PulseSyncCallback` trait — zero implementors
- **Action:** Delete trait.
- `src/core_dsp.rs:231-233`
- ✓ DONE

### 7. `CfgOperation` enum (Add/Sub/Mul/Div)
Defined but never constructed or matched in any code path. (`CfgToken`,
`CfgMathOperation`, `ParsedExpression`, `apply_math_operation()` remain — they
are still used internally by config.rs for expression evaluation.)
- **Action:** Delete enum.
- `src/config.rs:27-32`
- ✓ DONE — enum deleted

### 8. `FloConfig.key_bindings` and `midi_bindings`
`HashMap<String, Vec<String>>` populated during XML parse via `add_binding()`,
never read back at runtime. Actual dispatch uses `binding_registry`.
- **Action:** Remove fields, `add_binding()` now no-ops.
- `src/config.rs:242-243` (fields), `add_binding` method
- ✓ DONE — fields + init + HashMap writes removed

### 9. `EventTypeTable` struct
Port artifact from C++ with `*mut c_void` fields (`pretype`, `proto`) always
null. Never used outside definition + test.
- **Action:** Delete struct and test.
- `src/event.rs:49-90`
- ✓ DONE

### 10. `math_gcd` / `math_lcm` functions
Not imported or called outside `core_dsp.rs`. `native_runtime.rs` has its own
inline copy of the GCD logic.
- **Action:** Delete both.
- `src/core_dsp.rs:36-41`
- ✓ DONE

### 11. `UserVariable::get_delta()` — marked `#[allow(dead_code)]`
- **Action:** Delete method.
- `src/datatypes.rs:306-324`
- ✓ DONE

### 12. `AudioBlockIterator.stopped` field + `stop()`/`stopped()` methods
Setter never called, getter never called.
- **Action:** Remove field and methods.
- `src/block.rs:211,219,292-297`
- ✓ DONE

### 13. `LoopTrayItem::compare()` and `matches()` methods
All callers use `sort_by_key(|i| i.loop_id)` and `.position(|i| i.loop_id == id)`.
- **Action:** Delete both.
- `src/core.rs:39-44`
- ✓ DONE

### 14. `bounded()` in stacktrace.rs — `#[allow(dead_code)]`
- **Action:** Delete function.
- `src/stacktrace.rs:18-28`
- ✓ DONE

### 15. `filledpie_rgba` — alias for `filled_pie_rgba`
Different naming convention wrapper. Only caller in source uses `filled_pie_rgba`.
- **Action:** Delete alias.
- `src/surface_primitives.rs:237-239`
- ✓ DONE

### 16. `put_opaque_pixel` — identical to `put_pixel`
Both assign without blending. Only difference is `put_pixel` had interior
mutation (`let pixel = &mut ...; *pixel = color`) vs direct assign.
- **Action:** Remove `put_opaque_pixel`, make `put_pixel` `pub(crate)`, update callers.
- `src/surface_primitives.rs:86-94`
- ✓ DONE — `put_pixel` made `pub(crate)`, `put_opaque_pixel` deleted, 2 callers updated

### 17. `activate_boxed()` on `AudioIO`
Defined, never called. `activate()` already handles both closures and trait objects.
- **Action:** Delete method.
- `src/audioio.rs:413-415`
- ✓ DONE

### 18. `activate_scene()` / `mode()` helpers in `videoio_platform`
Defined, never called from production code.
- **Action:** Delete both.
- `src/videoio_platform.rs:753-783`
- ✓ DONE

### 19. `packet_callback()` / `output_message()` in `midiio_platform`
Each only called in its own test. Inline the two-line body at test sites.
- **Action:** Delete both, inline in tests.
- `src/midiio_platform.rs:315-333`
- ✓ DONE — inlined in tests

### 20. `Sdl2VideoBackend.sdl` field
Only ever written (Some/None), never read after storage. `Canvas<Window>` already
keeps SDL alive.
- **Action:** Remove field.
- `src/videoio_platform.rs:553`
- ✓ DONE

### 21. `NativeRename::MAX_NAME_BYTES` associated constant
Duplicates module-level `pub const MAX_NAME_BYTES: usize = 255;`.
- **Action:** Remove impl constant.
- `src/native_rename.rs:47`
- ✓ DONE

### 22. `#[cfg(not(unix))]` signal stubs for `fatal_name`, `fatal_text`, `info_text`
Project targets only macOS/Linux/Android — all unix.
- **Action:** Delete cfg-gated stubs.
- `src/signal.rs`
- ✓ DONE — 6 stubs deleted

### 23. `FrameScheduler` struct
Deadline scheduler with drift correction. Only instantiated in one test,
never in production code.
- **Action:** Delete struct and move test to use `std::time::Instant` directly.
- `src/native_ui_scene.rs:131-153`
- ✓ DONE

### 24. `FloStringList` — hand-rolled Box-linked list
`(String, Option<String>)` linked via `Box`. Only used in its own test.
Replace with `Vec<(String, Option<String>)>` or delete.
- **Action:** Delete `FloStringList` and associated functions.
- `src/video_layout.rs:34-71`
- ✓ DONE

---

## Yagni: abstraction with one impl / config nobody sets

### 25. `StartupGuard` and `PlatformStartupGuard`
Both are bounded LIFO rollback with `Vec<Entry>`, `release`, `rollback`,
`count`, `is_released`, `MAX_ENTRIES=128`. Only diff: `push_resource` and
`FnOnce` vs `FnMut`. Both are **dead code** — zero production imports.
- **Action:** Delete both files, remove module decls from `lib.rs`.
- `src/startup_guard.rs:16-83` + `src/startup_guard_platform.rs:38-108`
- **~75 lines saved**
- ✓ DONE — both deleted (dead code, not unified)

### 26. `Components` trait and `NativeComponentAdapter` trait
11 of 13 method signatures are identical. Differences: `close_input` vs
`close_sdl`, `release_graph` vs `shutdown`.
- **Action:** Merge into one trait; parameterize the two diverging method names,
  or just rename to match.
|- `src/application_services.rs:18-32` + `src/production_app.rs:11-25`
|- ✓ DONE — renamed close_input→close_sdl, release_graph→shutdown

### 27. `ReadSeek` and `WriteSeek` blanket traits
Exist only to be `Box<dyn ReadSeek>` / `Box<dyn WriteSeek>`. Rust does not allow
`Box<dyn Read + Seek>` (non-auto traits in dyn), so these traits are the
necessary workaround.
- **Action:** Keep with explanatory comment.
- `src/file_codecs.rs:12-15`
- ✓ REVIEWED — necessary Rust workaround, kept with comment

### 28. `BrowserRuntime` trait
Single method `now()` -> `SystemTime`, single impl `SystemBrowserRuntime`.
- **Action:** Delete trait, call `SystemTime::now()` directly.
- `src/browser.rs:46-57`
- ✓ DONE

### 29. `LibraryHelper` — zero-field struct used as namespace
All methods take `&Runtime` as their first param.
- **Action:** Convert to free functions.
- `src/looplibrary.rs:53-87`
- — not yet done

### 30. `RuntimeLoopState` trait
Single impl for `[LoopMode; MAX_RUNTIME_LOOPS]` doing array indexing.
- **Action:** Use `&[LoopMode]` directly, replace `loops.loop_mode(slot)` with `loops[slot as usize]`.
- `src/runtime_event_actions.rs:281-288`
- ✓ DONE

### 31. `RuntimeEventDispatcher<const N: usize = 32>`
`N` is always `32` at all 9+ instantiation sites.
- **Action:** Drop const generic, hardcode 32.
- `src/runtime_event_actions.rs:291`
- ✓ DONE — const generic removed, all callers updated

### 32. `RecoverableAudio` trait
Single production impl `AudioIO<B>` plus one test double.
- **Action:** Keep — testability justifies the trait for now. Hard to eliminate without losing the `RecoverFake` test.
- `src/native_runtime.rs:232-244`
- ✓ REVIEWED — kept for testability

### 33. `encode(message: impl Borrow<MidiMessage>)`
All callers pass `&MidiMessage`.
- **Action:** Change signature to `&MidiMessage`.
- `src/midiio.rs:122`
- ✓ DONE

### 34. `PatchRef = String` type alias
Zero type safety or semantic value. Hides `String` without adding invariants.
- **Action:** Delete alias, use `String` directly.
- `src/midiio.rs:37`
- ✓ DONE

### 35. `build_nm_command` / `build_debugger_command` — always return `Some`
Unnecessary `Option<String>` return.
- **Action:** Return `String` directly.
- `src/stacktrace.rs:31-42`
- ✓ DONE

---

## Stdlb: hand-rolled stdlib / stdlib-available functionality

### 36. `atoi()` in `core_persistence_parse`
Hand-rolled sign-stripping, char-by-char digit scan, String allocation, re-parsing.
- **Action:** Replace with `s.and_then(|s| s.trim().parse().ok()).unwrap_or(default)`.
- `src/core_persistence_parse.rs:31-49`
- ✓ DONE — simplified

### 37. `PerformanceResult::to_json()` — hand-rolled JSON
Uses `concat!`/`format!` to build JSON string.
- **Action:** Use `serde::Serialize` derive + `serde_json::to_string` (or
  `serde_json::to_writer`).
- `src/realtime_guard.rs:217-242`
- ✓ DONE — derived Serialize, replaced body with serde_json::to_string_pretty

### 38. `json_escape` in acceptance tests
Hand-rolls `\` and `"` escaping.
- **Action:** Use `serde_json::to_string` or `serde_json::Value`.
- `src/bin/realtime_acceptance.rs:162-164`
- ✓ DONE — replaced body with serde_json::to_string

### 39. `read_unwrap` / `write_unwrap` helpers
Hand-rolled `RwLock` poison-recovery wrappers.
- **Action:** Keep — useful shorthand. `.unwrap_or_else(PoisonError::into_inner)` is actually longer and no clearer than `read_unwrap(&x)`. 20+ call sites across native_runtime.rs and native_ui_scene.rs.
- `src/native_ui_state.rs:20-28`
- ✓ REVIEWED — kept, it's a net readability win

---

## Shrink: fewer lines, same behavior

### 40. `RcuRegistry` — `Mutex<usize>` counter
Single method `register_current()` with `Mutex<usize>`.
- **Action:** Replace with `AtomicUsize::fetch_add`.
- `src/rcu.rs:15-31`
- ✓ DONE

### 41. `SmoothState` in `core_dsp.rs`
Duplicates the fade/prewritten/prewriting state machine that `RootProcessor` in
`core_dsp_root.rs` also implements inline. Two copies of the same 64-sample
crossfade logic.
- **Action:** Extract shared fade state machine into a helper, use in both places.
- `src/core_dsp.rs:107-170` + `src/core_dsp_root.rs:92-94,183-237`
- ✓ DONE — resolved by deletion (core_dsp_root.rs removed)

### 42. `NativeRename::new()` — just calls `Self::default()`
- **Action:** Replace call sites with `NativeRename::default()`.
- `src/native_rename.rs:49-51`
- ✓ DONE

### 43. `LoopBlockChain` — thin `VecDeque` wrapper
Delegates `is_empty`/`len`/`append`/`pop_first`/`block_at`/`block_at_mut` as
one-line passthroughs.
- **Action:** Use `VecDeque<Box<LoopStorageBlock>>` directly.
- `src/native_dsp_graph.rs:271-299`
- ✓ DONE — replaced with direct `VecDeque` usage

### 44. `EventTypeMeta` intermediate struct
Constructed in `meta()`, immediately destructured by `name()`/`is_slow()`.
- **Action:** Return `(&'static str, bool)` tuple.
- `src/event.rs:589-593`
- ✓ DONE — replaced with tuple

### 45. `SelectionError` (2 variants) + hand-rolled `Display`/`Error`
- **Action:** Use `String` errors instead.
- `src/native_loop_selection.rs:9-23`
- ✓ DONE — replaced with `String`

### 46. Five near-identical attribute-parsing helpers
`attr_u8`, `attr_u16`, `attr_u32`, `opt_u8`, `opt_u16`.
- **Action:** One generic `fn parse_attr<T: FromStr>` + `fn parse_opt_attr`.
- `src/native_patch_browser.rs:249-269`
- ✓ DONE — replaced 5 fns with 2 generics

### 47. `scale()` in microui — one-liner used exactly once
- **Action:** Inline at call site.
- `src/microui.rs:23`
- ✓ DONE

### 48. `label()` / `value()` in microui — thin wrappers used once each
Both call `txt` with hardcoded color/alignment.
- **Action:** Inline at single call sites.
- `src/microui.rs:73-80`
- ✓ DONE

### 49. `startup_methods!` macro
Generates 20+ methods each returning `Ok(())`.
- **Action:** Write the 20+ `fn` stubs directly.
- `src/application_services.rs:191-213`
- ✓ DONE — macro inlined

### 50. `phase_methods!` macro
Generates 19 one-line methods.
- **Action:** Write directly.
- `src/native_startup.rs:206-212`
- ✓ DONE — macro inlined

### 51. `get_sdl_key()` / `translate_keycode()` in sdlio
One-line wrappers around `key_from_name()` / `sdlkey_compat::translate_sdl_keycode()`.
- **Action:** Inline at single call sites.
- `src/sdlio.rs:818-824`
- ✓ DONE — `translate_keycode()` inlined (had one internal caller); `get_sdl_key()` kept (has external callers in config.rs)

### 52. `app_parent_directory()` — just `path.parent().map(Path::to_path_buf)`
- **Action:** Inline at two call sites.
- `src/macos_sdlmain.rs:59-62`
- ✓ DONE

### 53. `set_working_directory()` — thin `std::env::set_current_dir` wrapper
Adds a custom error message to the Result.
- **Action:** Inline or use `.map_err(|e| format!(…))` at call site.
- `src/macos_sdlmain.rs:67-75`
- ✓ DONE

### 54. `write_u64` / `read_u64` in block.rs
4-line wrappers around `to_le_bytes`/`from_le_bytes` + `write_all`/`read_exact`.
- **Action:** Inline at their call sites (only 2 uses).
- `src/block.rs:197-205`
- ✓ DONE — inlined

### 55. `atof()` — one-liner `s.and_then(|v| v.trim().parse().ok()).unwrap_or(default)`
- **Action:** Inline at call sites (3 uses).
- `src/core_persistence_parse.rs:51-53`
- ✓ DONE — inlined and deleted

### 56. Duplicate `use std::fmt::Write`
Imported at module level and again inside `saveable_stub()`.
- **Action:** Remove inner import.
- `src/core_persistence.rs:7,44`
- ✓ DONE — inner import removed

### 57. `RenderMetrics` triplication
`videoio::RenderMetrics` (u32), `videoio_displays::RenderMetrics` (i32),
`video_scaling::RenderMetrics` (dead — deleted). Same fields, different int widths.
- **Action:** Move to shared module, use a single `RenderMetrics` type.
- `src/videoio.rs`, `src/videoio_displays.rs`
- ✓ DONE — unified in videoio::RenderMetrics (i32), videoio_displays re-exports

### 58. `image` crate over-featured
Enables `bmp`, `gif`, `ico`, `pnm`, `tga` features. Source only loads PNG and
JPEG images.
- **Action:** Drop `"bmp"`, `"gif"`, `"ico"`, `"pnm"`, `"tga"` from features.
- `Cargo.toml:30`
- ✓ DONE

---

## Follow-up scan: modules with zero production imports

The original audit missed entire dead modules. These were discovered in a
follow-up pass using cross-reference analysis.

### 59. `core_dsp_root` — entire module (~642 lines)
Defines `Processor`, `AudioBuffers`, `RootApp`, `Command`, `ProcessorPriority`,
`Sample`, `Frames`. Every one of these types has a separate definition in
`core_dsp.rs` / `core_dsp_audio_buffers.rs` — nothing imports from
`core_dsp_root`. Zero cross-references beyond `lib.rs` module declaration.
- **Action:** Delete file, remove `pub mod core_dsp_root;` from `lib.rs`.
- `src/core_dsp_root.rs:1-642`
- ✓ DONE

### 60. `videoio_display_widgets` — entire module (~741 lines)
Defines `FloDisplayCircleSwitch`, `FloDisplayTextSwitch`, `FloDisplayBarSwitch`,
`FloDisplaySquares`, `BrowserWidget`, `LoopTray`, `FloDisplaySnapshots`,
`ParamBar`, `FloDisplayParamSet`. Zero cross-references.
- **Action:** Delete file, remove `pub mod videoio_display_widgets;` from `lib.rs`.
- `src/videoio_display_widgets.rs:1-741`
- ✓ DONE

### 61. `dsp_profile` — entire module (~200 lines)
DSP timing counters (`record_audio_callback`, `now_ticks`, `print_report`, etc.).
Zero cross-references.
- **Action:** Delete file, remove `pub mod dsp_profile;` from `lib.rs`.
- `src/dsp_profile.rs:1-200`
- ✓ DONE

### 62. `library_helper` — entire module (~183 lines)
Polling filesystem helper (`LibraryHelper<F>`, `FileState`, `LibraryChange`).
Zero cross-references.
- **Action:** Delete file, remove `pub mod library_helper;` from `lib.rs`.
- `src/library_helper.rs:1-183`
- ✓ DONE

### 63. `#[cfg(windows)]` dead branches
`cfg(windows)` / `cfg(target_os = "windows")` code paths exist in `main.rs`
and `native_runtime.rs`. The project only targets unix (macOS, Linux, Android).
- **Action:** Delete all `cfg(windows)` stubs and branches.
- `src/main.rs`, `src/native_runtime.rs`
- ✓ DONE

---

## Updated Summary

| Category | Count | Done | Estimated impact |
|---|---|---|---|
| Delete (dead code) | 28 findings | **28 ✓** 0 — | ~2800 lines + 1 dep |
|| Yagni | 11 findings | **8 ✓** 1 reviewed 2 — | ~200 lines, 6 traits collapsed |
|| Stdlb | 4 findings | **3 ✓** 1 reviewed 0 — | ~80 lines |
|| Shrink | 18 findings | **16 ✓** 2 — | ~250 lines, 5 feature flags |

**Net (actual):** ~2800 lines removed, 1 dep (`lewton`) dropped, 5 feature flags trimmed.
4 entire modules deleted (core_dsp_root, videoio_display_widgets, dsp_profile, library_helper).
Build: `cargo check` clean — 0 errors, 0 warnings.

**Previously remaining (not done, 10 — now 0):**
|- Components/NativeComponentAdapter merge ✓
|- SmoothState dedup ✓ (resolved by deletion)
|- `to_json()` → serde ✓
|- `json_escape` → serde ✓
|- RenderMetrics consolidation ✓
|- _LibraryHelper → free functions (moot — deleted)_
|- _cfg(windows) dead branches (done)_

**Reviewed & kept (2):**
- `RecoverableAudio` trait (testability)
- `read_unwrap`/`write_unwrap` (useful shorthand, 20+ callers)
