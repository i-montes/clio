#!/usr/bin/env bash
# shellcheck disable=SC2034  # out and rc are read inside the eval strings of check()
# Proves the backup scripts work, against throwaway repositories in a fake HOME; touches nothing real.
# Same on Linux and macOS (ADR 0002: shared Unix code is named .unix.sh). Needs restic.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
command -v restic >/dev/null || { echo "SKIP: restic is not installed"; exit 0; }
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
check "Linux with XDG_DATA_HOME: it is respected" '[ -f "$root/xdg/clio-backup/repo/config" ]'
new_home mac; OS=Darwin run mac backup-init.unix.sh >/dev/null 2>&1
check "macOS: repositorio por defecto en ~/Library/Application Support/clio-backup/repo" '[ -f "$root/mac/Library/Application Support/clio-backup/repo/config" ]'
check "the passphrase and the setup end up 0600" '[ "$(stat -c %a "$root/linux/.config/clio/restic-password" 2>/dev/null || stat -f %Lp "$root/linux/.config/clio/restic-password")" = 600 ]'
OS=Linux run linux backup-init.unix.sh >/dev/null 2>&1; rc=$?
check "init does not overwrite an existing setup" '[ $rc -ne 0 ]'
new_home warn; out="$(OS=Linux run warn backup-init.unix.sh 2>&1)"
check "init warns that a repository on the same disk does not protect against losing it" 'grep -q "same disk" <<<"$out"'
check "init does not print the passphrase" '! grep -qF "$(cat "$root/warn/.config/clio/restic-password")" <<<"$out"'

# 2. Only the last two copies are kept by default; the number can be raised.
for i in 1 2 3; do echo "$i" > "$root/linux/.config/clio/config.json"; OS=Linux run linux backup.unix.sh >/dev/null 2>&1; done
check "after 3 copies only the last 2 remain" '[ "$(snapshots "$root/linux")" = 2 ]'
for i in 4 5; do echo "$i" > "$root/linux/.config/clio/config.json"; OS=Linux KEEP=3 run linux backup.unix.sh >/dev/null 2>&1; done
check "with CLIO_BACKUP_KEEP=3, 3 remain" '[ "$(snapshots "$root/linux")" = 3 ]'
OS=Linux KEEP=0 run linux backup.unix.sh >/dev/null 2>&1; rc=$?
check "CLIO_BACKUP_KEEP=0 is refused (it never deletes everything)" '[ $rc -ne 0 ] && [ "$(snapshots "$root/linux")" = 3 ]'

# 3. What was backed up can be restored, and the work copy was left out.
OS=Linux run linux backup-restore-test.unix.sh >/dev/null 2>&1; rc=$?
check "the restore is checked" '[ $rc -eq 0 ]'
check "the working copy (sandbox) is not backed up" '! env HOME="$root/linux" bash -c "set -a; . \"\$HOME/.config/clio/backup.env\"; set +a; restic ls latest" 2>/dev/null | grep -q sandbox'
: > "$root/linux/.local/share/clio/vault.bin"; OS=Linux run linux backup.unix.sh >/dev/null 2>&1
OS=Linux run linux backup-restore-test.unix.sh >/dev/null 2>&1; rc=$?
# shellcheck disable=SC2034
check "an empty vault makes the restore test fail" '[ $rc -ne 0 ]'

[ "$fails" -eq 0 ] && echo "selftest: todo bien" || { echo "selftest: $fails fallo(s)"; exit 1; }
