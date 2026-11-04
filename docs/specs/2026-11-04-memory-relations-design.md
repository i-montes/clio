# Relaciones entre recuerdos: que el modelo pueda recorrer la memoria

Fecha: 2026-11-04. Repositorios: `i-montes/clio-memory` (motor, MCP para Claude Code) y Clio (Pi, Jev).

## Para qué

Iván lo pidió así:
- las relaciones no las crea solo Jev, que es opcional («un commodity»);
- sin Jev, «la relación se vuelve obligatoria para que las memorias no vivan sueltas»;
- todo automático, «sin que lo tenga que hacer el modelo»;
- que el modelo traiga un recuerdo, vea con qué se relaciona y pueda «explorar esas relaciones hasta pensar que ya tiene suficiente información»;
- algo que funcione de verdad y sea útil, no algo superficial;
- primero una prueba.

## Lo que mostró la prueba

Está en `clio-memory/bench/relations/results.md`, con datos inventados.

- **Guardar relaciones automáticas sin un juez que entienda da poco.** Al vecino más cercano por vectores acierta el 45 % y sube lo alcanzado de 72 % a 79 %. Los jueces locales que se probaron (un reranker y Qwen3-1.7B) no lo mejoran.
- **Mostrar con cada recuerdo sus 5 vecinos y dejar que el modelo lector elija funciona.** Un agente Claude real, a ciegas, alcanzó el 89 % de lo necesario (búsqueda sola: 72 %). Completó el 73 % de las tareas (búsqueda sola: 33 %), con 3,6 lecturas de media en vez de 6.
  - El lector ya entiende su pregunta, así que no hace falta otro juez.
  - Ningún recuerdo con vector queda suelto.
- **Las contradicciones son el vecino más cercano**, con similitud ≥ 0,88 en Granite. Por encima de ese umbral casi todo lo que aparece vale la pena verlo.

## Objetivos

1. **Vecinos a la vista.** Todo recuerdo que el modelo lee trae sus vecinos más cercanos. Se calculan al leer y no se guardan.
2. **Avisar al guardar.** Si lo nuevo es casi igual a algo que ya existe, decirlo en la respuesta, para que el modelo corrija en vez de duplicar o dejar una contradicción.
3. **Relaciones con tipo cuando hay juez.** Jev, en Clio, juzga los vecinos (no solo el primero) y guarda «contradice», «caso concreto», «compatible» o «relacionada». Se muestran antes que los vecinos sin tipo.
4. **Que las relaciones no se pierdan** cuando un recuerdo se corrige o se une con otro.
5. **Lo mismo en Claude Code (MCP) y en Clio (Pi).** La lógica va en la librería: los vecinos, copiar relaciones, el umbral por modelo y el texto de la línea y del aviso. Cada capa de herramientas solo la muestra.

## Lo que no se hace

- No se guardan relaciones automáticas sin juez. La prueba muestra que dan poco, y crearían ruido que después hay que mantener.
- No se agrega un modelo local de lenguaje como juez: los pequeños no sirven y los grandes pesan varios GB.
- No hay herramienta para que el modelo cree relaciones a mano (Iván no lo quiere como tarea del modelo).
- No cambia el orden de la búsqueda. Solo cambia lo que acompaña a cada resultado.

## Lo que sería un fracaso aunque funcione

- Que los vecinos llenen el contexto: líneas largas en cada resultado de búsqueda, o en `memory_context`.
- Que se muestren vecinos que la sesión no puede leer (olvidados, archivados, reemplazados, o de otra parte que no le toca), o que sus textos se cuelen sin pasar por los mismos filtros que una lectura normal.
- Que un recuerdo de origen `externo` llegue como vecino sin su marca.
- Que guardar se vuelva lento: el aviso usa el vector que ya se calcula al guardar.

## Cómo funciona

### Vecinos (motor y herramientas)

- **Motor:** `Engine::neighbors(id, scopes, k) -> Vec<(MemoryRow, f32)>`. Toma el vector del recuerdo con el modelo actual y busca los más cercanos con el índice que ya existe (`VectorIndex::nearest`). Lo hace en los ámbitos que la sesión lee y con las mismas reglas de visibilidad que la búsqueda: activos, vigentes, sin el propio recuerdo.
- **Sin vector:** no hay modelo, o el recuerdo todavía no tiene vector de este modelo. Los vecinos salen de la búsqueda por texto, usando el texto del recuerdo como consulta. Así nunca queda suelto.
- **Qué se muestra:**
  - `memory_get(id)` termina con la línea `Cerca: m28 «título o comienzo» · m29 «…» · …`, hasta 5 vecinos.
  - Las relaciones con tipo van primero, en las notas de siempre (`contradice a m61`).
  - Un vecino con origen `externo` lleva su marca.
  - Cada título o comienzo pasa por la misma limpieza que el resto de la salida (`clean_text`) y se corta a ~60 caracteres.
