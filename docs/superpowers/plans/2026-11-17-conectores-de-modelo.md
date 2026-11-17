# Conectores de modelo — plan de implementación

> **Para agentes:** SUB-SKILL REQUERIDA: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea por tarea. Los pasos usan casillas (`- [ ]`).

**Objetivo:** Clio usa un conector de modelo a la vez, OpenCode o Claude (suscripción), elegido con
`clio connect`, con la llave en el baúl.

**Arquitectura:** una tabla de conectores en `clio-brain` (`src/connector.rs`) es el único lugar con
servidor, API de Pi, variable de la llave, adaptador, secreto/destino y modelo por defecto. Todo lo
que hoy dice `opencode`/`OPENCODE_API_KEY` a mano le pregunta al conector de `model.provider`. La
llave sale del baúl (o de la variable del conector, para pruebas y banco) y llega a Pi por entorno y
a los adaptadores por un sobre en stdin.

**Stack:** Rust (workspace `clio`), Python 3 solo biblioteca estándar (adaptadores), Pi 1.4.0.

**Spec:** `docs/specs/2026-11-17-conectores-de-modelo-design.md`

## Restricciones globales

- Conectores: `opencode` (provider `opencode-go`) y `claude` (provider `claude`). Un `model.provider`
  distinto es error al cargar `config.json`.
- Secretos del baúl: `opencode` → destino `model:opencode.ai`; `claude` → `model:api.anthropic.com`.
  Tool `model` siempre.
- Variables de la llave: `OPENCODE_API_KEY` y `CLAUDE_CODE_OAUTH_TOKEN`. *Decisión del plan:* si la
  variable del conector está puesta y no vacía, manda sobre el baúl (como `CLIO_MEMORY_TEMIS_TOKEN`
  en clio-memory); así siguen las pruebas, el banco y el CI. Nunca se lee una llave de archivo.
- Modelo por defecto: `beam-free` (opencode), `claude-sonnet-5-5` (claude).
- Token de Claude: debe contener `sk-ant-oat`; si no, se rechaza sin guardarlo.
- `model.base_url` y `LLM_BASE_URL`: solo loopback (`http://127.0.0.1|localhost|[::1]`) o, para el
  adaptador, el servidor del propio conector.
- Ningún error, log, traza ni `config.json` lleva una llave.
- Espera del baúl: 2 s (como Jev).
- Mensajes al usuario en español; comentarios de código en inglés, como el resto del repo.

## Foco de revisión

1. **Baúl cerrado o detenido** (al abrir el chat, en `clio run`, de noche): el chat y `clio run`
   dicen `clio vault start`; el sueño duerme sin unir y lo dice. Nunca un pánico ni un error mudo.
   → pruebas en Tarea 2 y Tarea 3.
2. **Token pegado con espacios o salto de línea** en `clio connect`: se recorta; uno sin
   `sk-ant-oat` se rechaza sin tocar el baúl. → prueba en Tarea 5.
3. **`config.json` viejo** con `"fakeprov"` u otro proveedor, o `base_url` remoto: error claro al
   cargar que nombra `clio connect`, no un pánico. → prueba en Tarea 1.
4. **Un 401 de Anthropic que repite el token** en su cuerpo: el adaptador imprime solo estado y
   razón, nunca el cuerpo. → prueba en Tarea 4.
5. **`--model` mal escrito** (no está en el catálogo): `clio connect` lo acepta pero avisa que la
   ventana de contexto es desconocida, y la llamada de prueba dice qué respondió el servidor.
   → prueba en Tarea 5.

---

### Task 1: La tabla de conectores y la configuración que la usa

**Archivos:**
- Crear: `crates/clio-brain/src/connector.rs` (+ `pub mod connector;` en `src/lib.rs`)
- Modificar: `crates/clio-brain/src/pi/launch.rs` (quitar `Provider::opencode_go`, `read_api_key`)
- Modificar: `crates/clio-brain/src/cli/chat/local.rs:248-262` (`provider_of`)
- Modificar: `crates/clio-brain/src/cli/home.rs` (`Config::load` valida; `ModelCfg` doc)
- Modificar: `crates/clio-brain/catalog.example.json`
- Modificar: `testing/bench/src/pi_bench.rs:60-70` (llave solo de `OPENCODE_API_KEY`)
- Modificar pruebas que usan `"fakeprov"` (`tests/pi_chat.rs:55` y las que lo copien) → `"opencode-go"`
- Test: módulo `tests` de `connector.rs`; `tests/cli.rs`

