#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/../../.." && pwd)
CRATE="$ROOT/freewheeling-plus"
EVIDENCE_DIR=${FWP_ACCEPTANCE_EVIDENCE_DIR:-$ROOT/acceptance-evidence/linux-virtual}
RUNTIME=${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}/freewheeling-jack-${UID:-$(id -u)}}
RESULT="$RUNTIME/performance.json"
ATTESTATION="$EVIDENCE_DIR/attestation.json"
for command in cargo git python3; do command -v "$command" >/dev/null || { echo "error: missing command: $command" >&2; exit 127; }; done
REVISION=$(git -C "$ROOT" rev-parse --verify HEAD)
[ -n "$REVISION" ] || { echo "error: cannot determine checked-out revision" >&2; exit 1; }
rm -f "$ATTESTATION" "$RESULT"
mkdir -p "$EVIDENCE_DIR"
cargo build --manifest-path "$CRATE/Cargo.toml" --locked --bin realtime_acceptance
cargo test --manifest-path "$CRATE/Cargo.toml" --locked --test linux_virtual_acceptance
FWP_ACCEPTANCE_REVISION="$REVISION" FWP_ACCEPTANCE_EVIDENCE_MODE=virtual-jack \
FWP_PERFORMANCE_RESULT="$RESULT" FWP_REALTIME_ACCEPTANCE_SECONDS=${FWP_REALTIME_ACCEPTANCE_SECONDS:-3} \
  "$CRATE/scripts/linux/run-virtual-acceptance.sh"
python3 - "$RESULT" "$ATTESTATION" "$REVISION" <<'PY'
import hashlib, json, pathlib, sys
result_path, attestation_path, revision = map(pathlib.Path, sys.argv[1:])
result = json.loads(result_path.read_text(encoding="utf-8"))
if result.get("git_revision") != revision: raise SystemExit("error: result revision mismatch")
if result.get("evidence_mode") != "virtual-jack": raise SystemExit("error: result evidence mode mismatch")
attestation = {"schema_version": 1, "status": "passed", "git_revision": revision,
 "evidence_mode": "virtual-jack", "actions": ["cargo build --bin realtime_acceptance", "cargo test --test linux_virtual_acceptance", "JACK dummy runtime/ports/transport acceptance"],
 "performance_result_sha256": hashlib.sha256(result_path.read_bytes()).hexdigest()}
temporary = attestation_path.with_name(attestation_path.name + ".tmp")
temporary.write_text(json.dumps(attestation, sort_keys=True, indent=2) + "\n", encoding="utf-8")
temporary.replace(attestation_path)
print(f"virtual Linux workflow passed; attestation: {attestation_path}")
PY
