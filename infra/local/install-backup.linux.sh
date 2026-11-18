#!/usr/bin/env bash
# Installs the daily backup as a systemd USER timer (no root). Run it after backup-init.unix.sh.
# Linux only (ADR 0002). `systemctl --user disable --now clio-backup.timer` removes it.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
[ -r "${CLIO_BACKUP_ENV:-$HOME/.config/clio/backup.env}" ] || { echo "the backup is not set up: run backup-init.unix.sh first" >&2; exit 2; }
dest="$HOME/.config/systemd/user"
mkdir -p "$dest"
sed "s|@REPO@|$repo|g" "$here/clio-backup.linux.service" > "$dest/clio-backup.service"
cp "$here/clio-backup.linux.timer" "$dest/clio-backup.timer"
systemctl --user daemon-reload
systemctl --user enable --now clio-backup.timer
systemctl --user list-timers clio-backup.timer --no-pager
