## clio-memory · spec v0.2

Un motor de memoria genérico y open source para agentes, en Rust y sobre un solo archivo SQLite. Corre en un VPS pequeño, en una laptop o embebido en otra app, sin GPU y sin servicios externos. No asume ningún agente, modelo ni proveedor concreto: Clio es su primer usuario, no su único.

- **Un archivo, nada más** SQLite; sin Redis, sin Postgres, sin GPU
- **El registro es la verdad** las tablas se reconstruyen desde las operaciones
- **Un solo escritor** todo cambio pasa por el gestor
- **Nada destructivo con dudas** todo es reversible; ante la duda, NOOP
- **Funciona sin LLM** Jev o un LLM son plugins, no requisitos
- **Recursos acotados** lo pesado corre cuando la máquina está ociosa

### Objetivos y no objetivos de la v1

- **Sí**: Guardar, recuperar, actualizar y deshacer recuerdos de cuatro tipos; búsqueda de texto y vectorial; cuarentena; vigencia; ámbitos aislados; colas de trabajo; decisor en cascada; crate, daemon y CLI; exportar e importar.
- **No, por ahora**: Grafo de conocimiento, ajuste fino, multiusuario con permisos finos, sincronización entre réplicas, interfaz web. Tampoco es un RAG de documentos: guarda hechos atómicos; archivos y URLs entran como referencias, no como contenido.

> Nota (2026-10-29): el daemon (HTTP y MCP) pasa a la v2, como dice la hoja de ruta. PURGE volvió a la v1 (nota del 2026-10-31): `Engine::purge` y `clio-memory purge`, ver §7 y §8.

### 1. Modelo de datos

| Objeto | Campos clave | Notas |
| --- | --- | --- |
| Ámbito | id, nombre, idioma, política de retención | Aislamiento duro: una consulta nunca cruza ámbitos salvo que se pida. El idioma elige el stemmer. |
| Recuerdo | id, ámbito, tipo {trabajo, episódico, semántico, procedimental}, contenido, `topic_key`, origen {usuario, agente, externo}, actor que lo propuso, procedencia (sesión, episodio, fuente), estado {activo, cuarentena, archivado, reemplazado}, **vigente desde / hasta** (cuándo fue cierto) y **sabido desde** (cuándo lo supo el sistema), reemplazado por, confianza, usos, último uso, señales de retroalimentación | Es una proyección del registro: nunca se edita a mano. Un hecho atómico por recuerdo. |
| Operación | id monotónico, ámbito, tipo {ADD, UPDATE, DELETE, NOOP, PROMOTE, ARCHIVE, REVERT, PURGE}, recuerdo, antes / después, decisión que la causó, momento | Solo se agrega. Es la fuente de verdad, la auditoría y la marcha atrás. |
| Bloque fijo | id, ámbito, nombre (identidad, reglas, preferencias), recuerdos en orden, presupuesto de tokens | Entra completo al iniciar cada sesión. Si se pasa del presupuesto, se recorta por prioridad, nunca se trunca a la mitad. |
| Actor | id, tipo {agente, sesión, persona, importador}, cuota de propuestas por ventana, estado | Permite frenar a un agente que inunda la memoria y deshacer todo lo de un actor de una vez. |
| Decisión | pregunta, opciones, elección, confianza, proveedor, razón opcional, modo {activo, sombra} | Permite comparar decisores con datos reales. |
| Episodio | id, ámbito, inicio / cierre, resumen, padre | Los resúmenes forman un árbol (episodio, día, semana). |
| Entidad | id, ámbito, tipo, nombre canónico, alias | Enlaza recuerdos que hablan de lo mismo. No es un grafo: una tabla y una relación. |
| Vector | recuerdo, modelo, dimensión, vector | Guardar el modelo permite cambiarlo y re-embeber sin mezclar espacios. |
| Trabajo | id, tipo, carga, prioridad, estado, intentos, `run_after`, `lease_until`, `dedupe_key` | La cola vive en el mismo archivo. |