**Interfaces:**
- Produce:
  ```rust
  pub struct Connector {
      pub name: &'static str,          // "opencode" | "claude" (lo que se escribe en `clio connect`)
      pub provider: &'static str,      // "opencode-go" | "claude"
      pub base_url: &'static str,
      pub pi_api: &'static str,        // "openai-completions" | "anthropic-messages"
      pub pi_headers: &'static [(&'static str, &'static str)],
      pub key_env: &'static str,
      pub adapter: &'static str,       // "openai_compat.py" | "anthropic_oauth.py"
      pub secret: &'static str,
      pub domain: &'static str,
      pub default_model: &'static str,
  }
  pub const CONNECTORS: [Connector; 2];
  pub fn by_name(name: &str) -> Option<&'static Connector>;
  pub fn by_provider(provider: &str) -> Option<&'static Connector>;
  pub fn is_loopback(url: &str) -> bool;
  impl Connector { pub fn pi_provider(&self, model: &str) -> crate::pi::launch::Provider; }
  ```
  `pi_provider` arma el `Provider` (id = `provider`, `api_key_env` = `key_env`, ventana 128 000 y
  precio 0 por defecto; quien llama los ajusta como hoy `provider_of`).
- `Config::load` falla con `"model.provider {x:?} no es un conector: usa clio connect opencode|claude"`
  y con `"model.base_url solo puede apuntar a esta máquina (127.0.0.1, localhost, [::1])"`.

- [ ] **Paso 1: pruebas que fallan** en `connector.rs`:
  - `the_two_connectors_and_nothing_else`: `by_name("claude").unwrap().provider == "claude"`,
    `by_provider("opencode-go").unwrap().name == "opencode"`, `by_name("gpt").is_none()`.
  - `claude_goes_to_anthropic_with_the_oauth_variable`: `pi_provider("claude-sonnet-5-5")` da
    `base_url == "https://api.anthropic.com"`, `api == "anthropic-messages"`,
    `api_key_env == "CLAUDE_CODE_OAUTH_TOKEN"`, sin encabezados, y `models_json()` contiene
    `"${CLAUDE_CODE_OAUTH_TOKEN}"` y no contiene `sk-ant`.
  - `opencode_is_what_it_was`: el `models_json()` de `pi_provider("beam-free")` es igual al que daba
    `Provider::opencode_go("beam-free")` (copiar el JSON esperado de la prueba actual en
    `launch.rs:600-606`).
  - `only_loopback_is_loopback`: `is_loopback` sí para `http://127.0.0.1:9/v1`,
    `http://localhost:1`, `http://[::1]:2`; no para `https://opencode.ai/x`,
    `http://127.0.0.1.evil.test`, `http://user@127.0.0.1`.
  - En `tests/cli.rs`, `a_config_with_an_unknown_provider_says_to_connect`: `config.json` con
    `"provider": "fakeprov"` → `clio doctor` (o cualquier orden que cargue) sale con un texto que
    contiene `clio connect`; y otra con `base_url: "https://evil.test"` → contiene `esta máquina`.
- [ ] **Paso 2:** `cargo test -p clio-brain connector` y la de `cli.rs` → FALLAN (no existe).
- [ ] **Paso 3:** implementar `connector.rs`; `provider_of` usa `by_provider(&m.provider)` (la
  validación de `Config::load` garantiza que existe); validar en `Config::load`; borrar
  `Provider::opencode_go` y `read_api_key` (y sus pruebas); `pi_bench` lee
  `std::env::var("OPENCODE_API_KEY")`; el catálogo gana `claude-sonnet-5-5` y `claude-opus-5-5`
  (`context_window` 200 000 salvo que la skill `claude-api` diga otro valor, precio 0,
  `"retention": "retains"`, `tools: true`, `vision: true`, `tier` `"fast"`/`"strong"`) y `"claude"` en
  `data_routes.public` y `.personal`. Las pruebas de integración que usaban `"fakeprov"` pasan a
  `"opencode-go"` con su `base_url` de loopback.
- [ ] **Paso 4:** `cargo test -p clio-brain` → pasan las nuevas y las que ya pasaban (salvo
  `run_tests`, que ya falla en `main`).
