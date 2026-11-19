#!/usr/bin/env bash
# check-package.sh PACKAGE VERSION: says `ok` when mcp_propose would take them (an npm name by
# npm's rules, lowercase, maybe @scope/; an exact version, never a range, a tag, a URL or a path),
# else why not, and fails. The same rules as Clio's own check, character for character.
set -u
p=${1-} v=${2-}
part='[a-z0-9][a-z0-9._-]*'
name=${p##*/}
if [[ ${#p} -gt 214 || ! $p =~ ^(@$part/)?$part$ || $name == node_modules || $name == favicon.ico ]]; then
    echo "no: $p is not an npm package name (lowercase, like mcp-server-time or @scope/server-name; no URL, git source, path or tag)"
    exit 1
fi
n='(0|[1-9][0-9]{0,8})'
id='[0-9A-Za-z-]+'
bad=0
if [[ ${#v} -gt 64 || ! $v =~ ^$n\.$n\.$n(-$id(\.$id)*)?$ ]]; then
    bad=1
elif [[ $v == *-* ]]; then
    # A numeric part of the prerelease has no leading zero (1.0.0-01 is not exact).
    IFS=. read -r -a ids <<<"${v#*-}"
    for i in "${ids[@]}"; do
        if [[ $i =~ ^[0-9]+$ && ! $i =~ ^$n$ ]]; then
            bad=1
        fi
    done
fi
if [[ $bad == 1 ]]; then
    echo "no: $v is not an exact version (like 1.2.3; never ^1.2.3, ~1.2, 1.x, latest or a tag)"
    exit 1
fi
echo "ok: $p@$v"