### 2. Escritura: un solo camino

1. **Propuesta.** Cualquiera puede llamar a `propose` con un candidato y su actor; nadie escribe directo. Si el actor superó su cuota, se rechaza.
2. **Normalizar y filtrar.** Limpieza, hash del contenido y detector de secretos: si parece una llave o un token, se rechaza y no queda ni en el registro.
3. **Reglas.** Hash idéntico → NOOP; mismo `topic_key` → candidato a UPDATE; origen externo → cuarentena.
4. **Vecinos.** Si hay al menos un decisor registrado y un embedder, búsqueda de los recuerdos activos más parecidos del mismo ámbito (los que ya tienen vector).
5. **Decisor en cascada** (sección 3): las reglas deciden primero; después los decisores registrados pueden dejar el candidato como nuevo o pedir revisión humana (cuarentena).
6. **Parecidos pero distintos conviven.** Dos recuerdos que se parecen mucho pero dicen cosas distintas («vive en Medellín», «vive en Bogotá») se dejan como cosas distintas: ni se reemplazan ni se marcan en conflicto. Reemplazar es siempre explícito (mismo `topic_key`), nunca por similitud.
7. **Una sola transacción:** se agrega la operación, se actualiza la proyección y se encolan los trabajos que dispara. Si algo falla, no queda nada a medias.

Por defecto la escritura se hace al cerrar el episodio, en lote; la memoria de trabajo es la excepción y se escribe al momento.

### 3. El decisor: con Jev o sin él

```
trait Decider {
    fn decide(&self, q: &Question) -> Result<Option<Decision>>;  // None = no sé
}
struct Decision { choice: Choice, confidence: f32, reason: Option<String> }   // el proveedor es el nombre del decisor
enum Choice { Add, Review }   // un decisor nunca descarta, actualiza, archiva ni borra
```

| Orden | Proveedor | Viene incluido | Qué hace |
| --- | --- | --- | --- |
| 1 | Reglas | Sí | Casos seguros y deterministas. Siempre corren primero y pueden vetar. |
| 2 | Casi-duplicado por embeddings | Opcional (apagado por defecto) | Un vecino del mismo tipo con similitud muy alta → revisión humana. Nunca descarta ni reemplaza por similitud. |
| 3 | Clasificador externo (Jev u otro) | Plugin | Elige entre opciones cerradas con probabilidad. |
| 4 | LLM | Plugin | Cualquier API compatible con OpenAI o un modelo local. |
| 5 | Revisión humana | Sí | Un decisor puede pedirla: el candidato entra en cuarentena y una persona lo resuelve con `promote` o `archive`. |

- **Política sin plugins**: lo que las reglas no resuelven se agrega como algo distinto. DELETE no existe como decisión; se marca «reemplazado» o se archiva y se puede revertir.
- **Modo sombra**: Un decisor nuevo puede correr en paralelo sin actuar. Sus decisiones se guardan junto a las reales (tabla `decisions`, en la misma transacción que la operación) para medir si mejora antes de activarlo.

### 4. Lectura

1. **Dos búsquedas en paralelo:** FTS5 (BM25) y vecinos por vector.
2. **Fusión** por rangos recíprocos (RRF), con un pequeño impulso por recencia y frecuencia de uso.
3. **Filtros:** ámbito, estado activo (la cuarentena queda fuera salvo que se pida), vigencia en la fecha consultada, tipo.
4. **Presupuesto:** recorta a un máximo de tokens dado por quien consulta.
5. **Abstención:** si la mejor puntuación no supera el umbral, devuelve «sin resultados» en vez de rellenar.
6. **Parecidos distintos salen ambos:** si dos recuerdos parecidos dicen cosas distintas, `recall` devuelve los dos con su vigencia y procedencia; el agente decide cómo contarlo.
7. **Respuesta** con ids, procedencia y vigencia de cada recuerdo, para poder citarlos. El conteo de usos se actualiza con un trabajo, no en la lectura.

### 4b. Contexto y formato compacto

