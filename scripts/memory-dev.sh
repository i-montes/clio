#!/usr/bin/env bash
# clio-memory against the dev memory file (outside the repo), with the local model.
# remember runs as the agent "claude" and then embeds; see .claude/skills/clio-memory/SKILL.md.
set -euo pipefail
cd "$(dirname "$0")/.."
DB=${CLIO_DEV_DB:-$HOME/.local/share/clio-dev/memory.db}
MODEL=${CLIO_MODEL:-$HOME/.cache/clio-models/potion-multilingual-128M}
cargo build -q --release -p clio-memory --features model2vec --bin clio-memory
if [ ! -f "$DB" ]; then
  mkdir -p "$(dirname "$DB")"
  cargo run -q --release -p clio-memory --example init -- "$DB" clio-dev es
fi
cmd=${1:?usage: memory-dev.sh <clio-memory command> [args]}
bin=(target/release/clio-memory --db "$DB")
case $cmd in
  remember)
    "${bin[@]}" --model "$MODEL" --actor claude "$@"
    exec "${bin[@]}" --model "$MODEL" run-jobs ;;
  recall|context|run-jobs) exec "${bin[@]}" --model "$MODEL" "$@" ;;
  *) exec "${bin[@]}" "$@" ;;
esac
