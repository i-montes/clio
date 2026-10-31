# Usar Clio en tu PC

Clio corre en tu propia máquina (ADR 0006): sin servidor, sin puertos públicos. Este documento explica cómo usarla hoy con el comando `clio`.

## La regla central

**El agente nunca toca tu proyecto.** `clio run` hace una COPIA del proyecto y el agente trabaja solo en esa copia. Al terminar, tú ves el diff y decides con `clio apply` qué vuelve al original. Si no te gusta, no pasa nada: el original sigue igual. Si aplicas y te arrepientes, `clio undo` lo deja como estaba.

## Instalar y preparar

```sh
just install                                        # compila y deja clio y clio-vault en ~/.local/bin
# (a mano: cargo build --release -p clio-brain -p clio-vault y copiar target/release/clio* a ~/.local/bin)
clio init
```

Ejemplo de salida:

```
ok      /home/tu/.config/clio (0700)
created /home/tu/.local/share/clio (0700)
created /home/tu/.local/share/clio/runs (0700)
created /run/user/1000/clio (0700)
created /home/tu/.config/clio/config.json (0600)
ok      model key found in /home/tu/.config/clio/opencode-api-key (0600, not shown)
next: clio doctor, then clio run --workspace DIR "instruction"
```

`init` es idempotente: no pisa lo que ya existe (si cambias `config.json`, queda como lo dejaste). Deja las carpetas en 0700, nunca imprime la clave y avisa si el archivo de la clave tiene permisos abiertos.

**La clave del modelo.** Está en la variable `OPENCODE_API_KEY` o en `~/.config/clio/opencode-api-key` (`chmod 600`). La lee solo el adaptador del modelo; Clio no la imprime ni la guarda en ningún run.

**Configuración** (`~/.config/clio/config.json`): proveedor `opencode-go`, modelo `beam-free` (gratuito), el comando del adaptador (`python3 .../adapters/models/openai_compat.py`), el prompt `clio-agent@2`, y los límites: 40 pasos, 600 segundos, 0,10 USD por tarea, y para la copia 5000 archivos y 200 MB. Si `init` no encuentra `adapters/models/openai_compat.py` junto al ejecutable o en el directorio actual, instala una copia en `~/.local/share/clio/adapters/`. Ojo: la ruta del adaptador queda escrita en el config; si mueves o borras el repositorio, edita `model.command`.

## Un trabajo de punta a punta

```sh
clio run --workspace ~/proyectos/mi-app "arregla parse_date en dates.py para que devuelva None con entradas inválidas"
```

```
run 20261031-181646-5a67: copied 2 files from /home/tu/proyectos/mi-app
NOT copied, they look like secrets: .env
not copied (heavy or internal folders): .git, node_modules
working in the copy (this can take a few minutes)...

run:      20261031-181646-5a67
level:    Simple   steps: 11   time: 21.8s   ending: Final
changes:  1 modified, 0 created, 0 deleted
  M dates.py
agent:    Modificado dates.py: ...
The original was NOT touched. Review:  clio diff 20261031-181646-5a67   then  clio apply 20261031-181646-5a67
```

(El ejemplo de salida del `run` es de una ejecución real con `beam-free`; las líneas de «NOT copied» aparecen solo cuando hay algo que excluir.)

Opciones de `run`: `--workspace DIR` (por defecto la carpeta actual), `--max-steps N`, `--allow-broad` (quita los topes de tamaño; no quita el rechazo de `/` ni de tu carpeta personal).

```sh
clio diff 20261031-1816          # basta un prefijo único del id
clio apply 20261031-1816         # muestra el diff y pregunta [y/N]
clio undo 20261031-1816          # restaura desde el respaldo
clio runs                        # los 20 más recientes (--all para todos)
clio show 20261031-1816          # informe y resumen
```

`diff` muestra un diff unificado de texto (para binarios, solo «binary file: changed»). `runs` lista, por ejemplo: `20261031-181646-5a67  applied    1 files  La función parse_date...`. Los estados son: `final` (el agente dijo que terminó), `stopped(...)`/`modelerror(...)` (se cortó), `applied`, `undone` e `incomplete` (el proceso murió antes de cerrar).

## Qué hace cada paso de seguridad

