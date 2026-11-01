# Decisiones pendientes de la persona

Escrito durante la fase 1, con la persona desconectada. Aquí está lo que se decidió por defecto (y se puede cambiar) y lo que solo ella puede decidir, con qué pasa si no se toca.

## Para decidir (bloquean trabajo)

### 1. Respaldos cifrados y Tailnet Lock en este PC · tareas 093, 094, 095
Decidiste que no hay servidor por ahora, así que el PC es la base (ADR 0006) y `infra/local/check-host.sh` ya confirma lo esencial: disco LUKS, cortafuegos, SSH apagado, nada de Clio fuera de loopback, llave de Tailscale con vencimiento.
- **Falta elegir:** una herramienta de respaldo (restic, borg o kopia; no hay ninguna instalada) y un destino fuera de este PC (un disco externo o un almacenamiento en la nube cifrado), y probar una restauración (093, 094). Mientras tanto, la memoria y el vault solo viven en este disco.
- **Tailnet Lock** no está activado; es opcional y lo decides tú (095).
- **Un VPS, si algún día quieres uno:** es opcional; el borrador está en `infra/vps/` y no bloquea nada.

### 2. Pi como arnés (decidido por ti el 2026-10-31, ADR 0007)
Pi 1.4.0 (publicada el 2026-10-21, activa) es el arnés: bucle, proveedores, sesiones y compactación; Clio es el servicio, la guardia y la interfaz alrededor. Quedan por decidir cosas menores que tomé por defecto: el modelo diario (hoy `beam-free`, gratuito; tu Aria usa `mimo-v2.6-flash`, que sí consume la cuota de tu plan Go) y si se corre el propio Pi dentro de `bwrap` más adelante.

### 3. Topes de gasto en el panel de cada proveedor · tarea 281 (S-45)
Se configura en la web del proveedor, no en código. Hoy se usa solo el modelo gratuito, así que el riesgo es nulo; hazlo antes de usar un modelo de pago.

### 4. Rotar la clave de OpenCode
Se pegó en el chat de esta sesión. Rótala cuando termines de probar. Después: `gh secret set OPENCODE_API_KEY < ~/.config/clio/opencode-api-key` (actualizando antes el archivo).

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
| — | Rutas protegidas del agente: `extensions`, `skills`, `.pi`, `.claude`, `.git` (una tarea que escriba en `.git` se niega) | ampliar o quitar `.git` |
| — | real-02: el README ahora dice «redondeo hacia arriba en la mitad» (antes ambiguo; el agente falló 6 de 6 por eso) | dejarla ambigua como prueba de especificación |
| — | Modelo gratuito para pruebas: `opencode-go/beam-free` (no `iceman-contributor-free`, que deja entrenar con tus prompts) | un modelo de pago |

## Cosas que quedan por hacer y no son decisiones
Ver la lista maestra (`docs/lista-maestra.md`): tareas de la fase 1 aún abiertas, sobre todo mediciones con el agente completo, el proceso aparte y el socket del vault (233, 234), y el aislamiento de preproducción (055).

## Pendiente de probar en un Mac
El flujo interactivo de la TUI (lanzar un trabajo y cancelarlo desde la pantalla, pruebas `pty_*`) se atasca en el CI de macOS y no se pudo depurar sin un Mac, así que esas dos pruebas corren solo en Linux. En macOS sí pasan abrir, salir, la restauración tras un pánico y la ayuda por tubería. Tampoco corre en macOS la prueba de la secuencia de atajos (`Ctrl-D`, `c`, `U`, `?`, `Ctrl-L`, `q`): allí `q` no sale a tiempo, y hay que mirar con un Mac si es un fallo real de la TUI o del controlador de la prueba. Tu PC es Linux, así que no te afecta; solo importa si algún día usas Clio en un Mac.

- **CI con Pi real:** los tests con Pi (`pi_real`, ataques 38–49) se saltan en CI porque el runner no trae Node ≥ 22.19. Para exigirlos hace falta `actions/setup-node` fijado por SHA (verificar el SHA antes de fijarlo) y `CLIO_REQUIRE_PI=1`. Localmente ya pasan todos.
- **Cortacircuito de la guardia:** se abre a las 10 negadas seguidas y no se cierra solo; hoy solo sirve empezar otra sesión. ¿Quieres un diálogo para reanudarla?
