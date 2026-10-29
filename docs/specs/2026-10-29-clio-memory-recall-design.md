# clio-memory · vectores y lectura híbrida (pieza 2 de la v1)

Diseño de la segunda pieza de `clio-memory`. Se apoya en el núcleo (`docs/specs/2026-10-29-clio-memory-core-design.md`) y en el spec completo (`docs/specs/clio-memory-spec-v0.2.md`, secciones 4, 4b y 5). Orden de las piezas de la v1: 1 núcleo (hecha) · **2 vectores y lectura híbrida (este documento)** · 3 decisor en cascada y conflictos · 4 colas de trabajo · 5 CLI, exportar e importar.

## Intención

Que un agente pueda pedirle a la memoria lo que necesita saber y recibir lo relevante, con su procedencia y vigencia, en pocos tokens, y que la memoria diga «no sé» cuando no tiene nada relevante en vez de rellenar. La búsqueda mezcla texto (la del núcleo) y significado (vectores).

## Objetivos

- `Embedder`: convierte texto en vectores. El crate trae una implementación con `model2vec-rs` y modelos estáticos, sin GPU.
- Vectores guardados en el mismo archivo SQLite, y búsqueda vectorial exacta detrás del trait `VectorIndex`.
- `recall`: texto + vectores, fusionados por rangos recíprocos (RRF), con filtros, impulso por recencia, presupuesto de tokens y abstención.
- `context()`: bloques fijos (identidad, reglas, preferencias) y resultados de `recall`, con presupuestos separados, en formato compacto de una línea por hecho (y un formato JSON completo para programas).
- Cada resultado de `recall` trae el desglose de su puntaje (texto, vector, recencia): eso es `explain`, sin LLM.

## No objetivos de esta pieza

Decisor, vecinos y conflictos (pieza 3). Cola de trabajo, trabajo `embed` automático, conteo de usos (`touch`) y retroalimentación (pieza 4). CLI, exportar e importar (pieza 5). PURGE e índice aproximado (v2 / después). Descargar modelos de internet.

## Anti-objetivos

- Que `propose` dependa del modelo de embeddings o se vuelva lento por él.
- Que una lectura devuelva recuerdos de otro ámbito, en cuarentena (sin pedirlo) o fuera de vigencia (sin pedirlo): los filtros son los mismos que ya usa la búsqueda de texto.
- Mezclar vectores de modelos distintos en una misma comparación.
- Rellenar con «los menos malos» cuando nada es relevante.
- Que el crate abra conexiones a internet. Cargar un modelo es leer un directorio local; descargarlo es trabajo de la aplicación.
- Que un vector exponga más que el texto: viven en el mismo archivo, con los mismos permisos (S-24).

## Restricciones

- Las del núcleo: `unsafe_code = "forbid"`, sin async, tiempos solo desde `Clock`, decisión 0002 para lo específico del sistema.
- Búsqueda vectorial exacta, en Rust seguro: se recorren las filas ya filtradas por ámbito y estado, se calcula el producto punto directamente sobre los bytes y se guardan los k mejores. Los vectores se guardan normalizados (producto punto = coseno). Medido en una prueba desechable (Ryzen 7 5700X, 256 dimensiones, archivo en caché): 25 ms con 50 000 vectores, 90 ms con 200 000; leer las filas es el 85 % del tiempo. No se usa `sqlite-vec` (exige `unsafe`) ni HNSW (no hace falta por debajo de unos 500 000 por ámbito).
- Los vectores son datos derivados: no pasan por el registro de operaciones, y `rebuild_projection` no los toca (el contenido de un recuerdo nunca cambia: un UPDATE crea otro recuerdo).
- La integración con `model2vec-rs` va detrás de una feature `model2vec`, apagada por defecto, para que el crate compile ligero y las pruebas no necesiten un modelo.

## Escritura de vectores

- Tabla `vectors(memory, model, dim, vec)`, clave `(memory, model)`. El `model` es el identificador que da el `Embedder`; guardarlo permite cambiar de modelo sin mezclar espacios.
- `Engine::embed_pending(limit)` calcula y guarda los vectores de los recuerdos que no tienen uno para el modelo actual y no están archivados (activos, reemplazados y en cuarentena), hasta `limit`, en lotes. Devuelve cuántos guardó. Es idempotente. Cambiar de modelo hace que todos queden pendientes: así queda resuelto el re-embebido. La cola de la pieza 4 solo tendrá que llamarlo.
- El cálculo del embedding no mantiene tomado el candado de escritura; solo el guardado lo toma.
- Un vector con dimensión distinta a la del modelo, o todo en ceros, se rechaza con un error claro.
- `propose` no calcula nada. Hasta que un recuerdo tenga vector, `recall` lo encuentra solo por texto.

