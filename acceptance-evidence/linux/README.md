# Virtual JACK run (2026-07-14)

`virtual-jack-performance.json` was produced by the Ubuntu 24.04 container in
`scripts/linux/Dockerfile` using JACK's dummy driver at 48 kHz/256 frames.
The runner verified all six application JACK ports, relocation, rolling and
stop commands, callback activity, and zero allocation/lock/xrun counters.

This is not a full `linux-jack-alsa` attestation: the container has no ALSA
control device, so direct mixer behavior and ALSA MIDI were not exercised.
The full acceptance driver must continue to report that gate as missing until
those checks run on real hardware or a reproducible virtual ALSA device.
