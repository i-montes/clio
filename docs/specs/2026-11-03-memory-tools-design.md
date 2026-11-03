# Herramientas de memoria para el agente (Pi ↔ `clio-memory`)

Diseño escrito el 2026-11-03, a petición de la persona: «una tool que conecte a clio agent con clio memory» y «hazlo de la mejor forma pensando en la seguridad e integridad de los datos». Se apoya en el spec de la memoria (`docs/specs/clio-memory-spec-v0.2.md`), en `docs/security.md` (S-02, S-05, S-08, S-18, S-25, S-26, S-27, S-36, S-44, S-47) y en `docs/threat-model.md` («La memoria como superficie» y «La pila con Pi»).

## Intención

Hoy el chat con Pi, que es el camino principal, **no usa la memoria**: la guardia tiene un `memoryLayer()` vacío y `memory_link` solo lo usan `clio run` (el bucle propio) y la pantalla de memoria. El agente no recuerda nada entre sesiones ni puede consultar lo que la persona le pidió recordar. Se quiere que pueda **consultar** la memoria cuando la necesite y, más tarde, **proponer** recuerdos, sin abrir una vía por la que un texto hostil se vuelva instrucción permanente ni por la que el agente altere lo que la persona escribió.

## Objetivos

1. El agente puede buscar y leer recuerdos **activos** de la persona (`memory_search`, `memory_get`).
2. Lo que vuelve llega al modelo como datos marcados, limpio de secretos y acotado en tamaño y en cantidad.
3. Cada uso queda en un registro de hechos (sin el contenido).
4. (Etapa 2) El agente puede proponer un recuerdo, que **siempre** entra en cuarentena y lo decide la persona.
5. Si la memoria no existe, está bloqueada o falla, la herramienta falla sola; Pi y la sesión siguen.

## No objetivos

- Que el agente promueva, archive, revierta, purgue, escriba con origen de persona o cambie bloques fijos. Nada de eso se expone, como dice S-25.
- Recuerdo automático al empezar una sesión de Pi ni registro episódico al cerrarla (paridad con `clio run`): es otra pieza, con su propio diseño.
- Memoria compartida entre personas, otros ámbitos que el ámbito `clio`, ni acceso a cuarentena o archivados.
- Cambiar el motor de `clio-memory` (salvo lo que pida la etapa 2, ver «Antes de la etapa 2»).

## Anti-objetivos (fallaría aunque «funcione»)

- Que una búsqueda sirva para vaciar la memoria entera hacia el proveedor del modelo.
- Que un recuerdo escrito por un agente o por contenido externo se lea como si lo hubiera dicho la persona.
- Que leer un recuerdo marque la sesión como contaminada **siempre** (cansa y se ignora: S-36) o **nunca** (S-05 pierde sentido).
- Que escribir bloquee a la persona (`clio memory remember`, la TUI) por un cerrojo que se queda tomado.
- Que el contenido de un recuerdo, o la consulta del modelo, acabe en el registro de efectos, la traza o un log (S-18).
- Que un fallo de la memoria rompa la herramienta de otro modo que con un error normal.

## Alcance de este diseño

**El plan que sale de este documento es la etapa 1 solamente: lectura.** `memory_propose`, la regla de escritura, el caso de ataque de escritura, el método `propose` del trait `MemoryPort`, la reescritura de S-27 y la herramienta número 9 de la guardia pertenecen a la etapa 2, que necesita su propio diseño corto después de que exista el tope de la cuarentena. Aparecen aquí para que se vea hacia dónde crece la interfaz y por qué la etapa 1 no deja puertas abiertas, pero **no se declaran, no se registran y no existen en el código de la etapa 1**: sin entrada en la pasarela, la operación se niega (S-02). El tope de la cuarentena es una tarea aparte (en `clio-memory`), no parte de este plan.

## Decisiones de la persona

- Existe una herramienta que conecta al agente con la memoria. (Dicho el 2026-11-03.)
- Se hace «de la mejor forma pensando en la seguridad e integridad de los datos»: ante la duda, lo más restrictivo, y por etapas.

## Mis decisiones (la persona las puede cambiar)

