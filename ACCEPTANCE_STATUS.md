# Full-port acceptance status

Overall: **NOT ACCEPTED** (2026-07-14)

`../scripts/run_full_acceptance.sh` is authoritative. The latest run is recorded in `target/full-acceptance.tsv`.

## Passing gates

- formatting, all-target compilation, strict Clippy, full test inventory, and diff checks;
- complete, checksummed C++ codec/DSP/MIDI/persistence/renderer/screenshot/startup fixtures;
- normal macOS native-process launch and clean SIGTERM shutdown were exercised locally;
- the Linux implementation compiles in Ubuntu and its static JACK/ALSA contract tests pass.

## Required gates still open

- Pixel parity fails the 99.5%/delta-2 threshold. Current whole-frame results are 93.627279% (640x480), 95.820417% (configured), 97.419484% (fullscreen), and 89.064372% (HiDPI).
- Two separate real-hardware, two-hour 48 kHz runs at 128 and 256 frames are absent.
- The arm64 bundle is structurally built and signed, but distribution verification fails closed because redistribution permission for `data/basic.sf2` is not established. See `fixtures/license-audit/basic-sf2-provenance.md`.
- Revision-bound macOS real-hardware workflow evidence is absent.
- Revision-bound Linux JACK/ALSA real-hardware or reproducible-virtual workflow evidence is absent. A CI-ready `scripts/linux/run-virtual-workflow.sh` now performs the virtual JACK/MIDI/transport exercise and emits an attestation only after success; it requires a Linux host with JACK tools.

These are acceptance failures, not skipped checks. Unit tests and `--smoke-test` do not satisfy them.
