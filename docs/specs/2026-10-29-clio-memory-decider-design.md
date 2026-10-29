# clio-memory · decisor en cascada (pieza 3 de la v1)

Diseño de la tercera pieza de `clio-memory`. Se apoya en el núcleo y en la lectura (`docs/specs/2026-10-29-clio-memory-core-design.md`, `…-recall-design.md`) y en el spec completo (`docs/specs/clio-memory-spec-v0.2.md`, secciones 2 y 3). Piezas que faltan después de esta: 4 cola de trabajos · 5 retroalimentación y confianza · 6 CLI, exportar e importar · 7 telemetría.

## Intención

Dejar un punto de enganche claro para que algo más listo que las reglas (un clasificador, Jev, un LLM) pueda opinar sobre qué hacer con una propuesta, sin que ese algo pueda destruir memoria, saltarse la cuarentena ni bloquear la escritura, y con un registro de lo que decidió para poder compararlo con datos reales. Sin ningún decisor instalado, el comportamiento no cambia.

## Decisión de producto (del autor)

Dos recuerdos que se parecen mucho pero dicen cosas distintas **se dejan como cosas distintas**. Reemplazar es siempre explícito (mismo `topic_key`); nunca por similitud. Consecuencias: no hay detección de conflictos en la v1, el estado «en conflicto» no se usa, y la prueba de aceptación 14 del spec cambia a «dos recuerdos parecidos con contenido distinto conviven y `recall` devuelve ambos».

## Objetivos

- Trait `Decider` y un `Engine::with_decider` que registra decisores en orden, cada uno en modo activo o sombra.
- Cascada: primero las reglas que ya existen (duplicado exacto, confianza/cuarentena, mismo `topic_key`), que pueden vetar; después los decisores registrados; si ninguno decide, el candidato se agrega (distinto).
- Un decisor solo puede elegir entre **agregar** o **pedir revisión humana**. Nunca descartar una propuesta (NOOP), actualizar, borrar ni archivar: así ninguna decisión pierde el texto propuesto.
- Registro de decisiones (proveedor, elección, confianza, razón, modo, si actuó), consultable.
- Vecinos para los decisores: los recuerdos más parecidos del mismo ámbito, si hay embedder y vectores.
- Un decisor incluido, **opt-in**: `NearDuplicateDecider` (casi-duplicado por similitud), apagado por defecto.

## No objetivos de esta pieza

Conflictos entre recuerdos (descartados arriba). Decisores de Jev o de LLM (v2). Reemplazar por similitud. Borrar o archivar por decisión de un decisor. Herramienta de comparación de decisores más allá de consultar el registro. Cola de trabajos (pieza 4).

## Anti-objetivos

- Que un decisor pueda causar pérdida de datos, saltarse la cuarentena (un recuerdo externo sigue entrando en cuarentena decida lo que decida) o promover algo.
- Que un decisor que falla, se cuelga lógicamente o devuelve basura impida o corrompa una escritura.
- Que `propose` sea más lento o dependa de un modelo cuando no hay decisores registrados.
- Que un decisor reciba contenido que no pasó el detector de secretos.

## Comportamiento

**Tipos** (públicos):
- `Question`: el candidato (contenido normalizado, tipo, `topic_key`, origen efectivo, clase de actor, ámbito) y `neighbors: Vec<Neighbor>` (`Neighbor { memory, similarity }`, hasta 5, mejor primero).
- `Choice`: `Add`, `Review`.
- `Decision { choice, confidence: f32, reason: Option<String> }`; el proveedor es el `name()` del decisor registrado, así que un decisor no puede registrarse con el nombre de otro. Una confianza fuera de `[0, 1]` o NaN hace la decisión inválida (se registra y no se aplica).
- `trait Decider: Send + Sync { fn name(&self) -> &str; fn decide(&self, q: &Question) -> Result<Option<Decision>>; }` (`None` = no sé).
- `Mode`: `Active` o `Shadow`.

**Cascada en `propose`** (después de cuota, tamaño, secretos y normalización; antes de escribir):
1. Reglas, como hoy: duplicado exacto → NOOP; origen no confiable o procedimental de un agente → cuarentena; mismo `topic_key` → UPDATE. Si las reglas dan algo distinto de «agregar activo sin más», la cascada termina ahí: los decisores no se consultan para eso.
2. Si las reglas dicen «agregar activo» y hay decisores registrados: se calculan los vecinos (solo si hay embedder; si no, lista vacía) y se consulta a cada decisor en orden de registro.
3. Decisor en modo **activo** que devuelve `Some` con `confidence >= min_confidence` (por registro, 0,8 por defecto): su elección se aplica y los siguientes decisores activos no se consultan. Con menos confianza se registra pero no se aplica y sigue la cascada.
4. Decisor en modo **sombra**: se consulta siempre (aunque otro activo ya haya decidido) y solo se registra.
5. Aplicación: `Add` → agregar activo; `Review` → el candidato entra en **cuarentena** (razón «revisión pedida por <proveedor>») y una persona lo ve con `list` y lo resuelve con `promote` o `archive`. El resultado de `propose` (`Outcome`) no cambia; quién pidió la revisión se ve en la `reason` de la operación (`history`) y en el registro de decisiones.
6. Si nadie decide: agregar (distinto).

**Aislamiento de fallos**: un decisor que devuelve `Err` o entra en pánico se registra como error y se salta; `propose` sigue. Un decisor que tarda mucho no se puede interrumpir (API síncrona); queda documentado.

