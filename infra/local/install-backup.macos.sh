#!/usr/bin/env bash
# Installs the daily backup as a launchd user agent (no root). Run it after backup-init.unix.sh.
# macOS only (ADR 0002). `launchctl bootout gui/$(id -u)/com.clio.backup` removes it.
# Not tried on a Mac yet: run backup-selftest.unix.sh and one manual backup.unix.sh first.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
[ -r "${CLIO_BACKUP_ENV:-$HOME/.config/clio/backup.env}" ] || { echo "falta la configuración: corre backup-init.unix.sh primero" >&2; exit 2; }
dest="$HOME/Library/LaunchAgents/com.clio.backup.plist"
mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
sed -e "s|@REPO@|$repo|g" -e "s|@HOME@|$HOME|g" "$here/com.clio.backup.macos.plist" > "$dest"
plutil -lint "$dest"
launchctl bootout "gui/$(id -u)/com.clio.backup" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$dest"
launchctl print "gui/$(id -u)/com.clio.backup" | head -5
