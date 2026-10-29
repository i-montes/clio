# clio-memory · núcleo (pieza 1 de la v1)

Diseño de la primera pieza de `clio-memory`. El spec completo (v0.2) describe el motor entero; este documento acota lo que se construye ahora y fija las decisiones. Las siguientes piezas tendrán su propio documento.

Orden de las piezas de la v1: **1 núcleo** (este documento) · 2 vectores y lectura híbrida · 3 decisor en cascada y conflictos · 4 colas de trabajo · 5 CLI, exportar e importar. El daemon (HTTP y MCP) es de la v2.

## Intención

Un motor de memoria para agentes, genérico y publicable por sí solo, sobre un único archivo SQLite. Clio es su primer usuario, no el único. El núcleo es la parte que guarda y deshace recuerdos de forma segura: lo que se escribe queda en un registro que solo crece, y todo lo demás (la tabla de recuerdos, el índice de texto) se puede reconstruir desde ese registro.

## Objetivos

- Guardar, actualizar y deshacer recuerdos de los cuatro tipos (trabajo, episódico, semántico, procedimental) en ámbitos aislados.
- Un solo camino de escritura (`propose`), con cuota por actor, detector de secretos y reglas deterministas.
- Cuarentena para lo externo y `promote` para sacarlo.
- Dos fechas por hecho: vigente desde/hasta (cuándo fue cierto) y sabido desde (cuándo lo supo el sistema).
- Búsqueda de texto (FTS5, con raíz de palabra según el idioma del ámbito).
- Un solo escritor por archivo; el segundo proceso recibe un error claro.
- Reproducible en pruebas: todo lo que depende del tiempo lee un `Clock` inyectable.

## No objetivos de esta pieza

Vectores, `recall` híbrido, abstención, `context()` y bloques fijos (pieza 2). Decisor en cascada, vecinos y detección de conflictos (pieza 3). Cola de trabajo, olvido automático, `feedback` y `explain` (pieza 4 o posterior). CLI, exportar e importar (pieza 5). PURGE (v2). Daemon, MCP, grafo, multiusuario, réplicas e interfaz web (fuera de la v1).

## Anti-objetivos

- Una API que permita escribir sin pasar por `propose`.
- Un secreto que quede en alguna tabla, en el registro o en un log, aunque la propuesta se rechace (S-18).
- Que un recuerdo externo, o un procedimiento propuesto por un agente, aparezca en una lectura normal antes de una aprobación (S-25, S-26).
- Reescribir la historia: deshacer agrega operaciones, nunca edita ni borra las anteriores.
- Que el crate dependa de otro crate de Clio.

## Restricciones

- Rust edición 2024, `unsafe_code = "forbid"`, `unwrap_used = "deny"` fuera de pruebas. Licencia `MIT OR Apache-2.0`; toda dependencia nueva debe pasar `cargo deny`.
- `rusqlite` con SQLite incluido, modo WAL. No depende de `clio-store` (se quita esa dependencia del `Cargo.toml`).
- Lo específico del sistema operativo (bloqueo del archivo y comprobación de permisos 0600, S-23) va aislado y marcado según la decisión 0002: módulo `platform/linux.rs` tras `#[cfg(target_os = "linux")]`, con `compile_error!` en otros sistemas.
- Cada regla S-xx que toque esta pieza tiene una prueba en `crates/clio-memory/tests/` que nombra su id.

## Modelo (resumen)

- **Registro (`operations`)**: solo se agrega. Campos: id monotónico, ámbito, tipo (ADD, UPDATE, NOOP, PROMOTE, ARCHIVE, REVERT), recuerdo, antes/después, actor, motivo de la decisión, momento. Es la fuente de verdad.
- **Proyección (`memories`)**: estado actual de cada recuerdo, derivado del registro, nunca editada a mano. Estados: activo, cuarentena, archivado, reemplazado. (El estado «en conflicto» llega con la pieza 3.)
- **Ámbitos, actores y cuotas**: tablas simples. El ámbito guarda el idioma; el actor guarda su cuota de propuestas por ventana de tiempo.
- **Índice de texto**: tabla FTS5 derivada de la proyección.
- El esquema lleva versión (`PRAGMA user_version`). Cambiarlo exige reproyectar desde el registro.

## Escritura (`propose`)

Entrada de `propose`: ámbito, actor, tipo, contenido, `topic_key` opcional, origen, procedencia (sesión, episodio, fuente) y, opcionalmente, `valid_from` y `valid_to` (por defecto, `valid_from` es la hora del `Clock` y no hay `valid_to`). «Sabido desde» siempre es la hora del `Clock` al escribir; el llamador no puede fijarla.

