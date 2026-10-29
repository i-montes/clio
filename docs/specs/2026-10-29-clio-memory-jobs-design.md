# clio-memory · cola de trabajos (pieza 4 de la v1)

Diseño de la cuarta pieza de `clio-memory`. Se apoya en las anteriores (`docs/specs/2026-10-29-clio-memory-{core,recall,decider}-design.md`) y en el spec completo (`docs/specs/clio-memory-spec-v0.2.md`, sección 6 y partes de la 7 y la 4). Piezas que faltan después: 5 retroalimentación y confianza · 6 CLI, exportar e importar · 7 telemetría.

## Intención

Que lo que no debe hacerse dentro de `propose` ni de una lectura (calcular embeddings, contar usos, olvidar lo que no se usa, respaldar, compactar) viva en una cola dentro del mismo archivo SQLite, que nunca pierda un trabajo (se encola en la misma transacción que la operación que lo causa), que no repita trabajo y que se recupere sola si un proceso muere a medias. Con la cola, `embed_pending` deja de ser algo que alguien tiene que acordarse de llamar.

## Objetivos

- Tabla `jobs` con reintentos con espera creciente, arrendamientos que vencen, claves de deduplicación y cola de errores.
- Encolado atómico con la operación: la operación y sus trabajos se escriben en una sola transacción, o no se escribe nada.
- Un ejecutor síncrono, dirigido por quien llama: `Engine::run_jobs(&RunOpts)`.
- Tipos de trabajo: `embed`, `touch`, `decay`, `backup`, `vacuum`.
- Uso de los recuerdos (`touch`) y su efecto pequeño en el orden de `recall` (frecuencia de uso).
- Olvido por desuso (`decay`) con la política de retención del ámbito, determinista con el reloj fijado.
- Respaldo en caliente con la API de respaldo de SQLite, con permisos 0600.
- Consultar la cola: listar trabajos y estadísticas.

## No objetivos de esta pieza

Un planificador periódico (el daemon de la v2 decide cuándo; aquí quien llama encola `decay`, `backup` o `vacuum`). Los trabajos `summarize`, `consolidate` y `dedupe` (v2: necesitan episodios, resúmenes o un LLM). Los trabajos de retroalimentación (pieza 5, reutilizarán esta cola). Varios trabajadores en paralelo. Detección de máquina ociosa o de batería (la decide quien llama con `allow_heavy`). Cifrado de respaldos (decisión abierta del spec: SQLCipher).

## Anti-objetivos

- Que un trabajo se pierda o quede sin su operación (ni al revés): atomicidad.
- Que un trabajo pueda hacer algo que ninguna API permitiría: el olvido solo archiva (nunca borra), y no se ejecuta sobre lo que está en un bloque fijo.
- Que una lectura deje de ser rápida o falle por no poder registrar su uso.
- Que la cola crezca sin límite.
- Que una carga útil guarde texto de recuerdos: solo ids y rutas.

## Comportamiento

**Tabla `jobs`**: `id`, `kind` (`Embed`, `Touch`, `Decay`, `Backup`, `Vacuum`), `payload` (JSON, solo ids, ámbito o ruta), `priority`, `state` (`Pending`, `Running`, `Done`, `Failed`), `attempts`, `max_attempts` (5), `run_after`, `lease_until`, `dedupe_key`, `error`, `created_at`, `finished_at`. Índice único parcial sobre `dedupe_key` **solo mientras el trabajo esté `Pending`** (no `Running`): un segundo encolado con la misma clave se ignora (`INSERT OR IGNORE`). Así un recuerdo nuevo que llega mientras un `Embed` corre encola otro y nunca se pierde; como mucho hay un `Embed` en ejecución y uno pendiente.

**Encolado con la operación**: `append_with_decisions` recibe además los trabajos nuevos y los escribe en la misma transacción. Reglas: toda operación que deje algún recuerdo no archivado (Add, Update, Revert, Promote) encola un trabajo `Embed` con clave `embed`, pero solo si el motor tiene un `Embedder`. Así diez actualizaciones seguidas generan un solo trabajo `Embed` pendiente (prueba 9). El trabajo `Embed` vacía todo lo pendiente: repite lotes (como `embed_pending`) hasta que no queda nada pendiente.

