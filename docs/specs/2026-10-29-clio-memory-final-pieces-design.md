# clio-memory · retroalimentación, CLI/exportación y telemetría (piezas 5, 6 y 7 de la v1)

Diseño conjunto de las tres piezas que cierran la v1 de `clio-memory`. Se apoya en las anteriores (`docs/specs/2026-10-29-clio-memory-{core,recall,decider,jobs}-design.md`) y en el spec completo (`docs/specs/clio-memory-spec-v0.2.md`, secciones 7, 8, 9 y 11). Las tres se construyen en este orden: 5 → 6 → 7, y quedan en un solo plan.

## Intención

- **Pieza 5:** que el sistema aprenda de lo que se usa de verdad: una señal de «útil» o «equivocado» sobre un recuerdo cambia cuánto se confía en él, lo protege del olvido o, si se repite, lo manda a revisión humana.
- **Pieza 6:** que una persona pueda inspeccionar, revisar, deshacer, respaldar, exportar e importar la memoria sin escribir código, y que el crate pueda comprobar su propia salud (`doctor`, `stats`).
- **Pieza 7:** que cada operación importante emita tramos de `tracing` sin contenido de recuerdos, desactivable por completo, y que cada operación del registro guarde el identificador de la traza que la causó.

## Pieza 5 · Retroalimentación y confianza

**Qué hace:** `Engine::feedback(memory, Signal::{Useful, Wrong}, actor)`. Cualquier actor que no sea `System` puede dar la señal (cuenta contra su cuota, como cualquier escritura); el recuerdo debe estar activo.
- Tabla `feedback(id, memory, actor, signal, at, spent)`, solo se agrega (salvo `spent`); no pasa por el registro de operaciones (es un dato derivado de uso, como `usage`). `spent` marca las señales «equivocado» ya consumidas por un `Flag` o un `Promote`: el contador de «equivocado» es el de señales **sin gastar**, así no depende de comparar horas (el reloj puede estar fijo). Un `revert` de un `Flag` no las devuelve: volver a mandar a revisión exige tres señales nuevas.
- **Confianza** (no se guarda en la fila del recuerdo): `clamp(0,5 + 0,1·útiles − 0,2·equivocados, 0, 1)`; se calcula al consultar con `Engine::confidence(memory)`.
- **Efecto en `recall`:** el puntaje se multiplica por `1 + 0,1·(2·confianza − 1)`, entre 0,9 y 1,1; cada `RecallHit` trae su `confidence`.
- **Protección del olvido:** una señal «útil» cuenta como actividad reciente para `Decay`.
- **Revisión:** la tercera señal «equivocado» sin gastar (de cualquier actor, mientras el recuerdo siga activo) lo manda a cuarentena con una operación nueva `OpKind::Flag` (razón «equivocado marcado 3 veces»), firmada por el actor que dio esa tercera señal; el `Flag` no gasta cuota aparte (la señal ya la gastó). Una persona lo resuelve con `promote` o `archive`, y `revert` de esa operación lo devuelve a activo. `Flag` y `promote` gastan las señales «equivocado» del recuerdo.
- `Engine::feedback_summary(memory) -> FeedbackSummary { useful, wrong, confidence }`.

**Aceptación:** prueba 19 del spec.

## Pieza 6 · CLI, exportar, importar, doctor, stats

**API del crate (nueva):**
- `Engine::scopes() -> Vec<ScopeInfo>` y `Engine::actors() -> Vec<ActorInfo>` (para que la CLI resuelva nombres).
- `Engine::stats() -> Stats`: tamaño del archivo y del WAL, recuerdos por estado y ámbito, operaciones, decisiones, vectores, embeddings pendientes y `QueueStats`.
- `Engine::doctor() -> DoctorReport { checks: Vec<Check { name, ok, detail }> }`. Comprueba: permisos 0600 del archivo, versión del esquema, que la proyección coincide con lo que da volver a plegar el registro (comparación en memoria, sin tocar nada), que no hay vectores ni uso ni retroalimentación de recuerdos que no existen, que ninguna entrada de bloque apunta a un recuerdo inexistente, trabajos fallidos y pendientes de embebido.
- `Engine::export(&self, out: &mut dyn Write) -> Result<ExportSummary>` y `Engine::import(&self, input: &mut dyn BufRead) -> Result<ImportSummary>`.
- `Engine::import_memories(&self, input, scope, actor, opts) -> Result<ImportSummary>`.

**Formato de exportación** (JSON Lines, `EXPORT_FORMAT_VERSION = "1.0.0"`, semver): primera línea `{"type":"header","format":"clio-memory-export","format_version":"1.0.0","schema_version":1,"exported_at":…}`; después, en este orden, `scope`, `actor`, `operation` (con las fotos antes/después completas), `decision`, `block`, `block_memory`, `usage`, `feedback`. Sin vectores (se recalculan) ni trabajos ni cuotas (estado operativo). La exportación lee una instantánea consistente. El archivo se crea nuevo con permisos 0600 (no sobrescribe); no está cifrado.

