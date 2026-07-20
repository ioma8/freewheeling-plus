# `basic.sf2` clean-room replacement handoff

Status: **commission required; no lawful drop-in is currently available**
(reviewed 2026-07-14).

## Finding

The checked-in `data/basic.sf2` is technically readable by the installed
FluidSynth 2.5.4, but its metadata and history do not identify a sample
rightsholder or grant permission to redistribute the recordings. The project
GPL notice, repository presence, Debian/Arch packaging, and the SWAMI editor
tag are not substitutes for that grant. The existing blob must therefore
remain unchanged and excluded from distributable packages until evidence is
obtained.

Its legacy SHA-256 is
`2e6cf4a8a1d78e6be3b00a0c22358d3ceec8c5a27a000714e65215e3f9b1d15a`; a
replacement verifier must reject that digest.

The host has FluidSynth and `ffmpeg`, but no installed SF2 authoring utility
(`sox` and common SF2 packers are unavailable). FluidSynth can validate and
render a candidate; it cannot establish provenance or turn arbitrary WAVs
into a valid, reviewable bank. The failed prototype noted in
`freewheeling-plus/fixtures/license-audit/basic-sf2-provenance.md` is not a
candidate artifact.

## Commission brief

Create a new bank from independently authored or generated material. Do not
inspect, extract, resample, compare against, or use `data/basic.sf2` or its
samples as source material. The deliverable must include:

- a valid SF2 containing exactly bank `0`, program `0`, preset name
  `El Cheapo Organ`;
- editable clean-room generator inputs and a reproducible build command;
- source and toolchain versions, build environment, and SHA-256 for every
  input and output;
- an explicit SPDX-compatible license and copyright/author attribution for
  the samples, synthesis inputs, SoundFont programming, and generated bank;
- a signed or otherwise attributable provenance statement granting
  redistribution in the application and its source/binary packages;
- a bundled `basic.sf2-LICENSE.txt` containing the complete grant, source URL
  or archival reference, SPDX identifier, attribution, and output digest.

Do not use FluidR3 or another system SoundFont as a silent replacement:
although documented package licenses may permit redistribution, it changes
the required organ preset and is a product decision rather than a drop-in
repair.

## Acceptance and verifier requirements

Before replacing the asset, an automated verifier must fail closed unless all
of these checks pass:

1. The output is a structurally valid RIFF `sfbk`; its digest matches the
   reviewed manifest and the license notice names that digest.
2. FluidSynth loads it without errors, enumerates exactly the required
   `(bank=0, program=0, name=El Cheapo Organ)` tuple, and renders note-on,
   note-off, velocity, pitch-bend, and release cases across representative
   MIDI notes. Capture the FluidSynth version and render hashes.
3. The pinned FluidLite integration loads the same file, enumerates the same
   tuple, and exercises the same playback smoke cases.
4. The generated bank contains no unexpected presets, external sample paths,
   or missing sample data; its source manifest and license are present beside
   the artifact.
5. `data/basics.xml`, patch/browser selection, saved scenes, and package
   staging continue to resolve `basic.sf2` and bank/program zero. Existing
   package verifiers must still reject missing or inadequate license evidence.
6. The verifier rejects the legacy digest, a changed output digest, a missing
   notice, a non-SPDX/insufficient grant, a tuple mismatch, and a technically
   loadable but unlicensed bank.

The clean-room producer should run the verifier in a fresh environment and
submit its logs, manifest, source archive, license notice, and reviewable
provenance statement. Only then should the asset owner decide whether to
replace the filename and update the pinned fixture digest.
