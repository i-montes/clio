# clio-toolhost

El servicio por sesión que ejecuta las operaciones primitivas de las herramientas de Pi (ADR 0007:
Pi trae `read`, `write`, `edit`, `bash`, `find`, `ls`; Clio solo cambia dónde corren). Es lo único que
toca la máquina: la extensión `clio-guard` de Pi solo le manda peticiones.

```
clio-toolhost --socket-dir D --work W --scratch S --run-dir R [--rules FILE] [--jail auto|off] [--vault-socket P]
```

- `clio toolhost ...` (hidden subcommand of the `clio` binary) takes the same arguments and is what the interface starts for a Pi session: no second binary to install.
- `D` debe existir con modo 0700 y ser del usuario; el socket es `D/toolhost.sock` (0600). Si no, no arranca.
- `W` es la copia de trabajo de la sesión; `S` (instantáneas) y `R` (registro de efectos) van fuera de `W`.
- Imprime `READY <ruta del socket>` y corre hasta que se cierra su stdin (así no sobrevive a quien lo lanzó).
- Necesita `HOME`: la autoprueba de la jaula planta ahí su señuelo y comprueba que no se ve.
- `--jail auto` ofrece `exec` (el `bash` de Pi) solo si `bwrap` arranca y la autoprueba de la jaula pasa; si no, `exec` no está declarada y la pasarela la niega (S-02): `bash` falla cerrado. `--jail off` no la ofrece.
- `--rules` es un JSON de reglas de la pasarela: solo se toman de ahí el umbral del cortacircuito y rutas protegidas extra. La carpeta de trabajo, las herramientas declaradas y las rutas protegidas obligatorias (`extensions`, `skills`, `.pi`, `.claude`, `.git`, `.agents`, `.vscode`, `.husky`, `.githooks`, `.cargo`, `.devcontainer`) son siempre las del servicio.
- `CLIO_TOOLHOST_SCRUB_JSON='{"nombre":"valor"}'` enseña valores al limpiador; `--vault-socket` añade el filtro del vault (si no contesta, la salida se retiene). El enlace real con el vault de la sesión está por hacer.

## Protocolo

JSON por línea, varias peticiones por conexión, tope de 4 MiB por línea y 30 s de lectura.

```
→ {"id","tool","args":{...},"session","call_id","tainted":false}
← {"id","ok","content","data","decision":{"outcome","rule","reason"},"tainted","external"}
```

`tool` es una operación primitiva de las herramientas de Pi (la extensión `clio-guard` las arma con `operations`):

| Operación | Argumentos | Resultado |
|---|---|---|
| `fs_read` | `path`, `raw?` | `content` (limpio; con `raw` los bytes tal cual, solo para `edit`) |
| `fs_stat` | `path`, `mode?` (`exists`, `r`, `rw`) | `data.exists`, `data.is_dir`; con `r`/`rw` falla si no hay acceso |
| `fs_list` | `path` | `data.entries` |
| `fs_glob` | `path`, `pattern`, `limit?` | `data.paths` (no sigue enlaces; ignora `.git` y `node_modules`) |
| `fs_write` | `path`, `content` | reemplazo atómico, crea las carpetas |
| `fs_mkdir` | `path` | |
| `exec` | `command`, `timeout?` (s, tope 120) | `content` = salida (stdout y stderr), `data.exit_code`, `data.signal`, `data.timed_out`; siempre en la jaula, solo si la autoprueba pasó |
| `memory_search` | `query` (hasta 200 caracteres), `limit?` (1 a 8, por defecto 5) | `content` = una línea por recuerdo activo, `[m{id}] {tipo} · {persona\|agente\|externo} · {fecha}: {texto}`, envuelta como dato (`UNTRUSTED-DATA memory`); `data.ids`, `data.count`, `data.masked`. Solo si el servicio recibió un `MemoryPort` |
| `memory_get` | `id` (entero > 0) | lo mismo para un recuerdo activo. Sin `MemoryPort` las dos se niegan (S-02) |
| `filter` | `items: [{text, wrap?: "file"\|"tool-result"}]` | `data.items`: cada texto saneado, limpiado y, si pide `wrap`, envuelto como dato externo. Sin efectos: no pasa por la pasarela |