| Paso | Qué hace | Por qué |
|---|---|---|
| Validar el workspace | Rechaza `/`, tu `$HOME` completo, carpetas que contengan las de Clio, y más de 5000 archivos o 200 MB (salvo `--allow-broad`) | Que un descuido no copie medio disco |
| Copiar | Excluye `.git`, `node_modules`, `target`, `.venv`, `__pycache__`, enlaces simbólicos y todo lo que parece secreto (`.env*`, `*.pem`, `*.key`, `id_rsa*`, `id_ed25519*`, `.ssh`, `.aws`, `credentials*`) y te lo dice | El agente no debe leer secretos ni historia de git |
| Carpeta del run | `~/.local/share/clio/runs/<id>/`, todo 0700/0600 | Solo tu usuario la lee (S-23) |
| El agente | Triaje, contrato (desde el nivel 2), pasarela, ejecutor y límites de pasos/tiempo/costo; solo ve la copia | Reglas S-05, S-13, S-16, S-47 |
| `apply` | Comprueba cada ruta, pide confirmación, respalda lo que sobrescribe | S-14: lo que sale del sandbox lo aprueba una persona |

Qué comprueba `apply` antes de escribir nada (todo o nada: si una ruta falla, no se aplica ningún archivo):

- La ruta debe ser relativa y simple (sin `..`, sin ruta absoluta).
- Nunca escribe en `.git`, `.claude`, `.pi`, `extensions`, `skills` (rutas protegidas), ni en archivos con nombre de secreto. `--force` NO levanta esto.
- Si hay un enlace simbólico en el camino (por ejemplo, `docs` convertido en enlace a otra carpeta), se niega. `--force` NO levanta esto.
- Si un archivo del original cambió desde la copia (su hash ya no coincide), se niega a pisarlo: se lo señala. Con `--force` lo pisa, y el respaldo guarda TU versión para poder deshacer.
- Un archivo nuevo que ya existe en el original cuenta igual como conflicto.
- El respaldo de cada archivo que se sobrescribe o borra queda en `runs/<id>/backup/`, y lo aplicado en `runs/<id>/applied.json`. `undo` solo restaura si el original sigue como lo dejó `apply` (o con `--force`).

Qué hay en la carpeta de un run: `sandbox/` (la copia, con los cambios del agente), `manifest.json` (hash de cada archivo copiado), `meta.json`, `contract.json` (desde nivel 2), `effects.jsonl`, `transcript.json` (la conversación completa), `report.json` (hecho, pendiente, siguiente paso), `summary.json`.

## La bóveda de secretos (opcional)

`clio run` funciona sin secretos y hoy no usa la bóveda. Si quieres guardar claves:

```sh
clio vault start            # pide la frase; crea la bóveda la primera vez, la lanza y la desbloquea
clio vault put github --dest http_fetch:api.github.com   # el valor se lee sin eco, nunca se imprime
clio vault list
clio vault revoke github
clio vault status
clio vault stop
```

Es un envoltorio del binario `clio-vault` (lo busca junto a `clio`, o en `CLIO_VAULT_BIN`/`PATH`). Los dos sockets quedan en `$XDG_RUNTIME_DIR/clio/` con permisos 0600; el archivo de la bóveda en `~/.local/share/clio/vault.bin`.

## Comprobar la máquina

```sh
clio doctor
```

Revisa: permisos de las carpetas de Clio (0700) y de sus archivos (0600), la clave del modelo (permisos), sockets 0600, que ningún proceso `clio*` escuche fuera de loopback, y ejecuta `infra/local/check-host.sh` (lo busca junto al repositorio o en `CLIO_HOST_CHECK`) mostrando su salida. Sale con 1 si hay algún FAIL; los WARN son decisión tuya.

## Qué NO hace Clio todavía

- **No ejecuta comandos ni pruebas.** No hay jaula `bwrap` aún (ADR 0006), así que el agente solo lee, lista, escribe y crea carpetas en la copia. Dice en su mensaje final que no pudo correr nada: ejecuta tú los tests antes de `apply`.
- **No hay red para el agente**; solo el adaptador del modelo habla con internet (con tu clave).
- **No ejecuta tareas largas con aprobación interactiva del contrato**: como escribiste tú la instrucción en la terminal, se toma como aprobación.
- **No borra archivos** (el agente no tiene herramienta para eso); `apply` sí sabría aplicar un borrado si apareciera en la copia.
- La bóveda no está conectada al agente en `run`; los secretos que excluye la copia simplemente no existen para él.
- Nada corre en segundo plano, no hay memoria entre runs, ni PWA. El agente no ve cambios que hagas en el original después de la copia (por eso `apply` los detecta por hash).

## Riesgos que conviene tener presentes

- El adaptador corre como tu usuario y manda al modelo el texto de los archivos copiados. Lo que no quieras que salga de tu PC, no debe estar en el workspace (los nombres de secreto se excluyen, pero un secreto escrito dentro de un archivo normal sí se copia y se envía al modelo: el filtro del agente aún no está conectado a tu bóveda).
- `apply` escribe con tu usuario: revisa el diff, sobre todo scripts y configuración.
- Estar en el grupo `docker` equivale a ser root; mantén Docker fuera del alcance del agente (`clio doctor` lo avisa).