**Registro**: tabla `decisions(id, scope, op, provider, mode, status, choice, confidence, reason, acted, at)`. `status` es uno de: `decided`, `none` (el decisor no supo), `below_threshold`, `invalid`, `error`; `choice` es nulo cuando no hubo elección. `op` es la operación que se escribió en esa misma transacción: las decisiones se guardan junto con la operación (`append` las recibe), así que no quedan filas huérfanas si la escritura falla. El registro es de auditoría: `rebuild_projection` no lo toca, y la exportación de la pieza 6 lo incluirá. `Engine::decisions(scope, desde, hasta) -> Vec<DecisionRecord>` filtra por `at` (segundos Unix) con el intervalo `[desde, hasta)`. Las decisiones de las reglas no se registran aquí: ya quedan en el `reason` de la operación; si las reglas terminan la cascada, no se consulta a ningún decisor y no se registra nada.

**`NearDuplicateDecider { threshold }`** (por defecto 0,97): si el vecino más cercano es del mismo tipo, activo, y su similitud llega al umbral, decide `Review` (con la razón «parece duplicado de m12») y confianza igual a la similitud; si no, `None`. Una persona decide si es un duplicado (archiva el candidato) o algo distinto (lo promueve). No se registra por defecto. Una prueba `#[ignore]` contra el modelo real imprime la similitud de una paráfrasis y la de una contradicción para que se elija el umbral con datos (no se exige que las separe).

## Decisiones tomadas por el asistente (revisar)

- Los decisores no pueden elegir NOOP, UPDATE, archivar ni borrar. El spec les daba el NOOP, pero un NOOP descarta el texto propuesto sin dejar rastro, y una paráfrasis y una contradicción son casi indistinguibles para un modelo de embeddings («no quiero café» absorbido por «quiero café»). Por diseño, el peor caso de un decisor malicioso o roto es una propuesta en cuarentena.
- `Review` reutiliza la cuarentena, sin un estado nuevo; así la revisión humana ya existente (`list`, `promote`, `archive`) sirve tal cual.
- Las reglas van siempre primero y terminan la cascada si dan algo distinto de «agregar activo».
- Confianza mínima 0,8 por registro y modo sombra incluido en esta pieza (el spec lo manda a la v2, pero la tabla de decisiones ya lo necesita y el costo es pequeño).
- **Frontera de confianza** (tras la review): el `name` y el `reason` de un decisor se recortan a `max_field_bytes` y se reemplazan si parecen un secreto antes de entrar al registro; una `min_confidence` NaN se trata como 1,0 (no actúa) y las demás se acotan a `[0, 1]`; un decisor no debe llamar a métodos de escritura del motor donde está registrado (el candado de escritura no es reentrante y se bloquearía), lo cual queda documentado en el trait; las lecturas sí se pueden.
- Los vecinos solo se calculan si hay al menos un decisor registrado y un embedder: `propose` embebe el candidato en ese momento, dentro del candado de escritura; si el embedder falla o no puede representar el texto, los vecinos van vacíos y la escritura sigue. Los vecinos solo incluyen recuerdos que ya tienen vector (los pendientes de `embed_pending` son invisibles).
- El esquema (tabla `decisions`) se añade al DDL de la versión 1 sin subir `VERSION`, como las demás tablas: no existe ningún archivo real. Un `confidence` inválido (NaN o fuera de `[0, 1]`) se registra como `invalid`. El tipo interno `rules::Decision` se renombra para no chocar con el público.
- El casi-duplicado se ofrece pero apagado; el umbral 0,97 es provisional.
- Se actualizan el spec (conflictos fuera de la v1, prueba 14 reemplazada, el paso 4 y 6 de la escritura y la fila de heurísticas de la tabla de decisores).

## Pruebas de aceptación que cubre esta pieza

Del spec: 7 (ninguna operación DELETE se ejecuta sola: no existe esa operación y un decisor hostil no destruye nada), 14 (nueva versión: parecidos pero distintos conviven y `recall` devuelve ambos). Además: sin decisores el comportamiento es idéntico al actual; el orden de la cascada; `rebuild_projection` no borra el registro; un decisor activo con poca confianza no actúa pero queda registrado; el modo sombra registra y no actúa; un decisor con `Err` o pánico no impide la escritura; una confianza NaN o fuera de rango se registra como inválida y no se aplica; las decisiones se guardan en la misma transacción que la operación (si la escritura falla no queda ninguna); `none` y `error` quedan registrados; `Review` deja el candidato en cuarentena; un candidato externo no puede ser promovido por un decisor; un decisor no puede descartar texto (la propuesta siempre queda escrita, activa o en cuarentena); los vecinos llegan a los decisores en orden de similitud y dentro del ámbito; el registro se puede consultar por rango.

Reglas de seguridad: S-25 y S-26 (un decisor no saca nada de cuarentena), S-18 (los decisores solo ven contenido ya filtrado), S-01 (la seguridad no depende de que el decisor «obedezca»: la validación está en código).

## Riesgos conocidos

- Un decisor lento bloquea la escritura (el candado de escritura se mantiene durante la cascada). Aceptable para decisores locales; el daemon de la v2 lo manejará con un decisor remoto asíncrono.
- La similitud de una paráfrasis y la de una contradicción pueden ser casi iguales con un modelo real; por eso el casi-duplicado va apagado.
- El contenido que recibe un decisor es dato, no instrucción: cuando exista el decisor LLM (v2) tendrá que delimitarlo (S-08).