- [ ] **Paso 5:** commit `feat(models): one table of model connectors, OpenCode and Claude`.

### Task 2: La llave sale del baúl

**Archivos:**
- Modificar: `crates/clio-brain/src/cli/vault.rs` (nueva `model_key`)
- Modificar: `crates/clio-brain/src/cli/chat/local.rs:121-125` (usa `model_key`)
- Modificar: `crates/clio-brain/src/cli/debug.rs:68-80` (`scrubber` usa `model_key`)
- Modificar: `crates/clio-brain/src/pi/session.rs:222` (`"opencode_api_key"` → `"model_api_key"`)
- Test: módulo `tests` de `vault.rs`; `tests/pi_chat.rs`

**Interfaces:**
- Consume: `Connector` (Tarea 1).
- Produce: `pub fn model_key(dirs: &Dirs, c: &Connector) -> Res<Secret>` — la variable `c.key_env`
  si está y no vacía; si no, `VaultClient::new(gateway).with_timeout(2 s).resolve(c.secret,
  "model", c.domain, "")`. Errores (en español, sin la llave): sin baúl o detenido →
  `"el baúl no está corriendo: clio vault start"`; cerrado →
  `"el baúl está cerrado: clio vault start"`; sin el secreto o destino que no coincide →
  `"falta la llave de <name> en el baúl: clio connect <name>"`.

- [ ] **Paso 1: pruebas que fallan:**
  - `the_variable_wins_over_the_vault` y `no_vault_says_how_to_start_it` (con `HOME` y
    `XDG_RUNTIME_DIR` en un tempdir, sin baúl): el error contiene `clio vault start` y no contiene
    el valor de ninguna variable.
  - En `pi_chat.rs`, `a_chat_without_key_or_vault_says_what_to_do`: sin `OPENCODE_API_KEY` y sin
    baúl, abrir una sesión falla con un texto que contiene `clio vault start`.
- [ ] **Paso 2:** correrlas → FALLAN.
- [ ] **Paso 3:** implementar `model_key` y cambiar los tres llamadores; `debug::scrubber` añade la
  llave con el nombre `"model-api-key"` si `model_key` la da, y nada si no.
- [ ] **Paso 4:** `cargo test -p clio-brain` → pasan.
- [ ] **Paso 5:** commit `feat(models): the model key comes from the vault`.

### Task 3: Los adaptadores reciben la llave en un sobre

**Archivos:**
- Modificar: `crates/clio-brain/src/agent.rs:96-125` (`CommandModel`)
- Modificar: `adapters/models/openai_compat.py`
- Modificar: `crates/clio-brain/src/cli/home.rs:191-275` (`model_command` y `builtin_adapter` por conector; `names_builtin_adapter` reconoce los dos archivos)
- Modificar: `crates/clio-brain/src/cli/runs.rs:523` y `crates/clio-brain/src/cli/sleep.rs:18-37`
- Modificar: `crates/clio-brain/src/sleep_merger.rs:51-64` (`from_command` recibe `Option<Secret>`)
- Test: `tests/adapter_egress.rs`, `tests/sleep.rs`, módulo `tests` de `agent.rs`

**Interfaces:**
- Consume: `model_key` (Tarea 2), `Connector.adapter` (Tarea 1).
- Produce:
  - `CommandModel::with_key(self, key: Secret) -> Self`. Con llave, stdin es
    `{"key": "<llave>", "request": <WireRequest>}`; sin llave, el `WireRequest` solo, como hoy.
  - `ModelMerger::from_command(command, model, key: Option<Secret>, judge, scrub, budget)`.
  - `Config::model_command(&self, dirs) -> Res<Vec<String>>` escribe y devuelve el adaptador del
    conector de `model.provider` (`<data>/adapters/<c.adapter>`); `CLIO_ADAPTER` sigue mandando.
  - Contrato de los dos adaptadores: si stdin trae `key` y `request`, usan esa llave; si trae un
    Request suelto, la llave de su variable (`OPENCODE_API_KEY` / `CLAUDE_CODE_OAUTH_TOKEN`); nunca
    de archivo.
- La llave solo se adjunta cuando `names_builtin_adapter(&cfg.model.command)`.

