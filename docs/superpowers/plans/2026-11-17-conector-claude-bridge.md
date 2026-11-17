# Conector Claude por Claude Code (claude-bridge) y conector Anthropic API — plan

> **Para agentes:** SUB-SKILL: superpowers:subagent-driven-development. Pasos con casillas (`- [ ]`).

**Objetivo:** que la suscripción de Claude funcione en Clio pasando por el Claude Code oficial, como
en Aria, y sumar un tercer conector con llave de API de Anthropic.

**Por qué cambia:** el camino actual (Pi habla con `api.anthropic.com` presentándose como Claude
Code) lo rechaza Anthropic: `400 invalid_request_error: "Third-party apps now draw from extra usage,
not plan limits."` (sesión `20261117-173026-2152`). Aria usa el paquete de Pi
`pi-claude-agent-sdk@0.9.2`, que registra el proveedor `claude-bridge` y hace cada turno con el
Claude Code oficial (Agent SDK). Clio hace lo mismo.

**Arquitectura:** el puente toma la credencial `anthropic` que Pi resolvió y se la da a Claude Code.
Pi resuelve `ANTHROPIC_OAUTH_TOKEN` del entorno como credencial con `source` que contiene `oauth`
(`pi-ai/dist/providers/anthropic.js`), y el puente entonces pone `CLAUDE_CODE_OAUTH_TOKEN` al
subproceso (`pi-claude-agent-sdk/src/child-env.ts`). Ese es el uso oficial del token de
`claude setup-token`. Por eso: el token sigue en el baúl (secreto `claude`), sin inicio de sesión en
navegador ni `auth.json` (diferencia con Aria, a propósito: menos piezas). Las llamadas sueltas
(sueño, arnés `builtin`, sonda de `clio connect`) usan el binario de Claude Code que trae el SDK
(`claude -p`), nunca la API directa.

**Spec:** `docs/specs/2026-11-17-conectores-de-modelo-design.md` (este plan la corrige donde dice
que Pi habla directo con Anthropic con el token de suscripción).

## Restricciones globales

- Conectores (nombre para `clio connect` → `model.provider` en `config.json` → proveedor de Pi):
  - `opencode` → `opencode-go` → `opencode-go` (sin cambios).
  - `claude` → `claude` → `claude-bridge` (suscripción, por Claude Code).
  - `anthropic-api` → `anthropic` → `anthropic` (llave de API, cobro por token).
- Secretos del baúl (tool `model`): `opencode` → `model:opencode.ai`; `claude` →
  `model:api.anthropic.com`; `anthropic-api` → `model:api.anthropic.com`.
- Variables que mandan sobre el baúl: `OPENCODE_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN`,
  `ANTHROPIC_API_KEY`.
- Variable con la que la llave llega a Pi: `OPENCODE_API_KEY`; `ANTHROPIC_OAUTH_TOKEN` (claude);
  `ANTHROPIC_API_KEY` (anthropic-api).
- Prefijo exigido al guardar: claude `sk-ant-oat`; anthropic-api `sk-ant-api`.
- Modelo por defecto: `beam-free`; `claude-sonnet-5-5`; `claude-sonnet-5-5`.
- Paquete del puente: `pi-claude-agent-sdk` **0.9.2**, fijado en `adapters/pi/package.json` y su
  `package-lock.json` (se instala con `clio pi install`, `npm ci --ignore-scripts`). Entrada:
  `node_modules/pi-claude-agent-sdk/src/index.ts`.
- Binario de Claude Code para llamadas sueltas: el que trae el SDK dentro del Pi instalado
  (`node_modules/@anthropic-ai/claude-agent-sdk-<plataforma>/claude`, en Linux x64
  `claude-agent-sdk-linux-x64`, o `-musl` si es el que existe).
- A Claude Code (dentro y fuera de la jaula) siempre: `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`,
  `DISABLE_AUTOUPDATER=1`, `DISABLE_TELEMETRY=1`.
