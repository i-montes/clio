# Usar Clio en tu PC

Clio corre en tu propia máquina (ADR 0006): sin servidor, sin puertos públicos. Este documento explica cómo usarla hoy con el comando `clio`.

## La regla central

**El agente nunca toca tu proyecto.** `clio run` hace una COPIA del proyecto y el agente trabaja solo en esa copia. Al terminar, tú ves el diff y decides con `clio apply` qué vuelve al original. Si no te gusta, no pasa nada: el original sigue igual. Si aplicas y te arrepientes, `clio undo` lo deja como estaba.

## La interfaz: escribe `clio`

En una terminal, `clio` sin argumentos abre una interfaz de pantalla completa (TUI). Todo lo que hacen los comandos de abajo se puede hacer desde ella, con las mismas protecciones (`apply` y `undo` son el mismo código). Fuera de una terminal (una tubería, un script, CI), `clio` imprime la ayuda y sale con 0; `clio --help` también; un comando desconocido sale con 2.

Teclas generales: `?` ayuda, `q` o `Ctrl+C` salir (desde Inicio), `Esc` volver. Las listas se mueven con flechas o `j`/`k`.

**Inicio**: el estado de Clio y los runs recientes.

```
 clio  Inicio  (el original nunca se toca sin tu permiso)
╭ Estado ──────────────────────────────────────────────────────────────────────╮
│Modelo opencode-go / beam-free    Clave presente                              │
│Vault sin crear (opcional)    Doctor 12 ok · 3 avisos · 0 fallos              │
╰──────────────────────────────────────────────────────────────────────────────╯
╭ Runs recientes ──────────────────────────────────────────────────────────────╮
│▶ 1031-1816-5a67 pendiente de revisar   1 arch arregla parse_date en dates.py │
│  1030-0900-0b1c aplicado               2 arch agrega un README               │
│  1029-1200-ffff fallido                0 arch refactoriza todo               │
╰──────────────────────────────────────────────────────────────────────────────╯
n nuevo   Enter abrir   d diff   a aplicar   u deshacer   r refrescar   D doctor
? ayuda   q salir
```

`n` nuevo trabajo, `Enter` abrir el run, `d` ver su diff, `a` aplicar, `u` deshacer, `r` refrescar, `D` doctor. Estados: `aplicado` (verde), `pendiente de revisar` (amarillo: el agente terminó y hay cambios sin aplicar), `fallido` (rojo), `cancelado`, `deshecho` y `sin cambios` (atenuados), `incompleto` (el proceso murió antes de cerrar). El id corto `1031-1816-5a67` es `MMDD-HHMM-sufijo` del id completo `20261031-181646-5a67`. La clave del modelo nunca se muestra, solo si está.

**Nuevo trabajo** (`n`): la carpeta (por defecto la actual) y la instrucción. La carpeta se valida mientras escribes con las reglas de `clio run` (rechaza `/`, tu carpeta personal y las que son demasiado grandes) y la pantalla te dice qué NO se copiará (secretos, `.git`, `node_modules`, enlaces simbólicos...) antes de empezar.

```
┌ Carpeta del proyecto ─ /home/ivan/proyecto
┏ Instrucción ━━━━━━━━ arregla parse_date en dates.py▏
╭ Qué se va a copiar ──────────────────────────────────────────────────────────╮
│✓ se copiarán 12 archivos (3.0 MB)                                            │
│NO se copiarán a la copia de trabajo:                                         │
│  parecen secretos: .env                                                      │
│  carpetas pesadas o internas: .git, node_modules                             │
╰──────────────────────────────────────────────────────────────────────────────╯
```

`Tab` cambia de campo, `Enter` lanza (en la carpeta, pasa a la instrucción), `Alt+Enter` o `Ctrl+J` agrega una línea a la instrucción, `Ctrl+U` borra el campo, `Esc` vuelve. Pegar texto no puede lanzar nada: el texto pegado se limpia y sus saltos de línea no pulsan Enter.

**En vivo**: el agente corre en un hilo aparte; ves cada acción que pide el modelo y la decisión de la pasarela (permitido, pedido o negado, con la regla), el tamaño del resultado, los pasos y el tiempo contra el límite, un indicador de actividad y si la sesión está contaminada (leyó archivos).

