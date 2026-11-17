# Conectores de modelo: OpenCode y Claude (suscripción), con la llave en el baúl

Fecha: 2026-11-17. Repo: `clio`. Pedido por Iván.

## Corrección 2026-11-17

Lo que este diseño decía del conector `claude` falló en la práctica y se corrigió; el resto vale.

- **Qué falló.** Pi hablaba con `api.anthropic.com` presentándose como Claude Code con el token de
  suscripción, y Anthropic lo rechaza: `400 invalid_request_error: "Third-party apps now draw from
  extra usage, not plan limits."`. Ese camino (el atajo de Pi, el adaptador `anthropic_oauth.py` y los
  encabezados y bloque de sistema de Claude Code) se borró.
- **El puente.** La suscripción pasa ahora por el Claude Code oficial. El paquete de Pi
  `pi-claude-agent-sdk@0.9.2` (fijado en `adapters/pi`, lo instala `clio pi install`, unos 300 MB una
  sola vez) registra el proveedor `claude-bridge` y hace cada turno con Claude Code (Agent SDK). El
  token de `claude setup-token` sigue en el baúl (secreto `claude`) y llega al puente como
  `ANTHROPIC_OAUTH_TOKEN`, que el puente entrega a Claude Code como `CLAUDE_CODE_OAUTH_TOKEN`. Las
  llamadas sueltas (sueño, arnés `builtin`, la sonda de `clio connect`) usan el binario de Claude
  Code que trae el SDK, con `claude -p`: `adapters/models/claude_code.py` reemplaza a
  `anthropic_oauth.py`. Nada se hace pasar por Claude Code ante la API.
- **Tres conectores.** `opencode` (`opencode-go`), `claude` (`claude`, por el puente, suscripción) y
  `anthropic-api` (`anthropic`, llave de la API `sk-ant-api…`, cobro por token, adaptador
  `anthropic_api.py`, secreto `anthropic-api` hacia `model:api.anthropic.com`). `clio connect` acepta
  los tres.
- **Riesgo.** Ya no hay riesgo de suspensión por presentarse como otro programa: se usa Claude Code
  oficial y la cuota del plan, como al usarlo directamente. El paquete del puente avisa que Anthropic
  podría cambiar cómo cobra el Agent SDK.
- Plan: `docs/superpowers/plans/2026-11-17-conector-claude-bridge.md`. Donde el texto de abajo
  contradice esto (el riesgo aceptado, el enfoque 1, la columna `claude` de la tabla, el adaptador
  `anthropic_oauth.py`, el aviso de `clio connect claude`), manda esta sección.

## Por qué

Hoy Clio tiene un solo proveedor de modelo, OpenCode (`opencode-go/beam-free`), escrito a mano en
unos diez lugares, y su llave vive en un archivo (`~/.config/clio/opencode-api-key`) o en
`OPENCODE_API_KEY`. Iván quiere poder usar su cuenta de Claude con suscripción, y que Clio la use
**como un conector más**: él elige OpenCode **o** Claude, y el elegido hace todo.

Sus condiciones:
- **Conectores intercambiables.** Uno a la vez. No conviven: no hay uno para el chat y otro para el
  sueño.
- **Claude es opcional.** Quien nunca lo conecta no ve ningún cambio de comportamiento, salvo que su
  llave de OpenCode pasa al baúl.
- **Las llaves viven en el baúl, nunca en archivos.** Vale para los dos conectores.
- ~~El atajo de Pi para Claude~~ (corregido: ver «Corrección 2026-11-17»; ahora va por Claude Code).

## Objetivos

1. Un conector es un paquete: servidor, forma de API para Pi, adaptador para las llamadas sueltas,
   secreto del baúl con su destino, modelo por defecto y su entrada del catálogo.
2. Dos conectores: `opencode` (lo de hoy) y `claude` (nuevo).
3. Elegir conector con `clio connect opencode|claude [--model M]`, que también guarda la llave en el
   baúl.
4. Con el conector elegido funcionan las cuatro cosas que hoy usan el modelo: el chat (Pi), `clio run`
   (con cualquiera de los dos arneses), y el paso de unir del sueño (`ModelMerger`).
