#!/bin/sh
set -eu

# Resolve the crate from this script's real location (independent of the
# caller's cwd, symlinks and the checkout's directory name).
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
ROOT=$(CDPATH= cd -- "$script_dir/.." && pwd -P)
if [ ! -f "$ROOT/Cargo.toml" ]; then
  echo "cannot locate the crate: no Cargo.toml at $ROOT" >&2
  exit 1
fi
EVIDENCE=${FWP_ACCEPTANCE_EVIDENCE:-"$ROOT/acceptance-evidence"}
REFERENCES=${CPP_SCREENSHOT_DIR:-"$ROOT/fixtures/cpp-golden/screenshots"}

if [ -f "$REFERENCES/PROVENANCE" ]; then
  echo "using genuine C++ captures from $REFERENCES"
else
  echo "C++ provenance absent; emitting Rust candidates without references" >&2
fi

FW_PIXEL_EVIDENCE="$EVIDENCE" FW_CPP_SCREENSHOTS="$REFERENCES" \
  cargo test --manifest-path "$ROOT/Cargo.toml" --test pixel_parity_runtime \
  emit_candidates_and_compare_genuine_cpp_references_when_requested -- --exact --nocapture

candidate_count=$(find "$EVIDENCE/pixels" -name candidate.fwrgba 2>/dev/null | wc -l | tr -d " ")
if [ "$candidate_count" -eq 0 ]; then
  # `--exact` exits 0 when the filter matches no test, so the evidence itself
  # is asserted rather than the harness exit status.
  echo "no Rust pixel candidates were produced under $EVIDENCE/pixels" >&2
  exit 1
fi
if [ -f "$REFERENCES/PROVENANCE" ]; then
  reference_count=$(find "$EVIDENCE/pixels" -name reference.fwrgba 2>/dev/null | wc -l | tr -d " ")
  if [ "$reference_count" -eq 0 ]; then
    echo "genuine C++ references were available but no comparison used them" >&2
    exit 1
  fi
fi
echo "Rust FWRGBA1 pixel evidence written to $EVIDENCE/pixels ($candidate_count candidate(s))"