- [ ] **Paso 1: pruebas que fallan:**
  - `agent.rs`: `the_key_travels_in_the_envelope_only_when_given` con un programa de prueba (`sh -c
    'cat > $OUT; echo {\"content\":\"\",\"input_tokens\":0,\"output_tokens\":0}'`): con `with_key`,
    lo escrito es un objeto con `key` y `request.model`; sin ella, tiene `model` arriba y no `key`.
  - `adapter_egress.rs`: `the_adapter_takes_the_key_from_the_envelope` (sin variable de entorno, con
    sobre, el servidor falso con `--expect-key` la recibe) y
    `the_adapter_never_reads_the_old_key_file` (sin sobre, sin variable, con
    `~/.config/clio/opencode-api-key` presente → sale con error que nombra `OPENCODE_API_KEY`).
  - `adapter_egress.rs`: `llm_base_url_only_to_its_server_or_this_machine`:
    `LLM_BASE_URL=https://evil.test/v1` → error sin llamar.
  - `sleep.rs`: `without_the_vault_the_night_sleeps_without_merging`: sin baúl ni variable, `clio
    memory sleep --now` sale 0 y su salida contiene `sin modelo para unir`.
- [ ] **Paso 2:** correrlas → FALLAN.
- [ ] **Paso 3:** implementar. `runs.rs` y `sleep.rs` piden `model_key` al conector y la pasan
  solo al adaptador propio de Clio; si falla, `clio run --harness builtin` sale con el error de
  `model_key` y el sueño sigue sin unir. `openai_compat.py`: quitar `read_key` de archivo; `check_base`
  acepta `https://opencode.ai/...` o loopback.
- [ ] **Paso 4:** `cargo test -p clio-brain` → pasan.
- [ ] **Paso 5:** commit `feat(models): adapters take the key in an envelope on stdin`.

### Task 4: El adaptador `anthropic_oauth.py`

**Archivos:**
- Crear: `adapters/models/anthropic_oauth.py`
- Modificar: `testing/fake-provider/server.py` (responder también `POST .../v1/messages`, con y sin
  `stream`, formato Anthropic; con `--log-body` escribe el cuerpo de cada petición en el log)
- Modificar: `crates/clio-brain/src/cli/home.rs` (incluir el segundo adaptador con `include_str!`)
- Test: crear `crates/clio-brain/tests/adapter_anthropic.rs`

**Interfaces:**
- Consume: el contrato de adaptador de la Tarea 3.
- Produce: mismo contrato de salida que `openai_compat.py`
  (`{"content", "input_tokens", "output_tokens"}`). Petición a `<base>/v1/messages` con:
  `authorization: Bearer <token>`, `anthropic-version: 2023-06-01`,
  `anthropic-beta: claude-code-20250219,oauth-2025-04-20`, `user-agent: claude-cli/<versión>`,
  `x-app: cli`; `system` es una lista cuyo primer bloque es
  `{"type":"text","text":"You are Claude Code, Anthropic's official CLI for Claude."}` y el
  segundo el texto de los mensajes `system` del Request; los demás mensajes van en `messages` con
  roles `user`/`assistant`; `max_tokens` del Request (4096 si falta). Copiar `<versión>` y cualquier
  otro valor exacto de `~/.local/share/clio/pi/1.4.0/node_modules/@earendil-works/pi-ai/dist/api/anthropic-messages.js`.
  Base: `https://api.anthropic.com`, o loopback por `LLM_BASE_URL`. Errores HTTP: solo
  `HTTP <code> <reason>`, nunca el cuerpo.

- [ ] **Paso 1: pruebas que fallan** en `adapter_anthropic.rs` (estilo de `adapter_egress.rs`,
  contra el servidor falso en loopback):
  - `asks_as_claude_code_and_answers_in_clios_shape`: los encabezados de arriba están en el log,
    `system[0].text` empieza por `You are Claude Code`, el `system` de Clio va en `system[1]`, no hay
    rol `system` en `messages`, y la salida es `{"content":"hola","input_tokens":N,"output_tokens":M}`
    con los números del guion.
  - `an_error_never_prints_the_body`: el servidor responde 401 con cuerpo que contiene el token →
    stderr contiene `HTTP 401` y no el token.
  - `refuses_a_redirect_and_a_remote_base`: 302 → error; `LLM_BASE_URL=https://evil.test` → error
    sin llamar.