5. La llave sale del baúl en cada sesión o llamada, y el limpiador de secretos la conoce.

## No objetivos

- Usar dos conectores a la vez, o pasar al otro cuando uno falla (el respaldo entre proveedores de
  `clio-models-spec`). Si el elegido falla, falla.
- Elegir conector desde la interfaz (TUI). Se elige por línea de órdenes.
- Guardar con qué conector se hizo cada sesión.
- Renovar tokens. El token de Claude es el de larga duración de `claude-setup-token` (ver abajo).
- Más conectores que estos dos. El diseño los admite; no se agregan.
- Medir los límites de uso de la suscripción.

## Anti-objetivos (aunque funcione, sería un fracaso)

- Que una llave quede en un archivo, un log, una traza, un error o en `config.json`.
- Que la llave de un conector pueda viajar al servidor del otro (cada secreto tiene un solo destino).
- Que quien sigue con OpenCode pierda algo además de tener que abrir el baúl.
- Que el sueño, sin baúl, falle con ruido: debe dormir sin el paso de unir, como hoy sin modelo.

## Restricciones

- **El baúl tiene que estar abierto** para tener modelo. Hoy el chat funciona sin baúl; con este
  cambio no. El baúl no se cierra solo: se abre una vez por arranque (`clio vault start`). En un
  servidor hace falta `loginctl enable-linger` para que siga vivo sin sesión SSH. Esto se documenta.
- **Pi corre en su jaula** (ADR 0009) y su salida está limitada al servidor del proveedor. La jaula
  ya deriva lo permitido de `base_url`, así que `api.anthropic.com` sale de la configuración del
  conector.
- **Pi es el arnés** (ADR 0007). No se cambia.
- **Riesgo (corregido 2026-11-17):** `claude` usa el Claude Code oficial y la cuota del plan, como
  Claude Code; el paquete del puente avisa que Anthropic podría cambiar cómo cobra el Agent SDK.
  `clio connect claude` lo dice en una línea antes de pedir el token.

## Enfoques

1. **Pi con el token de suscripción (elegido; reemplazado por el puente, ver «Corrección»).** El Pi instalado (1.4.0, `@earendil-works/pi-ai`,
   `dist/api/anthropic-messages.js`) ya trata como OAuth cualquier llave que contenga `sk-ant-oat`
   con `api: "anthropic-messages"`, y pone solo la identidad de Claude Code. Para el chat es casi
   todo configuración. Cambio chico y fiel a ADR 0007. Anthropic lo rechaza (ver «Corrección»).
2. **Clio maneja el `claude` oficial (`claude -p`).** Es el uso permitido de la suscripción, pero
   para el chat Claude Code reemplaza a Pi como arnés: choca con ADR 0007 y 0009, y las herramientas
   de Clio tendrían que ir por MCP. Descartado por tamaño.
3. **Claude para Pi y OpenCode para el resto.** Descartado por Iván: los conectores no conviven.

## Diseño

### El conector

Un tipo en Rust, una fila por conector, y el único lugar donde aparece cada dato:

| | `opencode` | `claude` |
|---|---|---|
| id de proveedor (Pi, catálogo, trazas) | `opencode-go` | `claude` (en Pi: `claude-bridge`) |
| servidor | `https://opencode.ai/inference/go/openai/v1` | `https://api.anthropic.com` |
| `api` de Pi | `openai-completions` | `anthropic-messages` (corregido: el puente) |
| encabezados extra en Pi | `User-Agent: clio/0.1`, `x-opencode-session: ${CLIO_SESSION_ID}` | ninguno |
| variable con la llave dentro de la jaula | `OPENCODE_API_KEY` | `ANTHROPIC_OAUTH_TOKEN` |
| adaptador de llamadas sueltas | `openai_compat.py` | `claude_code.py` (corregido; era `anthropic_oauth.py`) |
| secreto del baúl → destino | `opencode` → `model:opencode.ai` | `claude` → `model:api.anthropic.com` |
| modelo por defecto | `beam-free` | `claude-sonnet-5-5` |

