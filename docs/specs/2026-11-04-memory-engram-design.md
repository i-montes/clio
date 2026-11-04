# `clio-memory` al nivel de Engram

Diseño escrito el 2026-11-04, a petición de la persona: «que clio-memory llegue al nivel de producción que tiene Engram, siguiendo nuestras directrices, nuestras herramientas y sistemas; la diferencia es que nosotros usamos vectores». Referencia: [Gentleman-Programming/engram](https://github.com/Gentleman-Programming/engram) (README y `DOCS.md`, leídos el 2026-11-04). Se apoya en el diseño de Jev (`2026-11-03-jev-design.md`), el ADR 0010 y `docs/security.md` (S-05, S-18, S-24 a S-27, S-44, S-47, S-60 a S-65).

## Qué tiene ya Clio y Engram no

Registro de operaciones como fuente de verdad (con REVERT y reconstrucción), PURGE con lápida, validez temporal (`as_of`, `known_at`), cuarentena y cuotas por actor, detector de secretos y limpiador del vault, vectores además de texto, abstención y presupuesto en el recuerdo, olvido por reglas, señales de utilidad, trabajos en segundo plano, Jev como juez. La búsqueda ya ignora acentos (`unicode61 remove_diacritics 2`). Nada de esto se pierde.

## Decisiones de la persona (2026-11-04)

1. **Proyectos:** memoria del proyecto, más la personal (identidad y preferencias), y **aprender de los errores de otros proyectos**.
2. **Otros agentes:** sí, por MCP, para **leer y proponer**; nunca PROMOTE, PURGE ni REVERT por MCP (S-25).
3. **Sesiones:** resumen al cerrar y al compactar; sus mensajes **no** se guardan aparte (Pi ya los tiene).
4. **Sincronizar entre máquinas:** no por ahora.
5. Trabajo autónomo de noche; cada etapa en su PR, sin fusionar.

## Lo que falta, y cómo se hace en Clio

### 1. Memorias estructuradas

- Cada memoria puede llevar **título** (corto, buscable) y **categoría**: `decision`, `architecture`, `bugfix`, `pattern`, `config`, `discovery`, `preference`, `learning` y, desde el 2026-11-04 a petición de la persona, `profile` (su identidad: nombre, dónde vive, a qué se dedica; `preference` queda para cómo quiere que le respondan, escriban o trabajen). Las dos van a lo personal y no caducan. Ambos opcionales en el motor (lo viejo sigue valiendo) y obligatorios en la herramienta del agente.
- El contenido del agente pasa de 400 a **1200 caracteres** en una línea con la forma `Qué: … · Por qué: … · Dónde: … · Aprendido: …` (el formato de Engram en una línea: el servicio aplana los saltos para que una memoria no pueda imitar a otra).
- El índice de texto busca en título y contenido.
- Esquema 2 con migración desde el 1 (columnas nuevas; la reconstrucción desde el registro lee las filas viejas sin título ni categoría).

### 2. Proyecto, personal y lecciones

Tres ámbitos, que ya existen en el motor como `scopes`:

- **`clio`** (el que ya existe) es lo **personal**: identidad y preferencias. Vale en todos los proyectos. Sin migración.
- **`proyecto:<nombre>`**: decisiones, arquitectura, configuración de ese repositorio. Solo aparece en él.
- **`lecciones`**: errores, causas raíz, patrones y trampas que valen en otros proyectos. Guardan el proyecto de origen (`source`) y salen en cualquier proyecto marcadas «aprendido en X».

El proyecto lo detecta el cerebro (no el modelo) desde la carpeta de trabajo: nombre del remoto `origin` de git, si no la carpeta raíz del repositorio, si no el nombre de la carpeta; normalizado (NFC, minúsculas, sin espacios en los bordes), como Engram. La búsqueda y el contexto del agente juntan los tres ámbitos (el del proyecto primero). **Dónde va una memoria nueva lo decide Jev** con una opción cerrada (`proyecto` / `personal` / `leccion`), a partir de la categoría y el texto; sin Jev, la regla: `preference` → personal, `bugfix`/`pattern`/`learning` → lecciones, el resto → proyecto. *(Decisión mía: la regla sin Jev.)*

### 3. Sesiones y resumen

- Herramienta `memory_session_summary` con la forma de Engram (objetivo, instrucciones, descubrimientos, hecho, siguientes pasos, archivos), guardada como memoria **episódica** del proyecto, título «Sesión <fecha>», con la sesión como `episode`. Pasa por el mismo camino que una propuesta (vault, detector, Jev o cuarentena).
- **Al compactar:** el cerebro ve el evento de compactación de Pi y, en el turno siguiente, el sobre `<clio-contexto>` le pide al modelo que guarde el resumen antes de seguir.
- **Al cerrar:** cuando la persona sale del chat de una sesión con al menos 4 mensajes suyos, el cerebro manda un último turno corto pidiendo el resumen (con el tope de tiempo y de coste de cualquier turno). Desactivable en la configuración (`memory.session_summary`). *(Decisión mía: el umbral de 4 mensajes.)*
- Los mensajes de la persona **no** se guardan aparte.
- *Decisiones mías al construirlo:* el resumen cabe en 1200 caracteres como cualquier propuesta del agente (así `memory_get` lo devuelve entero); cuenta entre las propuestas de la sesión; Jev no elige su sitio (siempre el proyecto, o lo personal sin proyecto) y no se le ofrecen vecinos (un resumen no reemplaza a otro); el turno de cierre tiene cuatro pasos y 30 s, no se manda si hay un turno en marcha, y al salir de la TUI se espera a los cierres pendientes diciéndolo en la terminal.

### 4. Revelación progresiva

- `memory_search` devuelve vista previa (título, categoría, ámbito, primeros caracteres).
- `memory_get` devuelve el contenido completo y, si se pide, el historial de versiones (las operaciones que lo tocaron).
- Nueva `memory_timeline {id}`: las memorias de la misma sesión (o del mismo día si no hay sesión) antes y después de esa, en orden.
- Nueva `memory_context`: lo de la última sesión del proyecto (su resumen), lo fijado y lo más reciente, con presupuesto (lo que Engram usa tras compactar).

### 5. Fijar y revisar

- **Fijar** (`pinned`): sale primero en el contexto. Lo fija la persona (`clio memory pin`); el modelo solo si la persona lo pidió (`memory_pin`, con la misma confirmación de Jev que S-65).
- **Revisión por tipo:** cada categoría tiene un plazo (`config` 90 días, `decision`/`architecture` 180, `discovery` 120; `preference`, `bugfix`, `pattern`, `learning` sin plazo). Pasado el plazo la memoria sigue activa pero sale marcada «revisar»; `clio memory review-due` las lista y `clio memory reviewed ID` reinicia el plazo. *(Decisión mía: los plazos.)*

### 6. Relaciones entre memorias

Cuando Jev decide una propuesta y hay memorias parecidas que **no** reemplaza, se le pregunta (una opción cerrada) qué relación tiene con la más parecida: `related`, `compatible`, `scoped`, `conflicts_with`, `not_conflict`. Se guarda en una tabla `relations` (fuera del registro, como las decisiones), y la búsqueda anota cada resultado: «contradice a #N», «reemplaza a #N», «reemplazada por #N». `supersedes` ya lo da el reemplazo del motor.

*Decisiones mías al construirlo:* `not_conflict` no se le ofrece a Jev (solo tiene sentido para corregir un «contradice» que marcó alguien); a Jev se le ofrece «ninguna»; umbral 0,7; solo se pregunta con lo que Jev guarda como nuevo (no con lo que espera revisión) y contra la memoria más parecida; las notas solo nombran memorias activas que la sesión puede leer, así que «reemplaza a» / «reemplazada por» no se muestran (la vieja ya no es legible); `memory_pin` usa el camino de `memory_forget` (palabras de la persona y Jev, 0,9) y fija firmado como la persona.

### 7. Servidor MCP

`clio memory mcp`: JSON-RPC 2.0 por la entrada estándar (sin dependencias nuevas). Herramientas: `memory_search`, `memory_get`, `memory_timeline`, `memory_context`, `memory_propose`, `memory_session_summary`, `memory_current_project`. Escribe como un actor propio por cliente (`mcp:<nombre>`, de tipo agente) y **siempre** con origen externo, así que lo que propone pasa por Jev o espera en cuarentena; nunca promueve, archiva, revierte ni purga (S-25). Lee solo lo activo. Abre la memoria en solo lectura salvo durante una propuesta (como el servicio de Pi). Cuota propia.

*Decisiones mías al construirlo:* el servidor monta en su proceso el mismo servicio de herramientas de Pi (pasarela, topes, filtro, contaminación, tope de propuestas, Jev) y solo deja pasar las herramientas de memoria que leen o proponen, así no hay un segundo camino que mantener; el esquema de cada herramienta sale de los argumentos que comprueba la pasarela. Escribe como el actor `agente` (la misma cuota por hora que Pi, no una propia por cliente: una cuota por cliente necesita actores nuevos en el motor y queda para después) con la sesión `mcp:<cliente>`. El tope de lectura (20 llamadas, 24 KiB) es por proceso (se reinicia con él; las propuestas siguen acotadas por la cuota por hora del actor, que vive en la base). Toda llamada se trata como de una sesión contaminada (cinco propuestas, listón alto de Jev): el servidor no sabe qué leyó el cliente. El nombre de la sesión se fija en el primer `initialize`. `memory_pin` y las demás que cambian la memoria no están: piden las palabras de la persona, que un cliente MCP no puede dar.

### 8. Protocolo en el prompt

Una sección del prompt (`clio-agent@10`) con el protocolo de Engram adaptado: qué guardar (conocimiento verificado y caro de reconstruir; nada de secretos, especulación ni avances rutinarios), formato (título, categoría, Qué/Por qué/Dónde/Aprendido), cuándo buscar (cuando la persona pide recordar o hay trabajo relacionado), cuándo resumir (cierre, compactación).

### 9. TUI y CLI

La pantalla de memoria de la TUI gana: tablero (cifras por ámbito y estado), búsqueda, recientes, detalle con historial, línea de tiempo y sesiones. La CLI gana `projects`, `pin`/`unpin`, `review-due`/`reviewed`, `timeline`.

### 10. Calidad de producción

Migración probada desde una base del esquema 1; doctor con las comprobaciones nuevas; medición de búsqueda con 10 000 memorias; documentación al día; reglas nuevas en `docs/security.md` con su cobertura.

*Decisiones mías al construir la etapa 6:* la CLI gana `projects`, `review-due`, `reviewed`, `timeline` y `pin`/`unpin`, y `show` dice sitio, título, categoría, fijado, plazo y relaciones; la pantalla de memoria de la TUI no gana un tablero aparte: cada fila dice su sitio y su título y la línea de estado cuenta lo que hay por revisar (el tablero por sitio es `clio memory projects`). El doctor dice cuántos recuerdos activos hay, en cuántos proyectos, cuántos esperan revisión y cuántos pasaron su plazo. Medición con 10 000 memorias (con vectores): búsqueda en los tres sitios p50 22,5 ms, p95 37 ms; ver «At scale» en `crates/clio-memory/README.md`.

### Lo que faltaba frente a Engram (revisado contra su README el 2026-11-04)

Hecho después de las seis etapas: el proyecto se puede fijar a mano (`CLIO_PROJECT`, `.clio-project`, `clio memory project set`; orden: variable, archivo, detección), y `clio apply` nunca aplica un `.clio-project` de una sesión; `clio memory setup <agente>` imprime la configuración MCP de Claude Code, Cursor, Codex, Gemini y OpenCode sin escribirla; el modelo ve lo vencido (nota «revisar» en la línea y hasta tres en `memory_context`) en vez de una herramienta `mem_review` aparte. Después: `memory_update` (el agente propone un texto nuevo para una memoria concreta; es una propuesta cuya única vecina es esa memoria, así que Jev decide como siempre y lo de la persona solo cambia con sus palabras: «todo lo debe aprobar Jev») y la guarda de proyecto (un proyecto detectado se recuerda por repositorio; si la detección cambia, se usa la de ahora y se avisa con el comando para unirlos, en vez de detenerse como Engram: Clio no tiene a quién preguntar en mitad de un chat, y heredar el nombre viejo mezclaría dos repositorios clonados en la misma ruta). Una memoria fijada no la reemplaza una propuesta sin las palabras de la persona; `memory_update` solo toca hechos y conserva título y categoría si no se dan. Por último: `memory_consolidate` (une de 2 a 5 hechos del mismo sitio en uno; como Engram pide confirmación explícita, aquí son las palabras de la persona confirmadas por Jev, como para olvidar; los originales se archivan, no se borran) y copiar al portapapeles en la TUI (`y`, OSC 52). Lo único de Engram que no se hace es lo que la persona descartó: sincronizar entre máquinas, la nube, Obsidian, guardar sus mensajes, y la API HTTP (se eligió MCP para otros agentes).

## Qué no se hace (por decisión o por diseño)

- Sincronizar entre máquinas, nube, Obsidian.
- Guardar los mensajes de la persona.
- Capturar llamadas de herramientas en bruto (Engram tampoco).
- Consolidar varias memorias en una por el agente (`mem_consolidate`): queda para después; la persona puede archivar y el reemplazo de Jev cubre el caso común.

## Etapas (cada una un PR, apiladas sobre #28)

1. **Motor:** esquema 2 y migración, título y categoría, `pinned`, plazos de revisión, tabla `relations`, línea de tiempo, búsqueda en varios ámbitos.
2. **Proyecto y estructura en el agente:** detección del proyecto, ámbitos en el servicio de Pi, `memory_propose` con título y categoría (sin `topic_key`: el reemplazo que decide Jev entre las memorias parecidas cubre el «mismo tema, dato nuevo»; *decisión mía*), Jev elige el ámbito, búsqueda y contexto juntando los tres, `memory_timeline`, `memory_context`, prompt @10.
3. **Sesiones:** `memory_session_summary`, resumen al compactar y al cerrar.
4. **Relaciones** juzgadas por Jev y anotaciones en la búsqueda; `memory_pin`.
5. **MCP.**
6. **TUI y CLI**, doctor, medición, documentación.
