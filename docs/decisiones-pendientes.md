# Decisiones pendientes de la persona

Escrito durante la fase 1, con la persona desconectada. Aquí está lo que se decidió por defecto (y se puede cambiar) y lo que solo ella puede decidir, con qué pasa si no se toca.

## Para decidir (bloquean trabajo)

### 1. Servidor (VPS) y Tailscale · tareas 085 a 096
Sin esto no hay un segundo nodo real ni se pueden probar las reglas S-38, S-39 y S-40. Hay un borrador provider-neutral en `infra/vps/` (no probado).
- **Qué elegir:** proveedor, región y tamaño del servidor; una cuenta de Tailscale (con vencimiento de llaves y Tailnet Lock, S-31).
- **Costo:** un VPS pequeño y una cuenta de Tailscale personal; consulta los precios actuales del proveedor que elijas, no los inventé.
- **Si no decides:** el agente sigue corriendo en tu máquina, que es suficiente para desarrollar, pero no se puede cerrar la fase 1 ni medir el corte «un nodo = el VPS».

### 2. Adaptador de Pi · tareas 098 a 102
Pi está fijado (0.73.1, MIT, sin scripts de instalación, lockfile, auditoría diaria) pero el adaptador no se escribió.
- **Qué elegir:** si el adaptador es TypeScript dentro de `adapters/pi` o un proceso aparte que habla con el cerebro en Rust; y si se adopta Pi o se sigue con el bucle mínimo propio (`clio-brain::agent`), que ya cumple el mismo papel detrás de un trait.
- **Dato nuevo:** la última publicación de Pi en npm es de mayo de 2026; si el proyecto se detuvo, conviene saberlo antes de depender de él.
- **Si no decides:** se sigue con el bucle propio; no se pierde nada.

### 2 bis. Usuario propio del vault y dueño de cada socket · tarea 233 en producción
El vault ya corre como proceso aparte con dos sockets Unix 0600 (uno para la pasarela, otro para la persona), pero con un solo usuario del sistema la separación es una convención, no una pared. En producción hay que correr la pasarela y el vault con usuarios distintos y ajustar dueño y grupo del socket de la pasarela. Va con la decisión del servidor (punto 1).

### 2 ter. ¿Parar la tarea tras N negaciones seguidas de la pasarela?
Hoy el cortacircuito solo frena lo que correría solo; lo que acaba el bucle es el tope de pasos. Añadir un `Stop::Denials` cortaría antes a un modelo que insiste con herramientas prohibidas. Es más estricto: ¿lo quieres? Por defecto no.

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
