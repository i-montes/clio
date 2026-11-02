#!/usr/bin/env bash
# S-40: prove the backup can be read back. Restores the latest snapshot into a throwaway folder and
# checks what came out (the memory opens and passes SQLite's integrity check; the vault and the
# configuration are not empty). Touches nothing of the real data. Linux and macOS (ADR 0002).
set -euo pipefail
ENV_FILE="${CLIO_BACKUP_ENV:-$HOME/.config/clio/backup.env}"
[ -r "$ENV_FILE" ] || { echo "restore-test: falta $ENV_FILE" >&2; exit 2; }
set -a
# shellcheck source=/dev/null
. "$ENV_FILE"
set +a
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
chmod 700 "$tmp"
restic restore latest --tag clio --target "$tmp" >/dev/null
files=$(find "$tmp" -type f | wc -l)
[ "$files" -gt 0 ] || { echo "restore-test: la restauración salió vacía" >&2; exit 3; }
bad=0
while IFS= read -r f; do
  case "$f" in
    */memory.db)
      if command -v sqlite3 >/dev/null; then
        [ "$(sqlite3 "$f" 'PRAGMA integrity_check;')" = ok ] || { echo "FALLA: $f no pasa integrity_check" >&2; bad=1; }
      fi ;;
    */vault.bin|*/config.json)
      [ -s "$f" ] || { echo "FALLA: $f está vacío" >&2; bad=1; } ;;
  esac
done < <(find "$tmp" -type f)
[ "$bad" -eq 0 ] || exit 4
echo "restore-test: OK, $files archivos restaurados del último snapshot y comprobados"
