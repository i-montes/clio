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
- `--rules` es un JSON de reglas de la pasarela: solo se toman de ahí el umbral del cortacircuito y rutas protegidas extra. La carpeta de trabajo, las herramientas declaradas y las rutas protegidas obligatorias (`extensions`, `skills`, `.pi`, `.claude`, `.git`, `.agents`) son siempre las del servicio.
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
| `filter` | `items: [{text, wrap?: "file"\|"tool-result"}]` | `data.items`: cada texto saneado, limpiado y, si pide `wrap`, envuelto como dato externo. Sin efectos: no pasa por la pasarela |

Las rutas son relativas a la copia de trabajo. Cada operación (menos `filter`) pasa por: `Gateway::decide_or_deny` (reglas propias, negar por defecto, falla cerrada) → regla `S-12/bash` para `exec` (no vacío, sin NUL, hasta 8 KiB) → `clio_node::Executor` en la copia (rutas canonizadas, enlaces hacia fuera rechazados, rutas protegidas) o la jaula para `exec` → saneado y limpiador del vault.

- **Contaminación (S-05)**: por sesión, solo sube. La marca la pone leer un archivo, listar, buscar o ejecutar un comando; `tainted` en la petición la sube (no la baja); `tainted` va en toda respuesta.
- **«Preguntar»** de la pasarela se niega (`NobodyHome`); `with_asker` permite poner otro. A las 10 negadas seguidas de una sesión, lo que iba a pasar «pregunta» y por tanto se niega.
- **`exec` y las carpetas protegidas**: las que no existen se crean vacías durante el comando (la jaula las monta de solo lectura) y se quitan después.
- Archivos de más de 8 MiB no se leen; salidas de más de 2 MiB se retienen enteras (no se cortan antes de limpiar).
- Una línea inválida, una sesión con caracteres raros o un campo desconocido no ejecutan nada.

## Pruebas

`cargo test -p clio-toolhost`. `tests/toolhost.rs` prueba el servicio solo; `tests/pi_stack.rs`,
`pi_failclosed.rs` y `pi_latency.rs` lanzan el Pi real (de `adapters/pi/node_modules`) con las
banderas endurecidas contra un modelo falso OpenAI-compatible (`tests/common/fake_model.rs`, solo
127.0.0.1). Sin `node` o sin `npm ci` dicen `SKIPPED`; con `CLIO_REQUIRE_PI=1` fallan. Los procesos
hijos tienen guardias `Drop`.
