#!/usr/bin/env bash
# Installs the default embedding model of clio-memory (potion-multilingual-128M, decision 0004)
# on Linux. The crate never downloads models, so this script is the only way in.
# Pinned to one Hugging Face commit; every file is checked against its sha256 before it is used.
#
# Destination: $CLIO_MODEL, or ~/.cache/clio-models/potion-multilingual-128M (the default of
# scripts/memory-dev.sh).
set -euo pipefail

[ "$(uname -s)" = Linux ] || { echo "this script is for Linux" >&2; exit 1; }

REPO=minishlab/potion-multilingual-128M
REV=73908c3438cf03b6a01bcb9611d62b23d0726f08
DEST=${CLIO_MODEL:-$HOME/.cache/clio-models/potion-multilingual-128M}
FILES=(
  "config.json 595e4cab2093732efd5dbe084fd5c1826b5eea693b73b4c1fd971672867d2e54"
  "tokenizer.json 19f1909063da3cfe3bd83a782381f040dccea475f4816de11116444a73e1b6a1"
  "model.safetensors 14b5eb39cb4ce5666da8ad1f3dc6be4346e9b2d601c073302fa0a31bf7943397"
)

ok() { [ -f "$1" ] && [ "$(sha256sum "$1" | cut -d' ' -f1)" = "$2" ]; }

mkdir -p "$DEST"
tmp=$(mktemp -d "$DEST/.download.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

for entry in "${FILES[@]}"; do
  read -r name sha <<<"$entry"
  if ok "$DEST/$name" "$sha"; then
    echo "ok        $name"
    continue
  fi
  echo "download  $name"
  curl -fL --retry 3 --progress-bar -o "$tmp/$name" "https://huggingface.co/$REPO/resolve/$REV/$name"
  ok "$tmp/$name" "$sha" || { echo "sha256 mismatch for $name; nothing replaced" >&2; exit 1; }
  mv "$tmp/$name" "$DEST/$name"
done
echo "model ready in $DEST"