```
╭  ────────────────────────────────────────────────────────────────────────────╮
│⠋ trabajando en la copia…   20261031-181646-5a67   nivel Simple               │
│Pasos  ██░░░░░░░░░░░░░░░░░░░░░░ 4/40                                          │
│Tiempo ░░░░░░░░░░░░░░░░░░░░░░░░ 14s/600s                                      │
│Sesión contaminada: leyó archivos, la pasarela es más estricta                │
╰──────────────────────────────────────────────────────────────────────────────╯
╭ Línea de tiempo ─────────────────────────────────────────────────────────────╮
│   1  list_dir   .  permitido (step-8-reversible-in-sandbox)  812 car.        │
│   2  read_file  dates.py  permitido (step-8-reversible-in-sandbox)  812 car. │
│   3  write_file .git/hooks/pre-commit  negado (S-16)                         │
╰──────────────────────────────────────────────────────────────────────────────╯
```

`Esc` o `Ctrl+C` piden cancelar: el agente termina el paso en curso (un paso puede tardar lo que tarde el modelo en contestar) y el run queda `cancelado`. Una segunda pulsación sale de Clio ya; el run queda `incompleto`. En los dos casos el original no se tocó. Al terminar, `Enter` abre el detalle.

**Detalle del run**: resumen, mensaje final del agente (con ajuste de línea), archivos cambiados (`M` modificado, `A` nuevo, `D` borrado) y, con `Enter`, el diff del archivo elegido: `+` en verde, `-` en rojo, `@@` en cian.

```
╭ Resumen ─────────────────────────────────────────────────────────────────────╮
│Estado pendiente de revisar   20261031-181646-5a67                            │
│Carpeta /home/ivan/proyecto                                                   │
│nivel Simple · 7 pasos · 12.3s · terminó · 0 denegadas                        │
╰──────────────────────────────────────────────────────────────────────────────╯
╭ Mensaje del agente ──────────────────────────────────────────────────────────╮
│Arreglé parse_date. No pude correr las pruebas.                               │
╰──────────────────────────────────────────────────────────────────────────────╯
╭ Archivos cambiados (2) ──────────────────────────────────────────────────────╮
│▶ M dates.py                                                                  │
│  A docs/new.md                                                               │
╰──────────────────────────────────────────────────────────────────────────────╯
```

En el diff: `j`/`k` desplazan, `PgUp`/`PgDn` una página, `g`/`G` principio y final, `[` y `]` cambian de archivo, `Esc` vuelve a la lista. `a` aplica: primero pasa todas las comprobaciones de `apply` (si algo falla, un aviso te dice qué; no se escribe nada) y luego un diálogo repite cuántos archivos y dónde, y solo una `y` sigue (cualquier otra tecla cancela). `u` deshace un run aplicado, también con `y`. Si el original cambió desde la copia, la TUI no pisa nada: usa `clio apply <id> --force` desde la línea de comandos, sabiendo que guarda tu versión para poder deshacer.

**Doctor** (`D`): las comprobaciones con `PASS`/`WARN`/`FAIL` coloreados y, bajo cada aviso o fallo, una línea con qué hacer (`r` repite, `j`/`k` desplazan).

```
╭ Doctor: 1 ok · 1 aviso · 1 fallo ────────────────────────────────────────────╮
│PASS  /home/ivan/.config/clio es privada (700)                                │
│WARN  no hay clave del modelo                                                 │
│      → Ponla en OPENCODE_API_KEY o en ~/.config/clio/opencode-api-key        │
│FAIL  /x/y tiene modo 755, debe ser 700 (S-23)                                │
│      → chmod 700 /x/y                                                        │
╰──────────────────────────────────────────────────────────────────────────────╯
```

**Colores**: solo los 8 colores ANSI básicos más negrita, tenue y video inverso, así que la paleta de tu terminal decide los tonos (sirve en tema oscuro y claro y sin 24 bits). Con `NO_COLOR=1` o `TERM=dumb` queda sin color. Requiere una terminal de al menos 40x10.

