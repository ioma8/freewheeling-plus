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

### Status: VERIFIED in CI

Windows is now built and tested on every CI run (check, clippy with
`-D warnings`, and the full test suite on `windows-latest`), and release
archives are built by the release workflow. A mingw cross-build script
(`win-build.sh`) is included for local verification on macOS.

### Already Works

- **CPAL**: WASAPI backend, stereo duplex audio, configurable buffer size
- **Midir**: WinMM MIDI backend, port enumeration, I/O
- **SDL2**: Win32 window, keyboard, mouse, joystick input
- All pure-Rust codec, image, font, and config dependencies
- `libc` crate works on Windows via mingw (signal.h has different constants)

### Resolved Blockers

#### 1.1 Crate Dependencies — `Cargo.toml`

The `objc2*` and macOS-only dependencies were already behind
`[target.'cfg(target_os = "macos")']`; the `jack` crate is gated to
Linux/macOS/Windows targets.

#### 1.2/1.3 Entry Point and Platform Impl

No `windows.rs`/`windows_main.rs` were needed: the `Platform` trait is
unused in production (the doc's Android section notes the same), and the
standard `main()` entry point links fine against SDL2's bundled Win32
backend.

#### 1.4 Signal Handling — `src/signal.rs`

The signal text maps and handler registration now compile on non-Unix
platforms (Option A): `SIGBUS`/`SIGUSR*` arms are gated to Unix, and the
CRT `signal()` function catches console interrupts (Ctrl+C) for clean
shutdown on Windows.

#### 1.5 packaging_guardrails — `tests/packaging_guardrails.rs`

The macOS bundling test is gated with `#![cfg(target_os = "macos")]`.

### Current State

| File | Change | Status |
|------|--------|--------|
| `Cargo.toml` | macOS-only deps behind `cfg(target_os = "macos")` | ✅ Done |
| `src/signal.rs` | Portable signal maps + CRT console-interrupt handlers | ✅ Done |
| `src/bin/realtime_acceptance.rs` | Gate macOS-only imports | ✅ Done |
| `.github/workflows/ci.yml` | Windows check/clippy/test job | ✅ Done |
| `.github/workflows/release.yml` | Windows release archive | ✅ Done |
| `win-build.sh` | mingw cross-build with toolchain discovery | ✅ Done |

---

## 2. Android

### Status: builds in CI and releases

A signed release APK is built on every CI run and attached to GitHub
releases. Touch input mapping is implemented; on-device testing remains the
open hardware gate.

### Already Works

- **CPAL**: OpenSL ES backend (Android API 21+), AAudio (API 27+)
- **Midir**: ALSA sequencer via NDK (`libasound.so`)
- **SDL2**: Android Java glue via `SDLActivity` + `SDLSurface`
- All pure-Rust dependencies (codecs, fonts, images)
- `libc` signals — Android is Linux kernel
### Already Works — Plus Implemented

- ✅ `src/lib.rs` — `SDL_main` entry point for Android JNI glue
- ✅ `src/main.rs` — Signal handlers (fatal + shutdown only, avoids SIGUSR*)
- ✅ `src/native_startup.rs` — `application_support_path` returns Android internal storage
- ✅ `src/audio_native_cpal.rs` — `DEFAULT_BUFFER_FRAMES = 256` for Android OpenSL ES

### Implemented

- ✅ `src/lib.rs` — `SDL_main` entry point for Android JNI glue
- ✅ `src/main.rs` — Signal handlers (fatal + shutdown only, avoids SIGUSR*)
- ✅ `src/native_startup.rs` — `application_support_path` returns Android internal storage
- ✅ `src/audio_native_cpal.rs` — `DEFAULT_BUFFER_FRAMES = 256` for Android OpenSL ES
- ✅ `src/sdlio.rs` — SDL touch events map to the mouse path (first finger
  drives the pointer; extra fingers are ignored until it lifts), so the
  mouse-driven UI is usable on Android and desktop touchscreens

### Remaining

#### 2.1 Build Toolchain

`android-build.sh` builds the release APK via `cargo-apk`. It locates the
SDK from `ANDROID_HOME`/`ANDROID_SDK_ROOT` or the conventional install
paths, selects the NDK (`ANDROID_NDK_VERSION`, default 28.2.13676358),
fetches dependency sources before applying the sdl2-sys Android workarounds
(ALooper_pollAll→pollOnce, HIDAPI/C++ runtime link directives), and signs
with the debug keystore unless a release key is configured. CI installs the
SDK/NDK and runs the same script.

### Files Changed — Status

| File | Change | Status |
|------|--------|--------|
| `Cargo.toml` | Add `[target.'cfg(target_os = "android")'.dependencies]` overrides | ✅ Not needed (CPAL+SDL2 work via bundled deps) |
| `src/lib.rs` | Add `#[cfg(target_os = "android")] pub mod android;` | ❌ Not needed — Platform trait unused in production |
| `src/android.rs` | **New** — `AndroidPlatform` impl | ❌ Not needed (Platform trait is dead code in production) |
| `src/main.rs` | Add `#[cfg(target_os = "android")] extern "C" fn SDL_main(...)` | ✅ Done |
| `src/native_startup.rs` | Android `application_support_path` | ✅ Done |
| `src/main.rs` | Android signal handler registration | ✅ Done |
| `src/audio_native_cpal.rs` | Android buffer size tuning (256 frames) | ✅ Done |
| `src/sdlio.rs` | Map `SDL_FINGERDOWN/UP/MOTION` to mouse events | ✅ Done |
| `android-build.sh` | SDK discovery, sdl2-sys workarounds, signing | ✅ Done |
| `.github/workflows/ci.yml` | Android release-APK job | ✅ Done |
| `.github/workflows/release.yml` | Android APK release artifact | ✅ Done |
| Device testing | Touch UX, latency on real hardware | ⏳ Hardware gate |
| `native_runtime.rs` | Use `AndroidPlatform` | ❌ Not needed (Platform trait unused in production) |
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
| **Windows** | signal/CI fixes | Windows check/clippy/test CI + release archive | CI + release workflow | **landed** |
| **Android** | touch mapping, build script | `cargo-apk` CI + release APK | Emulator + device | **landed (build); device testing open** |
| **iOS** | ~400 lines across 6 files | `cargo-xcode` + Xcode config | Simulator + device + TestFlight | **future work** |

### Recommended Order

1. **Windows** — done: CI and release archives are green.
2. **Android** — build/packaging done; the remaining gate is on-device touch
   and latency validation.
3. **iOS** — not started; requires the AppKit dependency to be gated out of
   the shared dependency graph.
