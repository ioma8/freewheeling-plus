# Cross-Platform Porting Plan

FreeWheeling+ currently runs on macOS (daily-driver) and Linux (tested occasionally). This document maps the work needed for Windows, Android, and iOS.

---

## Current Architecture (Platform Dependencies)

| Layer | Dependency | macOS | Linux | Windows | Android | iOS |
|-------|-----------|-------|-------|---------|---------|-----|
| Audio | CPAL (`audio_native_cpal.rs`) | ✅ CoreAudio | ✅ ALSA | ✅ WASAPI | ✅ AAudio/OpenSL | ✅ AVFoundation |
| Audio (native) | `macos_audio_unit.rs` | ✅ HAL AudioUnit | N/A | N/A | N/A | ❌ RemoteIO needed |
| MIDI | Midir (`midiio_platform.rs`) | ✅ CoreMIDI | ✅ ALSA seq | ✅ WinMM | ❌ (no MidiBackend impl) | ✅ CoreMIDI |
| MIDI (JACK) | `jack.rs` | ✅ Optional | ✅ Optional | N/A | N/A | N/A |
| UI | SDL2 (`sdlio.rs`) | ✅ Cocoa | ✅ X11/Wayland | ✅ Win32 | ✅ SDL Java glue | ✅ UIKit |
| App Platform | `macos.rs` (`CocoaPlatform`) | ✅ AppKit | ❌ | ❌ | ❌ | ❌ |
| Signals | `signal.rs` (`libc`) | ✅ | ✅ | ❌ (no mingw test) | ✅ (Linux kernel) | ✅ (BSDsig) |
| Entry point | `macos_sdlmain.rs` | ✅ SDL_main | ✅ standard | ❌ WinMain | ❌ JNI | ❌ SDL_main (ObjC) |

---

## 1. Windows

### Effort: ~3 days

### Already Works

- **CPAL**: WASAPI backend, stereo duplex audio, configurable buffer size
- **Midir**: WinMM MIDI backend, port enumeration, I/O
- **SDL2**: Win32 window, keyboard, mouse, joystick input
- All pure-Rust codec, image, font, and config dependencies
- `libc` crate works on Windows via mingw (signal.h has different constants)

### Blockers

#### 1.1 Crate Dependencies — `Cargo.toml`

```toml
# Current (objc2 is top-level, always compiled):
[dependencies]
objc2 = "0.6"
objc2-foundation = "0.3"
objc2-app-kit = "0.3"
jack = "0.13.5"              # already Linux-only

# Change to:
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
objc2-foundation = "0.3"
objc2-app-kit = "0.3"
jack = "0.13.5"              # move here too (both macOS and Linux need it)
```

Also gate `coreaudio-sys` and `macos_audio_unit.rs` — already done.

#### 1.2 Entry Point — new `src/windows_main.rs`

SDL2 on Windows expects `WinMain` (or provides `SDL_main`). Create:

```rust
// src/windows_main.rs — gated #[cfg(windows)]
#![cfg(windows)]

#[link_section = ".CRT$XCU"]
#[used]
static INIT_SDL: unsafe extern "C" fn() = init_sdl;

unsafe extern "C" fn init_sdl() {
    // SDL2 sets up console and calls our main
}
```

Or use `sdl2::compile_link()` which provides `SDL_main` via the `sdl2` crate's `bundled` feature (already used). The `main.rs` just needs a `#[cfg(windows)] fn main()` that calls the same `Application::run()`.

#### 1.3 Platform Impl — new `src/windows.rs`

```rust
//! Windows application platform (replaces macos::CocoaPlatform on Windows).

pub struct WindowsPlatform {
    // No Cocoa objects needed
}

impl Platform for WindowsPlatform {
    type Error = String;

    fn application_support_dir(&self) -> Result<PathBuf, Self::Error> {
        // Use %APPDATA%/FreeWheeling or knownfolder FOLDERID_RoamingAppData
        let appdata = std::env::var_os("APPDATA")
            .ok_or_else(|| "APPDATA is not set".to_string())?;
        Ok(Path::new(&appdata).join("FreeWheeling"))
    }

    fn initialize(&mut self) -> Result<(), Self::Error> {
        // Nothing needed — SDL2 handles window creation
        Ok(())
    }

    fn set_menu_and_foreground(&mut self) -> Result<(), Self::Error> {
        // No-op: SDL2 manages the window
        Ok(())
    }

    fn cleanup(&mut self) {
        // No-op
    }
}
```

