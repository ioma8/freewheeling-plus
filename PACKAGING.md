# macOS arm64 packaging

Packaging fails closed when required resource-license evidence, architecture,
signing, or relocatable dependency checks are missing.

Run on Apple Silicon with Rust’s `aarch64-apple-darwin` target and pinned
`cargo-bundle` 0.11.0. Packaging deliberately stops before compiling unless a
reviewed distribution license for `data/basic.sf2` is supplied:

```sh
cargo install cargo-bundle --version 0.11.0 --locked
BASIC_SF2_LICENSE_FILE=/reviewed/basic.sf2-LICENSE.txt \
  ./scripts/package-macos-arm64.sh
```

The output is
`target/aarch64-apple-darwin/release/bundle/osx/FreeWheeling.app`. The script
adds microphone purpose text and Finder document declarations, recursively
copies non-system dylibs into `Contents/Frameworks`, rewrites their install
names to `@rpath`, ad-hoc signs the finished bundle, and verifies it. Ad-hoc
signing is for local distribution testing; this process neither claims nor
fabricates Developer ID signing, notarization, or stapling.

## Resource and license inventory

| Bundle path | Source | Purpose/license evidence |
|---|---|---|
| `Contents/MacOS/freewheeling-plus` | release target | arm64 executable |
| `Contents/Info.plist` / `NSMicrophoneUsageDescription` | packaging metadata | required microphone purpose string for macOS consent |
| `Contents/Resources/data/*.xml` | `../data` | authoritative configuration, mappings, patches, layouts |
| `Contents/Resources/data/Vera.ttf` | `../data/Vera.ttf` | Bitstream Vera Sans 1.10; full embedded notice extracted during packaging |
| `Contents/Resources/data/VeraBd.ttf` | `../data/VeraBd.ttf` | Bitstream Vera Sans Bold 1.10; embedded notice must exactly match Vera.ttf |
| `Contents/Resources/licenses/Bitstream-Vera-NOTICE.txt` | both font name tables | distributable font license and required copyright/trademark notice |
| `Contents/Resources/data/basic.sf2` | `../data/basic.sf2` | soundfont; distribution remains blocked without separately reviewed evidence |
| `Contents/Resources/licenses/basic.sf2-LICENSE.txt` | `BASIC_SF2_LICENSE_FILE` | mandatory reviewed distribution evidence |
| `Contents/Resources/licenses/COPYING` | `../COPYING` | project GPL-2.0 text; does not establish asset licenses |
| `Contents/Resources/licenses/AUTHORS` | `../AUTHORS` | project attribution |

## `basic.sf2` provenance finding

The SoundFont INFO chunk identifies the bank only as `example`, identifies
SWAMI 0.9.1a as the producing tool, and leaves copyright and comment metadata
empty. Git history shows an unexplained binary import in revision
`5bf4c275a3dec39410ee130a0d90384be3ef6388` on 2007-01-03 and a later mode-only
change. Neither the editor name, filename, project GPL, nor repository presence
proves permission to redistribute its samples. The packager therefore fails
closed when reviewed license evidence is absent.

This gate remains intentional. The 2026-07-14 audit found no asset-specific
grant for the existing `basic.sf2`; its SHA-256 is
`2e6cf4a8a1d78e6be3b00a0c22358d3ceec8c5a27a000714e65215e3f9b1d15a`. FluidR3
GM is documented as redistributable, but does not preserve this bank's
`El Cheapo Organ` preset contract and has not been silently substituted.

`verify_macos_bundle.py` checks all non-license gates first: required
resources/notices, `NSMicrophoneUsageDescription` and Finder plist entries,
arm64-only Mach-O files, recursively bundled relocatable dependencies, and the
final sealed-resource code-signature structure. Only after those pass does it
check the separately reviewed `basic.sf2` license evidence. Consequently, the
currently built arm64 app passes every technical bundle gate and reports the
missing SoundFont license as its sole distribution blocker. Real Finder launch,
microphone consent, MIDI, playback, persistence, and shutdown still require a
macOS acceptance run.