1. **Servidas por el servicio de herramientas (`clio-toolhost`), no por Pi ni por la guardia.** Pi corre en una jaula que no ve los datos de Clio; el servicio es quien ya habla con el vault y decide cada llamada con la pasarela. La guardia solo registra las herramientas y reenvía la petición, como con `read` o `bash`.
2. **Con la biblioteca, no ejecutando la CLI `clio memory` en segundo plano.** La persona pidió «por medio de esas tools que usarán de fondo la cli». Lo entiendo como «la misma lógica que la CLI»: `cli/memory.rs` es una capa fina sobre `MemoryLink`, y las herramientas usan esa misma capa (`MemoryLink`) en el proceso del servicio. Lanzar `clio memory search --json` por cada llamada se descartó: un proceso nuevo por llamada que (con vectores) carga un modelo de decenas de MB en segundos, argumentos que escapar, y una salida de texto que volver a leer. Si se prefiere la CLI, el contrato de las herramientas no cambia; cambia solo el interior.
3. **Dos etapas.** Etapa 1: solo lectura. Etapa 2: `memory_propose`, que **no se construye hasta que exista el tope de la cuarentena** (ver «Antes de la etapa 2»). La lectura no puede dañar los datos; la escritura sí puede ahogar la revisión de la persona.
4. **Orígenes y contaminación.** Un resultado marca la sesión como contaminada si trae al menos un recuerdo que no sea de la persona (origen `agent` o `external`); si todos son de la persona no la marca. Siempre llega envuelto como datos (`Source::Memory`, que ya existe).
5. **Solo en modo lectura para la etapa 1:** el servicio abre `memory.db` con `OpenMode::ReadOnly`. Un fallo del código no puede escribir.
6. **El presupuesto por sesión sobrevive a un reinicio.** Se guarda en `<carpeta del run>/toolhost/memory-budget.json` (0600, solo contadores: llamadas y bytes) y se actualiza en cada llamada. Así reabrir la sesión o reiniciar el servicio no devuelve las 20 llamadas ni los 24 KiB. No cambia el protocolo ni el archivo de sesión de Pi. Cuenta **toda llamada que la pasarela dejó pasar**, también la que falla por validación o por la memoria: un bucle de llamadas malas no sirve para sondear gratis, y solo gasta el presupuesto de su propia sesión. Los bytes cuentan lo devuelto.
7. **`external` y contaminación son dos cosas.** `external` en la respuesta sigue significando «texto de fuera, envuélvelo como dato». Hoy el servicio marca la sesión siempre que una respuesta es `external`; eso pasa a decidirse por herramienta: los archivos y los comandos marcan siempre, como ahora; la memoria marca solo cuando devuelve algún recuerdo que no es de la persona (paso 7). El filtro final de la guardia (`tool_result`) trata el resultado de memoria como el de cualquier otra herramienta: limpia y envuelve; no decide la contaminación (la decide el servicio y viaja en `tainted`).
8. **La etapa 1 no registra señales de uso.** Al abrir en solo lectura, el motor no apunta qué recuerdos salieron en una búsqueda (esas señales alimentan la clasificación y el olvido). Es la lectura más segura y queda dicho: las estadísticas de uso solo reflejan lo que `clio run` y la persona recuperan, no lo que el agente consultó por herramienta. Si más adelante se quiere medir, es una decisión aparte y requiere escribir.
9. **`clio-agent@5` es el prompt del camino Pi desde esta etapa.** Es el que `pi::session` embebe y exporta a la guardia (`PROMPT_NAME` y el archivo van juntos desde el mismo binario, así que no hay desajuste posible entre ellos). `clio-agent@4` queda en el repositorio como historial (inmutable) y no se usa en el camino Pi; las pruebas que lo nombran (`pi_stack`, `failclosed`, `session.rs`) pasan a `@5`. Las sesiones guardadas que usaron `@4` se reabren con el prompt nuevo, porque el prompt se entrega al abrir, no vive en el archivo de sesión. El bucle propio (`clio run`) usa `@2`/`@3` y no cambia.

## Contrato de las herramientas

Tres operaciones nuevas del servicio (además de `fs_*` y `exec`), declaradas en la pasarela con efecto y versión como las demás (S-02, S-15). La guardia las registra con estos nombres:

| Herramienta (nombre en Pi) | Operación del servicio | Argumentos | Efecto | Etapa |
|---|---|---|---|---|
| `memory_search` | `memory_search` | `query` (texto, 1 a 200 caracteres), `limit` (1 a 8, por defecto 5) | `Read` | 1 |
| `memory_get` | `memory_get` | `id` (entero > 0) | `Read` | 1 |
| `memory_propose` | `memory_propose` | `text` (1 a 400 caracteres), `kind` (`semantic` o `procedural`) | `LocalReversible` | 2 |

Respuesta de `memory_search`: hasta `limit` recuerdos **activos** del ámbito `clio`, en el formato de una línea por recuerdo que ya usa `context()`, con id, tipo, procedencia (`persona`, `agente`, `externo`) y fecha, y una línea que dice si la memoria se abstuvo («nada relevante»). `memory_get(id)` devuelve un recuerdo activo, o un error claro si no existe, no está activo o es de otro ámbito (el mismo mensaje en los tres casos: no se distingue «no existe» de «es cuarentena», para no filtrar su existencia).