#### 1.4 Signal Handling — `src/signal.rs`

`libc::sigaction`, `libc::write(STDERR_FILENO, ...)`, and `libc::_exit()` all work on mingw. But Windows native structured exception handling (SEH) is different. Two options:

- **Option A (low effort):** Use `libc` mingw signals (SIGSEGV, SIGFPE, etc.) — works but may not catch all crash types. Test with `#[cfg(windows)]`.
- **Option B (better):** Add `windows-sys` crate and `AddVectoredExceptionHandler` for SEH translation. ~50 lines.

**Recommendation:** Option A for initial port, Option B as follow-up.

#### 1.5 packaging_guardrails — `tests/packaging_guardrails.rs`

The existing macOS bundling test (`bundle_verifier_requires_executable...`) needs a `#[cfg(not(windows))]` gate.

### Files Changed

| File | Change |
|------|--------|
| `Cargo.toml` | Move `objc2*` + `jack` behind `[target.'cfg(target_os = "macos")']` |
| `src/lib.rs` | Add `#[cfg(windows)] pub mod windows;` |
| `src/windows.rs` | **New** — `WindowsPlatform` impl |
| `src/windows_main.rs` | **New** — `WinMain` entry point |
| `src/signal.rs` | `#[cfg(windows)]` path using mingw signals |
| `src/native_runtime.rs` | Use `WindowsPlatform` when `#[cfg(windows)]` |
| `tests/packaging_guardrails.rs` | `#[cfg(not(windows))]` on bundling test |

---

## 2. Android

### Effort: ~2 weeks

### Already Works

- **CPAL**: OpenSL ES backend (Android API 21+), AAudio (API 27+)
- **Midir**: ALSA sequencer via NDK (`libasound.so`)
- **SDL2**: Android Java glue via `SDLActivity` + `SDLSurface`
- All pure-Rust dependencies (codecs, fonts, images)
- `libc` signals — Android is Linux kernel
- Threading (`std::thread`) — works on Android (pthreads)

### Blockers

#### 2.1 Build Toolchain

Android builds require `cargo-ndk` + NDK r26+:

```sh
cargo install cargo-ndk
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo ndk -t arm64-v8a -o app/src/main/jniLibs build --release
```

SDK packaging needs `cargo-apk`:

```sh
cargo install cargo-apk
cargo apk run --release
```

The `sdl2` crate's `bundled` feature compiles SDL2 from source for Android targets. Requires NDK tools on PATH.

#### 2.2 Entry Point — `android_main`

SDL2 on Android expects a JNI entry point named `Java_org_libsdl_app_SDLActivity_nativeInit`. Add to `src/main.rs`:

```rust
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn SDL_main(_argc: i32, _argv: *const *const i8) -> i32 {
    Application::run()
}
```

The `sdl2` crate's `use_main` or `raw` feature controls whether it provides `SDL_main` or expects the caller to. On Android, the Java glue calls `nativeInit` which eventually invokes the app's `SDL_main`.

#### 2.3 Platform Impl — extend `src/windows.rs` or create `src/android.rs`

```rust
//! Android application platform.

pub struct AndroidPlatform;

impl Platform for AndroidPlatform {
    type Error = String;

    fn application_support_dir(&self) -> Result<PathBuf, Self::Error> {
        // Internal storage: /data/data/<package>/
        // SDL2 provides the internal path via JNI, but the simplest
        // approach is a fixed relative path from the app's private dir.
        Ok(PathBuf::from("/data/data/org.freewheeling.freewheeling-plus"))
    }

    fn initialize(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn set_menu_and_foreground(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn cleanup(&mut self) {}
}
```

A more robust approach reads the app's internal path from `SDL_AndroidGetInternalStoragePath()` (via `sdl2::filesystem::pref_path()`).

#### 2.4 Audio Buffer Tuning — `src/audio_native_cpal.rs`

