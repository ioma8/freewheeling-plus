# `data/basic.sf2` provenance and license audit

Status: **redistribution permission not conclusively established** (audited
2026-07-14). This report is evidence, not legal advice. Do not use the
repository's `COPYING` file or this report as an asset license.

**Disposition:** the existing file remains blocked for distribution. No
license notice has been fabricated, and no replacement has been substituted
under the old filename.

## Identified object

* Path: `data/basic.sf2`
* Size: 3,576 bytes
* Git blob: `7fb39c07e4ae5ab71cb76f1bdf9a212e61e56fa8`
* SHA-256: `2e6cf4a8a1d78e6be3b00a0c22358d3ceec8c5a27a000714e65215e3f9b1d15a`
* Format: RIFF SoundFont bank (`sfbk`)

The SoundFont INFO chunk says `INAM=example`, `isng=EMU8000`, and
`ISFT=SWAMI v0.9.1a`. Its author, copyright, product, and comment fields are
empty. The bank contains one visible preset, `El Cheapo Organ`, backed by the
instrument `ADDITIVE 3` and four samples (`add3_a1` through `add3_a4`). These
labels and the editor identification do not identify the samples' author or
convey a license.

## Repository and upstream history

The first reachable commit containing the file is
`5bf4c275a3dec39410ee130a0d90384be3ef6388`, authored by `jpmercury
<swirlee@vcn.bc.ca>` at `2007-01-03 22:39:41 +0000`. It adds the blob along
with the initial imported source tree, with no commit message and no
asset-specific notice. The contemporaneous `data/Makefile.am` installs
`basic.sf2`, but says nothing about its origin or license.

Commit `38fe5ba9159e3879ff9b41679cec4f2da9897634` (`initial commit after
fork/conversion from svn`, author date 2016-10-31) changes only the executable
mode. No other blob for this path exists in the reachable Git object history.
The exact SHA-256 is also present in Debian's upstream 0.6.6 tarball.

The original project download page identifies SourceForge SVN as the former
bleeding-edge source and says it was converted to the GitHub free-wheeling
repository in December 2016:
<https://freewheeling.sourceforge.net/download.shtml.html>. The upstream
SourceForge project describes the project generally as GPLv2, but does not
publish an asset-specific grant for this bank:
<https://sourceforge.net/projects/freewheeling/>.

## Authoritative public packaging records

Debian's current copyright record assigns `GPL-2+` to `Files: *` and names
Jan Pekau (JP Mercury) as copyright holder:
<https://metadata.ftp-master.debian.org/changelogs/main/f/freewheeling/freewheeling_0.6.6-1_copyright>.
Debian publishes the 0.6.6 source tarball and its checksums here:
<https://packages.debian.org/source/stable/freewheeling>. Arch Linux also
lists `basic.sf2` in its installed package:
<https://archlinux.org/packages/extra/x86_64/freewheeling/files/>.

These are credible evidence that established distributions have treated the
file as part of GPL-licensed FreeWheeling. They are not conclusive provenance
or permission for the samples: the records do not identify how the packagers
established Jan Pekau's ownership of the four recordings, do not quote an
asset-specific upstream grant, and conflict with the bank's absent author and
copyright metadata. Repository inclusion and a package-level license
classification cannot cure a missing grant from an unknown rightsholder.
Consequently no license notice has been added under `data/`.

## Safe replacement strategy (not performed)

The configuration contract is small: `data/basics.xml` loads the filename
`basic.sf2`; FreeWheeling enumerates SoundFont presets, and this bank supplies
one preset. A replacement should preserve:

1. a valid SF2 bank loadable by the supported FluidSynth versions;
2. bank 0, program 0, with preset name `El Cheapo Organ` (so saved/browser
   selections remain stable); and
3. the `basic.sf2` installed filename, unless all config/package consumers are
   deliberately migrated and tested together.

Preferred route: commission or synthesize new organ waveforms without using
the existing samples, author an SF2 containing the same bank/program/name,
and obtain an explicit SPDX-compatible license plus a signed/source-linked
provenance statement for both samples and bank programming. Keep the editable
generator inputs and add tests that load the result with FluidSynth, enumerate
exactly the expected preset tuple, exercise notes across the MIDI range, and
verify a pinned digest and bundled notice. This preserves patches without
depending on a workstation package.

System fallback: distributions may depend on FluidR3 GM (`soundfont-fluid` /
`fluid-soundfont-gm`) and select bank 0/program 0, but that preset is a piano,
not this organ, so it is only selection-compatible and must not silently
replace the asset. Debian records FluidR3 as MIT and includes its upstream
grant here:
<https://metadata.ftp-master.debian.org/changelogs/main/f/fluid-soundfont/fluid-soundfont_3.1-5.3_copyright>.
Before adopting it, pin the actual package/file per supported OS, ship its MIT
notice, test FluidSynth preset enumeration and playback, and explicitly accept
the timbral change. Vendoring FluidR3 would also greatly increase bundle size.

## Clean-room replacement attempt (2026-07-14)

An isolated prototype was attempted using only analytically generated additive
sine samples and a hand-built SoundFont 2 RIFF container. It did not produce a
shippable artifact: installed FluidSynth 2.5.4 rejected the prototype during
`phdr`/preset-bag parsing (`Preset bag chunk size mismatch`). The prototype was
discarded; no generated file was copied into `data/`, and the legacy digest
above is unchanged. This is an implementation failure, not evidence that the
legacy asset is licensed.

The replacement remains blocked until a clean-room generator emits a valid
bank and the following commands pass against the generated output:

```sh
python3 freewheeling-plus/scripts/generate_original_basic_sf2.py /tmp/basic.sf2
fluidsynth -ni -q /tmp/basic.sf2 <<'EOF'
inst 0
select 0 1 0 0
noteon 0 60 100
noteoff 0 60
quit
EOF
(cd freewheeling-plus && cargo test --test soundfont_compatibility)
```

That future test must enumerate exactly bank 0/program 0/name `El Cheapo
Organ`, load and render notes across the MIDI range through both FluidSynth
and the pinned FluidLite dependency, and verify a pinned SHA-256 plus an
SPDX-compatible notice and source/provenance statement. The command above is
a required plan, not a currently present script.

Do not replace the current blob until the new artifact's sample provenance,
license, preset tuple, FluidSynth behavior, packaging notices, and patch/browser
compatibility are all evidenced and reviewed.

The official FluidSynth documentation identifies FluidR3 GM/GS as Creative
Commons licensed files (<https://www.fluidsynth.org/wiki/GettingStarted/>),
and Debian publishes packaged copyright/grant evidence
(<https://metadata.ftp-master.debian.org/changelogs/main/f/fluid-soundfont/fluid-soundfont_3.1-5.3_copyright>).
That bank is nevertheless not a lawful drop-in resolution here: it does not
preserve the required `El Cheapo Organ` preset contract. Adoption would need
an explicit product decision and behavioral testing.