Las rutas son relativas a la copia de trabajo. Cada operación (menos `filter`) pasa por: `Gateway::decide_or_deny` (reglas propias, negar por defecto, falla cerrada) → regla `S-12/bash` para `exec` (no vacío, sin NUL, hasta 8 KiB) → `clio_node::Executor` en la copia (rutas canonizadas, enlaces hacia fuera rechazados, rutas protegidas) o la jaula para `exec` → saneado y limpiador del vault.

- **Contaminación (S-05)**: por sesión, solo sube. La marca la pone leer un archivo, listar, buscar o ejecutar un comando; `tainted` en la petición la sube (no la baja); `tainted` va en toda respuesta.
- **Memoria (S-60)**: solo lectura, por el `MemoryPort` de `Config`. Sale limpia (el limpiador antes del corte), en una línea por recuerdo (600 caracteres como máximo) y de 4 KiB como máximo en total, envuelta como dato y `external`; pero marca la sesión solo si algún recuerdo no es de la persona, a diferencia de archivos y comandos, que marcan siempre. El puerto corre en otro hilo con plazo de 10 s y red contra pánicos; un fallo es un error corto de esa llamada.
- **Presupuesto de memoria (S-47, regla `S-47/memory`)**: 20 llamadas y 24 KiB de texto devuelto por servicio (por carpeta `R`), con corte duro. Va después de la pasarela y antes de validar y de llamar al puerto; cuenta toda llamada que la pasarela dejó pasar (también las inválidas o con el puerto en error, no las que la pasarela negó). Cada respuesta se limita a lo que queda (`min(4 KiB, 24 KiB - gastado)`); con menos de 1 KiB o 20 llamadas gastadas, se niega con `S-47/memory`, sin tocar el puerto. Esa negación no cuenta para el cortacircuito de la pasarela (igual que `S-12/bash`). El gasto está en `R/memory-budget.json` (`{"calls","bytes"}`, 0600, escrito con archivo temporal y `rename`): sobrevive a un reinicio del servicio. Si no se puede leer, entender o escribir, la llamada se niega: nunca queda sin tope.
- **Registro de hechos**: `R/memory.jsonl` (0600, solo añadir), una línea por llamada que pasó el presupuesto: `at` (ms), `tool`, `results`, `ids`, `bytes`, `tainted` (esta llamada marcó la sesión) y `query_hash` (8 hex de SHA-256 de la consulta limpia; `null` en `memory_get`). Nunca el texto de la consulta ni el de un recuerdo. Si no se puede escribir, la llamada se niega.
- **«Preguntar»** de la pasarela se niega (`NobodyHome`); `with_asker` permite poner otro. A las 10 negadas seguidas de una sesión, lo que iba a pasar «pregunta» y por tanto se niega.
- **`exec` y las carpetas protegidas**: las que no existen se crean vacías durante el comando (la jaula las monta de solo lectura) y se quitan después.
- Archivos de más de 8 MiB no se leen; salidas de más de 2 MiB se retienen enteras (no se cortan antes de limpiar).
- Una línea inválida, una sesión con caracteres raros o un campo desconocido no ejecutan nada.

## Pruebas

`cargo test -p clio-toolhost`. `tests/toolhost.rs` y `tests/hardening.rs` prueban el servicio solo.
Las pruebas con el Pi real están en `clio-brain` (`cargo test -p clio-brain --test pi_stack`, carpeta
`crates/clio-brain/tests/pi_stack/`): lanzan Pi por `pi::launch::build`, dentro de su jaula, con las
banderas endurecidas y `clio toolhost`, contra un modelo falso OpenAI-compatible
(`common/fake_model.rs`, solo 127.0.0.1). Sin `node` o sin `npm ci` dicen `SKIPPED`; con
`CLIO_REQUIRE_PI=1` fallan. Los procesos hijos tienen guardias `Drop`.
