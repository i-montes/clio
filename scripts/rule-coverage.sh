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

# Acceptance tests: the numbered list under "Pruebas de aceptación" in docs/specs/<crate>-spec-v*.md.
# A test covers number n when it is named t<n>_… (t06_t17_… covers both) or says "spec-<crate> #<n>".
for spec in docs/specs/*-spec-v*.md; do
  [ -e "$spec" ] || continue
  name=$(basename "$spec" | sed -E 's/-spec-v.*//')
  want=$(awk '/^#+ .*Pruebas de aceptación/{on=1; next} /^#/{on=0} on && /^[0-9]+\./{sub(/\..*/, ""); print}' "$spec" | sort -u)
  have=$({ grep -rhoE 'fn t[0-9]+(_t[0-9]+)*_' "crates/$name" 2>/dev/null | grep -oE 't[0-9]+' | tr -d t
           grep -rhoE "spec-$name #[0-9]+" "crates/$name" 2>/dev/null | grep -oE '[0-9]+$'
         } | sed -E 's/^0+//' | sort -u || true)
  gap=$(comm -23 <(echo "$want") <(echo "$have") | sort -n)
  n=$(echo "$want" | grep -c . || true)
  g=$(echo "$gap" | grep -c . || true)
  echo "spec-$name: $((n - g))/$n acceptance tests have a test"
  if [ "$g" -gt 0 ]; then
    echo "warning: spec-$name acceptance tests without a test:"
    echo "$gap" | sed 's/^/  #/'
  fi
done
