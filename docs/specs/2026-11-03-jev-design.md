# Jev en Clio: las decisiones que el modelo no debe tomar solo

Diseño escrito el 2026-11-03, a petición de la persona: que la memoria sea autónoma (sin cuarentena para lo normal), que la memoria se inyecte **antes** de que el modelo responda, que cada mensaje lleve la fecha y la hora, y que Jev sugiera qué herramientas usar. Se apoya en `docs/security.md` (S-05, S-07, S-18, S-25, S-26, S-27, S-44, S-47, S-54, S-61), en el ADR 0010 y en el spec de herramientas de memoria (`2026-11-03-memory-tools-design.md`). El diagrama v2 (`docs/clio-architecture.svg`) ya prevé a Jev como opcional en recuperación, cuarentena, gestor de memoria, triaje y pasarela.

## Qué es Jev (verificado en su documentación)

Un modelo «System One» de TypeSafe: **no escribe texto**. Recibe un `state` y preguntas, y contesta cada una con una opción (`choice`), un nivel (`score`) o un sí/no con probabilidad (`noul`), más su confianza. API: `POST https://api.typesafe.ai/v1/systemone`, `Authorization: Bearer`, modelo `jev-latest` (hoy `jev-1.14.0`). Las preguntas de una llamada corren en paralelo e independientes. **No documenta** retención de datos, límites de tamaño, límite de tasa ni tiempos: son datos que faltan, no supuestos (ver «Preguntas abiertas»).

Consecuencias de diseño: Jev **elige**, nunca redacta (el texto de una memoria lo escribe la persona o el modelo); y se le pregunta lo que una persona informada decidiría de un vistazo, no tareas largas.

## Intención

Que Clio sea autónoma de verdad en tres cosas que hoy dependen de que el modelo se acuerde o de que la persona revise: (1) qué hacer con una memoria nueva, (2) qué recordarle al modelo antes de que conteste, (3) qué herramientas le convienen. Y que todo siga funcionando, con menos autonomía, cuando Jev no esté.

## Objetivos

1. **Jev es opcional.** Sin clave, sin red, sin crédito o con Jev apagado, todo funciona como hoy: las reglas fijas deciden y lo dudoso va a cuarentena. Nada de lo que se construya puede exigir a Jev para arrancar, conversar o guardar.
2. Una sola etapa en el cerebro antes de que el mensaje llegue a Pi (el «antes del prompt», donde ya vive el triaje de `pi/level.rs`), con la entrada **solo del mensaje de la persona** (S-07).
3. **Fecha y hora** en cada mensaje, puestas por código, ocultas en el chat de la persona.
4. **Memoria inyectada** antes de la respuesta: Jev decide si hace falta y cuáles recuerdos pesan; el código arma el bloque.
5. **Sugerencias de herramientas** (nunca prohibiciones), con las preguntas generadas desde el catálogo real de herramientas: sin lista a mano.
6. **Memoria autónoma** con las operaciones ADD, UPDATE, DELETE (archivar), PROMOTE y REVERT, cada una con su lógica y sus condiciones; PURGE solo la persona.
7. Sin tope de cantidad de memorias, para sesiones largas.

## No objetivos

- Que Jev redacte, resuma o reescriba memorias.
- Que Jev haga PURGE (borrado definitivo) o toque las memorias de identidad.
- Un modelo local de respaldo (el benchmark mostró que solo Kev-4B sirve y cuesta 4,6 GB de GPU; se descartó por ahora). El cliente queda con una interfaz que permitiría enchufar uno después.
- Cambiar el nivel de triaje con Jev en esta primera entrega (ver «Entregas»).
- Que Jev vea resultados de herramientas, páginas, archivos o correos (diagrama: «Jev no ve salidas ni contenido externo»).

## Anti-objetivos (fallaría aunque «funcione»)

- Que Clio deje de funcionar o de guardar cuando Jev falla.
- Que un texto hostil, al pasar por Jev, se vuelva memoria activa o instrucción permanente (la inyección persistente de S-26).
- Que Jev decida borrar algo «porque no se usa» o «no parece relevante»: la persona lo dijo con su nombre, que casi nunca se menciona y es lo más importante que hay guardado.
- Que la clave de Jev, o un secreto de la persona, salga hacia TypeSafe o aparezca en un log (S-18, S-44).
- Que una etiqueta o una sugerencia de Jev cierre una puerta: Jev solo puede **añadir** señales (sugerir, subir peligro, mandar a revisión), nunca quitar un control (S-54).

