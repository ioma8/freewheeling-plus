# Manual macOS diagnostics

`run_macos_diagnostics.sh` launches the source-tree debug binary and records
an operator-readable troubleshooting directory. It sets `FWEELIN_DATADIR`,
captures a CoreAudio device inventory and best-effort macOS unified-log output,
and extracts audio/callback, SDL/input/event, and Rust stderr rejection lines
from the application output when present. After the input test it also copies
new or modified macOS `.crash` reports when the user can read them.

Run from `freewheeling-plus` on a macOS development machine:

```sh
FWEELIN_DIAGNOSTIC_SECONDS=30 scripts/run_macos_diagnostics.sh
```

Override `FWEELIN_DIAGNOSTIC_APP`, `FWEELIN_DIAGNOSTIC_DATADIR`, or
`FWEELIN_DIAGNOSTIC_OUTPUT` when needed. Interact with the app during the
dwell, then inspect `README.txt`, `coreaudio-devices.txt`,
`coreaudio-unified-log.txt`, `audio-callback-diagnostics.txt`,
`rust-stderr-rejections.txt`, `application.log`, `sdl-input-events.txt`, and
`crash-reports/`. Missing diagnostics are recorded as `unavailable:` and do
not imply that the input test passed.

This is a manual diagnostic aid only. It does not write under
`acceptance-evidence/`, create hashes, attest results, or declare success.