- [ ] **Paso 2:** correrlas → FALLAN.
- [ ] **Paso 3:** implementar el adaptador y lo que necesita el servidor falso.
- [ ] **Paso 4:** `cargo test -p clio-brain --test adapter_anthropic` → pasan.
- [ ] **Paso 5:** commit `feat(models): the Anthropic adapter for the Claude connector`.

### Task 5: `clio connect`

**Archivos:**
- Crear: `crates/clio-brain/src/cli/connect.rs` (+ registrarlo en `cli/mod.rs`, `main.rs` y `USAGE`)
- Modificar: `crates/clio-brain/src/cli/vault.rs` (sacar de `run` una `put(dirs, name, dest, value) -> Res<()>`
  y una `unlocked(dirs) -> Res<()>` que distingue detenido de cerrado; `run` las usa)
- Test: `tests/cli.rs` (con baúl real en tempdir, como `tests/vault_link.rs`)

**Interfaces:**
- Consume: `by_name`, `model_key`, `vault::put`, `CommandModel` + `with_key`, `Config`.
- Produce: `pub fn run(dirs: &Dirs, args: Vec<String>) -> Res<i32>` para
  `clio connect opencode|claude [--model M] [--new-key]`.

Flujo (en este orden; cada fallo sale antes de cambiar nada):
1. `vault::unlocked(dirs)?`.
2. Si `model_key` no la encuentra en el baúl, o con `--new-key`:
   - `claude`: imprimir antes de pedir:
     `Aviso: Clio usará tu suscripción presentándose como Claude Code; Anthropic puede suspender la cuenta por esto. El token sale de: claude setup-token`.
     Leer sin eco, recortar; sin `sk-ant-oat` → error, nada guardado.
   - `opencode`: si `OPENCODE_API_KEY` o `~/.config/clio/opencode-api-key` existen, preguntar
     `¿Tomo la llave de <origen>? [s/N]`; si es el archivo y la toma, preguntar
     `¿Borro <archivo>? [s/N]` y borrarlo solo con `s`. Si no, pedirla sin eco.
   - `vault::put(dirs, c.secret, "model:<c.domain>", &valor)`.
3. Reescribir `model.provider` y `model.model` (`--model` o `c.default_model`) en `config.json`
   conservando el resto. Si `model.command` no es el adaptador propio, dejarlo y avisar
   `model.command es un programa tuyo: recibirá el modelo de <name> pero no su llave`.
4. Si el modelo no está en el catálogo, avisar `ventana de contexto desconocida para <model>`.
5. Llamada de prueba: `CommandModel` del adaptador del conector, con la llave, mensaje
   `Responde solo: ok`, `max_tokens` 16 → `conectado: <name> / <model> respondió` o
   `guardado, pero <name> no respondió: <error>` (sale 0 igual: la llave queda).

- [ ] **Paso 1: pruebas que fallan** en `tests/cli.rs`:
  - `connect_without_the_vault_changes_nothing`: sin baúl → sale ≠0, `config.json` igual byte a byte.
  - `connect_claude_refuses_a_token_that_is_not_oauth`: baúl abierto, stdin `sk-ant-api03-x\n` →
    error, `clio vault list` sin `claude`, `config.json` igual.
  - `connect_claude_trims_stores_and_switches`: stdin `  sk-ant-oat01-decoy  \n`, `LLM_BASE_URL`
    al servidor falso → `vault list` tiene `claude` con destino `model:api.anthropic.com`,
    `config.json` tiene `provider: "claude"`, `model: "claude-sonnet-5-5"` y los demás campos
    intactos, y la salida contiene el aviso del riesgo. El valor no aparece en ninguna salida.
  - `connect_opencode_takes_the_old_file_and_deletes_it_only_on_yes`: con el archivo y respuestas
    `s\nN\n` → secreto guardado, archivo sigue; con `s\ns\n` → archivo borrado.
  - `connect_with_an_unknown_model_warns`: `--model claude-nada` → salida contiene
    `ventana de contexto desconocida`.
- [ ] **Paso 2:** correrlas → FALLAN.
- [ ] **Paso 3:** implementar.
- [ ] **Paso 4:** `cargo test -p clio-brain --test cli connect` → pasan.
- [ ] **Paso 5:** commit `feat(cli): clio connect opencode|claude`.

### Task 6: Doctor, init, interfaz y documentación

