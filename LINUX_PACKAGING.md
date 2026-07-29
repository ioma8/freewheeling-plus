# FreeWheeling Linux 1.1 packaging and virtual acceptance

The Linux release is a binary-plus-data archive. It requires glibc, JACK 2,
ALSA (`libasound`), and the usual X11/OpenGL runtime libraries used by SDL2.
The SDL2 and FluidLite C implementations are built by their Rust crates; JACK
and ALSA remain system runtime dependencies. A JACK server must be running for
audio, MIDI, and transport. Direct ALSA mixer events require access to the
selected `/dev/snd/controlC*` device and membership in the distribution's audio
group where applicable.

Build dependencies on Debian/Ubuntu are `build-essential`, `clang`, `cmake`,
`pkg-config`, `libjack-jackd2-dev`, `libasound2-dev`, `libudev-dev`, and Rust.
Run:

```sh
SOURCE_DATE_EPOCH=$(git log -1 --format=%ct) \
freewheeling-plus/scripts/linux/package-release.sh
```

The archive contains `bin/freewheeling-plus`, `share/freewheeling/data`, and
license/documentation files. The bundled `basic.sf2` is public domain. Set
`FWP_TARGET` for another installed Rust target. The packager normalizes owner,
timestamps, order, and gzip headers for reproducible output.

## Virtual acceptance (not hardware evidence)

Install `jackd2` and `jack-tools`, then run:

```sh
freewheeling-plus/scripts/linux/run-virtual-acceptance.sh
```

This starts JACK's dummy driver at 48 kHz/256 frames, verifies FreeWheeling's
audio and MIDI port registration, relocates/starts/stops JACK transport, and
runs the allocation/xrun/RSS acceptance binary. It does not access or validate
physical audio, MIDI, or mixer hardware. Direct ALSA mixer behavior is covered
deterministically by backend contract tests; release qualification on a chosen
ALSA device remains a separate hardware acceptance activity.

For PipeWire systems, a JACK-compatible server such as `pw-jack` may launch the
installed binary, but the deterministic CI setup intentionally uses JACK 2's
dummy backend.

CI uses `run-virtual-workflow.sh` to compile the acceptance binary, run the
Linux contract test, execute virtual JACK ports and transport actions, and
write `acceptance-evidence/linux-virtual/attestation.json` only after success.
That attestation contains the checked-out revision and the validated result's
SHA-256; it is not physical ALSA or MIDI hardware evidence.