## Cómo se comporta

### El «antes del prompt»

Para cada mensaje que la persona escribe, el cerebro hace, en este orden y sin que nada de lo que Pi lea o diga entre en la entrada:

1. **Hora** (código, sin Jev): una línea `[2026-11-03 18:42 -05:00]` con la hora local y su desfase, antes del texto de la persona. La pone el cerebro al enviar; la pantalla de la persona muestra solo lo que escribió.
2. **Una sola llamada a Jev** con el mensaje (limpio de secretos por el vault, S-44) y varias preguntas a la vez: ¿hace falta memoria?, y una pregunta sí/no por herramienta. Si la respuesta dice que hace falta memoria, el código consulta `clio-memory` (`recall`) y, con una segunda llamada de puntaje, ordena los candidatos por relevancia.
3. **Armado del bloque** (código): el mensaje que recibe Pi lleva la hora, un bloque de memoria marcado como datos y una línea de sugerencias, y debajo el texto de la persona sin tocar. Los tres textos que antepone el cerebro van con marca propia que el modelo aprende en el prompt del sistema.
4. **Contaminación (S-05):** si el bloque trae algún recuerdo que no es de la persona (origen `agente` o `externo`), la sesión queda contaminada como si el modelo lo hubiera leído con `memory_get`. Lo que la persona escribió no contamina.
5. **Sin Jev** (apagado, caído, confianza baja, tiempo agotado): se salta la llamada. La hora se pone igual. La memoria se consulta con las reglas de hoy (`recall_for`) o no se inyecta, según un interruptor de la configuración; las sugerencias no se ponen.

### Catálogo de herramientas: una sola fuente

Hoy los nombres de las herramientas viven en `TOOL_NAMES` (guardia, TypeScript) y se repiten en `GUARD_TOOLS` (Rust) con una prueba que los iguala. Para que Jev no obligue a mantener otra lista:

- Cada herramienta que la guardia registra trae, junto a su descripción, un campo **`use_when`**: una frase que dice cuándo conviene («hace falta leer archivos del equipo», «hace falta consultar lo que se recuerda de la persona»…). Es la misma frase que Jev recibe como criterio.
- La guardia ya anota, al arrancar la sesión, qué herramientas están activas (`TOOLS_ENTRY`); esa anotación pasa a incluir `use_when`. El cerebro la lee y **genera las preguntas de Jev desde ahí**. Agregar una herramienta a la guardia la agrega a Jev sin tocar otra lista.
- Una prueba falla si una herramienta activa no trae `use_when` (o si trae una frase vacía). Esa prueba reemplaza a mantener la lista a mano.
- Herramientas futuras (otros servidores, extensiones) entran por el mismo camino.

### Operaciones de memoria

El texto de la memoria nunca lo escribe Jev. Jev contesta una opción cerrada con su confianza; el motor sigue siendo la autoridad.

| Operación | Quién la pide | Cuándo se ejecuta |
|---|---|---|
| **ADD** | Jev, al llegar la propuesta del modelo (`memory_propose` o el cierre de un run) | Jev elige `add` con confianza ≥ umbral y la sesión no está contaminada (o supera el umbral alto): la memoria queda **activa**, firmada por el actor `jev`. Si no, cuarentena como hoy. |
| **UPDATE** | Jev | Jev elige `update_mN` (la propuesta reemplaza a una memoria parecida, mismo tema, dato nuevo): el motor reemplaza por `topic_key`; la anterior queda en el historial y se puede revertir. |
| **NOOP** | Jev | Jev elige `noop` (ya está dicho): no se escribe nada. |
| **REVIEW** | Jev, o la falta de confianza | Va a cuarentena para la persona. Es la salida por defecto ante la duda. |
| **DELETE** (archivar, reversible) | La persona, a través del modelo (herramienta `memory_forget`); o Jev solo como parte de UPDATE/NOOP (sustituida o duplicada) | Con la herramienta: solo si **Jev confirma, mirando únicamente el mensaje de la persona de esa sesión**, que ahí pide olvidar *esa* memoria. Nunca por «no se usa» ni «no parece relevante». |
| **PROMOTE** | Jev al aprobar un ADD; o la persona, a través del modelo (`memory_promote`) | La herramienta exige la misma confirmación de Jev contra el mensaje de la persona. |
| **REVERT** | La persona, a través del modelo (`memory_revert`); Jev solo sobre operaciones que él mismo firmó | Misma confirmación. |
| **PURGE** | Solo la persona, con `clio memory purge` | Nunca Jev, nunca el modelo. |

