# Fase de sueño de la memoria

Fecha: 2026-11-04. Repos: `clio-memory` (el motor y el sueño sin modelo) y `clio` (le suma el paso con modelo). Pedido por Iván.

## Por qué

La memoria crece con lo que guardan los agentes y nadie la limpia:
- lo olvidado se queda en cuarentena para siempre;
- los repetidos y los casi iguales se acumulan;
- lo que entró sin Jev (Claude Code) o antes de una regla nueva nunca se vuelve a mirar.

Iván pidió una fase de sueño: **al final del día, si no se está haciendo nada**, la memoria se revisa sola. Se chequea, manda a cuarentena lo que parece un ataque o un secreto, une lo repetido y purga lo que lleva tiempo en cuarentena.

Sus condiciones:
- **Autónoma.** Nadie está mirando y nunca le pregunta nada a la persona.
- **Estabilidad y seguridad** antes que todo.
- **Quirúrgica.** Toca solo lo que está claro; lo dudoso lo deja anotado.
- **Sin comerse los tokens.** Lo que ya está sólido no se vuelve a repasar. El modelo solo ve lo nuevo y sus vecinos.
- **Al final del día**, no entre sesiones.
- **El modelo de Clio puede unir recuerdos**, de forma planeada. En Clio se usa el conector y el modelo que la persona tenga configurados, sean cuales sean; no se fija ningún proveedor.

Base: la investigación de 2026-11-04 sobre cómo lo hacen otros. Se tomó de:
- Mem0, LightMem y Graphiti: solo lo nuevo y sus vecinos llegan al modelo.
- Anthropic Dreams y Graphiti: unir crea algo nuevo y conserva los originales.
- Unit 42: el paso que resume es el punto de inyección.
- MINJA, AgentPoison y OWASP ASI06: envenenamiento de la memoria.

## Objetivos

1. `clio-memory sleep` hace una noche de sueño sin modelo. `clio memory sleep` hace lo mismo y le suma la unión con modelo. Se instala solo para correr de noche.
2. Corre como mucho una vez por noche, solo con la memoria quieta, y para si alguien empieza a usarla.
3. Nunca pierde algo que no debía:
   - copia antes de tocar;
   - purga solo lo descartado y tras un plazo;
   - topes por noche;
   - todo lo que no es purga se puede deshacer.
4. Cada noche trabaja solo sobre lo que cambió desde la anterior.
5. El agente se entera a la mañana siguiente (una línea en `memory_context`) y la persona en `doctor`.

## No objetivos

- Que el modelo resuelva contradicciones o elija un ganador: solo se anotan.
- Unir recuerdos sin modelo, más allá de los repetidos casi exactos.
- El olvido por edad (`decay`) y la retención: siguen apagados como hoy.
- Windows. Solo Linux (systemd de usuario) y macOS (launchd), como el respaldo de Clio.
- Detectar inundaciones (muchos guardados de una sesión).

## Anti-objetivos (fallaría aunque funcione)

- Que el sueño borre para siempre un recuerdo de la persona, o uno que solo esperaba revisión.
- Que un texto guardado por un atacante haga que el sueño, o el modelo que une, purgue, ponga en cuarentena o reescriba recuerdos legítimos. Tampoco puede colar instrucciones en un recuerdo unido.
- Que deje a Claude Code o a Clio sin poder escribir mientras corre.
- Que dañe la base. Si el chequeo falla, no toca nada.
- Que llene la memoria de recuerdos sobre sí mismo. Sus informes no son recuerdos.
- Que repase cada noche toda la memoria.

## Cuándo corre

- Un timer de usuario `clio-memory-sleep` despierta el comando **cada 30 minutos entre las 23:00 y las 05:30**, hora local.
- El comando sale sin hacer nada si:
  - ya hubo un sueño **completo** en las últimas 20 horas;
  - la memoria no está quieta: en los **últimos 30 minutos** hubo una operación de un actor que no es el sueño, o un uso registrado;
  - otro sueño está corriendo. Un `flock` aparte (`<db>.sleep.lock`) impide que haya dos.
- Si la máquina está apagada o en uso toda la noche, esa noche no hay sueño, y no se recupera de día.
- `--now` se salta la hora y la quietud, para probar a mano.
- `--dry-run` analiza todo y no escribe nada: ni la copia ni el modelo.

## Lo sólido no se repasa

- El sueño guarda un **cursor**: el número de la última operación que ya revisó.
- Cada noche, lo «nuevo» es lo que se creó o cambió después del cursor.
- Un recuerdo viejo vuelve a revisarse solo si le aparece un vecino nuevo (similitud ≥ 0,87), una relación «contradice» nueva, o si alguien lo edita.
- El resto es **sólido** y no se toca.
- Las reglas de secretos y de ataque cambian poco. Cuando cambian (una versión nueva de sus reglas), esa noche pasan una vez por todo, sin modelo.
- El cursor y la versión de las reglas viven en el archivo de estado del sueño, no en la base.