**Seguridad de la pantalla.** Lo que viene de archivos, del modelo, de nombres de archivo, de diffs o de la instrucción puede traer secuencias de escape (cambiar el título de la ventana, borrar la pantalla, simular otra pantalla), un `\r` que sobreescribe una línea o caracteres bidireccionales (como U+202E) que invierten el texto. Antes de dibujar o imprimir cualquier texto externo, Clio elimina las secuencias de escape completas (CSI, OSC, DCS, APC...) y reemplaza los demás caracteres de control, los separadores de línea y los caracteres invisibles o bidireccionales por `�`; un tabulador pasa a cuatro espacios. Esto vale para la TUI y para lo que imprimen `run`, `runs`, `show`, `diff`, `apply` y `doctor` en una terminal. Los archivos del run guardan el texto tal cual llegó. Limitación: se reemplazan también los unidos de ancho cero (ZWJ), así que un emoji compuesto puede verse como varios. Si Clio se cae por un error, la terminal se restaura antes de mostrar el mensaje (sale de la pantalla alterna y del modo crudo); no puede hacerlo si la matan con `kill -9` o con una señal; en ese caso, escribe `reset`.

## Instalar y preparar

```sh
just install                                        # compila y deja clio y clio-vault en ~/.local/bin
# (a mano: cargo build --release -p clio-brain -p clio-vault y copiar target/release/clio* a ~/.local/bin)
clio init
```

Ejemplo de salida:

```
PASS  ya estaba    /home/tu/.config/clio (0700)
PASS  creada       /home/tu/.local/share/clio (0700)
PASS  creada       /home/tu/.local/share/clio/runs (0700)
PASS  creada       /run/user/1000/clio (0700)
PASS  creada       /home/tu/.config/clio/config.json (0600)
PASS  clave del modelo encontrada en /home/tu/.config/clio/opencode-api-key (0600, no se muestra)

siguiente: clio doctor, y luego clio
```

(En una terminal con Unicode, `PASS`/`WARN`/`FAIL` se ven como `✓`/`⚠`/`✗` y en colores; sin terminal el texto es plano y estable.)

`init` es idempotente: no pisa lo que ya existe (si cambias `config.json`, queda como lo dejaste). Deja las carpetas en 0700, nunca imprime la clave y avisa si el archivo de la clave tiene permisos abiertos.

**La clave del modelo.** Está en la variable `OPENCODE_API_KEY` o en `~/.config/clio/opencode-api-key` (`chmod 600`). La lee solo el adaptador del modelo; Clio no la imprime ni la guarda en ningún run.

**Configuración** (`~/.config/clio/config.json`): proveedor `opencode-go`, modelo `beam-free` (gratuito), el comando del adaptador (`python3 .../adapters/models/openai_compat.py`), el prompt `clio-agent@2` (con la jaula probada se usa `clio-agent@3`, que explica `run_tests`), y los límites: 40 pasos, 600 segundos, 0,10 USD por tarea, y para la copia 5000 archivos y 200 MB. Si `init` no encuentra `adapters/models/openai_compat.py` junto al ejecutable o en el directorio actual, instala una copia en `~/.local/share/clio/adapters/`. Ojo: la ruta del adaptador queda escrita en el config; si mueves o borras el repositorio, edita `model.command`.

## Un trabajo de punta a punta

```sh
clio run --workspace ~/proyectos/mi-app "arregla parse_date en dates.py para que devuelva None con entradas inválidas"
```

```
run 20261031-191737-ccb5: 2 archivos copiados de /home/tu/proyectos/mi-app
  WARN NO se copió, parece un secreto: .env
  - no se copió (carpetas pesadas o internas): .git
trabajando en la copia (puede tardar unos minutos; hasta 40 pasos o 600 s)...
    0s  [0/40] nivel de la tarea: Simple
    0s  [1/40] read_file app.py  permitido (step-8-reversible-in-sandbox)  193 car.
    0s  [2/40] write_file app.py  permitido (step-8-reversible-in-sandbox)  179 car.
    0s  [3/40] write_file .git/x  negado (S-16)
    0s  [4/40] el modelo dice que terminó

run:      20261031-191737-ccb5
nivel:    Simple   pasos: 4   tiempo: 0.1s   fin: terminó
cambios:  1 modificados, 0 creados, 0 borrados
  M app.py
nota:     leyó archivos del proyecto, así que su texto pudo influirlo; revisa el diff con cuidado
agente:   Corregí total() en app.py: ya no suma 1 de más. No pude correr las
          pruebas; ejecútalas antes de aplicar.

El original NO se tocó. Revisa:  clio diff 20261031-191737-ccb5   y luego  clio apply 20261031-191737-ccb5
```

