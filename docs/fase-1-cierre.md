# Cierre de la fase 1

Escrito el 2026-11-02. La fase 1 ya tiene lo que la hacía fase: un agente que corre en este PC, una línea base medida, y los ataques de inyección, de secretos y de comandos pasando (tareas 298 y 299). Lo que sigue abierto en la lista maestra no bloquea la fase 2; aquí se dice por qué, para que deje de contar como deuda y no se pierda.

Quedan 34 tareas sin marcar. Ninguna se da por hecha: se **aplazan** a propósito, con su motivo.

## Esperan una decisión o una acción de la persona

| Tarea | Qué falta |
|---|---|
| 093, 094 | Herramienta decidida el 2026-11-02: restic, ya instalado, con scripts probados contra un repositorio de juguete (`docs/respaldos.md`). Por defecto las copias van a una carpeta por sistema (Linux y macOS), solo las 2 últimas; ese lugar está en el mismo disco, así que falta **un destino fuera de este PC**, guardar la frase fuera del equipo y probar la primera restauración. Es el único pendiente con riesgo de pérdida: la memoria y el vault solo viven en este disco. |
| 095 | Tailnet Lock: opcional, lo decide la persona. La llave de este PC ya vence el 2027-03-11. |
| 281 | Topes de gasto en el panel de cada proveedor. Hoy solo se usa el modelo gratuito; hacerlo antes de usar uno de pago. |
| 137 | Dónde vive el registro del contrato. Por defecto: el archivo que se le pase (ver `decisiones-pendientes.md`). |

## Se hacen cuando exista lo que las necesita

| Tarea | Por qué espera |
|---|---|
| 085 (instalador y unidades de usuario), 089, 090, 091 | Son de un servicio permanente o de un VPS. Hoy todo corre con el usuario de la persona (ADR 0006); `infra/local/check-host.sh` ya comprueba lo que se puede comprobar. |
| 260 | La rotación programada necesita un programador (fase 5). |
| 284, 055 | La preproducción aislada tiene sentido cuando haya nodos y un vault real que aislar (fase 4). |

## Son mediciones: se hacen con el agente completo y con uso real

126, 232, 285 a 294 miden decisiones del diseño (D1 a D9, D13, D16, D22) y necesitan el agente con triaje, contrato y verificación en el camino que se use. 257, 295 y 296 necesitan una semana de uso real. 297 mide la sobrecarga de la telemetría. 074 fija los umbrales de la compuerta con esas mediciones, y 125 fija el objetivo de subidas del triaje.

La medición de Pi contra `builtin` (22 tareas, 57/66 contra 55/66) ya está hecha: `docs/mediciones/fase-1-pi-vs-builtin.md`.

## Son revisiones que no se terminan

- **096** (S-03): revisión por componente de qué puede hacer y por qué lo necesita. Conviene hacerla cuando el camino Pi tenga sus piezas de control (ver abajo).
- **102** (S-42): revisar cada actualización de Pi antes de adoptarla, en cada subida de versión.
- **194** (S-08): revisar el armado del contexto. **197** (S-05): la prueba de que leer una página web marca la sesión; hoy no hay herramienta web en el camino Pi, y leer un archivo sí marca la sesión.

## Lo que la fase 1 deja abierto en el diseño

El camino Pi, que es el predeterminado, aplica presupuesto y anti-bucle por prompt (S-47), pero **no triaje, contrato ni verificación**: esos tres siguen solo en el bucle `builtin`. Decidido el 2026-11-02: viven en el cerebro, alrededor de `run_prompt` ([ADR 0008](decisions/0008-control-wraps-the-pi-run-in-the-brain.md)); falta construirlos. Los huecos de presupuesto (una llamada ya iniciada todavía corre; el tope por día no tiene valor en la configuración) están en `decisiones-pendientes.md`.
