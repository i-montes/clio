# adapters/pi

Pi is the agent harness of Clio (ADR 0007). This folder pins it and, in `extensions/`, holds the extension that guards it.

- `package.json` pins `@earendil-works/pi-coding-agent` to an exact version (1.4.0, MIT; it brings its own agent core and model API). `package-lock.json` is committed.
- `.npmrc` sets `ignore-scripts=true`: nothing runs at install time (S-42). Two packages in the tree declare install scripts (`@google/genai`, `protobufjs`); they never run here.
- `overrides` forces patched versions of two transitive dependencies that `npm audit` flags in 1.4.0: `undici` (high, a memory issue in an EventSource parser Clio does not use) and `minimatch` (moderate, glob matching). Remove an override when Pi ships the fix. Checked: Pi 1.4.0 runs normally with them.
- The CI installs only from the lockfile (`npm ci`), fails if it would change, and runs `npm audit`; the nightly audits again. `CODEOWNERS` makes the owner review every change here.

**How Clio launches Pi** (the Rust side does it; here for reference): `pi --mode rpc --no-builtin-tools --no-extensions -e <clio-guard> --no-mcp --no-skills --no-prompt-templates --no-themes --no-context-files --no-approve --offline`, with `PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR`, `PI_OFFLINE=1`, `PI_TELEMETRY=0` and `PI_SKIP_VERSION_CHECK=1`, a clean environment, and the working folder set to the session's copy of the project.

## La guardia: `extensions/clio-guard`

Es la única extensión que Pi carga (`-e adapters/pi/extensions/clio-guard`). **Clio no construye las herramientas que Pi ya trae**: `read`, `write`, `edit`, `bash`, `find` y `ls` son las de Pi (truncado, diff del `edit`, resolución de rutas, renderizado), construidas con `createReadToolDefinition(cwd, { operations })` y compañía y registradas con los mismos nombres. Solo cambia dónde corre la operación primitiva: leer un archivo, escribirlo, comprobar acceso, crear una carpeta, listar, buscar con un patrón, ejecutar un comando. Cada una es una petición al servicio `clio-toolhost` (`crates/clio-toolhost`) por un socket Unix.

**Lista exacta de herramientas registradas** (la que el lanzador debe usar en `guard_tools`): `read`, `write`, `edit`, `bash`, `find`, `ls`.

**`grep` no se ofrece.** El `grep` de Pi siempre arranca `ripgrep` en la máquina (sus `operations` solo cubren leer las líneas de contexto), así que no se puede confinar con `operations`: podría leer lo que la persona puede leer. El modelo usa `bash grep` (dentro de la jaula) o `find`. Tampoco hay `powershell`, `codemode`, `tool_search` ni MCP.

**Qué hace**
- Cada operación primitiva viaja al servicio, que decide (pasarela sobre la operación normalizada), ejecuta en la copia de trabajo (sandbox, rutas canonizadas, enlaces hacia fuera rechazados, `extensions`, `skills`, `.pi`, `.claude`, `.git` y `.agents` sin escritura), limpia y marca la sesión como contaminada si el contenido vino de fuera. `bash` corre **siempre** en la jaula `bwrap` (el único shell libre, S-12); si la jaula no pasa su autoprueba, `bash` falla cerrado (la pasarela lo niega por S-02).
- `tool_call`: defensa en profundidad con lista exacta; bloquea cualquier herramienta que no sea una de las seis y los `write`/`edit` hacia fuera de la carpeta o hacia una carpeta protegida; lo anota en la sesión (`clio-guard.block`) y en stderr.
- `tool_result`: todo resultado, sea cual sea el formato que Pi le dio (avisos de truncado, diffs, `details`), pasa por la operación `filter` del servicio: saneado de secuencias de escape, limpiador del vault y envoltorio de datos externos. Si el servicio no puede, el resultado se retiene.
- `project_trust`: siempre «no». `user_bash` (el `bash` del cliente RPC): va por la misma jaula.
- `before_agent_start`: prompt de sistema versionado (`CLIO_GUARD_PROMPT_FILE`, `CLIO_GUARD_PROMPT_VERSION`, p. ej. `clio-agent@4`) más las notas de herramientas, y `memoryLayer()` como gancho para la memoria (sin implementar). Si el archivo falta o no coincide, la extensión no carga y **Pi se niega a arrancar** con un error claro (probado).
- `context`: lo que no venga ya con un envoltorio real (cabecera, pie y hash coherentes) se envuelve como dato; los resúmenes de compactación también.
- Contaminación (S-05): `pi.appendEntry("clio-guard.taint")`, restaurada en `session_start` (sobre todas las ramas), enviada al servicio en cada petición. No hay forma de borrarla.
- `session_start` informa (`clio-guard.tools`) qué herramientas hay activas y de dónde viene cada una, para la comprobación de arranque.
- Sin red, sin `eval`; lee solo el archivo del prompt y habla solo con el socket de `CLIO_TOOLHOST_SOCKET`.

**`--no-builtin-tools` se mantiene.** Comprobado: con la bandera y la extensión, las herramientas con nombre de incorporada que la extensión registra siguen activas y el modelo recibe exactamente las seis; sin la extensión el modelo recibe cero herramientas (`pi_failclosed.rs`); y una extensión que no carga detiene a Pi.

**Qué NO hace**
- No es un límite de seguridad: corre dentro del proceso de Pi, como la persona. Lo que protege la máquina es el servicio, la pasarela, el sandbox y la jaula (ver `docs/threat-model.md`).
- No pregunta nada a la persona (no hay diálogos en esta fase): un «preguntar» de la pasarela es un no.
- No implementa la memoria ni autentica al servicio más allá de los permisos del socket.
- `edit` lee el archivo tal cual (sin limpiar) para poder escribirlo de vuelta sin dañarlo; su resultado sí se limpia al final.

**Comprobación de arranque.** El RPC de Pi no tiene un comando que liste las herramientas. El lanzador lee la entrada `clio-guard.tools` (`get_entries`) y exige que las activas estén en su lista permitida (las seis) y que su origen sea la extensión de Clio; sin extensión no hay entrada y la lista permitida exige cero herramientas. Es lo que la extensión dice de sí misma; lo independiente es la lista `tools` que recibe el modelo en su primera petición (las pruebas la comprueban). Implementación de referencia: `crates/clio-toolhost/tests/common/mod.rs` (`Pi::verify_tools`).

**Cómo se prueba**
- `npm run check` (tsc sin emitir, TypeScript 5.9.3 fijado exacto) y `npm ci --ignore-scripts && npm audit --audit-level=high`.
- `cargo test -p clio-toolhost`: pruebas del servicio solo, y las del Pi REAL con un modelo falso (`tests/pi_stack.rs`: ataques 38 a 49; `tests/pi_failclosed.rs`; `tests/pi_latency.rs`). Necesitan `node` y `npm ci` hechos; si no están, cada prueba dice `SKIPPED`, y con `CLIO_REQUIRE_PI=1` fallan. `bash` necesita la jaula (`CLIO_REQUIRE_JAIL=1`).

**Updating Pi (task 102):** change the exact version, run `npm install --package-lock-only --ignore-scripts`, read the diff of the lockfile and the release notes, rerun `npm audit`, run the bench and the attack battery, and only then merge. Never `^` or `latest`.