`config.json` sigue igual de forma: `model.provider` dice el conector (`opencode-go`, `claude` o `anthropic`) y
`model.model` el modelo. Un `provider` que no es ninguno de los dos es un error claro al cargar.
`model.base_url` (para pruebas con un modelo falso) se mantiene.

`Provider::opencode_go` y `provider_of` (en `cli/chat/local.rs`) pasan a construir el `Provider` desde
la fila del conector, no desde OpenCode con el id cambiado.

### La llave

- **De dónde sale:** `VaultClient::resolve(secreto, "model", dominio, trace)` por el socket del
  gateway, con espera corta (2 s, como Jev). Una sola función, `connector_key(dirs, conector)`.
- **A Pi:** como hoy, por la variable de entorno del conector dentro de la jaula. `models.json` solo
  la nombra (`${VAR}`).
- **A los adaptadores** (arnés `builtin` y sueño): por stdin, como el adaptador de Jev
  (`{"key": …, "request": …}`), nunca por archivo. *Decisión de diseño:* el sobre con la llave solo
  lo reciben los adaptadores de Clio (`names_builtin_adapter`); un `model.command` propio de la
  persona sigue recibiendo el Request solo, como hoy, y se encarga de su llave. Los adaptadores
  dejan de leer `~/.config/clio/opencode-api-key`; si stdin trae un Request sin sobre, aceptan la
  llave de la variable (`OPENCODE_API_KEY`, o la del conector), para que `clio-bench` y el CI nocturno
  sigan igual.
- **Destino fijo y `base_url`:** *decisión de diseño:* la llave se pide siempre con el dominio fijo
  del conector, y `model.base_url` (y `LLM_BASE_URL`) solo se acepta si apunta a loopback; cualquier
  otro valor es un error al cargar. Así una dirección cambiada nunca recibe la llave real. Las
  pruebas con modelo falso usan un baúl de prueba en una carpeta temporal con un secreto señuelo, o
  la variable de entorno por el camino del Request sin sobre.
- **Al limpiador de secretos:** los cuatro sitios que hoy añaden la llave de OpenCode
  (`debug::scrubber`, `runs.rs`, `CLIO_TOOLHOST_SCRUB_JSON` en `pi/session.rs`, y lo que siga) añaden
  la del conector activo, con su nombre.
- **Sin baúl, cerrado o sin el secreto:** el chat y `clio run` se niegan con un mensaje que dice qué
  hacer (`clio vault start`, o `clio connect <conector>`). El sueño duerme sin el paso de unir y lo
  dice en su línea, como hoy cuando falta el modelo.

### El adaptador `anthropic_oauth.py` (borrado: ahora `claude_code.py`, ver «Corrección»)

Mismo contrato que `openai_compat.py`: lee un Request (`model`, `messages`, `max_tokens`) y escribe
`{"content", "input_tokens", "output_tokens"}`. Solo biblioteca estándar. Llama a
`POST https://api.anthropic.com/v1/messages` con lo mismo que Pi manda con un token OAuth (`Authorization:
Bearer`, `anthropic-version`, `anthropic-beta`, `user-agent: claude-cli/<versión>`, `x-app: cli`, y
el bloque de sistema de Claude Code primero, seguido del sistema de Clio). Convierte los `messages`
estilo OpenAI (con `system` dentro) a la forma de Anthropic. Como `openai_compat.py`: solo `https`
(o loopback para pruebas), redirecciones son error, y ningún error lleva la llave. Quien construye
copia los valores exactos de `anthropic-messages.js` de Pi 1.4.0.

### `clio connect`

```
clio connect opencode [--model M]
clio connect claude   [--model M]
clio connect anthropic-api [--model M]   # añadido en la corrección
```

