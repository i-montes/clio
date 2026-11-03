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
command -v restic >/dev/null || { echo "restic no está instalado" >&2; exit 2; }
case "${CLIO_OS_OVERRIDE:-$(uname -s)}" in   # CLIO_OS_OVERRIDE: only for the self-test
  Darwin) default_repo="$HOME/Library/Application Support/clio-backup/repo" ;;
  Linux)  default_repo="${XDG_DATA_HOME:-$HOME/.local/share}/clio-backup/repo" ;;
  *) echo "sistema no soportado (solo Linux y macOS)" >&2; exit 2 ;;
esac
repo="${1:-$default_repo}"
CONF="${CLIO_CONFIG:-$HOME/.config/clio}"
ENV_FILE="${CLIO_BACKUP_ENV:-$CONF/backup.env}"
PASS_FILE="${CLIO_BACKUP_PASSWORD_FILE:-$CONF/restic-password}"
[ ! -e "$ENV_FILE" ] && [ ! -e "$PASS_FILE" ] || { echo "ya hay una configuración ($ENV_FILE o $PASS_FILE); no la piso" >&2; exit 3; }
umask 077
mkdir -p "$CONF"
case "$repo" in /*) mkdir -p "$(dirname "$repo")" ;; esac   # a local repository: its parent, private (umask)
head -c 32 /dev/urandom | base64 | tr -d '\n' > "$PASS_FILE"
printf 'RESTIC_REPOSITORY=%q\nRESTIC_PASSWORD_FILE=%q\n' "$repo" "$PASS_FILE" > "$ENV_FILE"
chmod 600 "$PASS_FILE" "$ENV_FILE"
if ! RESTIC_REPOSITORY="$repo" RESTIC_PASSWORD_FILE="$PASS_FILE" restic init; then
  rm -f "$ENV_FILE" "$PASS_FILE"
  echo "no se pudo crear el repositorio; deshice la configuración" >&2
  exit 4
fi
cat <<MSG

Repositorio creado en $repo.

IMPORTANTE: la frase que cifra las copias está solo en $PASS_FILE, en este mismo disco.
Si este disco se pierde, las copias quedan ilegibles. Guarda ahora una copia de esa frase
FUERA de este equipo (un gestor de contraseñas):   cat $PASS_FILE
MSG
# Same disk? (only for a local repository; df -P is the same on both systems)
case "$repo" in /*)
  if [ "$(df -P "$(dirname "$repo")" | awk 'NR==2{print $1}')" = "$(df -P "$HOME" | awk 'NR==2{print $1}')" ]; then
    echo
    echo "AVISO: este repositorio está en el mismo disco que tus datos. Protege de borrados y de archivos"
    echo "dañados, pero no de perder el disco. Para eso, crea el repositorio en un disco externo o en la"
    echo "nube (vuelve a correr este script con otro REPOSITORIO)."
  fi ;;
esac