## Quién firma

- Las operaciones del sueño las firma el actor **System** que ya existe, con motivos que empiezan por `sueño:`.
- El motor gana funciones nuevas y **las reglas de seguridad viven dentro del motor**, no en quien llama:
  - `sleep_quarantine(id, reason)`: Active → Quarantine. Se niega a tocar lo que es **de la persona**: lo que `held_by_person` reconoce (escrito por ella, en un bloque fijo o adoptado por ella) y lo fijado (`pins`).
  - `sleep_purge(id, reason)`: purga solo lo que cumple las tres condiciones:
    - está en cuarentena porque se **descartó**: lo mandó `forget`, una marca `Flag` o el propio sueño. Nunca lo que solo espera revisión;
    - lleva ≥ 30 días en cuarentena, o ≥ 7 si lo mandó el sueño por un secreto;
    - no es de la persona, salvo que la propia persona lo haya descartado.
  - La unión del sueño escribe el recuerdo nuevo y deja los originales como `Replaced`, todo en una sola operación que se puede deshacer.
- Las cuarentenas del sueño **no cuentan** para el límite de cuarentena del autor. Si contaran, un atacante podría llenar ese límite a propósito.
- `forget` y `purge` siguen negándose a System, como hoy. Ningún agente ni la herramienta MCP puede llamar a las funciones nuevas.

## Una noche, en orden

### Cómo escribe

- El análisis usa una conexión de **solo lectura**.
- El candado de escritura se toma en **tramos cortos**, de menos de un segundo: un recuerdo por vez, o lotes chicos de vectores. Las escrituras de Claude Code y de Clio esperan unos 2 s, así que pasan.
- Entre tramos: si apareció una operación de otro actor desde que empezó el sueño, **para**. La noche queda incompleta y el timer lo reintenta a la media hora.

### Los pasos

1. **Chequeo.** `PRAGMA quick_check` en la conexión de lectura. Si no da `ok`, alarma y nada más esa noche.
2. **Copia.** Una copia de la base desde la conexión de lectura, en la carpeta de datos (`sleep/`, permisos 600).
   - Se guardan las **3 últimas**, así un secreto purgado no vive para siempre en una copia.
   - Si la copia falla, esa noche no hay purga ni unión. Lo demás se puede deshacer y sigue.
3. **Vectores.** Se calculan los que falten, si hay modelo de vectores, en lotes chicos.
4. **Secretos.** `find_secret` sobre el texto y el título de lo nuevo (o de todo, si cambiaron las reglas).
   - Los de la persona no se tocan: el informe lleva una alarma.
   - Los demás van a cuarentena con `sueño: secreto`.
5. **Posibles ataques.** Solo en lo nuevo que no es de la persona. Cada recuerdo suma puntos por familias de señales:
   - **caracteres ocultos** (de ancho cero, de control de dirección, etiquetas U+E0000–E007F): por sí solos mandan a cuarentena;
   - **frases de inyección**, en español e inglés («ignora las instrucciones anteriores», «ahora eres», «system prompt», «no le digas al usuario», etiquetas como `<system>` o `<|im_start|>`, cierres falsos como `</conversation>`): 1 punto;
   - **órdenes de exfiltración o ejecución** (`curl … | sh`, `base64 -d |`, URLs con `token=`, `key=` o `secret=`): 1 punto;
   - **origen externo**: 1 punto.
   - Con **2 puntos o más**, cuarentena con `sueño: posible ataque (<familias>)`. Una frase sola no basta, porque hay recuerdos legítimos que hablan de seguridad.
6. **Repetidos (sin modelo).** Dentro del mismo proyecto y del mismo nivel de confianza (externo con externo, propio con propio):
   - Con similitud ≥ 0,97 se queda uno: el de la persona; si no, el fijado; si no, el más usado; si no, el más nuevo.
   - El otro pasa sus relaciones al que queda y va a cuarentena con `sueño: repetido de mN`.
7. **Unión con modelo (solo `clio memory sleep`).** Ver la sección siguiente.
8. **Contradicciones.** Los pares «contradice» con los dos activos van a la lista «para revisar». No se toca ninguno.
9. **Purga.** Lo que cumple la regla de `sleep_purge`. Solo si la copia salió bien.
10. **Cursor.** Si la noche se completó, el cursor avanza.

## La unión con modelo (Clio)

- **El modelo.** El que la persona tiene configurado en Clio (`config.model`: su conector y su modelo, cualquiera que sea), por el mismo camino de siempre: la llave sale del baúl y el texto sale limpio por el baúl.
  - Si no hay modelo, el baúl está cerrado o el modelo no responde, este paso se salta y el resto del sueño sigue.
