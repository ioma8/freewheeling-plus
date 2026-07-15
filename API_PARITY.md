# API parity audit

Audit date: 2026-07-14. Scope: original `src/fweelin_*.h`, `.c`, and `.cc`
declarations compared with `freewheeling-plus/src/*.rs`.

## Re-audit findings

All nine previously flagged helper surfaces now have public Rust counterparts:
`EventTypeTable`, `InputMatrix`, `RTDataStruct_Updater`, `BrowserDivision`,
`RenameUIVars`, `ItemRenamer`, `LoopTrayItem`, `AutoWriteControl`, and
`AutoReadControl` (`src/event.rs`, `src/config.rs`, `src/datatypes.rs`,
`src/browser.rs`, `src/core.rs`, and `src/block_managers.rs`). Focused tests
cover defaults, rename/tray behavior, and automatic manager lifecycle. The
control types are Rust trait boundaries, not C++ ABI-compatible classes.

The prior codec streaming/stereo gap is closed for WAV and Vorbis:
`write_samples_to_disk` writes fragments to the live encoder and `read_samples`
copies both channels through `AudioBlockIterator`. `tests/codec_pipeline.rs`
covers incremental writes, mono/stereo managed chains, finalization errors, and
recycling. The full `cargo test --manifest-path freewheeling-plus/Cargo.toml`
run passes.

FLAC and AU now have real mono/stereo encoders and decoders. Format-signature
and round-trip tests cover both; AU writes sample data incrementally and fixes
its header length on close. The pure-Rust FLAC encoder assembles its encoded
stream at finalization, so long FLAC recordings have a higher memory footprint
than the original libsndfile implementation. This is a documented performance
risk, not a missing API or a format stub.

## Remaining boundary and risk

Host audio, MIDI, SDL/video, mixer, synth, and OSC implementations remain
injected through the compatibility traits documented in `MIGRATION_STATUS.md`.
The FLAC finalization memory footprint is the main remaining performance risk.

## Acceptance verdict

**ACCEPTED for the migrated portable Rust surface.** All 30 top-level C/C++
implementation files have Rust counterparts, the previously missing public
helper surfaces are present, all four configured file formats work, and the
formatting, clippy, full test, golden-parity, threading, and macOS smoke gates
pass. This is source/behavioral compatibility through documented Rust traits;
it is not C++ ABI compatibility or a claim that one concrete hardware backend
is portable across every host.

## Evidence commands

```sh
rg --files -g 'src/fweelin_*.cc' -g 'src/fweelin_*.c' -g 'src/fweelin_*.h' \\
  -g 'freewheeling-plus/src/*.rs'
rg -n 'EventTypeTable|InputMatrix|RTDataStruct_Updater|BrowserDivision|RenameUIVars|ItemRenamer|LoopTrayItem|AutoWriteControl|AutoReadControl' \\
  src/fweelin_* freewheeling-plus/src freewheeling-plus/tests
cargo test --manifest-path freewheeling-plus/Cargo.toml
cargo clippy --manifest-path freewheeling-plus/Cargo.toml --all-targets --all-features -- -D warnings
```