- Ninguna llave en errores, logs, trazas, salidas, argv ni `config.json`.
- Mensajes al usuario en español; comentarios de código en inglés.
- Se borra todo lo que se hace pasar por Claude Code ante la API (encabezados `claude-cli/…`,
  `x-app: cli`, betas `claude-code-20250219,oauth-2025-04-20`, bloque «You are Claude Code…»):
  `anthropic_oauth.py` deja de existir.

## Foco de revisión

1. Jaula: Claude Code corre como subproceso de Pi dentro de bwrap; necesita ejecutar el binario del
   SDK (montado solo lectura), escribir en `$HOME/.claude` (la `home` de Pi, ya montada rw) y salir
   solo a `api.anthropic.com` por el proxy (respeta `HTTPS_PROXY`). Nada más debe hacer falta con
   las tres variables de arriba.
2. El token de suscripción nunca llega a Pi como `CLAUDE_CODE_OAUTH_TOKEN` ni como argumento; a
   Claude Code suelto, solo por entorno.
3. `config.json` viejo con `provider: "claude"` sigue cargando y ahora va por el puente.
4. Paquete del puente no instalado: el chat y `clio connect claude` lo dicen (`clio pi install`)
   en vez de un error de Pi.
5. Respuesta de Claude Code con `is_error: true` o JSON roto: error claro, sin traceback ni token.

---

### Task 1: Pi usa claude-bridge para la suscripción y anthropic para la llave de API

**Archivos:**
- Modificar: `crates/clio-brain/src/connector.rs` (tercer conector; campos nuevos)
- Modificar: `crates/clio-brain/src/pi/launch.rs` (`Provider`, `models_json`, `hardened_args`, `hardened_env`)
- Modificar: `adapters/pi/package.json`, `adapters/pi/package-lock.json`
- Modificar: `crates/clio-brain/catalog.example.json` (filas de `anthropic`)
- Test: módulos `tests` de `connector.rs` y `launch.rs`

**Interfaces:**
- Produce en `Connector`: `pi_provider: &'static str` (id para `--provider`), `pi_key_env: &'static str`
  (variable de la llave dentro de Pi), `key_prefix: &'static str`, `pi_extension: Option<&'static str>`
  (ruta relativa a la instalación de Pi: `node_modules/pi-claude-agent-sdk/src/index.ts` para claude).
  `Connector::pi_provider(model)` pasa a llamarse `pi_launch_provider(model) -> Provider` (o se
  mantiene el nombre si el campo nuevo choca; decide quien implementa y lo dice).
- `Provider` gana `extension: Option<String>` (relativa a `install.dir`) y `child_env: Vec<(String, String)>`.
- `models_json()`: para un proveedor con `extension` no declara el proveedor (Pi lo registra la
  extensión; `{"providers": {}}`).
- `hardened_args`: con `extension`, añade `-e <install.dir>/<extension>` justo después del de la
  guardia. `--provider` usa `pi_provider`.
- `hardened_env`: la llave va en `pi_key_env`; con `extension`, añade las tres variables de Claude
  Code de las restricciones globales.
- Si el archivo de la extensión no existe en la instalación, `build` falla con
  `"falta el puente de Claude (pi-claude-agent-sdk): clio pi install"`.

