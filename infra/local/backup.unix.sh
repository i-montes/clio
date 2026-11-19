#!/usr/bin/env bash
# S-40: encrypted backup of what only lives here: the memory, the vault and the configuration of
# Clio (and the record of each run, without its working copy). Linux and macOS (ADR 0002: .unix.sh).
# It keeps the last CLIO_BACKUP_KEEP copies (2 by default): restic stores only what changed, but
# the person asked for few copies.
# Reads ~/.config/clio/backup.env (RESTIC_REPOSITORY, RESTIC_PASSWORD_FILE; made by
# backup-init.unix.sh). Safe to run by hand and from the user timer. Prints no secret.
set -euo pipefail
ENV_FILE="${CLIO_BACKUP_ENV:-$HOME/.config/clio/backup.env}"
DATA="${CLIO_DATA:-$HOME/.local/share/clio}"
CONF="${CLIO_CONFIG:-$HOME/.config/clio}"

[ -r "$ENV_FILE" ] || { echo "backup: $ENV_FILE is missing; run infra/local/backup-init.unix.sh first" >&2; exit 2; }
command -v restic >/dev/null || { echo "backup: restic is not installed" >&2; exit 2; }
set -a
# shellcheck source=/dev/null
. "$ENV_FILE"
set +a
: "${RESTIC_REPOSITORY:?RESTIC_REPOSITORY is missing in $ENV_FILE}"
: "${RESTIC_PASSWORD_FILE:?RESTIC_PASSWORD_FILE is missing in $ENV_FILE}"
KEEP="${CLIO_BACKUP_KEEP:-2}"
[[ "$KEEP" =~ ^[1-9][0-9]*$ ]] || { echo "backup: CLIO_BACKUP_KEEP must be an integer of 1 or more (it never deletes everything)" >&2; exit 2; }

# A fixed staging folder (private, in memory when there is XDG_RUNTIME_DIR): the path is part of
# what restic stores, so a random one would make every snapshot a different file.
base="${XDG_RUNTIME_DIR:-${TMPDIR:-$HOME/.cache}}"
stage="${base%/}/clio-backup-stage"
rm -rf "$stage"; mkdir -p "$(dirname "$stage")"; mkdir -m 700 "$stage"; trap 'rm -rf "$stage"' EXIT
paths=()
add() { [ -e "$1" ] && paths+=("$1") || true; }

# The memory is SQLite and may be in use: a hot copy with SQLite's own API, never the raw file.
if [ -f "$DATA/memory.db" ]; then
  if command -v clio-memory >/dev/null; then clio-memory --db "$DATA/memory.db" backup "$stage/memory.db" >/dev/null
  elif command -v sqlite3 >/dev/null; then sqlite3 "$DATA/memory.db" ".backup '$stage/memory.db'"
  else echo "backup: there is a memory.db but neither clio-memory nor sqlite3 to copy it while in use; a file in use is not backed up" >&2; exit 3; fi
  paths+=("$stage/memory.db")
fi
add "$DATA/vault.bin"          # already encrypted by the vault; restic encrypts it again
add "$CONF/config.json"
[ -d "$DATA/runs" ] && paths+=("$DATA/runs")
# The services (S-69): what the person approved, its records, runs and each one's state folder.
add "$DATA/services"
# MCP servers (S-70): approved manifests, records, state, data, logs and the installed packages.
add "$DATA/mcp"
if [ "${#paths[@]}" -eq 0 ]; then echo "backup: there is nothing to back up" >&2; exit 4; fi

restic backup --tag clio --exclude "$DATA/runs/*/sandbox" --exclude "$DATA/runs/*/scratch" --exclude "$DATA/services/locks" --exclude "$DATA/mcp/npm" --exclude "$DATA/mcp/npm-cache" "${paths[@]}"
# Keep only the last copies, then look at the repository.
restic forget --tag clio --keep-last "$KEEP" --prune
restic check --read-data-subset=5%
echo "backup: listo"