- **`memory_search`:** cada resultado lleva `cerca: m28, m29, m5` (3 ids, sin texto).
- **`memory_context`:** no cambia. Ya tiene su presupuesto de tokens, y para explorar está `memory_get`.
- **Topes:** no cambian. Las respuestas siguen limitadas a 4096 bytes, y la sesión a 20 lecturas y 24 KiB (en Clio es un requisito de seguridad, S-47). Por eso `memory_get` sigue abriendo un recuerdo por llamada (decisión mía). La prueba alcanzó lo necesario con 3,6 lecturas de media, dentro de esos topes.
- **Sesión contaminada:** los comienzos de texto de los vecinos cuentan como memoria devuelta. Siguen la regla que ya existe: leer recuerdos que no son de la persona contamina la sesión, y eso se dejó como está.
- **Instrucciones:** el servidor MCP y la skill dicen, en una línea, que cada recuerdo trae sus vecinos, que los abra con `memory_get` cuando sirvan a lo que busca, y que pare cuando tenga suficiente.

### Aviso al guardar

- Después de guardar y calcular el vector (ya ocurre desde v0.2.2), si el vecino más cercano supera el umbral del modelo, la respuesta de `memory_save` dice: `muy parecida a m5 «…»: si la corrige o la contradice, usa memory_update o memory_consolidate`.
- **Umbral por modelo:** `granite-embedding-311m-multilingual-r2@384` usa 0,88. Para potion u otro modelo sin umbral medido no se avisa.
- No se guarda nada; es solo texto en la respuesta.

### Relaciones con tipo (Jev, en Clio)

- **Hoy:** Jev juzga solo el recuerdo más parecido, con la búsqueda híbrida.
- **Después:** en un segundo paso, después de guardar y calcular el vector (los vecinos solo existen entonces), juzga los 3 vecinos más cercanos (los mismos de `neighbors`) y guarda cada relación con confianza suficiente, con su `by = "jev"` de siempre. Respeta el presupuesto diario de Jev; si no responde, no se guarda nada y todo sigue igual.
- **Antes de cambiar Jev, se mide:** se corre Jev sobre los pares candidatos de `bench/relations` (tipo de relación frente a `pairs.jsonl`, y sobre todo las 3 contradicciones). Hace falta que Iván abra el baúl. Criterio (decisión mía), por tipo:
  - «contradice» se guarda solo si Jev reconoce al menos 2 de las 3 contradicciones y no marca como contradicción ningún par que no lo sea;
  - los demás tipos, solo si su precisión es ≥ 80 %;
  - lo que no pase se queda como vecino sin tipo.

  El resultado y lo decidido quedan en `results.md` y se le muestran a Iván. Si el baúl no está abierto cuando toque, esta parte espera y lo demás sale igual.

### Las relaciones siguen al recuerdo

- Cuando `memory_update` reemplaza un recuerdo (m5 → m17), sus relaciones con tipo se **copian** al nuevo. Lo mismo al consolidar: las de los recuerdos unidos se copian al resultado, sin duplicados ni relaciones de un recuerdo consigo mismo. Se conservan `by` y `confidence`.
- **«contradice» no se copia** (decisión mía): corregir suele ser resolver la contradicción, y si sigue, el aviso al guardar o Jev la vuelven a señalar.
- **Se copian, no se mueven.** Así, si `memory_revert` deshace la corrección, el recuerdo viejo vuelve con sus relaciones (las relaciones no están en el registro de cambios).
- Copiarlas es una función del motor, `Engine::copy_relations(from, to)`, que llaman las dos capas al corregir y al consolidar (consolidar no es una operación del motor).

## Pruebas que deben pasar

- **Motor:**
  - los vecinos excluyen el propio recuerdo y los inactivos, reemplazados, olvidados y de ámbitos no leídos;
  - sin vector, salen de la búsqueda por texto;
  - las relaciones pasan al recuerdo nuevo al corregir y al consolidar.
- **Herramientas (MCP y Clio):**
  - `memory_get` muestra la línea `Cerca:`; los vecinos `externo` llevan su marca;
  - un texto con secuencias de terminal o un secreto en un vecino sale igual de limpio que en una lectura;
  - la búsqueda lleva `cerca:` con 3 ids;
  - los topes de 4096 bytes por respuesta y de 20 lecturas / 24 KiB por sesión se cumplen con los vecinos incluidos;
  - un vecino que no es de la persona contamina la sesión como cualquier recuerdo devuelto.
- **Aviso:** con un embedder de prueba, un guardado casi igual a otro trae el aviso y uno distinto no; sin umbral para el modelo, no se avisa.
- **Benchmark:** `bench/relations/run.py` queda en el repositorio. La prueba con el agente lector se documenta como procedimiento en `results.md` (no corre en la CI).
- **En la máquina de Iván:** desde una sesión nueva de Claude Code, `memory_get` de un recuerdo real muestra vecinos que tienen sentido, y Clio muestra lo mismo.

## Lo que queda al constructor

- Medir con Granite en Rust (ONNX) la similitud de los pares de contradicción del benchmark, y ajustar el umbral de 0,88 si difiere del de Python.
- Cómo pasar los ámbitos de lectura de la sesión a `neighbors` (la librería ya los conoce en `agent::Memory`).
- El formato exacto de las líneas, dentro de los límites de arriba.
- Si los vecinos se calculan en una sola consulta para todos los resultados de una búsqueda.
- La versión de clio-memory (v0.4.0, porque cambia la salida de las herramientas) y cómo Clio la fija (por commit, como hoy).
