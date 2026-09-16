#!/usr/bin/env bash
set -euo pipefail

# Rerun the most recent tag-triggered release workflow, or a specific run:
#   ./scripts/retrigger-release.sh [RUN_ID]

if [[ $# -gt 1 ]]; then
  echo "usage: $0 [release-run-id]" >&2
  exit 2
fi

run_id="${1:-}"
RUN_ID_FILE=$(mktemp -t freewheeling-run-id)
trap 'rm -f "$RUN_ID_FILE"' EXIT INT TERM
# The id is passed to `gh run rerun`/`gh run view`: a value starting with `-`
# would be parsed as a flag and could redirect the command at another repo.
if [[ -n "$run_id" && ! "$run_id" =~ ^[0-9]+$ ]]; then
  echo "error: run id must be numeric: $run_id" >&2
  exit 2
fi
if [[ -z "$run_id" ]]; then
  # Cross-check the run against the tag list rather than trusting a branch
  # name heuristic, and let `gh` failures surface (an auth or network error
  # must not look like "no release run").
  # The scans below are windowed (`gh`'s `--limit`): a run that falls outside
  # the window would look like "no release run" and silently retrigger nothing,
  # so an exhausted window is an error that names the way out.
  release_limit=50
  tags=$(gh release list --limit "$release_limit" --json tagName --jq '.[].tagName')
  if [[ -z "$tags" ]]; then
    echo "error: no releases found (is this a release repository?)" >&2
    exit 1
  fi
  if [[ "$(printf '%s\n' "$tags" | wc -l | tr -d ' ')" -ge "$release_limit" ]]; then
    echo "error: the release list is longer than the $release_limit-tag scan window;" >&2
    echo "       pass the run id explicitly or raise release_limit" >&2
    exit 1
  fi
  run_id=""
  # `gh run list` returns the newest run first. The list goes through a pipe
  # (not a process substitution, which `sh` does not support) so the file stays
  # POSIX-shell compatible.
  run_limit=100
  runs=$(gh run list --workflow release.yml --event push --limit "$run_limit" \
           --json databaseId,headBranch \
           --jq '.[] | "\(.headBranch) \(.databaseId)"')
  if [[ "$(printf '%s\n' "$runs" | grep -c . )" -ge "$run_limit" ]]; then
    echo "error: the run list is longer than the $run_limit-run scan window;" >&2
    echo "       pass the run id explicitly or raise run_limit" >&2
    exit 1
  fi
  printf '%s\n' "$runs" | while read -r branch id; do
    [ -n "$id" ] || continue
    if printf '%s\n' "$tags" | grep -qxF -- "$branch"; then
      printf '%s' "$id"
      break
    fi
  done >"$RUN_ID_FILE"
  run_id=$(cat "$RUN_ID_FILE")
fi

if [[ -z "$run_id" || "$run_id" == "null" ]]; then
  echo "No tag-triggered release run found." >&2
  exit 1
fi

echo "Rerunning release workflow run $run_id"
if ! gh run rerun "$run_id"; then
  echo "error: cannot rerun run $run_id" >&2
  gh run view "$run_id" --json status,conclusion,url >&2 || true
  exit 1
fi
# The rerun was only just queued: report the run's status on the next poll
# rather than the previous attempt's conclusion.
gh run watch "$run_id" --exit-status --interval 15 || true
gh run view "$run_id" --json status,conclusion,url
