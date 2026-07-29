#!/usr/bin/env bash
set -euo pipefail

# Rerun the most recent tag-triggered release workflow, or a specific run:
#   ./scripts/retrigger-release.sh [RUN_ID]

if [[ $# -gt 1 ]]; then
  echo "usage: $0 [release-run-id]" >&2
  exit 2
fi

run_id="${1:-}"
if [[ -z "$run_id" ]]; then
  run_id="$({
    gh run list \
      --workflow release.yml \
      --event push \
      --limit 50 \
      --json databaseId,headBranch,createdAt \
      --jq 'map(select(.headBranch | startswith("v"))) | sort_by(.createdAt) | last | .databaseId'
  } || true)"
fi

if [[ -z "$run_id" || "$run_id" == "null" ]]; then
  echo "No tag-triggered release run found." >&2
  exit 1
fi

echo "Rerunning release workflow run $run_id"
gh run rerun "$run_id"
gh run view "$run_id" --json status,conclusion,url