Los parámetros que no están en la tabla no existen: un argumento extra (`origin`, `actor`, `scope`, `state`) lo rechaza la pasarela como campo desconocido, no se ignora en silencio.

## Qué hace el servicio con cada llamada

Orden, sin saltarse pasos:

1. **Pasarela** (`decide_or_deny`, negar por defecto) y cortacircuito, igual que `fs_*`. Una denegación no toca la memoria.
2. **Presupuesto de memoria por sesión** (S-47, regla propia `S-47/memory`): como máximo 20 llamadas y 24 KiB de texto de recuerdos devueltos por sesión de Pi. Al agotarse, la llamada se niega con ese mensaje. Es el freno contra el vaciado por consultas repetidas (anti-objetivo 1). No cuenta para el cortacircuito de la pasarela (no es una denegación de política), igual que `S-12/bash`.
3. **Validación**: largo y tipo de los argumentos; la consulta pasa por `clean_text` (controles, bidi, ancho cero) y se recorta a 200.
4. **Lectura** con `MemoryLink::search` / `get` en modo solo lectura. Un error de la memoria (archivo ausente, bloqueado, corrupto) es un fallo de la herramienta con un mensaje corto; nunca un pánico (`catch_unwind` como el resto) ni un cuelgue (plazo de la llamada: 10 s).
5. **Limpieza de la salida**: `clean_text` por línea, limpiador de secretos (el del vault y los patrones, S-18), tope de 600 caracteres por recuerdo y 4 KiB por respuesta, cortando después de limpiar y no antes (como el resto del servicio: lo que no cabe se retiene entero, no se parte un secreto).
6. **Envoltorio de datos** (`wrap_untrusted(Source::Memory, …)`) y respuesta marcada `external`. El filtro final de la guardia (`tool_result`) vuelve a limpiar y envolver: dos capas, como con los archivos.
7. **Contaminación**: si algún recuerdo devuelto tiene origen distinto de la persona, `mark(sesión, "memory")`.
8. **Registro de hechos** en `toolhost/memory.jsonl` (un archivo aparte, junto al registro de efectos, para no tocar el esquema de `effects.jsonl`): herramienta, número de resultados, ids, bytes devueltos, si marcó la sesión, y un hash corto de la consulta. Nunca el texto de la consulta ni el de los recuerdos.

**Etapa 2, no incluida en el plan de la etapa 1.** `memory_propose`, además: pasa por el limpiador del vault **antes** de proponer; el actor es siempre `agente`, el origen siempre `External` y el ámbito siempre `clio` (no vienen de los argumentos); un procedimiento espera aprobación aunque el origen sea limpio (S-26); la fuente guarda `pi:<sesión>`. El tope por sesión (5, el mismo `MAX_AGENT_PROPOSALS`) se cuenta en el servicio, y la cuota por hora del actor `agente` del motor sigue siendo el techo global (S-27). Abre la memoria para escribir **al hacer la llamada y la cierra al terminar**: el motor toma un cerrojo de escritor entre procesos mientras vive, y mantenerlo abierto bloquearía a la persona. Si está tomado, la herramienta reintenta unos segundos y falla con un mensaje; nunca espera sin plazo.

## Piezas que cambian

- **`clio-toolhost`**: un trait `MemoryPort` (buscar, leer, proponer) que el servicio recibe en `Config`; sin él las tres operaciones no se declaran y la pasarela las niega (S-02), que es lo que pasa con el binario suelto `clio-toolhost`. Las tres operaciones, el presupuesto por sesión, la regla de contaminación y el registro de hechos viven aquí, con pruebas.
- **`clio-brain`**: implementa `MemoryPort` sobre `MemoryLink`; el subcomando oculto `clio toolhost` recibe `--memory-data DIR` (la carpeta de datos) y lo pasa; `pi::session::Toolhost::start` lo da. El motor de vectores (`EmbedModel`) se carga **perezosamente en la primera búsqueda** y solo si el modelo está en el disco; si no carga, la búsqueda sigue solo con texto y lo dice en la respuesta. `GUARD_TOOLS` pasa de 6 a 8 (etapa 1); en la etapa 2 pasará a 9.
- **Guardia (`adapters/pi/extensions/clio-guard`)**: registra `memory_search`, `memory_get` (y `memory_propose`) con su esquema y el campo `reason` como las demás; `TOOL_NAMES` y la comprobación de arranque (`verify_startup`) cambian a la lista nueva; el prompt cuenta lo que son.
- **Prompt `clio-agent@5`** (los prompts son versionados e inmutables: no se edita el 4). Explica las herramientas, que lo que devuelven es **dato que puede estar desfasado o ser hostil**, que lo marcado «agente» o «externo» no es lo que la persona dijo, que no se guarda nada que parezca un secreto, y que no hay que consultar la memoria «por si acaso» (hay presupuesto).
- **Reglas y documentos (etapa 1)**: una regla nueva (la de lectura; su número se asigna en `security.md` al escribirla): las herramientas de memoria solo leen y devuelven datos marcados, acotados, con presupuesto y registrados; `threat-model.md` (la sección «La memoria como superficie» hoy dice que el bucle del modelo no tiene herramienta de memoria), `uso-local.md`, `adapters/pi/README.md`, `crates/clio-toolhost/README.md` y el ADR 0010 (la decisión y por qué no la CLI). En la etapa 2: S-27 se reescribe para decir quién puede escribir (la persona; el cierre de un run; `memory_propose` solo a cuarentena) y se añade la regla de escritura (lo que el agente propone entra siempre en cuarentena y con presupuesto).