```rust
// Current:
#[cfg(target_os = "macos")]
const DEFAULT_BUFFER_FRAMES: u32 = 16;
#[cfg(not(target_os = "macos"))]
const DEFAULT_BUFFER_FRAMES: u32 = 64;

// Android may need larger buffers for reliable performance:
#[cfg(target_os = "android")]
const DEFAULT_BUFFER_FRAMES: u32 = 256;
```

#### 2.5 MIDI — `MidirMidiBackend` on Android

The `midir` crate supports Android via the `default` feature, using ALSA sequencer through NDK. However, not all Android devices have ALSA MIDI — most use USB MIDI through the Android USB host API. This may require a separate `usb-midi` backend or fallback to no MIDI.

#### 2.6 Screen Resolution and Input

Android touch input comes through SDL2 as `SDL_FINGERDOWN`/`SDL_FINGERUP`/`SDL_FINGERMOTION`. The current `InputEvent` enum handles mouse events — touch events need mapping:

```rust
// In sdlio.rs or sdlkey_compat.rs:
InputEvent::Touch { x: f32, y: f32, down: bool, finger_id: i64 }
```

The FreeWheeling UI is designed for a mouse-driven interface. Touch mapping needs testing but SDL2 handles the translation.

### Files Changed

| File | Change |
|------|--------|
| `Cargo.toml` | Add `[target.'cfg(target_os = "android")'.dependencies]` overrides if needed |
| `src/lib.rs` | Add `#[cfg(target_os = "android")] pub mod android;` |
| `src/android.rs` | **New** — `AndroidPlatform` impl |
| `src/main.rs` | Add `#[cfg(target_os = "android")] extern "C" fn SDL_main(...)` |
| `src/audio_native_cpal.rs` | Android buffer size tuning (256 frames) |
| `src/sdlio.rs` | Optionally handle `SDL_FINGERDOWN/UP/MOTION` |
| `native_runtime.rs` | Use `AndroidPlatform` when `#[cfg(target_os = "android")]` |
| `Cargo.toml` | Move `objc2*` + `jack` behind macOS-only cfg |

---

## 3. iOS

### Effort: ~2-3 weeks

### Already Works

- **CPAL**: AVFoundation audio on iOS (stereo in/out, configurable buffer)
- **Midir**: CoreMIDI via `midir` crate (MIDI over Bluetooth LE, USB MIDI)
- **SDL2**: UIKit window via `SDL_UIKitAppDelegate`
- Pure-Rust codecs, fonts, images, config
- `std::thread` — works on iOS (POSIX threads)

### Blockers

#### 3.1 AppKit → UIKit — `src/macos.rs`

The entire `CocoaPlatform` uses AppKit (`NSApplication`, `NSAutoreleasePool`, `NSApplicationActivationPolicy`). iOS uses UIKit (`UIApplication`, `UIAutoreleasePool`, no activation policy concept).

**Option A (recommended):** Create `src/ios.rs` with a `UIKitPlatform`, keep `macos.rs` macOS-only.

```rust
//! iOS UIKit application platform.

pub struct UIKitPlatform {
    pool: Option<objc2::rc::Retained<NSAutoreleasePool>>,
}

impl Platform for UIKitPlatform {
    type Error = String;

    fn application_support_dir(&self) -> Result<PathBuf, Self::Error> {
        // Use NSSearchPathForDirectoriesInDomains(NSDocumentDirectory, ...)
        // via objc2-ui-kit or raw CoreFoundation C API.
        // Fallback: SDL_GetPrefPath()
        let sdl = sdl2::filesystem::pref_path("FreeWheeling", "freewheeling-plus")
            .map_err(|e| format!("SDL pref path: {e}"))?;
        Ok(PathBuf::from(sdl))
    }

    fn initialize(&mut self) -> Result<(), Self::Error> {
        // SAFETY: main-thread autorelease pool (same as macOS)
        self.pool = Some(unsafe { NSAutoreleasePool::new() });
        Ok(())
    }

    fn set_menu_and_foreground(&mut self) -> Result<(), Self::Error> {
        // No-op: iOS has no menu bar. SDL2 handles full-screen.
        Ok(())
    }

    fn cleanup(&mut self) {
        drop(self.pool.take());
    }
}
```

