#!/usr/bin/env bash
# S-04: list rule ids from docs/security.md and docs/specs that no test references.
# Warn-only for now: always exits 0.
set -euo pipefail
cd "$(dirname "$0")/.."

# grep exits 1 when nothing matches; that must not stop the script (it promises to always exit 0).
ids=$({ grep -rhoE --include='*.md' 'S-[0-9]{2}' docs/security.md docs/specs || true; } | sort -u)
# A rule counts as tested once any file under a tests/ directory names it, even a pending test.
dirs=()
for d in crates testing; do
  if [ -d "$d" ]; then dirs+=("$d"); else echo "warning: directory not found: $d" >&2; fi
done
tested=$({ [ "${#dirs[@]}" -gt 0 ] && find "${dirs[@]}" -path '*/tests/*.rs' -exec grep -hoE 'S-[0-9]{2}' {} + || true; } | sort -u)
missing=$(comm -23 <(echo "$ids") <(echo "$tested"))

total=$(echo "$ids" | grep -c . || true)
count=$(echo "$missing" | grep -c . || true)
echo "rule coverage: $((total - count))/$total rules referenced by a test"
if [ "$count" -gt 0 ]; then
  echo "warning: rules without a test:"
  echo "$missing" | sed 's/^/  /'
fi
