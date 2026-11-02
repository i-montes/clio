# Respaldos con restic (S-40)

La memoria de Clio, el vault y la configuración viven en tu disco. Estos scripts hacen copias cifradas y comprueban que se pueden leer de vuelta. Funcionan igual en Linux y macOS (los scripts compartidos se llaman `.unix.sh`; lo propio de cada sistema, `.linux.` o `.macos.`). Decidido el 2026-11-02: la herramienta es `restic`.

## Dónde van las copias

Sin indicar nada, `backup-init.unix.sh` crea el repositorio en un lugar que depende del sistema y queda **fuera de la carpeta de datos de Clio**, para que borrar los datos no borre las copias:

| Sistema | Repositorio por defecto |
|---|---|
| Linux | `${XDG_DATA_HOME:-~/.local/share}/clio-backup/repo` |
| macOS | `~/Library/Application Support/clio-backup/repo` |

**Ese lugar está en el mismo disco que tus datos.** Protege de borrar o dañar archivos, no de perder el disco. Para sobrevivir a eso, pasa un destino distinto al crear el repositorio: una carpeta de un disco externo (`/run/media/$USER/DISCO/clio-backup`, `/Volumes/DISCO/clio-backup`), `sftp:usuario@host:/ruta`, `b2:bucket:ruta`. `check-host.sh` te lo recuerda con un aviso mientras el repositorio siga en el mismo disco.

## Cuántas copias

Solo las **2 últimas** (`restic forget --keep-last 2 --prune` después de cada copia). Se cambia con `CLIO_BACKUP_KEEP=N` (entero de 1 o más; nunca borra todo). Con una copia diaria eso son dos días de historia: si un problema pasa desapercibido más tiempo, no habrá una copia anterior buena. restic guarda solo lo que cambió, así que subir el número cuesta poco espacio.

## Qué se respalda

- `~/.local/share/clio/memory.db` (copia en caliente con `clio-memory backup` o `sqlite3 .backup`, nunca el archivo en uso), si existe.
- `~/.local/share/clio/vault.bin` (ya cifrado por el vault; restic lo cifra otra vez), si existe.
- `~/.config/clio/config.json`.
- `~/.local/share/clio/runs/` sin las copias de trabajo (`sandbox/` y `scratch/`): quedan los registros, diffs y logs de efectos.

No se respaldan la clave del modelo (`opencode-api-key`), la frase de restic ni lo que se puede reinstalar (`pi/`).

## Puesta en marcha (una vez)

```sh
infra/local/backup-init.unix.sh [REPOSITORIO]  # crea el repositorio y la frase (0600)
infra/local/backup.unix.sh                     # primera copia
infra/local/backup-restore-test.unix.sh        # prueba que se puede restaurar
infra/local/install-backup.linux.sh            # copia diaria: timer de systemd (Linux)
infra/local/install-backup.macos.sh            # copia diaria: launchd (macOS, sin probar en un Mac)
```

`infra/local/backup-selftest.unix.sh` prueba todo esto contra repositorios desechables en un HOME falso, sin tocar nada real. Córrelo en un Mac antes de fiarte del instalador de macOS.

## La frase

`backup-init` crea una frase aleatoria en `~/.config/clio/restic-password`. **Está en el mismo disco que los datos**, así que si el disco se pierde, las copias quedan ilegibles aunque estén en otro lado. Guarda una copia de esa frase fuera de este equipo (un gestor de contraseñas) justo después de crear el repositorio. El script nunca la imprime.

## Restaurar

```sh
set -a; . ~/.config/clio/backup.env; set +a
restic snapshots                              # elegir un snapshot
restic restore latest --target /ruta/vacía    # los archivos salen bajo su ruta original
```

Solo se copia a mano lo que haga falta de `/ruta/vacía` a su sitio; nada se pisa solo.

## Cómo se comprueba

- `infra/local/check-host.sh` (Linux) avisa si no hay repositorio configurado, si el último snapshot tiene más de 8 días o si el repositorio está en el mismo disco.
- `backup-restore-test.unix.sh` restaura el último snapshot en una carpeta desechable y comprueba que la memoria pasa `integrity_check` y que el vault y la configuración no están vacíos. Conviene correrlo cada mes (S-40: «restauración probada cada mes»).