## Lectura (`recall`)

1. Texto (FTS5 del núcleo) y vectores (consulta embebida con el mismo `Embedder`), cada uno con su top-N, con los mismos filtros: ámbito, estado (la cuarentena solo con `include_quarantine`), `as_of` y `known_at`, tipo.
2. Fusión RRF (constante 60) sobre los candidatos elegibles, con un pequeño impulso por recencia. El impulso por frecuencia de uso llega con `touch` (pieza 4).
3. **Abstención.** Un candidato es elegible si su coseno llega al umbral vectorial, o si su coincidencia de texto contiene al menos la fracción mínima de las palabras de la consulta (cobertura, tras reducir a raíces). No se usa el puntaje BM25 como umbral: depende del tamaño del corpus (casi cero con uno o dos recuerdos) y puntúa palabras comunes como «de» como si importaran. BM25 solo ordena. Si ninguno lo es, el resultado es «sin resultados» (`abstained = true`). Los umbrales son parámetros de `RecallOpts`, con valores por defecto provisionales hasta tener el arnés de evaluación (v2).
4. Presupuesto: recorta a un máximo de tokens dado por quien consulta, sin cortar un recuerdo a la mitad.
5. Cada resultado trae: recuerdo (con id, procedencia y vigencia), puntaje final y su desglose (texto, vector, recencia).
6. Sin `Embedder` configurado, `recall` funciona solo con texto.

## `context()` y formato

- **Bloques fijos**: tabla `blocks` (ámbito, nombre, presupuesto de tokens) y tabla `block_memories` (bloque, recuerdo, posición). Son configuración, como los ámbitos y los actores: no pasan por el registro. Solo entran recuerdos activos en el momento de armar el contexto.
- **Edición de bloques**: `create_block(scope, name, budget_tokens)`, `block_set(block, memories)` y `block_get(block)`. Solo una persona puede editarlos (como `promote`), el recuerdo debe ser del mismo ámbito que el bloque y estar **activo** en el momento de añadirlo (nunca en cuarentena). Un bloque entra siempre en el prompt sin pasar por los filtros de `recall`, por eso la regla es estricta.
- **Un bloque sigue a las actualizaciones**: la entrada guarda un id, y al armar el contexto se sigue la cadena `replaced_by` hasta el recuerdo activo actual; si la cadena termina en uno archivado o no hay activo, la entrada se omite. Así «Vive en Medellín» pasa a «Vive en Bogotá» sin editar el bloque.
- `context(scope, ContextOpts)` arma primero los bloques fijos (cada uno hasta su presupuesto, en orden de posición; si el siguiente no cabe, se corta ahí) y luego los resultados de `recall` hasta el presupuesto de la consulta. Los dos presupuestos son independientes.
- **Formato compacto** (por defecto): una línea por hecho, `[m12] Vive en Medellín · desde 2026-03`. Fechas cortas (año-mes), ids cortos, sin campos vacíos. «Desde» se omite cuando `valid_from` es igual a «sabido desde» (el sistema lo supo cuando empezó a ser cierto, así que la fecha no aporta). Un recuerdo en cuarentena (solo si se pidió) lleva la marca `⚠ cuarentena`; no se muestran origen ni actor, que están en el formato JSON. «Sabido desde» solo aparece si la consulta es histórica (`known_at`); «hasta» solo si la vigencia está cerrada. **Formato JSON** completo para programas.
- **Presupuesto**: se cuenta sobre la línea completa ya formateada (id, texto y fechas), no solo sobre el contenido. Tanto en los bloques como en `recall` se incluyen los resultados en orden y se corta en el primero que no cabe; no se saltan para meter otros más pequeños, porque eso rompería el orden de relevancia. Si ni el primero cabe, el resultado queda vacío con `abstained = false` (`abstained` significa solo «nada relevante»).
- Conteo de tokens: un trait `TokenCounter` de un solo método, con una implementación aproximada por defecto (caracteres entre 4, redondeado hacia arriba) que se puede reemplazar por un tokenizador real.

## Decisiones tomadas por el asistente (revisar)