Requires `objc2-ui-kit = "0.3"` in `[target.'cfg(target_os = "ios")'.dependencies]`:
- `NSAutoreleasePool` → from `objc2-foundation` (already a dep)
- `NSSearchPathForDirectoriesInDomains` → `objc2-foundation::NSFileManager` or raw `libc` `NSSearchPathForDirectoriesInDomains` via `objc2-ui-kit`

**Important:** The `objc2-app-kit` crate must be gated to macOS only — it won't compile for iOS (AppKit headers don't exist in the iOS SDK).

#### 3.2 AudioUnit Backend — `src/macos_audio_unit.rs` iOS Variant

The `MacosAudioUnitBackend` uses `kAudioUnitSubType_HALOutput` which is macOS-only. iOS uses `kAudioUnitSubType_RemoteIO`. The two subtypes have different:
- Property constants (`kAudioOutputUnitProperty_EnableIO` has different scope defaults)
- Audio session requirements (`AVAudioSession` must be configured before any audio I/O)
- Buffer sizes (iOS typically uses smaller callbacks, 256-1024 frames)

**Option A (recommended):** Gate `macos_audio_unit.rs` to `#[cfg(target_os = "macos")]` only. Let iOS use CPAL (`audio_native_cpal.rs`) which uses AVFoundation. This avoids rewriting the AudioUnit backend and CPAL provides good latency on iOS.

If lower latency is needed, create `src/ios_audio_unit.rs`:
```rust
//! iOS RemoteIO AudioUnit backend.
//! Uses kAudioUnitSubType_RemoteIO for low-latency capture/playback.
//! Requires AVAudioSession configuration before AudioUnit creation.

// Key differences from MacosAudioUnitBackend:
// - kAudioUnitSubType_RemoteIO instead of kAudioUnitSubType_HALOutput
// - Input bus is bus 1, output bus is bus 0 (opposite of HAL)
// - Must call AVAudioSession.setCategory(.playAndRecord) first
// - No AudioComponent discovery for default device
// - AudioSession route changes are the only reconfiguration path
```

#### 3.3 Audio Session — new `src/ios_audio_session.rs`

iOS requires `AVAudioSession` configuration before any audio I/O:

```rust
//! AVAudioSession configuration for iOS audio backends.
//! Must be called before creating any AudioUnit or audio stream.

pub fn configure_audio_session() -> Result<(), String> {
    // Using objc2-audio-toolbox or raw CoreAudio C API:
    // AVAudioSession *session = [AVAudioSession sharedInstance];
    // [session setCategory:AVAudioSessionCategoryPlayAndRecord
    //            withOptions:AVAudioSessionCategoryOptionAllowBluetooth
    //                  error:nil];
    // [session setActive:YES error:nil];
    // double rate = [session preferredSampleRate];
    Ok(())
}
```

This can be done with `coreaudio-sys` (already a dep) using C functions, or with `objc2-audio-toolbox`.

#### 3.4 Signal Handling — `src/signal.rs`

iOS App Store policy restricts signal handlers. `libc::sigaction` exists on iOS but:
- `SIGSEGV`/`SIGBUS`/`SIGILL`/`SIGFPE` handlers are allowed for crash reporting
- `SIGUSR1`/`SIGUSR2` should be avoided (used by system frameworks)
- `libc::write(STDERR_FILENO)` works but stderr may not be visible in release builds

**Fix:** Add `#[cfg(target_os = "ios")]` guard on signal registration. Keep fatal-signal handlers (for crash logs), skip info-signal handlers.

#### 3.5 Entry Point — `src/main.rs`

SDL2 on iOS uses `SDL_main` linked from an Objective-C trampoline provided by the `sdl2` crate's bundled build. Same pattern as macOS (`macos_sdlmain.rs` is unnecessary on iOS — SDL2 handles it).

The `main.rs` on iOS just needs:
```rust
#[cfg(target_os = "ios")]
pub fn main() {
    Application::run()
}
```
SDL2's iOS glue calls this automatically.

#### 3.6 Packaging — `cargo-xcode`