1. Comprueba que el baúl está corriendo y abierto; si no, lo dice y sale.
2. Si el secreto del conector no está en el baúl, lo pide sin eco y lo guarda con su destino
   (`model:<dominio>`), por el mismo camino que `clio vault put`. Si ya está, no lo pide (una opción
   `--new-key` lo reemplaza).
   - `claude`: antes de pedirlo, una línea: «Clio usa tu suscripción a través de Claude Code (oficial). El token sale de: claude setup-token».
     El token tiene que empezar por `sk-ant-oat`; si no, se rechaza sin guardarlo.
   - `opencode`: si existen `OPENCODE_API_KEY` o `~/.config/clio/opencode-api-key`, ofrece tomar la
     llave de ahí. Si la toma desde el archivo, ofrece borrarlo (lo borra solo con un sí).
3. Escribe `model.provider` y `model.model` (el `--model` o el por defecto) en `config.json`, sin
   tocar lo demás. *Decisión de diseño:* si `model.command` es un programa propio, lo deja y avisa
   que ese programa recibirá el modelo del conector pero no su llave.
4. Hace una llamada mínima con el conector y dice si respondió.

### Sesiones al cambiar de conector

Iván cambia de conector cuando quiere, y una sesión vieja se sigue con el conector de ahora. Pi ya
lo soporta (`pi-ai/dist/api/transform-messages.js`): si un mensaje es de otro proveedor o modelo,
descarta su razonamiento interno y normaliza los ids de las llamadas a herramientas. Clio no guarda
el conector de cada sesión ni lo revisa al abrirla. Lo que se agrega son pruebas que lo garantizan
en los dos sentidos (abajo).

### Catálogo

`catalog.example.json` gana las filas de Claude (`claude-sonnet-5-5` y `claude-opus-5-5`), con su
ventana de contexto, precio 0 (suscripción), `tools: true`, y `claude` en `data_routes.public` y
`data_routes.personal`. Con precio 0, el tope de costo por tarea no limita; los de pasos y tiempo sí.

### Lo que también cambia

- `clio init`: deja de buscar la llave en archivo; dice que se conecta con `clio connect opencode`.
- `clio doctor`: el chequeo de «clave del modelo» pasa a «conector `<x>`: secreto en el baúl, solo
  hacia `<dominio>`», como el de Jev. Avisa si queda `~/.config/clio/opencode-api-key`.
- La TUI: el aviso de llave faltante (`tui/mod.rs`) usa el mismo mensaje.
- `README.md` y `docs/uso-local.md`: cómo conectar, el baúl obligatorio y `enable-linger` en servidor.

### Migración

Quien hoy tiene la llave en archivo y actualiza Clio queda sin modelo hasta correr
`clio connect opencode` (que toma la llave del archivo). El mensaje de error del chat lo dice.

## Pruebas

- Por conector: el `models.json` de Pi que sale (sin la llave, con `${VAR}`), el `base_url` y lo que
  permite la jaula.
- `anthropic_oauth.py` (corregido: ahora `claude_code.py` contra un Claude Code falso, y `anthropic_api.py` contra un servidor falso en loopback): conversión de mensajes, error sin la llave, redirección rechazada, solo `https` o loopback.
- `openai_compat.py`: la llave por stdin, y ya no lee entorno ni archivo.
- `clio connect`: baúl cerrado; token de Claude sin `sk-ant-oat` rechazado; configuración reescrita
  sin perder campos; el archivo de OpenCode solo se borra con un sí.
- La llave del conector activo está en el limpiador (un valor que aparece en una salida queda
  tapado).
- Sueño sin baúl: duerme sin unir y lo dice.
- Cambio de conector a mitad de una sesión, en los dos sentidos: una sesión con herramientas y
  razonamiento hecha contra un falso `openai-completions` sigue contra un falso
  `anthropic-messages`, y al revés; la petición que recibe el segundo es válida para su API (sin el
  razonamiento del otro, ids de herramientas aceptables, resultados emparejados con sus llamadas).
- Una prueba ignorada contra Anthropic real, para correrla a mano.

## Lo que queda a quien construye

- El nombre de la variable de la llave de Claude dentro de la jaula (la que Pi lee como `apiKey`).
- La ventana de contexto exacta de cada modelo de Claude.
- Dónde vive el tipo del conector (cerca de `pi/launch.rs` o en `cli/home.rs`).
- Los textos exactos de los mensajes.