**Importar (restaurar)**: solo sobre un archivo vacío: sin ámbitos ni operaciones y, como mucho, el actor interno `clio-memory:system` (que se reemplaza por el de la exportación); si no, `InvalidState`. La línea `operation` incluye el `trace_id`. La versión de esquema del encabezado debe ser igual a la del crate (restaurar entre versiones de esquema distintas necesitará una migración que no existe todavía). El resumen es un `ExportSummary` con lo restaurado. Valida que la versión mayor coincida y rechaza una menor más nueva; inserta todo con los mismos ids en una sola transacción, vuelve a plegar la proyección y encola el `Embed` si hay `Embedder`. Rechaza el archivo entero si alguna foto de recuerdo parece contener un secreto (S-18) o si una línea es inválida. Está pensado para restaurar una exportación propia, no para aceptar archivos ajenos.

**Importar recuerdos sueltos (`import_memories`)**: cada línea se procesa por separado (cada `propose` confirma por su cuenta): una línea con JSON inválido, sin `content`, con un `kind` desconocido, demasiado larga, con un secreto o que rebase la cuota se **rechaza y se cuenta**, y el resto sigue; el `kind` ausente vale `semantic`. `ImportSummary { read, added, noop, quarantined, rejected: Vec<(número de línea, motivo)> }` (la lista guarda como mucho 100). La CLI sale con 1 si hubo rechazos. Líneas JSON `{"content": …, "kind": "semantic"|…, "topic_key": …, "valid_from": …, "source": …}` hacia un ámbito, **a través de `propose`** con el actor indicado (que debe ser de tipo importador o persona): pasan por cuota, detector de secretos, reglas y decisores. Por defecto entran con origen `External` (cuarentena); `ImportOpts { origin }` permite cambiarlo, y declarar `User` solo lo respeta una persona o un importador. Un adaptador para el formato de Engram queda fuera de la v1 (no se conoce su formato).

**Qué actor y qué modo de apertura usa la CLI:** `--actor NOMBRE` reemplaza al actor por defecto `cli:<usuario>`; el actor se crea como persona con una cuota de un millón por hora si falta, y si ya existe debe ser una persona. `import-memories` usa un actor importador `cli-import:<usuario>` (se crea con la misma cuota). Los comandos que solo leen (`scopes`, `stats`, `doctor`, `list`, `get`, `history`, `review`, `jobs`, `export`, `recall`, `context`) abren el archivo en **solo lectura**: no toman el candado de escritura (se puede inspeccionar un archivo que otro programa tiene abierto) y `recall` desde la CLI no registra uso. Los que escriben (`promote`, `archive`, `revert`, `feedback`, `retry-job`, `run-jobs`, `backup`, `import`, `import-memories`) abren con escritura y, si otro proceso tiene el candado, fallan con un mensaje claro. `run-jobs` y `backup` ejecutan los trabajos de forma síncrona; `backup` encola y corre el respaldo con trabajos pesados permitidos.

**CLI** (`clio-memory`, un binario del mismo crate sin dependencias nuevas, con un analizador de argumentos mínimo): `clio-memory --db RUTA [--actor NOMBRE] [--model DIR] [--json] <comando>`. Comandos: `scopes`, `stats`, `doctor`, `list ÁMBITO [--state E]`, `get ID`, `history ID`, `recall ÁMBITO TEXTO`, `context ÁMBITO TEXTO`, `review ÁMBITO`, `promote ID`, `archive ID`, `revert OP`, `feedback ID útil|equivocado`, `jobs`, `retry-job ID`, `run-jobs [--heavy]`, `backup RUTA`, `export RUTA`, `import RUTA`, `import-memories ÁMBITO RUTA`. Los comandos de revisión usan un actor de tipo persona llamado `cli:<usuario>` (se crea si falta). `--model` (con la feature `model2vec`) carga un modelo local para `recall`, `context` y `run-jobs`; sin él todo funciona solo con texto. Códigos de salida: 0 bien, 1 error (o `doctor` con una comprobación fallida), 2 uso incorrecto.

**Aceptación:** pruebas 1 (reconstrucción) y 16 también por la CLI; ida y vuelta de exportar e importar; la CLI nunca expone `register_actor`, `create_scope` ni `set_retention` (no hay comando para ellos).

## Pieza 7 · Telemetría