(Esta salida es la de `clio run` sin terminal, con un modelo de mentira que sigue un guion; con un modelo real los pasos y el texto cambian. Las líneas de «NO se copió» aparecen solo cuando hay algo que excluir. Hay una línea por paso en vez de minutos de silencio; en una terminal con color la decisión sale en verde, amarillo o rojo. `clio run` sale con 0 si el agente terminó y con 1 si no.)

Opciones de `run`: `--workspace DIR` (por defecto la carpeta actual), `--max-steps N`, `--allow-broad` (quita los topes de tamaño; no quita el rechazo de `/` ni de tu carpeta personal).

```sh
clio diff 20261031-1816          # basta un prefijo único del id
clio apply 20261031-1816         # muestra el diff y pregunta [s/N]
clio undo 20261031-1816          # restaura desde el respaldo
clio runs                        # los 20 más recientes (--all para todos)
clio show 20261031-1816          # resumen, mensaje del agente y siguiente paso
```

`diff` muestra un diff unificado de texto (para binarios, solo «binary file: changed»), en color en una terminal. `runs` es una tabla alineada:

```
RUN                   ESTADO                ARCHIVOS  INSTRUCCIÓN
20261031-191737-ccb5  pendiente de revisar         1  arregla total en app.py
```

Los estados son: `pendiente de revisar` (terminó y hay cambios), `sin cambios`, `fallido` (se cortó por un límite, falló el modelo o se detuvo), `cancelado`, `aplicado`, `deshecho` e `incompleto` (el proceso murió antes de cerrar).

## En scripts: `--json`, `NO_COLOR`

`clio runs`, `clio show <id>` y `clio doctor` aceptan `--json` y escriben JSON en la salida estándar. El campo `status` de un run es una palabra estable: `pending_review`, `no_changes`, `failed`, `cancelled`, `applied`, `undone` o `incomplete`; `ending` es el final del agente (`Final`, `Stopped(Steps)`, `Cancelled`, ...).

```sh
clio runs --json | jq -r '.[] | select(.status=="pending_review") | .id'
clio doctor --json | jq '{ok, warn, fail}'
```

Color y símbolos solo salen cuando la salida es una terminal. `NO_COLOR=1` o `TERM=dumb` los quitan; si la terminal no declara UTF-8 (`LANG`/`LC_ALL`) o es la consola de Linux, se usan palabras ASCII (`PASS`, `WARN`, `FAIL`) en vez de `✓ ⚠ ✗`. Sin terminal (una tubería, un archivo) el texto es plano y estable. Los errores van a la salida de errores, cortos y con lo que se puede hacer, y el código de salida es 1 (2 para un comando desconocido).

## La jaula y `run_tests`

El agente no puede ejecutar nada por su cuenta. Lo único que puede ejecutar es `run_tests`, y solo cuando la **jaula** está disponible y probada. Es la herramienta que le permite verificar su trabajo: sin ella, nada comprobaba lo que escribía y el contrato nunca cerraba como hecho.

**Qué es la jaula.** Un programa de la persona corriendo como tu usuario podría leer `~/.ssh`, el perfil del navegador y salir a la red. La jaula usa `bwrap` (bubblewrap) para ejecutar UN comando, con argumentos explícitos (nunca un shell), dentro de espacios de nombres nuevos y sin privilegios.

**Qué puede hacer el código que corre dentro:**

- Leer y escribir la copia del proyecto, que ve como `/work` (la carpeta de trabajo del run y nada más).
- Leer `/usr` y lo necesario para arrancar (`/bin`, `/lib`, `/lib64`, el caché del cargador), todo de solo lectura.
- Escribir en su propio `/tmp` (un tmpfs de 256 MB que desaparece al terminar) y en un HOME falso dentro de él.
- Usar hasta 2 GiB de memoria virtual, 128 procesos o hilos, 120 s de CPU y 120 s de reloj, archivos de hasta 64 MB y 512 descriptores abiertos; guarda las últimas 64 KB de cada salida.