**Archivos:**
- Modificar: `crates/clio-brain/src/cli/doctor.rs:724-750` (y el chequeo S-18 de archivos)
- Modificar: `crates/clio-brain/src/cli/home.rs:220-235, 323-345` (borrar `KeyStatus`, `key_status`, `Dirs::key_file`)
- Modificar: `crates/clio-brain/src/cli/tui/mod.rs:365-369` y `tui/tests.rs:929, 1124`
- Modificar: `README.md`, `docs/uso-local.md`
- Test: `tests/doctor_audit.rs` o `tests/cli.rs`; `tui/tests.rs`

**Interfaces:**
- Consume: `by_provider`, `model_key`, `VaultClient::check`.
- `KeyState` de la TUI se calcula con `model_key(dirs, c).is_ok()`.

- [ ] **Paso 1: pruebas que fallan:**
  - doctor con secreto en el baúl → línea PASS
    `conector claude: la llave está en el baúl y solo puede ir a api.anthropic.com`; sin él → WARN
    con `clio connect claude`; con `~/.config/clio/opencode-api-key` presente → WARN
    `queda una llave en <archivo>: clio connect opencode la pasa al baúl`.
  - `clio init` sin llave → su salida contiene `clio connect opencode` y no `opencode-api-key`.
  - TUI: el aviso de llave faltante dice `clio connect` (actualizar las dos aserciones de
    `tui/tests.rs`).
- [ ] **Paso 2:** correrlas → FALLAN.
- [ ] **Paso 3:** implementar; README y `uso-local.md`: cómo conectar cada uno, `claude setup-token`,
  el baúl obligatorio (`clio vault start` una vez por arranque) y `loginctl enable-linger $USER` en
  servidor, y el riesgo de Claude en una línea.
- [ ] **Paso 4:** `cargo test -p clio-brain` y `cargo clippy --all-targets` → limpios.
- [ ] **Paso 5:** commit `feat(cli): doctor, init and the TUI speak of connectors`.

### Task 7: Una sesión sigue al cambiar de conector

**Archivos:**
- Modificar: `testing/fake-provider/server.py` (si la Tarea 4 no cubrió `stream` de Anthropic con
  bloques `thinking` y `tool_use`, agregarlo aquí)
- Test: `crates/clio-brain/tests/pi_chat.rs`

**Interfaces:**
- Consume: todo lo anterior; `FakeProvider` (con `--log-body`).

- [ ] **Paso 1: pruebas que fallan**, con Pi real y dos servidores falsos:
  - `a_session_moves_from_opencode_to_claude`: turno 1 con `opencode-go` (el guion pide una
    herramienta, luego responde con razonamiento); cambiar `config.json` a `claude` con `base_url`
    del falso Anthropic y `CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-decoy`; reabrir la misma sesión y
    mandar el turno 2. La petición que recibe el falso Anthropic: no tiene bloques `thinking`; cada
    `tool_use.id` cumple `^[a-zA-Z0-9_-]{1,64}$`; cada `tool_result` va en el mensaje `user`
    siguiente al `tool_use` con su mismo id; y la sesión responde.
  - `a_session_moves_from_claude_to_opencode`: lo mismo al revés; la petición al falso OpenAI no
    tiene bloques de razonamiento firmados y cada `tool_call_id` coincide con una `tool_call`.
- [ ] **Paso 2:** correrlas. Si pasan sin cambios en Clio, es lo esperado (Pi ya transforma):
  quedan como garantía. Si fallan, corregir del lado de Clio (no de Pi) o anotar el fallo y parar.
- [ ] **Paso 3:** commit `test(pi): a session survives a change of connector, both ways`.

### Task 8: Prueba real contra Anthropic

**Archivos:**
- Test: `crates/clio-brain/tests/pi_real.rs` (prueba `#[ignore]`)

- [ ] **Paso 1:** `claude_subscription_answers` (`#[ignore = "llama a Anthropic con tu suscripción; necesita el baúl abierto y clio connect claude"]`):
  con el `config.json` y el baúl reales, el adaptador responde a `Responde solo: ok` y una sesión
  de Pi responde a un turno.
- [ ] **Paso 2:** correrla a mano una vez (`cargo test -p clio-brain --test pi_real
  claude_subscription_answers -- --ignored --nocapture`) con la persona presente.
- [ ] **Paso 3:** commit `test(pi): a manual check against the Claude subscription`.