1. **Cuota del actor.** Si la superó, se rechaza. Toda propuesta cuenta contra la cuota, también las rechazadas por secreto; el contador guarda solo un número por actor y ventana, nunca contenido.
2. **Normalizar** el contenido y calcular su hash.
3. **Detector de secretos.** Si encuentra uno, se rechaza y no se escribe contenido en ningún lado (solo sube el contador de la cuota).
4. **Origen efectivo.** Lo declara el llamador, pero un actor de tipo agente o sesión no puede declarar origen «usuario»: se trata como «agente». Solo actores de tipo persona o importador pueden declarar otros orígenes.
5. **Reglas, en este orden** (la confianza se decide antes que la coincidencia):
   1. Hash idéntico a un recuerdo activo del ámbito → NOOP. Contra uno en cuarentena, archivado o reemplazado no hay coincidencia: se sigue con las reglas.
   2. ¿Debe ir a cuarentena? Origen externo, o tipo procedimental con origen que no sea «usuario» → cuarentena.
   3. Mismo `topic_key` (único por ámbito y tipo entre los activos) → UPDATE.
   4. Todo lo demás → ADD (no hay vecinos todavía).
6. **Una sola transacción:** se agrega la operación, se actualiza la proyección y el índice. Si algo falla, no queda nada.

UPDATE de un recuerdo con confianza suficiente crea un recuerdo nuevo, marca el viejo como «reemplazado» (con enlace al nuevo) y cierra su vigencia en el momento en que empieza la del nuevo. Así se conserva el pasado para las consultas con fecha.

Si una propuesta que coincide por `topic_key` debe ir a cuarentena (regla 2), entra en cuarentena como candidato con la marca «reemplazaría a mX». El recuerdo activo sigue activo y nada cambia hasta `promote`; al promoverlo se aplica el reemplazo. Así un origen no confiable nunca sustituye contenido de confianza por la puerta de atrás.

## Operaciones sobre lo guardado

Alta de ámbitos y actores: `create_scope` y `register_actor` explícitos, con el idioma del ámbito y, por actor, tipo (agente, sesión, persona, importador) y cuota (propuestas por ventana; por defecto 100 por hora, configurable).

Lecturas:
- `get(id)` devuelve un recuerdo en cualquier estado.
- `list(ámbito, estado, tipo)` lista por estado; así una persona encuentra lo que está en cuarentena para revisarlo.
- `history(id)` devuelve las operaciones de un recuerdo.
- `search(ámbito, texto, ...)` excluye la cuarentena y todo lo que no sea activo, salvo que se pida con `include_quarantine`.
- Las lecturas con fecha usan dos ejes opcionales: `as_of` (qué era cierto en esa fecha, sobre vigente desde/hasta) y `known_at` (qué sabía el sistema en esa fecha, sobre sabido desde). Sin ellos, devuelven el estado actual.

Escrituras: `promote` (cuarentena → activo, y aplica el reemplazo pendiente si lo hay), `archive`, `revert`.

**`revert`**: agrega una operación REVERT que restaura el estado `antes` de la operación deshecha. ADD → el recuerdo queda archivado (nunca se borra); UPDATE → el viejo vuelve a activo con su vigencia reabierta y el nuevo queda archivado; PROMOTE → vuelve a cuarentena; ARCHIVE → vuelve a su estado anterior; NOOP → nada que restaurar; REVERT de un REVERT → vuelve a aplicar la operación. «Sabido desde» no cambia al revertir. Una operación sola solo se deshace si el recuerdo sigue exactamente en el estado que esa operación dejó (equivale a «es la última sobre su recuerdo», y permite encadenar deshacer varias). Deshacer por actor o desde un punto recorre de la más nueva a la más vieja y deshace solo las operaciones que cumplen esa condición; las que están tapadas por una operación posterior de otro actor se omiten y se devuelven en una lista, sin deshacer nada de más.

Cada operación del registro guarda el estado completo `antes` y `después` que hace falta para esto.

## Decisiones que son del autor

Las del spec v0.2 de la sección 1 al 7, acotadas por este documento: un solo camino de escritura, el registro como verdad, un solo escritor, nada destructivo con dudas, API síncrona con un hilo escritor dedicado, `Store` y `Clock` como traits, PURGE en la v2, el daemon en la v2, el proyecto sigue en el monorepo hasta cerrar la v1 y entonces se separa a su propio repo.

## Decisiones tomadas por el asistente (revisar)

