# adapters/pi

Pi is the agent harness of Clio (ADR 0007). This folder pins it and, in `extensions/`, holds the extension that guards it.

- `package.json` pins `@earendil-works/pi-coding-agent` to an exact version (1.4.0, MIT; it brings its own agent core and model API). `package-lock.json` is committed.
- `.npmrc` sets `ignore-scripts=true`: nothing runs at install time (S-42). Two packages in the tree declare install scripts (`@google/genai`, `protobufjs`); they never run here.
- `overrides` forces patched versions of two transitive dependencies that `npm audit` flags in 1.4.0: `undici` (high, a memory issue in an EventSource parser Clio does not use) and `minimatch` (moderate, glob matching). Remove an override when Pi ships the fix. Checked: Pi 1.4.0 runs normally with them.
- The CI installs only from the lockfile (`npm ci`), fails if it would change, and runs `npm audit`; the nightly audits again. `CODEOWNERS` makes the owner review every change here.

**How Clio launches Pi** (the Rust side does it; here for reference): `pi --mode rpc --no-builtin-tools --no-extensions -e <clio-guard> --no-mcp --no-skills --no-prompt-templates --no-themes --no-context-files --no-approve --offline`, with `PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR`, `PI_OFFLINE=1`, `PI_TELEMETRY=0` and `PI_SKIP_VERSION_CHECK=1`, a clean environment, and the working folder set to the session's copy of the project.

## La guardia: `extensions/clio-guard`

Es la única extensión que Pi carga (`-e adapters/pi/extensions/clio-guard`). Con las herramientas incorporadas apagadas, todo lo que el modelo puede hacer lo registra ella, y cada herramienta es una llamada al servicio `clio-toolhost` (`crates/clio-toolhost`) por un socket Unix.

**Qué hace**
- Registra `read_file`, `list_dir`, `write_file`, `make_dir` y `run_tests` (parámetros `typebox`, descripciones cortas, y `promptGuidelines` que dicen que el resultado son DATOS). El servicio decide (pasarela), ejecuta (sandbox, jaula), limpia y envuelve.
- `tool_call`: bloquea cualquier herramienta que no sea de las cinco y las escrituras que el servicio rechazaría (`..`, rutas absolutas, `.pi`, `extensions`, `skills`, `.git`, `.claude`, `.agents`); lo anota en la sesión (`clio-guard.block`) y en stderr.
- `project_trust`: siempre «no». `user_bash` (el comando `bash` del cliente RPC): sin shell.
- `before_agent_start`: pone como prompt de sistema el prompt versionado (`CLIO_GUARD_PROMPT_FILE`, `CLIO_GUARD_PROMPT_VERSION`, p. ej. `clio-agent@4`) más las notas de herramientas, y deja `memoryLayer()` como gancho para la memoria (sin implementar). Si el archivo falta o no coincide, la extensión no carga y **Pi se niega a arrancar** con un error claro (probado).
- `context`: todo resultado de herramienta que no venga ya con un envoltorio real (cabecera, pie y hash coherentes) se envuelve como dato; los resúmenes de compactación también.
- Contaminación (S-05): se guarda con `pi.appendEntry("clio-guard.taint")`, se restaura al abrir la sesión (`session_start`, sobre todas las ramas), se manda al servicio en cada llamada y `tool_result` la marca si una herramienta devolvió contenido externo. No hay forma de borrarla.
- `session_start` también informa (`clio-guard.tools`) qué herramientas hay activas y de dónde viene cada una, para la comprobación de arranque.
- Sin red, sin `eval`; lee solo el archivo del prompt y habla solo con el socket de `CLIO_TOOLHOST_SOCKET`.

**Qué NO hace**
- No es un límite de seguridad: corre dentro del proceso de Pi, como la persona. Si Pi cae, la guardia cae; lo que protege la máquina es el servicio, la pasarela, el sandbox y la jaula (ver `docs/threat-model.md`).
- No pregunta nada a la persona (no hay diálogos en esta fase): un «preguntar» de la pasarela es un no.
- No implementa la memoria ni autentica al servicio más allá de los permisos del socket.

**Comprobación de arranque.** Pi no tiene un comando RPC que liste las herramientas. El lanzador debe leer la entrada `clio-guard.tools` (`get_entries`) y exigir que las activas estén en su lista permitida y que su origen sea la extensión de Clio; sin extensión no hay entrada, y la prueba `without_the_extension_there_are_no_tools` comprueba además que el modelo recibe cero herramientas. La implementación de referencia está en `crates/clio-toolhost/tests/common/mod.rs` (`Pi::verify_tools`).

**Cómo se prueba**
- `npm run check` (tsc sin emitir, TypeScript 5.9.3 fijado exacto) y `npm ci --ignore-scripts && npm audit --audit-level=high`.
- `cargo test -p clio-toolhost`: pruebas del servicio solo, y las del Pi REAL con un modelo falso (`tests/pi_stack.rs`: los ataques 38 a 47; `tests/pi_failclosed.rs`; `tests/pi_latency.rs`). Necesitan `node` y `npm ci` hechos; si no están, cada prueba dice `SKIPPED`, y con `CLIO_REQUIRE_PI=1` fallan. `run_tests` necesita la jaula (`CLIO_REQUIRE_JAIL=1`).

**Updating Pi (task 102):** change the exact version, run `npm install --package-lock-only --ignore-scripts`, read the diff of the lockfile and the release notes, rerun `npm audit`, run the bench and the attack battery, and only then merge. Never `^` or `latest`.
