# Historical C++ codec fixtures

`reference.wav`, `reference.flac`, and `reference.ogg` are emitted by the
encoder classes in `src/fweelin_block.cc`, linked to the host's historical
libsndfile/libVorbis dependencies. The input is a fixed 4096-frame stereo
signal at 48 kHz. See `PROVENANCE` for all capture controls and library
versions and `MANIFEST.sha256` for exact hashes.

The historical Vorbis encoder seeds its Ogg serial with `time(NULL)`, and
libsndfile writes the current time into WAV's `PEAK` chunk. The capture harness
interposes a fixed clock for those two metadata inputs. It does not rewrite or
normalize either output file after encoding.

`UNSUPPORTED` documents why no `.au` fixture exists despite AU appearing in
the codec enum: the historical constructor routes AU to a WAV container.