**Ejecutor**: `run_jobs(&RunOpts { max_jobs, allow_heavy })` devuelve `RunReport { done, retried, failed, skipped_heavy, recovered }`. Reclama el siguiente trabajo (mayor prioridad, luego el más antiguo) con `run_after <= ahora`, lo marca `Running` con un arrendamiento de 300 s y `attempts + 1`, ejecuta, y lo deja `Done`; si falla, `Pending` con `run_after = ahora + min(60 · 2^intentos, 3600)` s, y `Failed` al agotar `max_attempts`. Al empezar, un `Running` con arrendamiento vencido vuelve a `Pending` (un proceso que murió), salvo que ya haya agotado `max_attempts`: entonces pasa a `Failed` con el error «arrendamiento vencido». `attempts` cuenta reclamos, así que tras el primer fallo la espera es `60·2^0 = 60` s (`min(60 · 2^(intentos−1), 3600)`). Un error **permanente** va directo a `Failed` sin reintentos: no hay `Embedder`, ámbito inexistente, el destino de un respaldo ya existe, un id que ya no existe. Los demás (base ocupada, fallo del modelo) se reintentan. Los trabajos `Done` de más de 7 días se podan al final de cada corrida; los `Failed` (la cola de errores) se conservan 90 días para que alguien los revise. Los `Touch` listos se reclaman juntos (hasta 1000) y se aplican en una sola transacción de uso y otra para cerrarlos; el lote cuenta como un trabajo de `max_jobs`. `Backup` y `Vacuum` son pesados: solo con `allow_heavy`.

**Prioridades**: `Embed` 10, `Touch` 5, `Decay` 3, `Backup` 2, `Vacuum` 1.

**`touch` y frecuencia de uso**: tabla `usage(memory, uses, last_used)`. `recall`, en un motor con escritura, encola un `Touch` con los ids devueltos y la hora de la consulta; usa `try_lock` sobre el candado de escritura y, si está ocupado (un respaldo, una escritura, un decisor que llama a `recall` dentro de su propia consulta) o si algo falla, lo omite: es un efecto lateral de mejor esfuerzo que nunca bloquea ni falla la lectura, y por eso puede subcontar usos durante trabajos pesados. Un motor de solo lectura no encola, y las consultas históricas (`as_of` o `known_at`) no cuentan como uso. El ejecutor aplica `uses += 1` y `last_used = max(last_used, hora de la consulta)`, no la hora de la corrida. `recall` suma al impulso por recencia un impulso pequeño por uso: `0,05 · min(1, ln(1 + usos) / ln(21))`, siempre menor que el de recencia máximo.

**`decay`**: columna `retention_days` (nula = nunca) en el ámbito; se fija con `Engine::set_retention(scope, días, actor)` (solo una persona). Antes de decidir, `decay` aplica los `Touch` pendientes, para que un recuerdo recién usado no se archive. Recorre los recuerdos **activos de tipo trabajo o episódico** del ámbito cuya última actividad —el mayor entre su último uso, su `known_since` y la hora de su última operación `Revert` o `Promote` (una persona lo reactivó, así que no se vuelve a archivar enseguida)— es anterior a `ahora − días`, en orden de id, y los archiva con operaciones ARCHIVE de un actor interno `System`, con la razón «decay: sin uso desde hace N días». Nunca toca recuerdos protegidos por un bloque fijo: tanto los ids que el bloque lista como los recuerdos activos a los que esos ids resuelven siguiendo las actualizaciones (`current()`), que son los que el bloque muestra de verdad; tampoco los de tipo semántico o procedimental (un hecho no deja de ser cierto por no usarse). Con el reloj fijado, dos corridas dan exactamente el mismo resultado y la segunda no cambia nada (prueba 18).

**`backup`**: copia en caliente con la API de respaldo de SQLite a `<ruta>.tmp`, que se borra primero si quedó uno viejo, se crea de forma exclusiva con permisos 0600 y se publica con un enlace duro a `<ruta>` (falla si `<ruta>` ya existe: no sobrescribe, error permanente) y se borra el `.tmp`. **`vacuum`**: `VACUUM` sobre la conexión de escritura.

