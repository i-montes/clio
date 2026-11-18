# Relevo: Clio como asistente autónomo

Escrito el 2026-11-17 por la sesión que trabajó en el PC de Iván, para la sesión que sigue en el
VPS. Léelo entero antes de tocar nada. Bórralo cuando ya no haga falta.

## Cómo trabaja Iván

- Responde en español, corto y directo. Dicta por voz: a veces llegan palabras mal transcritas
  («Clear Memory» es clio-memory, «APR» es un PR, «ESA» es una skill o conexión).
- **El modelo principal no escribe código:** escribe el diseño y los planes, coordina y revisa. El
  código lo escriben subagentes: Opus para lo difícil, Sonnet para lo mecánico. Una revisión de la
  rama completa por un subagente nuevo antes de fusionar, y una sola ronda de correcciones.
- No hagas `push`, `merge` ni etiquetas sin que él lo pida. Pregunta poco.
- Su memoria principal es clio-memory (skill `clio-memory`). Las órdenes que no empiezan por
  `clio-memory m` son de la persona: no las corras.
- Las llaves van al baúl de Clio, nunca a archivos. No toques el baúl ni tokens.

## Qué se quiere

Clio como asistente real, despierto y autónomo, que vive en este VPS y es un producto para más
personas. El diseño aprobado está en `docs/specs/2026-11-17-clio-asistente-autonomo-design.md`.
Seis partes, cada una con su plan:

1. Recuerdos primordiales y presentación. **Hecha.**
2. Manos reales (modo anfitrión). **Es lo que sigue.**
3. Telegram. 4. La ronda. 5. Skills y herramientas propias. 6. Conexiones.

Con 1 a 4 Clio ya se despierta sola y avisa.

## Qué está hecho

- **Conectores de modelo** (`clio connect opencode|claude|anthropic-api`), llaves en el baúl. La
  suscripción de Claude pasa por el Claude Code oficial (paquete de Pi `pi-claude-agent-sdk`,
  proveedor `claude-bridge`). Probado con la suscripción real.
- **Jaula reforzada** para el conector `claude` (no carga `.claude/`, `.pi/` ni `.mcp.json` del
  proyecto; sin jaula no arranca).
- **clio-memory v0.13.0** publicada: ningún agente cambia lo que muestra un bloque fijo, y los
  bloques van primero en `memory_context`, en una sección `PERSON-SET`.
- **Parte 1** en `main` (hasta `882b803`): la identidad vive en los bloques `identidad`, `reglas`,
  `preferencias` y `vigilancia`; herramientas `memory_block` y `memory_block_edit` (esta se niega si
  la sesión leyó contenido de fuera); cuatro textos de fábrica; en la primera sesión nueva Clio
  habla primero y hace cuatro preguntas; `clio memory identity`. Plan:
  `docs/superpowers/plans/2026-11-17-recuerdos-primordiales.md`. Probado en el PC con el modelo real.

## Qué sigue: la parte 2, «manos reales»

No tiene plan escrito. El siguiente paso es escribirlo y ejecutarlo. Lo decidido (sección 2 del
diseño):

- Las herramientas de Clio actúan sobre el servidor real, con red. Hoy trabajan sobre una copia
  (`runs/<id>/sandbox`), sin red, con 40 pasos y 10 minutos por tarea.
- **La consola es libre, también con `sudo`, y todo queda registrado.**
- Copia antes de cambiar, cuando el cambio lo hacen sus herramientas de archivos.
- **Solo cuatro cosas preguntan:** borrar datos o respaldos, gastar dinero, enviar o publicar en
  nombre de la persona, y tocar lo que controla a Clio (baúl, configuración, registro).
- **Pocas preguntas y ninguna detiene el trabajo:** lo que necesita visto bueno se aparta y la tarea
  sigue. Tope de tres preguntas al día; lo demás va al resumen del día.
- Las órdenes corren en una jaula que muestra el sistema real pero tapa las carpetas y los sockets
  de Clio. Límite conocido: con `sudo` eso deja de ser un muro.
- **Texto ajeno** pasa a ser solo: lo que llega por una conexión, la web, y una lista de carpetas
  del servidor donde entra texto de fuera. Hoy cualquier lectura de archivo marca la sesión, y por
  eso la identidad solo se puede editar en una sesión que no leyó nada.
- Presupuesto por día, no por tarea. Los cambios de código salen como PR.
- Las copias de trabajo viejas se borran solas (más de 14 días, sin cambios pendientes).

Dónde mirar para el plan: `crates/clio-toolhost/src/lib.rs` (operaciones, `present()`, la marca de
sesión), `crates/clio-node/src/jail.rs`, `crates/clio-brain/src/cli/runs.rs` (`prepare`),
`crates/clio-brain/src/cli/chat/local.rs`, `crates/clio-gateway`.

## Pendientes pequeños

- El arnés `builtin` recorta los bloques a su tamaño antiguo en tokens (150/200/150).
- Si la persona responde una sola pregunta de la presentación, las otras no se vuelven a hacer.
- `cargo test -p clio-brain --test run_tests` falla desde antes («the run left no bytecode»).
- No se midió cómo cuenta el plan de Claude los tokens leídos de caché (cada sesión carga unos
  14.000 de base, una vez).

## Este VPS

- Ubuntu 24.04, ARM64, 4 núcleos, 23 GB de RAM. Usuario `ubuntu` con `sudo` sin contraseña.
- **Tiene cosas reales de Iván en producción (sus sitios, Temis). No toques otros servicios.**
- El Node del sistema es el 18 y se queda así. Para Clio hay un Node 22 aparte en
  `~/.local/share/node22`: ponlo delante en el `PATH` solo al correr `clio`
  (`PATH=$HOME/.local/share/node22/bin:$PATH clio …`).
- La preparación corrió sola con `~/clio-setup.sh`; su resultado está en `~/clio-setup.log`. Clona
  los repos en `~/Projects/personal/agent/`, instala Rust, clio-memory 0.13.0, compila Clio y corre
  `clio pi install` y `clio doctor`. **Mira el final del log antes de seguir:** si algo falló, es lo
  primero que hay que arreglar.
- La memoria del PC está exportada en `~/clio-memoria-pc.jsonl` (0600), sin importar.

## Lo que le toca a Iván en el VPS

Son órdenes suyas (guardan o usan credenciales, o actúan con su autoridad):

```sh
clio-memory import ~/clio-memoria-pc.jsonl   # antes de usar la memoria aquí; luego borra el archivo
clio-memory setup --yes                      # conecta la memoria a Claude Code
PATH=$HOME/.local/share/node22/bin:$PATH clio init
clio vault start                             # crea el baúl de este servidor
clio connect claude                          # token de: claude setup-token
```

## Comprobaciones del proyecto

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --no-fail-fast
(cd adapters/pi && npm run check)
python3 scripts/check-prompts.py
```
