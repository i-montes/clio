#!/bin/bash
# Runs a service's command twice from its folder, as the timer would: with a fresh state folder
# in CLIO_STATE_DIR, no input and a time limit. Both runs must end 0 or 10 (safe to run twice).
# Usage: check-run.sh FOLDER 'COMMAND' [SECONDS]. Exit 0 when it holds.
set -u
dir="${1:?the service folder}"
cmd="${2:?the command}"
secs="${3:-60}"
state=$(mktemp -d)
trap 'rm -rf "$state"' EXIT
for run in 1 2; do
    (cd "$dir" && CLIO_STATE_DIR="$state" CLIO_SERVICE=check timeout "$secs" bash -c "$cmd" </dev/null)
    code=$?
    case "$code" in
        0 | 10) ;;
        *) echo "check-run: run $run ended with $code" >&2; exit 1 ;;
    esac
done
echo "ok: two runs ended 0 or 10; the state folder has $(find "$state" -type f | wc -l) file(s)"