**Consulta**: `Engine::jobs(estado, límite) -> Vec<JobRecord>` y `Engine::queue_stats() -> QueueStats { pending, running, failed, oldest_pending_age_secs }`; `Engine::retry_job(id, persona)` devuelve un `Failed` a `Pending`.

**Encolado manual**: `enqueue_decay(scope)`, `enqueue_backup(ruta)`, `enqueue_vacuum()` (con claves de deduplicación `decay:<ámbito>`, `backup:<ruta>`, `vacuum`).

## Decisiones tomadas por el asistente (revisar)

- Un solo trabajo `Embed` global y deduplicado, en vez de uno por recuerdo: es lo que hace posible «diez actualizaciones, un trabajo».
- `Embed` solo se encola si hay `Embedder`; sin él, `embed_pending` sigue siendo manual.
- El olvido solo aplica a tipos trabajo y episódico, nunca a semántico ni procedimental, y nunca a lo que está en un bloque fijo (conservador: «nada destructivo con dudas»).
- Un actor interno `System` (nuevo `ActorKind::System`) firma las operaciones de olvido; `register_actor` no puede crear actores de ese tipo y `System` no tiene permisos de revisión.
- Sin planificador periódico: la cadencia la pone quien llama (el daemon de la v2 o la CLI de la pieza 6).
- `recall` encola `Touch` solo en motores con escritura y sin fallar si no puede.
- **Frontera de confianza** (tras la review): el actor `System` no puede ser autor de `propose` (de lo contrario tendría confianza de usuario y cuota sin límite), el prefijo de nombre `clio-memory:` está reservado, y el actor se busca por nombre **y** tipo; solo un trabajo `Running` puede terminarse, fallarse o reencolarse (un trabajador que perdió su arrendamiento no revive uno ya terminado); `retry_job` con un trabajo idéntico pendiente lo cierra como `Done` en vez de chocar con el índice; `decay` calcula qué protegen los bloques dentro del candado de escritura; la ruta de un respaldo se resuelve a absoluta al encolar y debe ser UTF-8.
- Reintentos: 5 intentos, espera `min(60·2^n, 3600)` s; arrendamiento de 300 s; poda de `Done` a los 7 días.
- Esquema: `jobs`, `usage` y `retention_days` se añaden al DDL de la versión 1 sin subir `VERSION` (no hay archivos reales).
- Respaldo: la API de respaldo de SQLite (feature `backup` de `rusqlite`, sin crates nuevos), permisos 0600, sin sobrescribir. El cifrado queda como decisión abierta del spec.

## Pruebas de aceptación que cubre esta pieza

Del spec: 8 (matar el proceso a mitad de una escritura no deja operaciones sin sus trabajos ni trabajos sin su operación: una escritura que falla no deja trabajos y una que sale bien deja el suyo, en la misma transacción), 9 (diez actualizaciones seguidas generan un solo trabajo de embedding), 18 (con el reloj fijado, dos corridas del olvido dan exactamente el mismo resultado). Además: la deduplicación; los reintentos con espera creciente y la cola de errores; la recuperación de un arrendamiento vencido; la prioridad; los trabajos pesados solo con `allow_heavy`; `Touch` fusionados y el efecto de uso en `recall` (acotado); el olvido respeta tipos, bloques fijos y la retención nula; el respaldo abre y tiene los mismos datos, con 0600 y sin sobrescribir; `vacuum`; la poda de `Done`; `set_retention` solo para personas; un motor de solo lectura no encola.

Reglas de seguridad: S-23 (el respaldo es 0600), S-18 (las cargas útiles no tienen texto de recuerdos), S-25 (`System` no puede promover ni revisar).

## Riesgos conocidos

- Cada `recall` en un motor con escritura hace una escritura pequeña (encolar `Touch`): añade del orden de milisegundos por la sincronización a disco. Si molesta, se puede agrupar en memoria y volcar en el ejecutor.
- Sin planificador, si nadie llama a `run_jobs` los trabajos se acumulan (los `Embed` y `Decay` están deduplicados; los `Touch` no, y crecen con cada `recall`).
- Los trabajos pesados (`vacuum`, `backup`) mantienen el candado de escritura mientras corren.
- El respaldo no está cifrado: el archivo es 0600 pero legible por quien tenga el disco.