- [ ] **Paso 1: pruebas que fallan**
  - `connector.rs`: `three_connectors`: nombres `opencode`, `claude`, `anthropic-api`; `claude` →
    `pi_provider == "claude-bridge"`, `pi_key_env == "ANTHROPIC_OAUTH_TOKEN"`, `key_env ==
    "CLAUDE_CODE_OAUTH_TOKEN"`, `key_prefix == "sk-ant-oat"`, extensión presente; `anthropic-api` →
    provider `anthropic`, `pi_provider == "anthropic"`, `pi_key_env == key_env == "ANTHROPIC_API_KEY"`,
    `key_prefix == "sk-ant-api"`, sin extensión, secreto `anthropic-api`, dominio `api.anthropic.com`.
  - `launch.rs`: `claude_runs_through_the_bridge`: con el conector claude, `hardened_args` contiene
    `-e <dir>/node_modules/pi-claude-agent-sdk/src/index.ts` y `--provider claude-bridge`;
    `hardened_env` tiene `ANTHROPIC_OAUTH_TOKEN` con la llave y las tres variables, y no tiene
    `CLAUDE_CODE_OAUTH_TOKEN`; `models_json()` no tiene `claude-bridge` ni la llave.
  - `launch.rs`: `the_bridge_must_be_installed`: sin el archivo de la extensión, `build` falla con el
    mensaje de arriba.
  - `launch.rs`: `anthropic_api_is_a_plain_provider`: `models_json()` declara `anthropic` con
    `api: anthropic-messages`, `${ANTHROPIC_API_KEY}`, sin encabezados.
- [ ] **Paso 2:** correrlas → FALLAN.
- [ ] **Paso 3:** implementar; fijar el paquete (`npm install --package-lock-only --ignore-scripts
  pi-claude-agent-sdk@0.9.2` en `adapters/pi`, revisar que el lock trae `@anthropic-ai/claude-agent-sdk`
  y sus binarios por plataforma como `optional`); `clio pi install` en la máquina para que el
  checkout tenga el paquete (sin tocar el Pi instalado de la persona fuera de lo que ya hace
  `clio pi install`). Catálogo: filas `anthropic` (`claude-sonnet-5-5`, `claude-opus-5-5`,
  ventana 200000, precio 0 salvo que conozcas el real, `retention` `"no_training"`), y `anthropic`
  en `data_routes.public` y `.personal`.
- [ ] **Paso 4:** `cargo test -p clio-brain` (salvo `run_tests`, que ya falla en main) y
  `cargo test -p clio-bench` pasan; las pruebas del conector claude que esperaban
  `anthropic-messages` directo se pasan al conector `anthropic-api`.
- [ ] **Paso 5:** commit `feat(models): the Claude subscription goes through Claude Code (claude-bridge); an Anthropic API key connector`.

### Task 2: Llamadas sueltas y `clio connect` sin hacerse pasar por nadie

**Archivos:**
- Crear: `adapters/models/claude_code.py`
- Crear: `adapters/models/anthropic_api.py` (desde `anthropic_oauth.py`, que se borra)
- Modificar: `crates/clio-brain/src/connector.rs` (`adapter` de claude y anthropic-api)
- Modificar: `crates/clio-brain/src/cli/home.rs` (`include_str!` de los dos; `model_command` pasa
  `--claude <ruta>` al adaptador de claude; `names_builtin_adapter` reconoce los tres)
- Modificar: `crates/clio-brain/src/cli/connect.rs` (tres conectores, prefijos, aviso nuevo)
- Modificar: `crates/clio-brain/src/cli/doctor.rs` (conector claude: puente instalado)
- Test: `tests/adapter_anthropic.rs` (pasa a `anthropic_api.py`), crear `tests/adapter_claude_code.rs`, `tests/cli.rs`

**Interfaces:**
- `claude_code.py`: mismo contrato que los otros (sobre `{"key","request"}` o Request suelto con
  `CLAUDE_CODE_OAUTH_TOKEN`; salida `{"content","input_tokens","output_tokens"}`). Ejecuta
  `<claude> -p --output-format json --model <model> --tools "" --no-session-persistence
  --setting-sources "" --strict-mcp-config --system-prompt <system>` con el resto de la conversación
  como texto por stdin. Entorno: solo `PATH`, `HOME` = carpeta temporal privada que borra al
  terminar, `CLAUDE_CODE_OAUTH_TOKEN`, y las tres variables; ninguna otra `ANTHROPIC_*`/`CLAUDE_*`.
  Lee `result` y `usage.input_tokens`/`usage.output_tokens`; `is_error: true` → falla con
  `"Claude Code respondió con error"` y el `result` recortado sin la llave. Sin `--claude` ni binario
  ejecutable → `"falta Claude Code (pi-claude-agent-sdk): clio pi install"`. Tiempo máximo 170 s.