- **Qué se une.** Los grupos de recuerdos nuevos con sus vecinos casi iguales (similitud entre 0,87 y 0,97):
  - solo dentro del mismo proyecto, del mismo nivel de confianza y del mismo tipo;
  - nunca con uno de la persona ni fijado;
  - nunca un par «contradice»;
  - nunca lo que está en cuarentena;
  - grupos de **2 a 4**, como mucho **20 grupos por noche**.
- **El texto es dato, no instrucción.**
  - El modelo recibe los recuerdos dentro de un bloque marcado como datos no confiables.
  - Una instrucción fija le dice que solo una lo que dicen, sin agregar nada.
  - No tiene herramientas.
  - Debe responder un JSON estricto: `{ "texto": …, "titulo": …, "fuentes": [ids] }`.
- **Validación determinista.** Si algo falla, ese grupo no se une y se anota «para revisar»:
  - el JSON es válido y `fuentes` son exactamente los ids del grupo;
  - el texto no es más largo que la suma de los originales;
  - no tiene URLs, rutas, comandos ni frases de inyección que no estén en los originales;
  - no tiene secretos;
  - su similitud con cada original es ≥ 0,80.
- **Validación con Jev** (si Jev está encendido): una pregunta cerrada por original, «¿lo que dice X está todo en el texto nuevo?». Un «no» o una duda cancelan esa unión.
- **Aplicar.** El recuerdo nuevo:
  - va al proyecto de los originales, con su tipo, y externo si alguno lo era;
  - hereda las relaciones de los originales;
  - deja a los originales como `Replaced`.
  - Todo es una operación que se deshace con `revert`.
- **Freno.** Si más de la mitad de los grupos de la noche falla la validación, el paso se detiene. Puede ser un ataque o un modelo que cambió.

## Topes por noche

- **Cuarentenas:** como mucho max(10, 10 % de los activos), sumando los pasos 4 a 6. Si hay más candidatos, van primero los secretos, y el informe lleva la alarma «demasiados: posible ataque o regla rota».
- **Purgas:** como mucho 50.
- **Grupos unidos con modelo:** como mucho 20, con un tope de tokens por noche que fija Clio.

## Qué se entera quién

- **Informe.** Un archivo JSON por noche en `sleep/` de la carpeta de datos. Se guardan los 60 últimos. Junto a ellos está el archivo de estado (cursor, versión de reglas, último sueño completo). No se toca el esquema de la base. Cada informe tiene:
  - inicio, fin y si se completó;
  - cuántos hubo de cada cosa, con sus ids;
  - la lista «para revisar»;
  - las alarmas;
  - los tokens usados.
- **El agente.** Si el último sueño hizo algo, tuvo alarmas o dejó algo para revisar, y fue en las últimas 24 horas, `memory_context` lleva una línea al principio:
  `Anoche (sueño): 2 a cuarentena (m12 secreto, m40 posible ataque), 3 unidas en m70, 1 purgada · para revisar: m5 contradice a m61`
  Va en clio-memory y en Clio.
- **La persona.** `clio-memory doctor` y `clio doctor` dicen cuándo fue el último sueño, si se completó y sus alarmas. `sleep --report` muestra el último informe.
- **Deshacer.** Toda cuarentena y toda unión del sueño se deshace con `revert` (la persona) o con `promote`. La purga no se deshace; por eso el plazo.

## Instalación

- `clio-memory setup claude-code` instala el timer, igual que hoy registra el servidor MCP. Nadie lo hace a mano.
- En Clio, `clio memory setup` lo instala apuntando a `clio memory sleep`.
- Es la misma unidad, así que la última instalación decide el binario. La regla de las 20 horas evita dos sueños en una noche.
- `setup sleep --remove` lo quita.

## Restricciones

- Todo lo que el sueño cambia en la memoria va al registro de operaciones.
- Repo público (clio-memory): ningún dato personal en pruebas ni ejemplos.
- Pruebas primero. Cada regla de seguridad del motor tiene su prueba:
  - System no purga ni pone en cuarentena fuera de las reglas;
  - lo de la persona no se pierde;
  - lo que espera revisión no se purga.
- Las señales de ataque se prueban contra los 64 recuerdos de `bench/` (ningún falso positivo) y contra un juego de textos de ataque (los atrapa).
- La validación de la unión se prueba con un modelo falso que intenta colar una URL, una instrucción o un secreto, o perder un dato.
- Clio sube `clio-memory` a la versión con el sueño, fijada por su commit.

## Lo que queda al constructor

- Los nombres exactos de tipos, funciones internas y archivos.
- La lista exacta de frases y patrones, dentro de las familias de arriba.
- Cómo se lee «uso en los últimos 30 minutos» a partir de lo que el motor ya registra.
- La redacción de la instrucción fija del modelo y de la pregunta a Jev.
- El tope de tokens por noche y el formato de `--report`.