```sh
cargo install cargo-xcode
cargo xcode
```

This generates an `.xcodeproj` that can be opened in Xcode for archiving, code signing, and App Store distribution. Requires:
- Apple Developer account
- `Info.plist` with microphone usage description (`NSMicrophoneUsageDescription`)
- `Entitlements.plist` for audio background modes

#### 3.7 App Sandbox

iOS apps run in a sandbox. File paths must use `NSSearchPathForDirectoriesInDomains` or `SDL_GetPrefPath`. Hardcoded paths (like `~/.fweelin`) will fail.

The `Platform::application_support_dir()` must return the app's Documents directory:
```
/var/mobile/Containers/Data/Application/<UUID>/Documents/
```

#### 3.8 Missing `#[cfg]` on `macos_sdlmain.rs`

```rust
// lib.rs:
#[cfg(target_os = "macos")]
pub mod macos_sdlmain;
```

Without this gate, it will try to compile `#include <SDL.h>` on iOS (which works if SDL2 is built for iOS, but the Objective-C trampoline is different).

### Files Changed

| File | Change |
|------|--------|
| `Cargo.toml` | Gate `objc2-app-kit` + `objc2` to `[target.'cfg(target_os = "macos")']`; add `objc2-ui-kit` for iOS |
| `Cargo.toml` | Gate `macos_audio_unit.rs` dependencies to `[target.'cfg(target_os = "macos")']` (already done for `coreaudio-sys`) |
| `src/lib.rs` | Add `#[cfg(target_os = "ios")] pub mod ios;`; gate `macos_sdlmain` |
| `src/ios.rs` | **New** — `UIKitPlatform` impl |
| `src/ios_audio_session.rs` | **New** — `AVAudioSession` configuration |
| `src/macos.rs` | Move entire module behind `#[cfg(target_os = "macos")]` |
| `src/macos_audio_unit.rs` | Already gated — confirm it's `#[cfg(target_os = "macos")]` ✅ |
| `src/signal.rs` | Skip info-signal handlers on iOS |
| `src/native_runtime.rs` | Use `UIKitPlatform` when `#[cfg(target_os = "ios")]` |
| `tests/packaging_guardrails.rs` | Skip macOS bundling test on iOS |
| `Info.plist` | **New** — iOS usage descriptions |
| `Entitlements.plist` | **New** — audio background entitlements |

---

## Dependency Gate Summary

```toml
# Current (objc2 is top-level):
[dependencies]
objc2 = "0.6"
objc2-foundation = "0.3"
objc2-app-kit = "0.3"

# Required:
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
objc2-foundation = "0.3"
objc2-app-kit = "0.3"

[target.'cfg(target_os = "ios")'.dependencies]
objc2 = "0.6"              # still needed for NSAutoreleasePool
objc2-foundation = "0.3"
objc2-ui-kit = "0.3"       # replaces objc2-app-kit

[target.'cfg(any(target_os = "linux", target_os = "macos"))'.dependencies]
jack = "0.13.5"            # already partially done
```

---

## Module Gate Summary

```rust
// lib.rs:
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub mod macos_sdlmain;
#[cfg(target_os = "macos")]
pub mod macos_audio_unit;        // already done

#[cfg(windows)]
pub mod windows;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(target_os = "ios")]
pub mod ios;
```

---

## Effort Summary

| Platform | Code Changes | Build Infrastructure | Testing | Total |
|----------|-------------|---------------------|---------|-------|
| **Windows** | ~200 lines across 4 files | None (cargo build works) | Manual smoke test | **~3 days** |
| **Android** | ~150 lines across 4 files | `cargo-ndk` + `cargo-apk` setup | Emulator + device | **~2 weeks** |
| **iOS** | ~400 lines across 6 files | `cargo-xcode` + Xcode config | Simulator + device + TestFlight | **~2-3 weeks** |

### Recommended Order

1. **Windows** — unlocks the largest user base with the least effort. Most of the code already works.
2. **iOS** — removes the AppKit dependency from the shared dep graph, making Windows cleaner too. CPAL on iOS works immediately, AudioUnit optimization can follow.
3. **Android** — requires build toolchain investment but the audio/MIDI stack is already portable.