**Qué NO puede hacer (y `clio doctor` lo comprueba cada vez):**

- Ver tu carpeta personal, `~/.ssh`, `~/.config`, el perfil del navegador ni los sockets de Clio: no existen dentro. Un enlace simbólico de la copia que apunte fuera apunta a nada.
- Abrir conexiones: no hay interfaces de red (solo un `lo` propio y vacío), ni DNS, ni los sockets abstractos del anfitrión.
- Escribir en `/usr`, `/etc`, la raíz ni en tu HOME real. Tampoco en `.git`, `.claude`, `.pi`, `extensions` y `skills` dentro de la copia (se montan de solo lectura).
- Ver ni matar otros procesos (tiene su propio espacio de procesos), ni crear otros espacios de nombres.
- Heredar tu entorno: ni `OPENCODE_API_KEY`, ni `SSH_AUTH_SOCK`, ni nada. Su entorno es `PATH=/usr/bin:/bin`, `HOME=/tmp/home`, `LANG` y `TMPDIR`.
- Sobrevivir al tiempo límite: al vencer se mata todo el espacio de procesos, incluidos los demonios que dejara.

**Cómo se ofrece.** Solo en Linux, con `bwrap` 0.8 o más nuevo y `prlimit` (util-linux) en `/usr/bin`, y `python3`. Al empezar un `clio run`, Clio arranca la jaula y corre una autoprueba (abajo); si falla cualquier cosa, el agente no recibe `run_tests` y usa el prompt `clio-agent@2` (sin la herramienta). Si está probada, usa `clio-agent@3`. `clio run` dice cuál de los dos casos es en una línea. `CLIO_JAIL=off` la apaga. En macOS y otros sistemas la herramienta no existe: se dice que la jaula es solo para Linux y todo lo demás funciona igual.

**La herramienta.** `{"runner": "unittest" | "pytest", "target": "<opcional>"}`. No es un shell: Clio arma el comando. `unittest` corre `python3 -m unittest -q [target]`; `pytest` corre `python3 -m pytest -q [target]` si pytest está instalado (si no, error claro). `target` es un archivo o carpeta de la copia, solo con letras, dígitos, `_`, `.`, `/` y `-`, sin empezar por `-`, sin `..`, y debe existir (los enlaces que salen de la copia se rechazan). No hay `cargo_test`: el `cargo` de `/usr/bin` suele ser un lanzador de rustup cuyas herramientas viven en tu HOME, y hacerlo funcionar exigiría montar parte de tu HOME dentro de la jaula.

**Qué hace Clio con lo que imprimen las pruebas.** Es contenido externo, como un archivo: pasa por el limpiador de secretos, llega al modelo envuelto como datos (`UNTRUSTED-DATA`) y marca la sesión como contaminada. El código de salida (0 = pasaron; "no se ejecutó ninguna prueba" cuenta como no pasar) es una señal de fuera del modelo: si la última ejecución de `run_tests` vino DESPUÉS de la última escritura y pasó, el contrato (nivel Full o superior) cierra como `ClosedOk`; si no se corrieron pruebas, fallaron o hubo escrituras después, queda `Blocked` como antes. Que pasen las pruebas existentes no prueba que la tarea esté completa: revisa el diff.

**Snapshots.** Con la jaula activa, el snapshot del sandbox copia los archivos en vez de usar enlaces duros: un programa que edite un archivo en sitio no puede corromper lo que restaura una cancelación.

**`clio doctor` y la autoprueba.** Dentro de la jaula corre un programa que intenta lo prohibido y comprueba que falla: leer un señuelo que Clio crea en tu HOME (y lo borra), abrir un socket hacia un puerto local y un socket abstracto que Clio abre, escribir en `/usr`, `/etc`, la raíz y tu HOME, ver y matar un proceso ajeno (Clio lanza un `sleep` para eso), heredar variables de tu entorno y crear otros espacios de nombres. Muestra `PASS` o `FAIL` por cada una (`clio doctor --json` trae `jail: {available, checks}`). Un `FAIL` cierra la herramienta y hace que `doctor` salga con 1. Si el núcleo no permite espacios de nombres de usuario sin privilegios (o AppArmor bloquea `bwrap`), `doctor` lo dice con un `WARN` y la herramienta queda desactivada; en ese caso, en Ubuntu reciente mira `kernel.apparmor_restrict_unprivileged_userns`.

