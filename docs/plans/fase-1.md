# Plan de la fase 1 · Corte mínimo: un nodo, sin memoria

Escrito el 2026-10-31, con la persona desconectada. Se ejecuta lo que se puede sin ella; las decisiones que le tocan quedan en [`docs/decisiones-pendientes.md`](../decisiones-pendientes.md) con un valor por defecto que se puede cambiar después.

## Cómo se reparte

Las tareas de la lista (085 a 299) se agrupan por crate. Los grupos A a E no dependen entre sí y se hacen en paralelo, cada uno en una rama aparte; yo integro, corro el CI y marco la lista. Lo único compartido son `Level` y `Effect`, que viven en `clio-core`.

| Grupo | Crate | Tareas | Qué es |
|---|---|---|---|
| A | `clio-triage`, `clio-contract` | 108–148 | Reglas del triaje; el contrato de tarea, sus estados y sus pruebas de aceptación |
| B | `clio-gateway` | 167–221 | El motor de decisión: orden de evaluación, efectos, comandos con argv, rutas canonizadas, contaminación de sesión, hash de aprobación, falla cerrada, cortacircuito |
| C | `clio-vault` | 233–272 | El vault: cifrado, marcadores, `put`/`resolve`/`issue_token`/`rotate`/`revoke`/`list`/`audit`, filtro de limpieza de salidas |
| D | `clio-brain` | 149–166, 273–283 | Capa de control (presupuesto, anti-bucle, reintentos, puntos de control), verificación, catálogo de modelos |
| E | `clio-node` | 222–231 | Sandbox, snapshot por tarea, cancelar, registro de efectos, arrendamiento de recursos, herramientas básicas con efecto declarado |

Yo me ocupo de: los prompts versionados y su escáner (104–107), la integración y el CI de todo lo anterior, las mediciones que se pueden hacer con el modelo gratuito (285, 286), y de mantener la lista y las decisiones.

## Lo que no se puede ejecutar sin la persona

- **VPS e infraestructura (085–096):** hace falta elegir proveedor y cuenta, una cuenta de Tailscale y dinero. Sin eso solo se puede escribir en seco. Queda como decisión. No invento credenciales ni creo recursos de pago.
- **Adaptador de Pi (097–103):** hace falta fijar una versión de Pi y decidir si el adaptador es TypeScript o un proceso aparte. Queda como decisión.
- **Mediciones con el agente real (284, 287–297):** necesitan el agente completo corriendo con un modelo; se hacen cuando el cerebro exista.
- **074** (umbrales) y la **055** (aislamiento de preproducción) dependen de lo anterior.

## Valores por defecto para las decisiones abiertas

| Tarea | Decisión | Por defecto |
|---|---|---|
| 136 | Formato del contrato | JSON (sin dependencia nueva y fácil de validar); se puede añadir markdown después |
| 137 | Dónde vive el registro | Archivo en el repositorio de trabajo; la memoria episódica llega en la fase 2 |
| 161 | El anti-bucle cuenta «sin avance» | No: solo pasos repetidos, como dice el spec |
| 221 | Formato de las reglas de la pasarela | Uno propio en JSON; se unifica con las políticas en la fase 5 |
| 270 | Implementación del vault | Propia en Rust (ChaCha20-Poly1305 y Argon2); OpenBao o Infisical quedan descartados por ahora |
| 271 | Desbloqueo | Frase por terminal en esta fase; passkey con la PWA (fase 4) |
| 205 | Aprobaciones | Por terminal |

## Cómo se prueba cada trabajo

Cada crate sigue las reglas del repositorio: `unsafe_code = "forbid"`, sin `unwrap` fuera de pruebas, una prueba por cada prueba de aceptación del spec (`tNN_...`), y un comentario `// S-xx` justo encima de cada prueba de una regla de seguridad. Antes de integrar: `cargo fmt`, `clippy -D warnings`, `cargo test`, `cargo deny` y `scripts/rule-coverage.sh`.
