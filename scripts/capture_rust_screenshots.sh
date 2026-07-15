#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
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

echo "Rust FWRGBA1 pixel evidence written to $EVIDENCE/pixels"