Con la feature `telemetry` (hoy vacía) el crate depende de `tracing` y emite tramos y eventos; sin ella no hay ningún código, dependencia, columna usada ni tabla de telemetría (prueba 21).
- **Tramos:** `propose` (con hijos: normalizar, secretos, reglas, vecinos, cada decisor, escritura), `recall` (texto, vector, fusión), `context`, `revert`, `promote`, `archive`, `feedback`, cada trabajo (`job` con tipo, intentos y resultado). **Eventos:** un segundo proceso rechazado por el bloqueo, cambio de modelo en `embed_pending`, REVERT, respaldo hecho/fallido, trabajo que pasa a `Failed`.
- **Sin contenido (prueba 22):** los campos llevan ids, tamaños, conteos, tiempos y decisiones; nunca el texto de un recuerdo, ni el `topic_key`, ni nombres.
- **`trace_id` (prueba 24):** columna `operations.trace_id` (siempre en el esquema, nula sin la feature) y campo `trace_id` en `Operation`. Con la feature, cada operación guarda el identificador de la traza que la causó: el que dé `Engine::with_trace_id_provider(Arc<dyn Fn() -> Option<String> + Send + Sync>)` (el programa que embebe lo conecta a su contexto OpenTelemetry) o, si no hay proveedor o no devuelve nada, un identificador aleatorio de 128 bits generado por operación (con `RandomState`, sin dependencias). Ese mismo valor va en el campo `trace_id` del tramo de la operación.
- **Métricas:** las latencias salen de la duración de los tramos; los calibres (tamaños, recuerdos por estado, profundidad de la cola, tasas) salen de `Engine::stats()`, que el programa consulta. El crate no depende de `opentelemetry`. El módulo de retención y el archivo aparte de trazas son de la v2 (prueba 23).

## No objetivos

Daemon, HTTP y MCP (v2). Cifrado de respaldos y exportaciones. Adaptador de Engram. Módulo de retención y resúmenes por hora/día de telemetría (v2). Una interfaz web. Comando de CLI para crear ámbitos, registrar actores o fijar la retención.

## Anti-objetivos

- Que una señal de retroalimentación pueda borrar o reemplazar algo: lo máximo que hace es mandar un recuerdo a cuarentena (reversible).
- Que importar o exportar eluda el detector de secretos o la cuarentena.
- Que un tramo o evento lleve texto de recuerdos.
- Que la CLI dé a un agente poderes que el crate reserva a una persona.
- Que la telemetría tenga costo con la feature apagada.

## Decisiones tomadas por el asistente (revisar)

- La retroalimentación se aplica de forma síncrona, no como trabajo de la cola (es barata y necesita ser atómica con la operación `Flag`).
- La confianza se calcula, no se guarda; no cambia la fila `MemoryRow`.
- Tres «equivocado» de cualquier actor mandan a revisión: es reversible y cada señal gasta cuota; no se distinguen actores.
- `import` es para restaurar exportaciones propias (con ids, raw), con un escaneo de secretos; `import_memories` es el camino seguro para contenido ajeno (pasa por `propose`, origen externo por defecto).
- La CLI es un binario del propio crate sin `clap`: son unos veinte comandos simples y no vale una dependencia nueva.
- La CLI actúa como persona (`cli:<usuario>`): quien tiene acceso al archivo ya tiene acceso a todo.
- `trace_id` se guarda siempre como columna del esquema; solo se llena con la feature.
- El crate no depende de `opentelemetry`; el programa conecta su contexto con `with_trace_id_provider`.
- Los calibres salen de `stats()`, no de un SDK de métricas.
- Todo el esquema nuevo (`feedback`, `operations.trace_id`) entra en el DDL de la versión 1 sin subir `VERSION`: no existe ningún archivo real (la v1 no se ha publicado) y los archivos de desarrollo creados con versiones anteriores de esta rama deben recrearse.
- `tracing` se añade como dependencia opcional (feature `telemetry`); las pruebas de telemetría usan un `Subscriber` mínimo propio escrito en las pruebas, sin `tracing-subscriber`; `with_trace_id_provider` solo existe con la feature.

## Pruebas de aceptación que cubre

Del spec: 19 (tres «equivocado» → revisión), 21, 22, 24 (telemetría), y refuerzo de 1 y 16 por la CLI. Además: ida y vuelta de exportar e importar (mismos recuerdos, historial, bloques, decisiones, uso y retroalimentación); `import` rechaza archivos con versión mayor distinta, con secretos o sobre un archivo con datos; `import_memories` respeta cuota, secretos y cuarentena; `doctor` detecta una proyección corrupta y huérfanos sembrados a mano; la CLI funciona de punta a punta sobre un archivo temporal.

Reglas de seguridad: S-18 (importar y telemetría no filtran ni guardan secretos), S-23 (exportación 0600), S-25 (la señal «equivocado» solo manda a cuarentena; `promote` sigue siendo de personas).

## Riesgos conocidos

- Un agente puede mandar a revisión un recuerdo correcto con tres señales falsas; es reversible y gasta cuota, pero molesta.
- `import` confía en el archivo salvo por el escaneo de secretos: un archivo manipulado podría traer recuerdos «activos» que nunca pasaron por la cuarentena. Por eso solo se documenta para exportaciones propias.
- Sin la feature `telemetry` no hay ningún tramo; con ella, el costo por operación es el de crear tramos de `tracing` (despreciable sin suscriptor).
- La CLI sin `--model` solo busca por texto; con él carga el modelo (unos segundos) en cada invocación.
