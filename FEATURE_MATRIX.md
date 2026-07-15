# Feature and acceptance matrix

“Automated” identifies a checked-in test. “Hardware” is a release gate and cannot be replaced by `--smoke-test`. C++ compatibility rows require genuine fixtures captured as described in `fixtures/cpp-golden/README.md`.

| Original workflow | Automated coverage | Hardware/release acceptance |
|---|---|---|
| Startup, rollback, clean shutdown | `core_startup_test`, `startup_guard_platform_test`, `macos_acceptance` | normal Finder `.app` launch and clean quit |
| Record and trigger loop | `realtime_acceptance`, `block_managers_test` | microphone record and audible trigger at 48 kHz |
| Overdub, mute, erase | `migration_acceptance`, `event_test` | perform each operation through the UI without xruns |
| Save/reload WAV, Vorbis, FLAC, AU | `codec_formats`, `codec_pipeline` | save and reload a recorded loop in every codec |
| C++ loops, configuration, persistence | `cpp_golden_parity`, required captured fixtures | load unmodified C++ library and configuration |
| Snapshots and scenes | `core_persistence_parse_test` | save, restart, and restore both |
| Key mapping and MIDI mapping | `event_test`, `migration_acceptance` | bind physical keyboard and MIDI controls |
| MIDI input/output and MIDI clock | `migration_acceptance`, `macos_acceptance` | physical input, virtual output, clock sync |
| Patch selection and FluidLite synthesis | `native_patch_browser`, `native_runtime_contract` | select `basic.sf2` preset and hear synthesis |
| OSC transmission | `migration_acceptance` | inspect transmitted packet with a loopback receiver |
| Fixed 640x480 rendering | `packaging_guardrails` plus required C++ screenshot | compare with `compare_screenshots.py` |
| Configured-size, fullscreen, High-DPI | `videoio` unit tests plus required C++ screenshots | compare all three captures; exercise fullscreen |
| Browser rename | `native_rename`, `runtime_event_actions` | native runtime text entry, rename on disk, save, restart, and verify persistence |
| Patch echo under suppression | `native_patch_browser` | verify external MIDI program suppression while preserving per-zone echo routing |
| Selected trigger volume | `native_runtime_contract` | change selected trigger volume during playback without retriggering |
| Live UI state synchronization | `native_ui_state` | native runtime publishes one coherent frame after loop/browser/patch changes |
| Finder document handling | `macos_acceptance` | open a supported document from Finder |
| Restart after audio device loss | native adapter contract tests when landed | disconnect/reconnect interface during playback |
| Real-time callback safety | `realtime_acceptance`, `threading_acceptance` | validate 128/256-frame two-hour performance results |
| Bounded recording/decoding RSS | `codec_pipeline` and performance-result schema | inspect stress-run RSS trend and peak |
| Linux JACK transport and ALSA MIDI/mixer | Linux adapter tests when landed | reproducible virtual setup, then real hardware |

Rows whose named adapter test has not landed remain open release gates. The new rows exercise deterministic seams; they do not replace the hardware/native-runtime acceptance gates above.