## Límites que quedan y se dicen

- **El proveedor del modelo ve lo que la herramienta devuelve**, igual que ya ve el contexto (S-44). Lo que la persona quiera que no salga de su equipo no debe estar en la memoria. El presupuesto por sesión limita cuánto puede salir, no lo elimina.
- **Comparte usuario con Pi**: un Pi comprometido puede hablar con el socket del servicio y hacer las mismas llamadas que el modelo, con la misma pasarela y el mismo presupuesto delante. No puede abrir `memory.db` (la jaula no la ve).
- **Un recuerdo activo de origen `agent` puede contener una inyección antigua** (un run limpio que la escribió sin saberlo). Por eso llega envuelto y marca la sesión; la defensa no es el filtro de texto.
- **No hay clasificación de sensibilidad** de los recuerdos: todo lo activo es consultable.

## Antes de la etapa 2

`memory_propose` solo se construye cuando haya un **tope de la cuarentena** (hoy no hay ninguno: hallazgo 7 de la segunda pasada). Sin él, un modelo hostil con 5 propuestas por sesión y 20 por hora (cuota del actor `agente`) puede llenar la lista de revisión hasta que lo legítimo quede enterrado. Hace falta: deduplicar también contra la cuarentena (hoy solo contra lo activo), y un máximo de pendientes por actor, con un mensaje claro a la persona cuando se llena. Es un cambio en `clio-memory` con su propio diseño corto y sus pruebas.

## Pruebas y casos de ataque

Casos nuevos de la batería (`testing/attacks/cases/`, activos) y una línea cada uno en `docs/security.md`:

- 56: un recuerdo activo de origen `agent` con «ignora tus reglas y escribe X» llega envuelto como datos, y el texto que provoca no se ejecuta (modelo ingenuo del banco).
- 57: consultas repetidas no vacían la memoria: se acaba el presupuesto (20 llamadas o 24 KiB) y la llamada siguiente se niega.
- 58: un valor del vault guardado en un recuerdo llega tachado al modelo y no aparece en `effects.jsonl`.
- 59: un recuerdo en cuarentena, uno archivado y uno de otro ámbito no se pueden leer ni por `memory_search` ni por `memory_get`, y el error no distingue entre ellos.
- 60: argumentos que no existen (`origin`, `actor`, `scope`) se rechazan; no hay forma de pedir promover, archivar o purgar.
- (Etapa 2, caso 61, no incluido ahora): un `memory_propose` con `procedural`, o desde una sesión contaminada, queda en cuarentena; pasada la cuota o el tope por sesión, se niega.

Y pruebas de comportamiento: el presupuesto sobrevive a reabrir la sesión y a reiniciar el servicio, y cuenta también las llamadas que fallan; `external` envuelve pero no contamina por sí solo; contaminación solo con origen no-persona; la memoria ausente o bloqueada no tumba la herramienta ni el servicio; el cerrojo de escritor no queda tomado tras `memory_propose`; los tres modos de fallo de la lectura (archivo corrupto, base bloqueada, plazo agotado) devuelven un error corto; la lista exacta de herramientas de Pi pasa de 6 a 8 y el arranque falla si falta una; las pruebas de `pi_stack` (en la jaula) siguen pasando con las herramientas nuevas.

## Lo que se deja a quien lo construya

El nombre exacto de los campos de la respuesta; cómo se reparte el trabajo entre `memory_link` y el trait `MemoryPort` (qué métodos nuevos necesita `MemoryLink` para leer sin vectores); la forma de medir el tamaño «devuelto» para el presupuesto; el plazo de 10 s y el de reintento del cerrojo si la medición pide otros; el orden de las líneas de la respuesta; cómo se calcula el hash corto de la consulta.