Reglas comunes:
- **El modelo no puede decir «la persona lo pidió».** `memory_forget`, `memory_promote` y `memory_revert` pasan por el código, que le pregunta a Jev (con el mensaje real de la persona, no con lo que el modelo cuente) si esa acción sobre esa memoria fue pedida. Sin Jev, esas tres herramientas responden que no pueden confirmar y la persona usa `clio memory`.
- Las memorias marcadas **identidad** (nombre, dónde vive…) no caducan y Jev no puede archivarlas ni sustituirlas sin la persona.
- Las memorias **procedimentales** siguen necesitando a la persona (S-26) hasta que ella decida otra cosa (pregunta abierta).
- **La excepción al «lo externo entra en cuarentena» (S-25) es una sola**, de código y firmada: el motor acepta un ADD/UPDATE activo de origen externo solo con un veredicto de `jev` registrado (probabilidades incluidas) en el registro de decisiones. Todo se puede revertir con `REVERT`.
- Detector de secretos, cuotas y registro de operaciones siguen **antes** de Jev y no dependen de él.
- **Sin tope de cantidad de memorias activas.** Lo que se acota es el **ritmo**, no el total (ver «Cómo se construye», punto 8). El tope de 50 candidatos en cuarentena se queda: acota lo que espera tu revisión, no lo que Clio sabe.

### Cuándo Jev actúa solo y cuándo no

Jev devuelve una opción y su confianza. En el benchmark, su confianza fue 0,94 cuando acertó y 0,39 cuando falló, así que sirve de interruptor. La acción automática exige confianza ≥ umbral (propuesto: 0,8; 0,9 si la sesión está contaminada); por debajo, REVIEW. Los umbrales viven en la configuración y se afinan con el set de pruebas.

## Datos que salen a Jev (S-44)

| Sale | No sale nunca |
|---|---|
| El mensaje de la persona, limpio de secretos por el vault | Resultados de herramientas, páginas, archivos, correos |
| El texto de una propuesta y el de las 1 a 3 memorias parecidas (activas), limpios de secretos | Memorias en cuarentena, archivadas o de otros ámbitos |
| Los criterios de las preguntas (fijos en código; los de herramientas, de `use_when`) | La clave (se pide al vault por el socket de la pasarela en cada llamada) |

El texto de una propuesta lo escribió el modelo, a veces tras leer una página hostil. Va a Jev **solo como `state` de una pregunta cerrada** («qué hacer con esto»): la respuesta posible es una de pocas opciones, y manipularla solo logra un ADD que el motor, el detector de secretos, las cuotas y el REVERT siguen acotando. Es una excepción acotada a «Jev no ve contenido externo»; sube el umbral en sesiones contaminadas y deja fuera lo procedimental. Las memorias de la persona que Jev pondera para inyectar (paso 2) son datos de la persona, no contenido externo.

## Configuración, clave y fallos

- **Clave:** secreto `jev` en el vault, destino `jev:api.typesafe.ai`. El cliente la pide por el socket de la pasarela en cada llamada y no la guarda.
- **Modelo:** `jev-latest`, por decisión de la persona: TypeSafe actualiza el modelo y deja de servir las versiones anteriores, así que fijar una versión terminaría fallando. El riesgo es que una versión nueva mueva las decisiones sin aviso. Se acota así: cada respuesta trae el modelo que contestó (`model`, p. ej. `jev-1.14.0`); el cliente lo anota en el registro de hechos y, cuando cambia, `clio doctor` y la pantalla lo avisan; al cambiar se vuelve a correr `testing/decisiones` (manual, con la clave) antes de confiar en los umbrales. Un alias exacto en la configuración queda como opción para quien lo quiera, no como valor por defecto.
- **Apagado de fábrica:** Jev viene **apagado**. Encenderlo es una decisión explícita (ajuste de configuración) y exige el secreto en el vault; `clio doctor` dice si está encendido, si hay clave y si responde. Así la entrega 2 se puede publicar sin que nada salga a TypeSafe hasta que la persona lo decida.
- **Interruptores:** Jev encendido o apagado, y por función (hora siempre encendida; memoria inyectada, sugerencias y operaciones por separado).
- **Fallos:** tiempo máximo corto por llamada, disyuntor tras fallos seguidos y presupuesto diario de tokens (Jev cobra la entrada); todo falla cerrado hacia las reglas de hoy (S-47).
- **Registro:** hechos solamente (qué pregunta, qué opción, confianza, milisegundos, fallo o no). Nunca el texto.

