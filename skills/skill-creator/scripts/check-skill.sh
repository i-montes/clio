#!/bin/bash
# Checks the shape of a skill folder for Clio: SKILL.md with its name and a description that
# routes, a non-empty "## How to verify" section, shell scripts that parse, no tab characters and
# nothing shaped like a secret. Usage: check-skill.sh FOLDER. Exit 0 when it holds.
set -u
dir="${1:-.}"
md="$dir/SKILL.md"
fail() { echo "check-skill: $*" >&2; exit 1; }

[ -f "$md" ] || fail "no SKILL.md in $dir"
[ "$(sed -n 1p "$md")" = "---" ] || fail "SKILL.md must start with its frontmatter (---)"
name=$(sed -n 2p "$md" | sed -n 's/^name: //p')
echo "$name" | grep -Eq '^[a-z0-9]+(-[a-z0-9]+)*$' || fail "line 2 must be 'name: <lowercase-name>'"
desc=$(sed -n 3p "$md" | sed -n 's/^description: //p')
[ "${#desc}" -ge 20 ] && [ "${#desc}" -le 1026 ] || fail "line 3 must be 'description: ...' (20 to 1024 characters)"
grep -qx -- '---' <(sed -n '4,$p' "$md") || fail "the frontmatter is not closed"
verify=$(sed -n '/^## How to verify$/,$p' "$md" | sed 1d | grep -v '^[[:space:]]*$' || true)
[ -n "$verify" ] || fail "no '## How to verify' section, or it is empty"
if grep -rlP '\t' "$dir" >/dev/null 2>&1; then fail "a file has a tab character"; fi
secret='(sk-[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16}|ghp_[A-Za-z0-9]{30,}|-----BEGIN [A-Z ]*PRIVATE KEY-----)'
if grep -rEl -- "$secret" "$dir" >/dev/null 2>&1; then fail "something in it looks like a secret"; fi
for s in $(find "$dir" -name '*.sh' -type f); do
    bash -n "$s" || fail "$s does not parse"
done
echo "ok: $name"
