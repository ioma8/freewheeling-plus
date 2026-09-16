#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/../../.." && pwd)
CRATE="$ROOT/freewheeling-plus"
EVIDENCE_DIR=${FWP_ACCEPTANCE_EVIDENCE_DIR:-$CRATE/acceptance-evidence/linux-virtual}
RUNTIME=${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}/freewheeling-jack-${UID:-$(id -u)}}
RESULT="$RUNTIME/performance-$$.json"
ATTESTATION="$EVIDENCE_DIR/attestation.json"
for command in cargo git python3; do command -v "$command" >/dev/null || { echo "error: missing command: $command" >&2; exit 127; }; done
REVISION=$(git -C "$CRATE" rev-parse --verify HEAD)
[ -n "$REVISION" ] || { echo "error: cannot determine checked-out revision" >&2; exit 1; }
# Only the stale result is removed: the previous attestation stays valid until
# the atomic `replace()` below publishes a new one, so a failing build cannot
# destroy existing evidence.
mkdir -p "$RUNTIME"
chmod 700 "$RUNTIME"
rm -f "$RESULT"
mkdir -p "$EVIDENCE_DIR"
cargo build --manifest-path "$CRATE/Cargo.toml" --locked --features jack --bin realtime_acceptance
cargo test --manifest-path "$CRATE/Cargo.toml" --locked --features jack --test linux_virtual_acceptance
FWP_ACCEPTANCE_REVISION="$REVISION" FWP_ACCEPTANCE_EVIDENCE_MODE=virtual-jack \
FWP_PERFORMANCE_RESULT="$RESULT" FWP_REALTIME_ACCEPTANCE_SECONDS=${FWP_REALTIME_ACCEPTANCE_SECONDS:-3} \
  "$CRATE/scripts/linux/run-virtual-acceptance.sh"
python3 - "$RESULT" "$RESULT.validation" "$ATTESTATION" "$REVISION" <<'PY'
import hashlib, json, os, pathlib, sys, tempfile
result_path, stamp_path, attestation_path = map(pathlib.Path, sys.argv[1:4])
revision = sys.argv[4]
result = json.loads(result_path.read_text(encoding="utf-8"))
if result.get("git_revision") != revision: raise SystemExit("error: result revision mismatch")
if result.get("evidence_mode") != "virtual-jack": raise SystemExit("error: result evidence mode mismatch")
# The acceptance step must have left its validation stamp: the status below is
# derived from that signal instead of being asserted unconditionally.
if not stamp_path.is_file():
    raise SystemExit(f"error: acceptance validation stamp missing: {stamp_path}")
stamp = dict(
    line.split("=", 1) for line in stamp_path.read_text(encoding="utf-8").splitlines() if "=" in line
)
result_sha = hashlib.sha256(result_path.read_bytes()).hexdigest()
if stamp.get("status") != "passed" or stamp.get("validated_result_sha256") != result_sha:
    raise SystemExit("error: acceptance validation stamp does not match the performance result")
attestation = {"schema_version": 1, "status": stamp["status"], "git_revision": revision,
 "evidence_mode": "virtual-jack",
 "actions": ["cargo build --features jack --bin realtime_acceptance", "cargo test --features jack --test linux_virtual_acceptance", "JACK dummy runtime/ports/transport acceptance"],
 "validation": {"tool": pathlib.Path(stamp.get("validator", "")).name,
                "tool_sha256": stamp.get("validator_sha256")},
 "performance_result_sha256": result_sha}
# `mkstemp` creates the temporary file exclusively in the destination directory:
# a fixed `<name>.tmp` sibling would collide between concurrent runs, could be
# pre-created as a symlink (the evidence directory is overridable), and would be
# left behind by a crash.
fd, temporary_name = tempfile.mkstemp(
    dir=str(attestation_path.parent), prefix=attestation_path.name + ".", suffix=".tmp"
)
temporary = pathlib.Path(temporary_name)
try:
    os.fchmod(fd, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        handle.write(json.dumps(attestation, sort_keys=True, indent=2) + "\n")
        handle.flush()
        os.fsync(handle.fileno())
    temporary.replace(attestation_path)
except BaseException:
    temporary.unlink(missing_ok=True)
    raise
print(f"virtual Linux workflow passed; attestation: {attestation_path}")
PY
