#!/usr/bin/env bash
# S-40: encrypted backup, off this disk, of what only lives here: the memory, the vault and the
# configuration of Clio (and the record of each run, without its working copy). Linux only (ADR 0002).
# Reads ~/.config/clio/backup.env (RESTIC_REPOSITORY, RESTIC_PASSWORD_FILE; made by
# backup-init.linux.sh). Safe to run by hand and from the user timer. Prints no secret.
set -euo pipefail
ENV_FILE="${CLIO_BACKUP_ENV:-$HOME/.config/clio/backup.env}"
DATA="${CLIO_DATA:-$HOME/.local/share/clio}"
CONF="${CLIO_CONFIG:-$HOME/.config/clio}"

[ -r "$ENV_FILE" ] || { echo "backup: falta $ENV_FILE; corre infra/local/backup-init.linux.sh primero" >&2; exit 2; }
command -v restic >/dev/null || { echo "backup: restic no está instalado" >&2; exit 2; }
set -a
# shellcheck source=/dev/null
. "$ENV_FILE"
set +a
: "${RESTIC_REPOSITORY:?falta RESTIC_REPOSITORY en $ENV_FILE}"
: "${RESTIC_PASSWORD_FILE:?falta RESTIC_PASSWORD_FILE en $ENV_FILE}"

# A fixed staging folder (private, in memory when there is XDG_RUNTIME_DIR): the path is part of
# what restic stores, so a random one would make every snapshot a different file.
stage="${XDG_RUNTIME_DIR:-$HOME/.cache}/clio-backup-stage"
rm -rf "$stage"; mkdir -p "$(dirname "$stage")"; mkdir -m 700 "$stage"; trap 'rm -rf "$stage"' EXIT
paths=()
add() { [ -e "$1" ] && paths+=("$1") || true; }

# The memory is SQLite and may be in use: a hot copy with SQLite's own API, never the raw file.
if [ -f "$DATA/memory.db" ]; then
  if command -v clio-memory >/dev/null; then clio-memory --db "$DATA/memory.db" backup "$stage/memory.db" >/dev/null
  elif command -v sqlite3 >/dev/null; then sqlite3 "$DATA/memory.db" ".backup '$stage/memory.db'"
  else echo "backup: hay memory.db pero no clio-memory ni sqlite3 para copiarla en caliente; no se respalda un archivo en uso" >&2; exit 3; fi
  paths+=("$stage/memory.db")
fi
add "$DATA/vault.bin"          # already encrypted by the vault; restic encrypts it again
add "$CONF/config.json"
[ -d "$DATA/runs" ] && paths+=("$DATA/runs")
if [ "${#paths[@]}" -eq 0 ]; then echo "backup: no hay nada que respaldar" >&2; exit 4; fi

restic backup --tag clio --exclude "$DATA/runs/*/sandbox" --exclude "$DATA/runs/*/scratch" "${paths[@]}"
# Keep a week of days, a month of weeks and half a year of months; then look at the repository.
restic forget --tag clio --keep-daily 7 --keep-weekly 4 --keep-monthly 6 --prune
restic check --read-data-subset=5%
echo "backup: listo"