**Lo que la jaula no cubre:** mira la lista honesta en `docs/threat-model.md` (ataques al núcleo, canales laterales, tiempo de CPU, disco). Si matan `clio` con `kill -9` durante `doctor`, puede quedar un señuelo `.clio-jail-decoy-*` en tu HOME: bórralo, no contiene nada.

## Qué hace cada paso de seguridad

| Paso | Qué hace | Por qué |
|---|---|---|
| Validar el workspace | Rechaza `/`, tu `$HOME` completo, carpetas que contengan las de Clio, y más de 5000 archivos o 200 MB (salvo `--allow-broad`) | Que un descuido no copie medio disco |
| Copiar | Excluye `.git`, `node_modules`, `target`, `.venv`, `__pycache__`, enlaces simbólicos y todo lo que parece secreto (`.env*`, `*.pem`, `*.key`, `id_rsa*`, `id_ed25519*`, `.ssh`, `.aws`, `credentials*`) y te lo dice | El agente no debe leer secretos ni historia de git |
| Carpeta del run | `~/.local/share/clio/runs/<id>/`, todo 0700/0600 | Solo tu usuario la lee (S-23) |
| El agente | Triaje, contrato (desde el nivel 2), pasarela, ejecutor y límites de pasos/tiempo/costo; solo ve la copia | Reglas S-05, S-13, S-16, S-47 |
| Las pruebas (`run_tests`) | Dentro de la jaula `bwrap`: sin red, sin tu HOME, sin tu entorno, con límites | Un comando sin jaula tendría tu usuario entero (ADR 0006) |
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

Revisa: la jaula (autoprueba, una línea por comprobación, ver arriba), permisos de las carpetas de Clio (0700) y de sus archivos (0600), la clave del modelo (permisos), sockets 0600, que ningún proceso `clio*` escuche fuera de loopback, y ejecuta `infra/local/check-host.sh` (lo busca junto al repositorio o en `CLIO_HOST_CHECK`) mostrando su salida (las líneas `PASS`/`WARN`/`FAIL` del script entran como comprobaciones más, en inglés). Bajo cada aviso o fallo imprime una línea `->` con qué hacer, y al final un resumen (`doctor: bien (5 ok, 1 aviso; ...)`). Sale con 1 si hay algún FAIL; los WARN son decisión tuya. Con `--json` escribe `{ok, pass, warn, fail, host_script, jail, checks: [{level, message, hint}]}`. La TUI muestra lo mismo con `D`.

## Qué NO hace Clio todavía

- **No ejecuta comandos libres.** Solo `run_tests` (unittest o pytest), y solo con la jaula probada. Sin jaula (otro sistema, sin `bwrap`, núcleo sin espacios de nombres, autoprueba fallida) el agente solo lee, lista, escribe y crea carpetas, y dice que no pudo correr nada: ejecuta tú los tests antes de `apply`. Tampoco corre `cargo test` (ver la jaula).
- **No hay red para el agente**; solo el adaptador del modelo habla con internet (con tu clave).
- **No ejecuta tareas largas con aprobación interactiva del contrato**: como escribiste tú la instrucción en la terminal, se toma como aprobación.
- **No borra archivos** (el agente no tiene herramienta para eso); `apply` sí sabría aplicar un borrado si apareciera en la copia.
- La bóveda no está conectada al agente en `run`; los secretos que excluye la copia simplemente no existen para él.
- Nada corre en segundo plano, no hay memoria entre runs, ni PWA. El agente no ve cambios que hagas en el original después de la copia (por eso `apply` los detecta por hash).

## Riesgos que conviene tener presentes

- El adaptador corre como tu usuario y manda al modelo el texto de los archivos copiados. Lo que no quieras que salga de tu PC, no debe estar en el workspace (los nombres de secreto se excluyen, pero un secreto escrito dentro de un archivo normal sí se copia y se envía al modelo: el filtro del agente aún no está conectado a tu bóveda).
- `apply` escribe con tu usuario: revisa el diff, sobre todo scripts y configuración.
- Estar en el grupo `docker` equivale a ser root; mantén Docker fuera del alcance del agente (`clio doctor` lo avisa).
