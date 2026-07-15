# Acceptance capture

Acceptance evidence is revision-bound and must be produced by the command that
is being attested. Short runs are useful for wiring validation only; they are
not evidence for the two-hour performance gates.

macOS (arm64, CoreAudio), one buffer size per run:

```sh
FWP_REALTIME_BUFFER_FRAMES=128 scripts/run_realtime_acceptance.sh \
  cargo run --locked --release --bin realtime_acceptance
FWP_REALTIME_BUFFER_FRAMES=256 scripts/run_realtime_acceptance.sh \
  cargo run --locked --release --bin realtime_acceptance
```

Linux JACK (with ALSA MIDI/mixer and the required virtual devices or hardware):

```sh
FWP_ACCEPTANCE_EVIDENCE_MODE=reproducible-virtual \
FWP_REALTIME_BUFFER_FRAMES=128 scripts/run_realtime_acceptance.sh \
  cargo run --locked --release --bin realtime_acceptance
FWP_ACCEPTANCE_EVIDENCE_MODE=reproducible-virtual \
FWP_REALTIME_BUFFER_FRAMES=256 scripts/run_realtime_acceptance.sh \
  cargo run --locked --release --bin realtime_acceptance
```

If a run is interrupted, resume its remaining duration with the same revision
and format using `scripts/run_realtime_acceptance.sh --resume ...`. The state
file is local progress metadata, not acceptance evidence. The runner publishes
the result atomically only after the segment has produced callbacks for its
entire requested duration. Its JSON records the revision, host, evidence mode,
prior elapsed time, segment duration, expected minimum callback count, and an
`attestation_complete` flag; review these fields before treating a resumed run
as evidence. A resumed result must still show the full 7200-second total and
must use the same revision and format for every segment.

The exact hardware command still required is:

```sh
FWP_REALTIME_BUFFER_FRAMES=128 scripts/run_realtime_acceptance.sh \
  cargo run --locked --release --bin realtime_acceptance
FWP_REALTIME_BUFFER_FRAMES=256 scripts/run_realtime_acceptance.sh \
  cargo run --locked --release --bin realtime_acceptance
```

Run each command on the claimed real hardware for the full two hours. Do not
replace the duration with a smoke run or infer missing ALSA, workflow, or
hardware evidence from the emitted JSON.