## Qué mide cada cosa

El benchmark (`testing/decisiones/`: `cases.py` con los casos y las preguntas, `run.py` con el medidor; los casos están sin revisar por la persona) usa 72 casos en español con la respuesta correcta conocida. Primera medición: Jev 93 % en la operación de memoria, 100 % en «¿hace falta memoria?» y «¿pide olvidar?», 88 % en herramientas (sin omitir ninguna necesaria); falló un intento de manipulación. Kev-4B local: 87/81/86/75 %. Julia-1 y Laya: cerca del azar en español. El CI no llama a Jev (no tiene clave): repite respuestas grabadas; la medición en vivo es manual.

## Entregas (cada una, su propio PR)

1. **Cliente de Jev y catálogo.** Cliente con tiempo máximo, disyuntor y presupuesto; `use_when` en la guardia y prueba que lo exige; preguntas generadas desde el catálogo. Más la **hora** en cada mensaje (no necesita a Jev).
2. **Antes del prompt:** inyección de memoria y sugerencias de herramientas, con el contagio de S-05 y la salida a las reglas si Jev falta.
3. **Memoria autónoma:** actor `jev`, veredicto firmado, ADD/UPDATE/NOOP/REVIEW; ritmo en vez de tope.
4. **Herramientas `memory_forget`, `memory_promote` y `memory_revert`** con la confirmación contra el mensaje de la persona.
5. (Más adelante) Jev como decisor del triaje.

Pendientes de la revisión de la entrega 1, para la entrega 2: un identificador (nonce) por sesión en el sobre `<clio-contexto>` o neutralizar el literal en lo que lee el modelo, y enseñarle en el prompt qué es el sobre; los mensajes de `steer` van sin sobre; el disyuntor y el presupuesto usan el reloj de pared (un reloj que retrocede los alarga o los reinicia: valorar un reloj monótono); y una prueba del camino TLS real con `env_clear`.

Cada entrega agrega reglas a `docs/security.md`, casos de ataque nuevos (manipular una propuesta para forzar ADD, decir «la persona lo pidió», sugerencias que pidan quitar un control) y su cobertura en `scripts/rule-coverage.sh`.

## Cómo se construye (decidido tras la revisión del diseño)

Un lector independiente leyó este documento y señaló ocho puntos; cada uno queda resuelto aquí (decisiones mías, a revisar):