- **`embed_pending` por pull**, en vez de calcular en `propose` (acordado en la conversación) o con una cola (pieza 4).
- **`model2vec` como feature apagada por defecto**, aunque el spec lo nombra «por defecto». Se enciende por defecto al cerrar la v1. Carga solo desde un directorio local (`Model2VecEmbedder::from_dir`); no descarga.
- **Los vectores y los bloques no pasan por el registro** (son derivados y configuración respectivamente).
- **Pruebas con un `Embedder` falso** y determinista (bolsa de palabras con hash, normalizada) definido en las pruebas, no en la API pública. Una prueba `#[ignore]` con el modelo real queda para cuando haya uno en disco.
- **`explain` como desglose dentro de cada resultado de `recall`**, no como operación aparte.
- **Los vectores se guardan en f32** (1 KB por recuerdo con 256 dimensiones). int8, una copia en RAM y HNSW son las palancas si las medidas lo piden.
- **Fechas del formato compacto en año-mes** (como en el ejemplo del spec), calculadas a mano desde los segundos Unix, sin dependencia nueva.
- **Prueba de rendimiento (aceptación 25)**: `#[ignore]`, 50 000 recuerdos, `recall` por debajo de 150 ms en la máquina de desarrollo (umbral mío; el spec dice «por fijar»).
- **Esquema**: `vectors`, `blocks` y `block_memories` se añaden al DDL de la versión 1 sin subir `VERSION`, porque no existe ningún archivo real creado con este crate (no hay CLI ni lanzamiento). Costo si me equivoco: un archivo de desarrollo viejo falla con «no such table»; la salida es borrarlo o subir la versión.
- **Contenido largo**: esta pieza no añade el tope de tamaño de `propose` (sigue diferido). `embed_pending` pasa el contenido tal cual al `Embedder`; truncar al límite del modelo es cosa de cada implementación (`model2vec` ya lo hace).
- **Umbrales por defecto inclinados a abstenerse**: ante la duda, «sin resultados» antes que ruido. Las pruebas del banco deben pasar con el embedder falso; calibrar contra un modelo real es trabajo del arnés de evaluación (v2).
- **Dónde se inyectan** `Embedder` y `TokenCounter`: métodos `with_embedder` y `with_token_counter` sobre `Engine`; `ContextOpts` lleva el texto de la consulta (opcional: sin él solo salen los bloques). La fórmula de recencia (decaimiento suave sobre `known_since`) la fija quien planifique.
- **Cobertura sin palabras vacías** (tras la review): las palabras funcionales de los idiomas conocidos (es, en) no cuentan en la cobertura; una consulta que es solo palabras vacías no usa el texto.
- **Textos que el modelo no puede representar** (por ejemplo solo emojis): `embed_pending` los marca como no embebibles y los salta, para que no bloqueen al resto; si todo un lote es así, una palabra de control distingue un embedder roto (error) de textos genuinamente irrepresentables (se saltan). Una consulta no representable cae a solo texto.
- **Un bloque solo sigue actualizaciones de personas**: si el recuerdo de un bloque se reemplaza por una versión cuyo origen no es `User`, la entrada se omite hasta que una persona edite el bloque. Evita que un agente reescriba lo que siempre entra al prompt.
- **El id del modelo `model2vec` incluye una huella** de los pesos, el tokenizador y la configuración, para que sustituir los archivos cuente como otro espacio vectorial.
- **Si un bloque fijo no cabe en su presupuesto**, se omiten los últimos por posición (prioridad = orden).
- **Distribución del código**, nombres de tipos y firmas exactas: las fija quien planifique.

## Pruebas de aceptación que cubre esta pieza

Del spec: 4 y 5 (ahora también por `recall`), 6 y 17 (`recall` con fechas), 12 (abstención), 13 (`context()` no pasa el presupuesto y no corta recuerdos), 20 (compacto usa menos tokens que JSON; umbral: estrictamente menor con diez recuerdos), 25 (rendimiento, ignorada). Además: `embed_pending` es idempotente; cambiar de modelo vuelve a dejar pendientes todos los recuerdos; sin vector, el recuerdo sale por texto; no se mezclan modelos; vector de dimensión incorrecta o en ceros se rechaza.

Reglas de seguridad: S-24 (los vectores viven en el mismo archivo con los mismos permisos 0600; PURGE los borrará cuando llegue), S-18 (el texto que llega al `Embedder` ya pasó por el detector de secretos de `propose`).

## Riesgos conocidos

- Los umbrales de abstención son provisionales: con pocos recuerdos, el puntaje de texto (BM25) de palabras comunes es poco fiable. El plan debe incluir un banco de prueba pequeño con casos que deben abstenerse y casos que no.
- `model2vec-rs` arrastra dependencias (tokenizador, safetensors) cuyas licencias hay que revisar con `cargo deny` antes de encender la feature por defecto. `cargo deny` no está instalado en esta máquina.
- El núcleo tiene hallazgos de la review sin resolver (tope de tamaño de contenido, actualización dentro del mismo segundo, apertura en solo lectura de un archivo inexistente). No bloquean esta pieza, pero el tope de tamaño afecta también a lo que se embebe.
