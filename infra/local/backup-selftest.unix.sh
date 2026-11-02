#!/usr/bin/env bash
# shellcheck disable=SC2034  # out and rc are read inside the eval strings of check()
# Proves the backup scripts work, against throwaway repositories in a fake HOME; touches nothing real.
# Same on Linux and macOS (ADR 0002: shared Unix code is named .unix.sh). Needs restic.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
command -v restic >/dev/null || { echo "SKIP: restic no está instalado"; exit 0; }
root="$(mktemp -d)"; trap 'rm -rf "$root"' EXIT
fails=0
ok()   { echo "PASS  $1"; }
bad()  { echo "FAIL  $1"; fails=$((fails + 1)); }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

# A fake home with the data Clio keeps (same relative paths on both systems).
new_home() {
  h="$root/$1"; mkdir -p "$h/.local/share/clio/runs/r1/sandbox" "$h/.config/clio"
  echo '{"model":"x"}' > "$h/.config/clio/config.json"
  head -c 2048 /dev/urandom > "$h/.local/share/clio/vault.bin"
  echo '{"id":"r1"}' > "$h/.local/share/clio/runs/r1/meta.json"
  head -c 50000 /dev/urandom > "$h/.local/share/clio/runs/r1/sandbox/big.bin"
}
run() { # run <home name> <script> [args]: the script as that home's user, with the OS the test names
  local home="$root/$1"; shift
  env -u XDG_DATA_HOME -u CLIO_BACKUP_ENV -u CLIO_CONFIG -u CLIO_DATA -u CLIO_BACKUP_KEEP HOME="$home" ${OS:+CLIO_OS_OVERRIDE="$OS"} ${XDGV:+XDG_DATA_HOME="$XDGV"} ${KEEP:+CLIO_BACKUP_KEEP="$KEEP"} "$here/$1" "${@:2}"
}
snapshots() { env HOME="$1" bash -c 'set -a; . "$HOME/.config/clio/backup.env"; set +a; restic snapshots --tag clio --json' 2>/dev/null | grep -o '"short_id"' | wc -l | tr -d ' '; }

# 1. The default place depends on the system, and is outside Clio's own data folder.
new_home linux; OS=Linux run linux backup-init.unix.sh >/dev/null 2>&1
check "Linux: repositorio por defecto en ~/.local/share/clio-backup/repo" '[ -f "$root/linux/.local/share/clio-backup/repo/config" ]'
new_home linuxxdg; mkdir -p "$root/xdg"; OS=Linux XDGV="$root/xdg" run linuxxdg backup-init.unix.sh >/dev/null 2>&1
check "Linux con XDG_DATA_HOME: se respeta" '[ -f "$root/xdg/clio-backup/repo/config" ]'
new_home mac; OS=Darwin run mac backup-init.unix.sh >/dev/null 2>&1
check "macOS: repositorio por defecto en ~/Library/Application Support/clio-backup/repo" '[ -f "$root/mac/Library/Application Support/clio-backup/repo/config" ]'
check "la frase y la configuración quedan 0600" '[ "$(stat -c %a "$root/linux/.config/clio/restic-password" 2>/dev/null || stat -f %Lp "$root/linux/.config/clio/restic-password")" = 600 ]'
OS=Linux run linux backup-init.unix.sh >/dev/null 2>&1; rc=$?
check "init no pisa una configuración existente" '[ $rc -ne 0 ]'
new_home warn; out="$(OS=Linux run warn backup-init.unix.sh 2>&1)"
check "init avisa que un repositorio en el mismo disco no protege de perderlo" 'grep -q "mismo disco" <<<"$out"'
check "init no imprime la frase" '! grep -qF "$(cat "$root/warn/.config/clio/restic-password")" <<<"$out"'

# 2. Only the last two copies are kept by default; the number can be raised.
for i in 1 2 3; do echo "$i" > "$root/linux/.config/clio/config.json"; OS=Linux run linux backup.unix.sh >/dev/null 2>&1; done
check "tras 3 copias quedan solo las 2 últimas" '[ "$(snapshots "$root/linux")" = 2 ]'
for i in 4 5; do echo "$i" > "$root/linux/.config/clio/config.json"; OS=Linux KEEP=3 run linux backup.unix.sh >/dev/null 2>&1; done
check "con CLIO_BACKUP_KEEP=3 quedan 3" '[ "$(snapshots "$root/linux")" = 3 ]'
OS=Linux KEEP=0 run linux backup.unix.sh >/dev/null 2>&1; rc=$?
check "CLIO_BACKUP_KEEP=0 se rechaza (no borra todo)" '[ $rc -ne 0 ] && [ "$(snapshots "$root/linux")" = 3 ]'

# 3. What was backed up can be restored, and the work copy was left out.
OS=Linux run linux backup-restore-test.unix.sh >/dev/null 2>&1; rc=$?
check "la restauración se comprueba" '[ $rc -eq 0 ]'
check "la copia de trabajo (sandbox) no se respalda" '! env HOME="$root/linux" bash -c "set -a; . \"\$HOME/.config/clio/backup.env\"; set +a; restic ls latest" 2>/dev/null | grep -q sandbox'
: > "$root/linux/.local/share/clio/vault.bin"; OS=Linux run linux backup.unix.sh >/dev/null 2>&1
OS=Linux run linux backup-restore-test.unix.sh >/dev/null 2>&1; rc=$?
# shellcheck disable=SC2034
check "una bóveda vacía hace fallar la prueba de restauración" '[ $rc -ne 0 ]'

[ "$fails" -eq 0 ] && echo "selftest: todo bien" || { echo "selftest: $fails fallo(s)"; exit 1; }