1. **Quién confirma `memory_forget/promote/revert`.** El puerto de memoria (`LinkPort`) vive en el cerebro, no en el toolhost; el toolhost solo lo llama. El cerebro ya ve cada mensaje de la persona en el «antes del prompt», así que guarda por sesión los **últimos 5 mensajes de la persona** (en memoria del proceso, nunca en disco) y el puerto confirma contra ellos con Jev. Las tres herramientas son métodos nuevos de `MemoryPort`. No hace falta canal nuevo ni que el toolhost salga a la red.
2. **Dónde vive el cliente y el candado.** Cliente de Jev en un crate nuevo `clio-jev` (HTTP, tiempo máximo de 3 s, disyuntor tras 3 fallos seguidos y 60 s abierto, presupuesto diario). El juicio se hace **antes** de tomar el candado de escritura, en el cerebro, y el motor recibe el veredicto ya hecho (`propose` con un `Judgement` firmado). `Decider` no cambia: sigue siendo para decisiones síncronas locales. El toolhost pasa al puerto si la sesión está contaminada (para elegir 0,8 o 0,9). El cierre de `clio run` usa el mismo juez, con el umbral alto.
3. **«Identidad».** No hay marca nueva: son los **bloques fijos** que `clio-memory` ya tiene (identidad, reglas, preferencias) más toda memoria de origen **persona**. Jev no puede sustituirlas ni archivarlas por su cuenta. Solo si Jev confirma, mirando únicamente los últimos mensajes de la persona, que ella misma dijo el dato nuevo («ahora vivo en Bogotá»); si no, REVIEW. Es la misma confirmación de DELETE/PROMOTE/REVERT, y también protege en sesiones contaminadas.
4. **Actor y origen.** `jev` es un `ActorKind` nuevo (`Judge`) que **solo se registra en código** (como `System`) y no puede registrarlo nadie desde fuera. El origen de la memoria **no cambia** (sigue `agente` o `externo`), así que leerla contamina la sesión como hoy (S-05); en el registro de decisiones queda que Jev la aprobó y, cuando aplica, que la persona la dijo. El motor solo acepta el veredicto de un valor construido por código del cerebro con el actor registrado. Sus operaciones no gastan la cuota del actor `agente` y sí entran en el límite de ritmo. Cambian S-25 (la excepción única), S-27 y S-61 en la entrega 3.
5. **Protocolo con Jev.** Las preguntas, los criterios y la forma del `state` son los de `testing/decisiones/cases.py` (primera versión, ya en el repo) y la entrega 1 los mueve al crate `clio-jev` con pruebas. La inyección de memoria usa dos llamadas: (a) «¿hace falta memoria?» y las preguntas de herramientas, juntas; (b) si hace falta, el motor trae los 8 candidatos mejores y una pregunta sí/no por candidato («¿ayuda esta memoria a responder?»), y se quedan los que pasan de 0,5 hasta el presupuesto de contexto. `update_mN` se codifica con los ids de las memorias parecidas (1 a 3) en el `state`. El CI repite respuestas grabadas.
6. **Valor por defecto.** Apagado (ver «Configuración»). Responder la pregunta abierta 2 decide si la persona lo enciende; no frena ninguna entrega.
7. **Ocultar lo antepuesto.** Pi guarda el mensaje completo en su archivo de sesión. Lo antepuesto va dentro de un sobre delimitado (`<clio-contexto>…</clio-contexto>`) al principio del mensaje, y el cerebro lo **quita al mostrar** el historial (TUI, `clio chat`, la PWA futura), con una prueba que lo exige. El archivo conserva el sobre: es lo que el modelo vio, y la hora que lleva dentro deja claro cuándo fue. Al reabrir una sesión, el modelo ve los sobres viejos con su fecha.
8. **Límite de ritmo.** Es la cuota por hora del **motor**, que se cuenta sobre las operaciones guardadas en la base de datos: sobrevive a reabrir la sesión y vale por actor, no por sesión. Valor inicial: 60 por hora para `agente` y para `jev` por separado, ajustable. El tope de 5 propuestas por sesión (S-61, archivo de la carpeta de la sesión) se queda **solo para sesiones contaminadas**; en las limpias lo gobierna la cuota por hora. S-61 y S-27 se reescriben en la entrega 3.

## Decisiones tomadas por mí (para que las revises)

- La hora lleva desfase de la zona local del equipo (`-05:00`), en formato fijo; no depende del modelo.
- Se usa `jev-latest` (decisión de la persona); el cambio de versión se detecta y se avisa, no se impide.
- Umbrales iniciales 0,8 y 0,9 (sesión contaminada), afinables.
- Límite de ritmo en lugar de tope de cantidad; los números van en la configuración.
- Lo procedimental sigue pidiendo a la persona hasta que decida.
- Sin Jev, `memory_forget/promote/revert` no funcionan (no hay forma de verificar el pedido); la persona usa `clio memory`.
- Se descartó el modelo local por su costo de GPU y RAM.
- Jev viene apagado; el juicio va antes del candado; los últimos 5 mensajes de la persona son la ventana de confirmación; el límite de ritmo es la cuota del motor (60 por hora por actor). Detalles en «Cómo se construye».

## Decisiones de la persona (2026-11-03)

1. **Procedimientos:** los aprueba Jev como todo lo demás («todo lo debe aprobar Jev»). El umbral es el alto y una sesión contaminada lo sube más; sigue valiendo REVIEW ante la duda.
2. **Retención:** acepta que TypeSafe no documente cuánto conserva de lo que recibe.
3. Trabajo autónomo: las entregas 2 a 4 se suben como PR sin esperar su revisión; las fusiones las decide ella.

## Preguntas abiertas (solo la persona puede responder)

1. **Procedimientos** (qué hacer y cómo): ¿Jev puede aprobarlos solo, o los sigue aprobando la persona (S-26)?
2. **Retención:** TypeSafe no documenta qué guarda de lo que recibe. ¿Se acepta que salgan los mensajes de la persona y las memorias que Jev pondera, limpios de secretos, o hay temas que deben quedarse solo con reglas?
3. **Etiquetas del set de pruebas:** la persona revisa `cases.py` antes de fijar umbrales.