- **API síncrona**, con una conexión de escritura serializada por un `Mutex` y lecturas en conexiones de solo lectura. Sin runtime async en el crate.
- **Texto con raíz de palabra sin código inseguro.** Un tokenizador propio de FTS5 requeriría `unsafe`, que el workspace prohíbe. En su lugar, el contenido se reduce a raíces con `rust-stemmers` antes de insertarlo en una tabla FTS5 con el tokenizador `unicode61`, y la consulta se reduce igual. Si el idioma del ámbito no tiene stemmer, se indexa sin reducir. Esto se aparta de la letra del spec («tokenizador propio»).
- **Ids**: enteros que se muestran como `m12` (recuerdo) y `op34` (operación).
- **Tiempo**: segundos Unix en UTC, leídos siempre del `Clock`.
- **Normalización**: recorte, espacios colapsados y forma Unicode NFC. Hash SHA-256 del texto normalizado.
- **Detector de secretos**: patrones de prefijos conocidos (`sk-`, `ghp_`, `AKIA`, `xox`, cabeceras PEM, JWT) más cadenas largas de alta entropía. Módulo propio, con casos de prueba de falsos positivos y negativos.
- **Procedimental**: un procedimiento propuesto por un agente entra en cuarentena (S-26); uno propuesto por una persona entra activo.
- **`Store`** es el límite de persistencia (registro, proyección e índice de texto) y se define en este crate. En esta pieza solo hay una implementación, la de SQLite; el trait existe para que otra (por ejemplo Postgres) sea posible más adelante, no para usarse genéricamente en todo el código.
- **Orden de reglas, origen efectivo, candidato en cuarentena con reemplazo pendiente, reglas de `revert`, cuota por defecto y qué cuenta contra ella:** como están descritos arriba.
- **Bloqueo del archivo** con un bloqueo asesor (`flock`) sobre un archivo `.lock` junto a la base, detrás de `platform/linux.rs`. Si los permisos del archivo no son 0600, no abre (S-23).
- **Revisión solo por personas:** `promote`, `archive` y `revert*` exigen un actor de tipo persona. Un agente no puede deshacer una revisión humana.
- **Un activo por tema:** `promote` busca el dueño actual del `topic_key`. Si el candidato es más nuevo, lo reemplaza; si es más viejo, queda como hecho histórico cerrado donde empieza el actual; si empiezan a la vez, falla con `InvalidValidity`. Un UPDATE nunca alarga una vigencia que ya estaba cerrada.
- **Consultas con dos fechas:** un reemplazo con fecha retroactiva no cierra el hecho viejo para quien consulta con `known_at` anterior a haberlo sabido. Nombres de ámbitos y actores pasan por el detector de secretos.
- **Actualizaciones en el mismo segundo:** si el llamador no indica `valid_from`, la versión nueva empieza ahora; si la anterior también empezó en ese segundo, queda con vigencia de largo cero (`valid_to == valid_from`) y no se inventan fechas. Un `valid_from` explícito debe ser posterior al de la versión anterior, y uno anterior a un dueño con fecha futura se rechaza.
- **Memoria de trabajo**: el tipo existe, pero su expiración al cerrar la sesión llega cuando haya episodios y cola.
- **Distribución del código** (módulos de error, reloj, registro, proyección, reglas, secretos, texto, plataforma): sugerida, la fija quien planifique.

## Pruebas de aceptación que cubre esta pieza

Del spec: 1 (reconstruir la proyección), 2 (ADD y luego NOOP), 3 (secreto rechazado sin rastro), 4 (externo no sale hasta PROMOTE), 5 (aislamiento de ámbitos), 6 y 17 (consultas con fecha), 10 (REVERT), 15 (cuota y revertir por actor), 16 (segundo escritor). La 11 (PURGE) pasa a la v2.

Reglas de seguridad: S-18, S-23, S-25 (la parte de cuarentena y PROMOTE), S-26, S-27.

## Riesgos conocidos para las piezas siguientes

- `register_actor` no comprueba autoridad en el crate, a propósito: el crate confía en quien lo embebe. Quien puede registrar un actor de tipo persona puede promover, archivar, revertir y editar bloques. El daemon (v2) no debe exponer `register_actor`, `create_scope`, `archive` ni las operaciones de bloques a un agente; esa decisión pertenece al diseño del daemon.
- Cargar `sqlite-vec` es cargar una extensión nativa, y eso puede chocar con `unsafe_code = "forbid"`. Se resuelve en el diseño de la pieza 2, no aquí.
- Los nombres `topic_key` y la política de qué es «el mismo tema» son simples en esta pieza (igualdad exacta). Los conflictos entre recuerdos de temas parecidos llegan con los vecinos de la pieza 3.
