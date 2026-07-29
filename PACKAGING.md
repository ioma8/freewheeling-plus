# macOS universal packaging

Packaging verifies required resources, architecture, signing, and relocatable
dependencies before producing the DMG.

Run on macOS with Rust’s `aarch64-apple-darwin` and `x86_64-apple-darwin`
targets and pinned `cargo-bundle` 0.11.0:

```sh
cargo install cargo-bundle --version 0.11.0 --locked
./scripts/package-macos-universal.sh
```

The output is
`target/aarch64-apple-darwin/release/bundle/osx/FreeWheeling-<version>-universal.dmg`.
The bundled app contains both arm64 and x86_64 slices. The script
adds microphone purpose text and Finder document declarations, recursively
copies non-system dylibs into `Contents/Frameworks`, rewrites their install
names to `@rpath`, ad-hoc signs the finished bundle, and verifies it. Ad-hoc
signing is for local distribution testing; this process neither claims nor
fabricates Developer ID signing, notarization, or stapling.

## Resource and license inventory

| Bundle path | Source | Purpose/license evidence |
|---|---|---|
| `Contents/MacOS/freewheeling-plus` | release targets | universal arm64 + x86_64 executable |
| `Contents/Info.plist` / `NSMicrophoneUsageDescription` | packaging metadata | required microphone purpose string for macOS consent |
| `Contents/Resources/data/*.xml` | `data` | authoritative configuration, mappings, patches, layouts |
| `Contents/Resources/data/Vera.ttf` | `data/Vera.ttf` | Bitstream Vera Sans 1.10; full embedded notice extracted during packaging |
| `Contents/Resources/data/VeraBd.ttf` | `data/VeraBd.ttf` | Bitstream Vera Sans Bold 1.10; embedded notice must exactly match Vera.ttf |
| `Contents/Resources/licenses/Bitstream-Vera-NOTICE.txt` | both font name tables | distributable font license and required copyright/trademark notice |
| `Contents/Resources/data/basic.sf2` | `data/basic.sf2` | public-domain SoundFont |
| `Contents/Resources/licenses/COPYING` | `COPYING` | project GPL-2.0 text; does not establish asset licenses |
| `Contents/Resources/licenses/AUTHORS` | `AUTHORS` | project attribution |

## `basic.sf2` status

The bundled `data/basic.sf2` is public domain and is included in release
artifacts.

`verify_macos_bundle.py` checks required resources/notices,
`NSMicrophoneUsageDescription` and Finder plist entries, expected Mach-O
architectures, recursively bundled relocatable dependencies, and the final
sealed-resource code-signature structure. Real Finder launch, microphone
consent, MIDI, playback, persistence, and shutdown still require a macOS
acceptance run.

## GitHub release builds

`.github/workflows/release.yml` runs for version tags (`v*`) and manual dispatch.
It uploads an x86_64 Linux tarball and a universal macOS DMG.