- `anthropic_api.py`: igual que `anthropic_oauth.py` pero con `x-api-key: <llave>` y
  `anthropic-version`, sin `authorization`, sin `anthropic-beta`, sin `user-agent: claude-cli`, sin
  `x-app`, sin bloque de identidad; variable `ANTHROPIC_API_KEY`.
- `model_command` para claude: `["python3", <copia de claude_code.py>, "--claude", <binario del SDK en el Pi instalado>]`.
- `clio connect claude`: aviso `Clio usa tu suscripción a través de Claude Code (oficial). El token sale de: claude setup-token`;
  sin el puente instalado avisa `falta el puente de Claude (pi-claude-agent-sdk): clio pi install`
  y no guarda nada. `clio connect anthropic-api`: pide la llave (prefijo `sk-ant-api`).
- Doctor, conector claude: PASS/WARN de que el puente y el binario existen en el Pi instalado.

- [ ] **Paso 1: pruebas que fallan**
  - `adapter_claude_code.rs`, con un `claude` falso (script que guarda argv, entorno y stdin en un
    archivo y responde un JSON guionado): `runs_claude_code_with_the_token_only_in_its_env` (el token
    no está en argv, está en `CLAUDE_CODE_OAUTH_TOKEN`, no hay otras `ANTHROPIC_*`, `HOME` no es el de
    la persona y ya no existe al terminar, argv tiene `-p --output-format json --tools`, el `system`
    va en `--system-prompt`, la salida trae `content` y tokens); `an_error_answer_is_a_clear_error`
    (`is_error: true` → sale ≠0, stderr sin la llave ni traceback); `broken_json_is_a_clear_error`;
    `without_the_binary_says_to_install`.
  - `adapter_anthropic.rs` → `anthropic_api.py`: los encabezados son `x-api-key` y
    `anthropic-version`; no hay `authorization`, `anthropic-beta`, `x-app` ni `claude-cli`; `system`
    es el de Clio sin bloque de identidad.
  - `cli.rs`: `connect_anthropic_api_stores_and_switches` (prefijo, secreto `anthropic-api`,
    `provider: "anthropic"`); `connect_claude_without_the_bridge_saves_nothing`; el aviso de claude ya
    no dice «presentándose» ni «suspender».
- [ ] **Paso 2:** correrlas → FALLAN.
- [ ] **Paso 3:** implementar; borrar `anthropic_oauth.py` y toda referencia.
- [ ] **Paso 4:** suites completas como en Task 1.
- [ ] **Paso 5:** commit `feat(models): one-shot calls go through Claude Code; clio connect anthropic-api`.

### Task 3: Sesiones, prueba real y documentación

**Archivos:**
- Modificar: `crates/clio-brain/tests/pi_chat.rs` (pruebas de cambio de conector: el lado
  Anthropic pasa a `anthropic-api`; quitar aserciones de OAuth/identidad)
- Modificar: `crates/clio-brain/tests/pi_real.rs` (`claude_subscription_answers` usa el puente:
  Pi con `claude-bridge` en el `Rig`, y el adaptador `claude_code.py`)
- Modificar: `README.md`, `docs/uso-local.md`, `docs/specs/2026-11-17-conectores-de-modelo-design.md`
  (sección «Corrección 2026-11-17»: qué falló, el puente, los tres conectores; quitar el riesgo de
  suspensión y poner: usa la cuota del plan como Claude Code; el paquete avisa que Anthropic podría
  cambiar cómo cobra el Agent SDK)

- [ ] **Paso 1:** ajustar pruebas; `cargo test -p clio-brain --test pi_chat` pasa;
  `cargo test -p clio-brain --test pi_real --no-run` compila.
- [ ] **Paso 2:** docs.
- [ ] **Paso 3:** suites completas, fmt, clippy.
- [ ] **Paso 4:** commit `docs: the Claude connector runs through Claude Code; three connectors`.