`context()` arma lo que entra al prompt al iniciar: primero los bloques fijos (identidad, reglas) hasta su presupuesto, luego lo que traiga `recall` hasta el suyo. Los dos presupuestos son separados, para que la identidad nunca se coma la ventana.

Las dos fechas se guardan siempre, pero al modelo solo se le envía lo necesario. El formato por defecto es una línea por hecho, sin JSON (las llaves y comillas de JSON gastan tokens sin aportar):

```
[m12] Vive en Medellín · desde 2026-03
[m40] Prefiere respuestas breves
[m51] Vive en Bogotá · desde 2026-05
```

«Sabido desde» solo aparece si la consulta es histórica («qué sabías en tal fecha»). Fechas en formato corto, ids cortos y nada de campos vacíos. Hay un formato JSON completo para programas, no para modelos.

### 5. Almacenamiento y modelo

- **SQLite**: Modo WAL, `rusqlite` con SQLite incluido. Lecturas concurrentes y un escritor serializado en una tarea dedicada.
- **Texto**: FTS5 con un tokenizador propio en Rust (`rust-stemmers`) para reducir a la raíz según el idioma del ámbito.
- **Vectores**: [sqlite-vec](https://github.com/asg017/sqlite-vec/releases) 0.1.9 (fuerza bruta, estable). Sus índices aproximados siguen en alfa; [vec1](https://sqlite.org/vec1/doc/trunk/doc/vec1.md) (IVF, v0.8) es la alternativa. Detrás del trait `VectorIndex`.
- **Embeddings**: Por defecto [potion-multilingual-128M](https://huggingface.co/minishlab/potion-multilingual-128m) con `model2vec-rs`: estático, sin GPU, 256 dimensiones, 101 idiomas. Opcional: un transformer pequeño vía ONNX. Detrás del trait `Embedder`.

- **Un solo dueño del archivo**: El primer proceso que abre el archivo toma un bloqueo y es el escritor. Los demás entran por su daemon o abren en solo lectura; si intentan escribir, reciben un error claro, no una base corrupta.
- **Reloj inyectable**: Todo lo que depende del tiempo (vigencia, olvido, colas) lee un trait `Clock`. En pruebas se fija la hora y el resultado es reproducible.

Cinco traits separan el núcleo de lo cambiable: `Store`, `VectorIndex`, `Embedder`, `Decider` y `Clock`. Un adaptador de Postgres sería otra implementación de `Store`, no un cambio de diseño.

### 6. Colas de trabajo

| Trabajo | Cuándo | Peso |
| --- | --- | --- |
| embed | tras ADD o UPDATE; en lotes | Ligero |
| summarize | al cerrar un episodio; sube por el árbol | Medio |
| consolidate | episódico → semántico, con la máquina ociosa | Pesado |
| dedupe | periódico, por ámbito | Pesado |
| decay | archiva lo que no se usa según la política del ámbito | Ligero |
| reembed | al cambiar de modelo | Pesado |
| touch | actualiza usos tras cada lectura | Ligero |
| feedback | aplica señales de útil o equivocado a la confianza y al olvido | Ligero |
| backup · vacuum | programados | Medio |

- **Eficientes**: Se encolan en la misma transacción que la operación (nunca se pierden); `dedupe_key` fusiona trabajos repetidos; los embeddings van en lotes; arrendamientos que vencen si un trabajador muere; reintentos con espera creciente y cola de errores.
- **Inteligentes**: Prioridades; lo pesado solo con la máquina ociosa; tope de hilos y de CPU configurable; pausa con batería; la consolidación espera a que cierre el episodio.

### 7. Ciclo de vida

- **Por tipo**: Trabajo: expira al cerrar la sesión. Episódico: se resume en árbol. Semántico: vale mientras esté vigente; un hecho nuevo cierra la vigencia del viejo. Procedimental: sube de confianza cada vez que funciona.
- **Cuarentena y promoción**: Lo externo entra en cuarentena y no aparece en lecturas normales. Pasa a activo solo con PROMOTE, que exige una verificación o una persona.
- **Olvidar no es borrar**: El olvido archiva. Borrar de verdad es PURGE: elimina el contenido también de las operaciones pasadas y deja una lápida. Necesario para proyectos que manejen datos personales.
- **Retroalimentación**: El agente o la persona marcan un recuerdo como útil o equivocado. Útil sube su confianza y lo protege del olvido; equivocado baja la confianza y, si se repite, lo manda a revisión. Es la señal más valiosa para saber si la memoria sirve.
- **Marcha atrás**: REVERT deshace una operación o todas desde un punto, agregando operaciones nuevas; nunca reescribe la historia (salvo PURGE).
- **PURGE, en concreto**: en una sola transacción borra el contenido del recuerdo de la tabla de recuerdos, del índice de texto, de las instantáneas (`before`/`after`) de todas las operaciones pasadas que lo tocaron (se vacían texto, hash, `topic_key`, sesión, episodio y fuente; quedan ids, estado, vigencia y los enlaces `replaced_by`/`supersedes`, para que la cadena no se rompa), del texto de las decisiones registradas de esas operaciones, y su vector, uso, retroalimentación y entradas de bloque. Agrega una operación PURGE sin cambios, que es la lápida (id, quién, cuándo, motivo; sin contenido ni hash). Después reescribe el archivo (`VACUUM`) y vacía el registro de escritura (WAL), porque SQLite deja copias del texto en páginas liberadas. Reconstruir la proyección respeta la lápida; REVERT de una operación anterior da `NotRevertible`; exportar no saca el contenido e importar rechaza un archivo que lleve una operación PURGE y todavía el contenido. **No alcanza** respaldos ni exportaciones hechos antes del PURGE, copias del archivo fuera de la máquina, ni restos que el sistema de archivos o el disco conserven por debajo de SQLite.

### 8. Interfaz

| Forma | Para qué | Operaciones |
| --- | --- | --- |
| Crate | embeber en otro programa en Rust | `context`, `propose`, `recall`, `explain`, `feedback`, `get`, `history`, `revert`, `review`, `promote`, `purge`, `scopes`, `actors`, `stats`, `doctor`, `export`, `import` (restaurar una exportación propia), `import_memories` (cargar recuerdos sueltos por `propose`) |
| Daemon | HTTP + JSON con token, y servidor MCP (`memory_recall`, `memory_propose`) para cualquier agente | ↑ |
| CLI | `remember`, inspeccionar, revisar, deshacer, `purge`, exportar, `doctor`; nunca `register_actor`, `create_scope` ni `set_retention` | ↑ |

`remember` (CLI: `clio-memory remember SCOPE TEXT... [--kind K] [--topic T] [--source S]`) es el atajo de una persona o de un agente para guardar un recuerdo: arma una `Proposal` y la manda por `propose`, el único camino de entrada, así que pasa por la cuota del actor, el detector de secretos y las reglas, igual que todo lo demás. Sin `--actor` el recuerdo es de la persona (`cli:$USER`, origen `User`); con `--actor NAME` es de un agente (origen `Agent`, cuota de agente, y un procedimiento propuesto por un agente espera en cuarentena). Imprime el resultado (`Add`, `Update` o `Noop`), el id del recuerdo, el de la operación y el estado. No existe como operación aparte del motor: en el crate es `propose`.

`purge` (crate: `Engine::purge(memory, actor, reason)`; CLI: `clio-memory purge ID [--reason TEXT] [--yes]`) solo la ejerce una persona, pide confirmación en la terminal (`--yes` la salta, para pruebas) y nunca se expone por MCP. Qué borra y qué no, en §7.

`explain` devuelve por qué salió cada recuerdo (puntaje de texto, de vector, de recencia y filtros aplicados). Sale de números que la búsqueda ya calculó: no usa un LLM ni gasta tokens, y solo se pide para depurar. Por MCP se expone leer, proponer y dar retroalimentación; promover, purgar y revertir quedan fuera de MCP por defecto, para que un agente engañado no pueda hacerlo. Lo mismo vale para `register_actor`, `create_scope`, `set_retention`, `archive` y las operaciones de bloques fijos (`create_block`, `block_set`): quien puede registrar un actor de tipo persona puede usar todos los poderes de revisión, así que ninguna interfaz que vea un agente debe exponerlas.

### 9. Operación y evaluación

- **Operación**: Respaldo en caliente con la API de backup de SQLite; exportación JSONL completa (operaciones incluidas); importadores (JSONL genérico y adaptadores, como Engram); métricas; migraciones de esquema reproyectando el registro. El esquema y el formato de exportación llevan versión con semver: un cambio que rompa exige versión mayor y un migrador.
- **Evaluación incluida**: Un arnés que corre un banco de preguntas con resultado conocido: recall@k, precisión de la abstención, tasa de NOOP y de revisión, y comparación de decisores en sombra. Además, corre sobre benchmarks públicos (LongMemEval, LoCoMo) para comparar con otros motores con números, no con promesas.

### 11. Telemetría y retención reutilizable

clio-memory se mide a sí mismo y, además, ofrece su motor de retención como pieza reutilizable: clio-trace lo usa para guardar trazas y métricas de todo Clio. Todo con OpenTelemetry, para que cualquier herramienta compatible pueda leerlo.

- **Extensión propia**: Feature `telemetry` del crate, desactivable: sin ella no se crea ninguna tabla ni se paga ningún costo. Con ella, cada operación emite tramos y métricas estándar (`tracing` + `tracing-opentelemetry`). clio-network trae la suya, con la misma forma.
- **Sin contenido**: Los tramos llevan ids, tamaños, tiempos y decisiones, nunca el texto de los recuerdos. Cada operación del registro guarda el `trace_id` que la causó. El crate depende solo de `tracing`: quien lo embebe conecta su contexto de OpenTelemetry con `Engine::with_trace_id_provider` y elige el suscriptor.

| Qué | Se mide |
| --- | --- |
| Tramos | `propose` (normalizar, secretos, reglas, vecinos, cada decisor, transacción), `recall` (texto, vector, fusión, filtros, abstención), `context`, cada trabajo de la cola |
| Métricas | latencia por operación; tamaño del archivo y del WAL; recuerdos por estado y ámbito; tasa de ADD, NOOP y revisión; decisiones por proveedor y confianza; profundidad de la cola y edad del trabajo más viejo; reintentos y cola de errores; tiempo de embedding por lote; abstenciones; uso del presupuesto de los bloques fijos |
| Eventos | migraciones, cambio de modelo y re-embebido, REVERT y PURGE, un segundo proceso rechazado por el bloqueo, respaldos |

### Retención: el dato crudo caduca, la métrica se queda

Un módulo `retention` en el mismo crate: dos trabajos más en la cola, `rollup` y `expire`. Antes de borrar un dato crudo, su información se resume por minuto, hora y día, y esos resúmenes se guardan mucho más tiempo. Va en un archivo aparte (por ejemplo `clio-trace.db`) para que la telemetría, que crece rápido, no infle la memoria.

| Dato | Vida por defecto | Nota |
| --- | --- | --- |
| Contenido de prompts | 7 días, solo si se activó | pasa antes por el detector de secretos |
| Tramos | 30 días | lo que ves en la cascada |
| Eventos del sistema | 90 días | estabilidad e incidentes |
| Resumen por hora | 1 año | conteos, sumas, costo, histogramas |
| Resumen por día | siempre | tendencias de uso y costo |

**Los percentiles no se promedian.** El p95 de un día no se saca promediando los p95 de cada hora. Los resúmenes guardan histogramas con cubetas fijas (o un sketch que se pueda combinar), y el p95 se calcula al leer. Así el p95 de un año sigue siendo correcto aunque los tramos crudos ya no existan.

### 10. Pruebas de aceptación de la v1

1. Borrar las tablas de proyección y reconstruirlas desde el registro da exactamente el mismo estado.
2. Proponer dos veces el mismo contenido produce un ADD y un NOOP.
3. Un texto con una llave de API se rechaza y no aparece en ninguna tabla ni en el registro.
4. Un recuerdo de origen externo no aparece en `recall` hasta un PROMOTE.
5. Una consulta en el ámbito A nunca devuelve recuerdos del ámbito B.
6. Consultar con fecha anterior devuelve el hecho vigente entonces, no el actual.
7. Sin ningún plugin, ninguna operación DELETE se ejecuta sola.
8. Matar el proceso a mitad de una escritura no deja operaciones sin sus trabajos ni trabajos sin su operación.
9. Diez actualizaciones seguidas del mismo recuerdo generan un solo trabajo de embedding.
10. REVERT de una operación devuelve el recuerdo al estado anterior y queda registrado.
11. PURGE elimina el contenido de todas las operaciones y deja la lápida.
12. Una consulta sin resultados relevantes devuelve «sin resultados», no los menos malos.
13. `context()` nunca supera el presupuesto de los bloques fijos y nunca corta un recuerdo a la mitad.
14. Dos recuerdos parecidos con contenido distinto conviven y `recall` devuelve ambos.
15. Un actor que supera su cuota recibe rechazo, y revertir por actor deshace todas sus operaciones.
16. Un segundo proceso que abre el mismo archivo no puede escribir y recibe un error claro.
17. Consultar «qué sabías el 1 de abril» no devuelve hechos aprendidos después, aunque su vigencia empiece antes.
18. Con el reloj fijado, dos corridas del olvido producen exactamente el mismo resultado.
19. Marcar un recuerdo como equivocado tres veces lo manda a revisión.
20. El formato compacto de diez recuerdos usa menos tokens que su versión JSON (umbral por fijar).
21. Con la feature `telemetry` apagada no existe ninguna tabla de telemetría y no se emite ningún tramo.
22. Ningún tramo ni evento contiene texto de un recuerdo.
23. Después de que caducan los tramos crudos, los totales y el p95 del resumen diario siguen dando lo mismo que antes.
24. Desde cualquier operación del registro se llega a la traza que la causó por su `trace_id`.
25. Con 50 000 recuerdos, `recall` responde en un tiempo objetivo medido en CPU modesta (umbral por fijar).

### Decisiones abiertas

| Decisión | Opciones | Qué la decide |
| --- | --- | --- |
| Nombre | clio-memory (decidido) | falta confirmar que esté libre en crates.io y GitHub |
| Licencia | MIT · Apache-2.0 · ambas | Apache-2.0 añade protección de patentes |
| Modelo por defecto | potion-multilingual-128M · uno más pequeño de la misma familia | peso real en disco y RAM, que no encontré publicado |
| Índice vectorial | sqlite-vec plano · sqlite-vec DiskANN · vec1 | cuándo salgan de alfa; medir a 50 000 y 500 000 recuerdos |
| Cifrado en reposo | ninguno · SQLCipher | coste en rendimiento y en compatibilidad de extensiones |
| API remota | HTTP + JSON · gRPC | facilidad para clientes en otros lenguajes |

### Hoja de ruta

- **v1**: PURGE, registro y proyección, escritura con reglas y heurísticas, lectura híbrida con abstención, ámbitos, cuarentena, dos fechas por hecho, bloques fijos y `context()`, formato compacto, actores y cuotas, dueño único del archivo, retroalimentación, `explain`, reloj inyectable, extensión de telemetría, colas, CLI y crate, exportar e importar con semver, pruebas de aceptación.
- **v2**: Daemon con HTTP y MCP, plugins de decisor (clasificador y LLM; el trait, el modo sombra y el registro de decisiones ya están en la v1), consolidación y resúmenes en árbol, entidades, arnés de evaluación con LongMemEval y LoCoMo, módulo `retention` reutilizable por clio-trace.
- **Después**: Índice vectorial aproximado, adaptador de Postgres, cifrado, sincronización entre réplicas, interfaz web de revisión.
