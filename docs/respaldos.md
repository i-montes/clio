# Respaldos con restic (S-40)

La memoria de Clio, el vault y la configuración viven en este disco. Estos scripts hacen copias cifradas **fuera** de él y comprueban que se pueden leer de vuelta. Decidido el 2026-11-02: la herramienta es `restic`.

## Qué se respalda

- `~/.local/share/clio/memory.db` (copia en caliente con `clio-memory backup` o `sqlite3 .backup`, nunca el archivo en uso), si existe.
- `~/.local/share/clio/vault.bin` (ya cifrado por el vault; restic lo cifra otra vez), si existe.
- `~/.config/clio/config.json`.
- `~/.local/share/clio/runs/` sin las copias de trabajo (`sandbox/` y `scratch/`): quedan los registros, diffs y logs de efectos.

No se respaldan la clave del modelo (`opencode-api-key`), la frase de restic ni lo que se puede reinstalar (`pi/`).

## Puesta en marcha (una vez)

```sh
infra/local/backup-init.linux.sh REPOSITORIO   # crea el repositorio y la frase (0600)
infra/local/backup.linux.sh                    # primera copia
infra/local/backup-restore-test.linux.sh       # prueba que se puede restaurar
infra/local/install-backup.linux.sh            # copia diaria con un timer de usuario
```

`REPOSITORIO` es el destino fuera de este PC: una carpeta de un disco externo (`/run/media/$USER/DISCO/clio-backup`), `sftp:usuario@host:/ruta`, `b2:bucket:ruta`, etc.

## La frase

`backup-init` crea una frase aleatoria en `~/.config/clio/restic-password`. **Está en el mismo disco que los datos**, así que si el disco se pierde, las copias quedan ilegibles. Guarda una copia de esa frase fuera de este equipo (un gestor de contraseñas) justo después de crear el repositorio. El script nunca la imprime.

## Restaurar

```sh
set -a; . ~/.config/clio/backup.env; set +a
restic snapshots                              # elegir un snapshot
restic restore latest --target /ruta/vacía    # los archivos salen bajo su ruta original
```

Solo se copia a mano lo que haga falta de `/ruta/vacía` a su sitio; nada se pisa solo.

## Cómo se comprueba

- `infra/local/check-host.sh` avisa si no hay repositorio configurado o si el último snapshot tiene más de 8 días.
- `backup-restore-test.linux.sh` restaura el último snapshot en una carpeta desechable y comprueba que la memoria pasa `integrity_check` y que el vault y la configuración no están vacíos. Conviene correrlo cada mes (S-40: «restauración probada cada mes»).
