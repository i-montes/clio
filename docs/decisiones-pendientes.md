# Decisiones pendientes de la persona

Escrito durante la fase 1, con la persona desconectada. Aquí está lo que se decidió por defecto (y se puede cambiar) y lo que solo ella puede decidir, con qué pasa si no se toca.

## Para decidir (bloquean trabajo)

### 1. Respaldos cifrados y Tailnet Lock en este PC · tareas 093, 094, 095
Decidiste que no hay servidor por ahora, así que el PC es la base (ADR 0006) y `infra/local/check-host.sh` ya confirma lo esencial: disco LUKS, cortafuegos, SSH apagado, nada de Clio fuera de loopback, llave de Tailscale con vencimiento.
- **Herramienta: restic (decidido el 2026-11-02),** instalado, con scripts en `infra/local/` (`docs/respaldos.md`). Lugar por defecto según el sistema (Linux y macOS, fuera de la carpeta de datos) y solo las 2 últimas copias (decidido el 2026-11-02). Ese lugar está en el mismo disco: **falta un destino fuera de este PC** (un disco externo o un almacenamiento en la nube cifrado), crear el repositorio y probar una restauración (093, 094). Mientras tanto, la memoria y el vault solo viven en este disco.
- **Tailnet Lock** no está activado; es opcional y lo decides tú (095).
- **Un VPS, si algún día quieres uno:** es opcional; el borrador está en `infra/vps/` y no bloquea nada.

### 2. Pi como arnés (decidido por ti el 2026-10-31, ADR 0007)
Pi 1.4.0 (publicada el 2026-10-21, activa) es el arnés: bucle, proveedores, sesiones y compactación; Clio es el servicio, la guardia y la interfaz alrededor. Quedan por decidir cosas menores que tomé por defecto: el modelo diario (hoy `beam-free`, gratuito; tu Aria usa `mimo-v2.6-flash`, que sí consume la cuota de tu plan Go) (Pi ya corre dentro de `bwrap`, con red propia y el proveedor como único destino: ADR 0009, hecho el 2026-11-03).

### 3. Topes de gasto en el panel de cada proveedor · tarea 281 (S-45)
Se configura en la web del proveedor, no en código. Hoy se usa solo el modelo gratuito, así que el riesgo es nulo; hazlo antes de usar un modelo de pago.

### 4. Rotar la clave de OpenCode
Se pegó en el chat de una sesión de la fase 1. **Decidido el 2026-11-02: no se rota por ahora.** Si se rota después: actualizar `~/.config/clio/opencode-api-key` y luego `gh secret set OPENCODE_API_KEY < ~/.config/clio/opencode-api-key`.

## Decididas por defecto (cámbialas si no te sirven)

| Tarea | Decisión | Alternativa |
|---|---|---|
| 136 | Formato del contrato: JSON | markdown con frontmatter |
| 137 | Dónde vive el registro del contrato: el archivo que se le pase (hoy en el workspace de trabajo) | memoria episódica (fase 2), o ambos |
| 161 | El anti-bucle cuenta solo repeticiones | contar también pasos «sin avance» |
| 221 | Formato de reglas de la pasarela: JSON propio | unificar con las políticas (fase 5) |
| 270 | Vault: implementación propia en Rust (XChaCha20-Poly1305 y Argon2id) | OpenBao o Infisical |
| 271 | Desbloqueo del vault: frase por terminal | passkey (fase 4), o llave en otro equipo |
| 205 | Aprobaciones por terminal | passkey con la PWA (fase 4) |
| — | `/rápido` fuerza nivel 1 y gana sobre «posible irreversible» | que lo irreversible siempre suba a 2 |
| — | El triaje «pregunta corta» = 12 palabras o menos; listas de verbos cortas y sin afinar con casos reales | afinar con el banco |
| — | La pasarela nunca permite lo irreversible por política: siempre pide aprobación | permitirlo con una política que cubra todo (fase 5) |
| — | Rutas protegidas del agente (sin escritura): `extensions`, `skills`, `.pi`, `.claude`, `.git`, `.agents`, `.vscode`, `.husky`, `.githooks`, `.cargo`, `.devcontainer` (una tarea que escriba en `.git` se niega) | ampliar o quitar `.git` |
| — | real-02: el README ahora dice «redondeo hacia arriba en la mitad» (antes ambiguo; el agente falló 6 de 6 por eso) | dejarla ambigua como prueba de especificación |
| — | Modelo gratuito para pruebas: `opencode-go/beam-free` (no `iceman-contributor-free`, que deja entrenar con tus prompts) | un modelo de pago |

## Cosas que quedan por hacer y no son decisiones
Ver la lista maestra (`docs/lista-maestra.md`): tareas de la fase 1 aún abiertas, sobre todo mediciones con el agente completo, el proceso aparte y el socket del vault (233, 234), y el aislamiento de preproducción (055).

- **Argon2id del vault (hallazgo 11 de la auditoría):** hecho. El coste se guarda en el archivo del baúl y los baúles nuevos usan 64 MiB y 3 pasadas. Tu baúl actual sigue con el coste antiguo hasta que corras `clio vault upgrade-kdf` (con el baúl en marcha; pide tu frase y deja una copia `vault.pre-argon2` que puedes borrar cuando compruebes que abre). `clio doctor` te avisa mientras siga con el coste antiguo.
- **Permiso `workflow` del token de GitHub:** se añadió para poder actualizar ramas con cambios en `.github/workflows`. Si ya no lo necesitas: `gh auth refresh -h github.com -r workflow`.

## Pendiente de probar en un Mac
El flujo interactivo de la TUI (lanzar un trabajo y cancelarlo desde la pantalla, pruebas `pty_*`) se atasca en el CI de macOS y no se pudo depurar sin un Mac, así que esas dos pruebas corren solo en Linux. En macOS sí pasan abrir, salir, la restauración tras un pánico y la ayuda por tubería. Tampoco corre en macOS la prueba de la secuencia de atajos (`Ctrl-D`, `c`, `U`, `?`, `Ctrl-L`, `q`): allí `q` no sale a tiempo, y hay que mirar con un Mac si es un fallo real de la TUI o del controlador de la prueba. Tu PC es Linux, así que no te afecta; solo importa si algún día usas Clio en un Mac.

- **CI con Pi real:** los tests con Pi (`pi_real`, ataques 38–49) se saltan en CI porque el runner no trae Node ≥ 22.19. Para exigirlos hace falta `actions/setup-node` fijado por SHA (verificar el SHA antes de fijarlo) y `CLIO_REQUIRE_PI=1`. Localmente ya pasan todos.
- **Presupuesto en el camino Pi (S-47):** cada prompt es una tarea con los topes de `config.json` (`limits`: pasos = llamadas a herramientas, segundos, costo) y el anti-bucle del `builtin` (la misma llamada más de 4 veces); el corte aborta Pi y, si no se detiene en 5 s, lo mata. Quedan dos huecos: (1) el aborto no es instantáneo, así que una llamada que Pi ya había empezado todavía corre (un contador dentro del toolhost lo cerraría); (2) el tope por día existe pero no tiene valor en la configuración y no se guarda entre ejecuciones, igual que en `builtin`. Prueba de extremo a extremo con un Pi y un modelo reales: pendiente.
- **Cortacircuito de la guardia:** se abre a las 10 negadas seguidas y no se cierra solo; hoy solo sirve empezar otra sesión. ¿Quieres un diálogo para reanudarla?
