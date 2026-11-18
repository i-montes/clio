#!/usr/bin/env bash
# One time: create the restic repository for the backups of Clio (S-40). Linux and macOS (ADR 0002).
#   backup-init.unix.sh [REPOSITORY]
# Without REPOSITORY the place depends on the system, outside Clio's own data folder so that
# deleting the data does not delete the copies:
#   Linux  ${XDG_DATA_HOME:-~/.local/share}/clio-backup/repo
#   macOS  ~/Library/Application Support/clio-backup/repo
# A repository on this same disk protects against deleting or corrupting files, NOT against losing
# the disk: for that give a REPOSITORY elsewhere (an external disk, sftp:user@host:/path, b2:bucket:path).
# Makes ~/.config/clio/restic-password (a random passphrase) and ~/.config/clio/backup.env, both
# 0600. It never prints the passphrase and never overwrites an existing setup.
set -euo pipefail
command -v restic >/dev/null || { echo "restic is not installed" >&2; exit 2; }
case "${CLIO_OS_OVERRIDE:-$(uname -s)}" in   # CLIO_OS_OVERRIDE: only for the self-test
  Darwin) default_repo="$HOME/Library/Application Support/clio-backup/repo" ;;
  Linux)  default_repo="${XDG_DATA_HOME:-$HOME/.local/share}/clio-backup/repo" ;;
  *) echo "unsupported system (Linux and macOS only)" >&2; exit 2 ;;
esac
repo="${1:-$default_repo}"
CONF="${CLIO_CONFIG:-$HOME/.config/clio}"
ENV_FILE="${CLIO_BACKUP_ENV:-$CONF/backup.env}"
PASS_FILE="${CLIO_BACKUP_PASSWORD_FILE:-$CONF/restic-password}"
[ ! -e "$ENV_FILE" ] && [ ! -e "$PASS_FILE" ] || { echo "there is already a setup ($ENV_FILE or $PASS_FILE); not overwriting it" >&2; exit 3; }
umask 077
mkdir -p "$CONF"
case "$repo" in /*) mkdir -p "$(dirname "$repo")" ;; esac   # a local repository: its parent, private (umask)
head -c 32 /dev/urandom | base64 | tr -d '\n' > "$PASS_FILE"
printf 'RESTIC_REPOSITORY=%q\nRESTIC_PASSWORD_FILE=%q\n' "$repo" "$PASS_FILE" > "$ENV_FILE"
chmod 600 "$PASS_FILE" "$ENV_FILE"
if ! RESTIC_REPOSITORY="$repo" RESTIC_PASSWORD_FILE="$PASS_FILE" restic init; then
  rm -f "$ENV_FILE" "$PASS_FILE"
  echo "could not create the repository; the setup was undone" >&2
  exit 4
fi
cat <<MSG

Repositorio creado en $repo.

IMPORTANT: the passphrase that encrypts the copies is only in $PASS_FILE, on this same disk.
If this disk is lost, the copies become unreadable. Keep a copy of that passphrase now
OUTSIDE this machine (a password manager):   cat $PASS_FILE
MSG
# Same disk? (only for a local repository; df -P is the same on both systems)
case "$repo" in /*)
  if [ "$(df -P "$(dirname "$repo")" | awk 'NR==2{print $1}')" = "$(df -P "$HOME" | awk 'NR==2{print $1}')" ]; then
    echo
    echo "WARNING: this repository is on the same disk as your data. It protects against deletions and damaged files,"
    echo "but not against losing the disk. For that, create the repository on an external disk or in the"
    echo "cloud (run this script again with another REPOSITORY)."
  fi ;;
esac
