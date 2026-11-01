# clio-toolhost

El servicio por sesión que ejecuta las herramientas del agente de Pi (ADR 0007). Es lo único que
toca la máquina: la extensión `clio-guard` de Pi solo le manda peticiones.

```
clio-toolhost --socket-dir D --work W --scratch S --run-dir R [--rules FILE] [--jail auto|off] [--vault-socket P]
```

- `D` debe existir con modo 0700 y ser del usuario; el socket es `D/toolhost.sock` (0600). Si no, no arranca.
- `W` es la copia de trabajo de la sesión; `S` (instantáneas) y `R` (registro de efectos) van fuera de `W`.
- Imprime `READY <ruta del socket>` y corre hasta que se cierra su stdin (así no sobrevive a quien lo lanzó).
- Necesita `HOME`: la autoprueba de la jaula planta ahí su señuelo y comprueba que no se ve.
- `--jail auto` ofrece `run_tests` solo si `bwrap` arranca y la autoprueba de la jaula pasa; si no, `run_tests` no está declarada y la pasarela la niega (S-02). `--jail off` no la ofrece.
- `--rules` es un JSON de reglas de la pasarela: solo se toman de ahí el umbral del cortacircuito y rutas protegidas extra. La carpeta de trabajo, las herramientas declaradas y las rutas protegidas obligatorias (`extensions`, `skills`, `.pi`, `.claude`, `.git`, `.agents`) son siempre las del servicio.
- `CLIO_TOOLHOST_SCRUB_JSON='{"nombre":"valor"}'` enseña valores al limpiador; `--vault-socket` añade el filtro del vault (si no contesta, la salida se retiene). El enlace real con el vault de la sesión está por hacer.

## Protocolo

JSON por línea, varias peticiones por conexión, tope de 4 MiB por línea y 30 s de lectura.

```
→ {"id","tool","args":{...},"session","call_id","tainted":false}
← {"id","ok","content","decision":{"outcome","rule","reason"},"tainted","external"}
```

Cada petición pasa por: `Gateway::decide_or_deny` (reglas propias, negar por defecto, falla cerrada) → `clio_node::Executor` en la copia (rutas canonizadas, enlaces hacia fuera rechazados, rutas protegidas) o la jaula para `run_tests` → saneado de secuencias de escape → limpiador del vault → corte → `wrap_untrusted` si el texto es externo. Las herramientas son `read_file`, `list_dir`, `write_file`, `make_dir` (reemplazo atómico) y `run_tests` (`runner` y `target` validados como en `clio-node`).

- **Contaminación (S-05)**: por sesión, solo sube. La marca la pone leer un archivo, listar una carpeta o correr pruebas; `tainted` en la petición la sube (no la baja); `tainted` va en toda respuesta.
- **«Preguntar»** de la pasarela se niega (`NobodyHome`); `with_asker` permite poner otro. A las 10 negadas seguidas de una sesión, lo que iba a pasar «pregunta» y por tanto se niega.
- Salidas de más de 2 MiB se retienen enteras (no se cortan antes de limpiar); lo que llega al modelo se corta a 32 000 caracteres.
- Una línea inválida, una sesión con caracteres raros o un campo desconocido no ejecutan nada.

## Pruebas

`cargo test -p clio-toolhost`. `tests/toolhost.rs` prueba el servicio solo; `tests/pi_stack.rs`,
`pi_failclosed.rs` y `pi_latency.rs` lanzan el Pi real (de `adapters/pi/node_modules`) con las
banderas endurecidas contra un modelo falso OpenAI-compatible (`tests/common/fake_model.rs`, solo
127.0.0.1). Sin `node` o sin `npm ci` dicen `SKIPPED`; con `CLIO_REQUIRE_PI=1` fallan. Los procesos
hijos tienen guardias `Drop`.
