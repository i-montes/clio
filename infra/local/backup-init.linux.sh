#!/usr/bin/env bash
# One time: create the restic repository for the backups of Clio (S-40). Linux only (ADR 0002).
#   backup-init.linux.sh REPOSITORY      e.g. /run/media/$USER/DISCO/clio-backup
#                                             sftp:user@host:/path   or   b2:bucket:path
# Makes ~/.config/clio/restic-password (a random passphrase) and ~/.config/clio/backup.env, both
# 0600. It never prints the passphrase and never overwrites an existing setup.
set -euo pipefail
[ $# -eq 1 ] || { echo "uso: $0 REPOSITORIO" >&2; exit 2; }
command -v restic >/dev/null || { echo "restic no está instalado" >&2; exit 2; }
CONF="${CLIO_CONFIG:-$HOME/.config/clio}"
ENV_FILE="${CLIO_BACKUP_ENV:-$CONF/backup.env}"
PASS_FILE="${CLIO_BACKUP_PASSWORD_FILE:-$CONF/restic-password}"
[ ! -e "$ENV_FILE" ] && [ ! -e "$PASS_FILE" ] || { echo "ya hay una configuración ($ENV_FILE o $PASS_FILE); no la piso" >&2; exit 3; }
umask 077
mkdir -p "$CONF"
head -c 32 /dev/urandom | base64 | tr -d '\n' > "$PASS_FILE"
printf 'RESTIC_REPOSITORY=%s\nRESTIC_PASSWORD_FILE=%s\n' "$1" "$PASS_FILE" > "$ENV_FILE"
chmod 600 "$PASS_FILE" "$ENV_FILE"
if ! RESTIC_REPOSITORY="$1" RESTIC_PASSWORD_FILE="$PASS_FILE" restic init; then
  rm -f "$ENV_FILE" "$PASS_FILE"
  echo "no se pudo crear el repositorio; deshice la configuración" >&2
  exit 4
fi
cat <<MSG

Repositorio creado en $1.

IMPORTANTE: la frase que cifra las copias está solo en $PASS_FILE, en este mismo disco.
Si este disco se pierde, las copias quedan ilegibles. Guarda ahora una copia de esa frase
FUERA de este equipo (un gestor de contraseñas):   cat $PASS_FILE
MSG
