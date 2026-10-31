#!/usr/bin/env bash
# The gate is the same for every change: a prompt, a model or a policy goes through the PR
# checks like code does, and a release cannot start unless the bench, the attacks and the
# human review passed. This checks that the workflows still say so.
set -euo pipefail
cd "$(dirname "$0")/.."

# 078: no path filter on the PR checks, so no kind of change (a prompt, a model, a policy
# file) can skip them.
if grep -qE '^\s*paths(-ignore)?:' .github/workflows/ci.yml; then
  echo "error: ci.yml filters by path; every change must run the same checks" >&2
  exit 1
fi

# 079: the release job depends on the bench, the attacks, the chaos run and the human review.
release=$(awk '/^  release:/{on=1; next} on && /^  [a-z-]+:/{on=0} on' .github/workflows/release.yml)
for job in bench attacks chaos review; do
  echo "$release" | grep -qE "needs:.*\b$job\b" || {
    echo "error: release.yml: the release does not wait for '$job'" >&2
    exit 1
  }
done
echo "gate: ok"
