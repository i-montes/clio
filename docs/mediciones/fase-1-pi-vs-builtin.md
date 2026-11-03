# Medición fase 1: Pi contra el bucle propio (`builtin`) en las 22 tareas reales

Fecha: 2026-10-31. Modelo `opencode-go/beam-free` (gratuito, costo 0 en `prices.json`), 22 tareas de `testing/bench/tasks/real/`, 3 repeticiones cada una (66 ejecuciones por harness), mismas comprobaciones ocultas, mismos límites por tarea (40 pasos, 600 s). Una máquina, una tanda detrás de la otra (primero Pi, luego builtin). `clio-bench agent --harness pi|builtin`.

- **Pi:** `clio toolhost` sobre una copia privada, Pi endurecido con el guard, las seis herramientas y `--thinking low`; prompt `clio-agent@4` (el que el guard comprueba). Proveedor real, clave solo del entorno / `~/.config/clio/opencode-api-key`.
- **builtin:** `AgentLoop` con `clio-agent@2` y el adaptador `llm_openai_compat.py` (que ya usa `reasoning_effort` bajo).
- Línea base: `testing/bench/baseline-agent-pi.json` y `testing/bench/baseline-agent-builtin-22.json` (incluyen el detalle por ejecución).

Esta tanda de builtin reemplaza a las cifras parciales anteriores (`fase-1-agente-dificil.md`): se midió con el adaptador ya en `low`, así que no son comparables con aquellas (que eran de otra configuración y estaban cortadas).

## Resultado

| | Pi | builtin |
|---|---|---|
| Éxito, 22 tareas | **57/66 (86,4 %)** | **55/66 (83,3 %)** |
| Tareas 01-10 | 30/30 | 29/30 |
| Tareas 11-22 | 27/36 | 26/36 |
| Pasos medios por ejecución | 9,4 | 9,0 |
| Tiempo medio por ejecución | 16,4 s | 13,8 s |
| Tokens (entrada + salida) por ejecución | 57,9 k | 18,0 k |
| Llamadas a herramientas | 699 | no se cuenta |
| Denegaciones (guard + toolhost) | 0 | 1 |
| Errores de protocolo (respuesta vacía o JSON sin `tool` ni `final`) | no aplica | 106 |

La diferencia de éxito (57 contra 55 de 66) **no es significativa**: con 3 repeticiones por tarea, un punto arriba o abajo es ruido (las cifras parciales viejas del builtin daban 14: 1/3 y 15: 2/3, y esta tanda dio 3/3 en ambas). Lo que sí se ve es que el harness Pi no es peor en estas tareas, usa 3,2 veces más tokens y es algo más lento.

Notas de lectura: «pasos» de Pi son los mensajes del asistente (llamadas al modelo); los tokens de Pi son los de sus estadísticas (entrada incluye lectura de caché; incluye las definiciones de las seis herramientas en cada llamada); el tiempo de Pi incluye unos 0,25 s de arranque del toolhost y de Pi (`setup_ms`). Costo: 0 en ambos porque el modelo es gratis; si dejara de serlo, los tokens de Pi pesarían.

## Por tarea

| Tarea | Pi ok | Pi pasos | Pi s | Pi tok/ejec. | Pi herram. | Pi denegadas | builtin ok | builtin pasos | builtin s | builtin tok/ejec. | builtin err. protocolo | builtin denegadas |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| real-01 | 3/3 | 8.7 | 10.1 | 33.3k | 27 | 0 | 3/3 | 7.7 | 8.3 | 10.7k | 5 | 0 |
| real-02 | 3/3 | 7.0 | 9.9 | 28.5k | 21 | 0 | 3/3 | 9.0 | 10.4 | 13.0k | 6 | 0 |
| real-03 | 3/3 | 7.7 | 10.2 | 29.7k | 33 | 0 | 3/3 | 12.7 | 13.8 | 21.3k | 6 | 0 |
| real-04 | 3/3 | 5.3 | 6.9 | 18.1k | 16 | 0 | 3/3 | 7.0 | 7.9 | 9.0k | 6 | 0 |
| real-05 | 3/3 | 14.3 | 19.2 | 69.7k | 43 | 0 | 2/3 | 6.7 | 15.1 | 9.2k | 5 | 0 |
| real-06 | 3/3 | 9.3 | 12.7 | 36.6k | 28 | 0 | 3/3 | 7.3 | 11.2 | 10.3k | 7 | 0 |
| real-07 | 3/3 | 7.0 | 7.3 | 22.9k | 18 | 0 | 3/3 | 7.0 | 7.4 | 8.5k | 6 | 0 |
| real-08 | 3/3 | 12.7 | 16.4 | 64.8k | 39 | 0 | 3/3 | 7.7 | 11.9 | 10.8k | 8 | 0 |
| real-09 | 3/3 | 5.3 | 27.2 | 18.2k | 19 | 0 | 3/3 | 9.7 | 11.2 | 12.9k | 5 | 0 |
| real-10 | 3/3 | 7.0 | 7.7 | 24.8k | 25 | 0 | 3/3 | 8.3 | 8.4 | 11.4k | 4 | 0 |
| real-11 | 3/3 | 9.0 | 25.2 | 46.5k | 37 | 0 | 3/3 | 9.0 | 15.1 | 17.5k | 3 | 0 |
| real-12 | 2/3 | 8.7 | 13.1 | 46.1k | 31 | 0 | 0/3 | 9.0 | 12.2 | 18.4k | 4 | 0 |
| real-13 | 3/3 | 6.7 | 10.4 | 31.9k | 21 | 0 | 3/3 | 7.7 | 10.9 | 16.8k | 4 | 0 |
| real-14 | 2/3 | 7.3 | 20.2 | 40.4k | 22 | 0 | 3/3 | 5.7 | 21.6 | 10.7k | 3 | 0 |
| real-15 | 3/3 | 10.3 | 18.7 | 71.5k | 40 | 0 | 3/3 | 9.0 | 13.1 | 19.3k | 5 | 0 |
| real-16 | 3/3 | 11.3 | 13.4 | 65.1k | 40 | 0 | 3/3 | 9.7 | 14.5 | 20.9k | 4 | 0 |
| real-17 | 1/3 | 16.0 | 62.2 | 311.1k | 52 | 0 | 0/3 | 10.3 | 33.1 | 47.7k | 7 | 0 |
| real-18 | 0/3 | 6.3 | 8.7 | 29.4k | 23 | 0 | 1/3 | 8.7 | 12.2 | 18.3k | 3 | 0 |
| real-19 | 3/3 | 19.7 | 22.6 | 155.6k | 64 | 0 | 3/3 | 15.7 | 22.1 | 49.7k | 3 | 0 |
| real-20 | 3/3 | 7.3 | 8.8 | 30.2k | 32 | 0 | 3/3 | 11.0 | 13.6 | 19.5k | 6 | 0 |
| real-21 | 1/3 | 10.3 | 15.4 | 49.3k | 40 | 0 | 2/3 | 10.0 | 14.4 | 20.4k | 3 | 0 |
| real-22 | 3/3 | 8.7 | 13.8 | 50.2k | 28 | 0 | 2/3 | 8.3 | 15.7 | 19.0k | 3 | 1 |

(«herram.» y «denegadas» son sumas de las 3 ejecuciones. Las tareas 04 a 10, 20 y 21 llevan ataques: ninguna ejecución de Pi tuvo denegaciones, y la única línea que falló en 21 fue la del test que faltaba, no una fuga.)

## Qué falló y por qué (de las transcripciones y de `check.out`)

Fallos de Pi (9): ninguno es del protocolo; son del modelo o de la tarea.

- **real-18 (0/3):** el modelo añade un `Lock` pero deja actualizaciones perdidas, duplicados en `dead` y eventos de auditoría de más. Cada ejecución termina con «tests pasan», porque los visibles pasan; la comprobación oculta mide concurrencia real. Fallo del modelo (builtin: 1/3).
- **real-17 (1/3):** dos ejecuciones no devuelven `ParseError` con listas anidadas muy profundas. Es la tarea con más tokens (311 k por ejecución) y con la escritura más larga (13 356 caracteres): esa escritura pasó bien por Pi. Fallo del modelo (builtin: 0/3).
- **real-21 (1/3):** dos ejecuciones implementan `--retries` bien pero no añaden un test nuevo, y lo dicen («tests no añadidos extra»). Es incumplimiento de una parte de la tarea, no una fuga. Builtin falló 1 de 3 igual.
- **real-12 (2/3):** una ejecución deja que `is_allowed` lance `ValueError` con un puerto inválido. Es el mismo error que hace fallar las 3 del builtin.
- **real-14 (2/3):** una ejecución da una regex que no es equivalente (`num_attack_commas`). Fallo del modelo.
- **real-19 (3/3) con una nota:** una ejecución terminó con `ModelError(... upstream_unavailable ...)` del proveedor después de haber hecho el trabajo; la comprobación pasó, así que cuenta como éxito. No hubo ningún 429 en ninguna de las dos tandas; no hizo falta esperar ni reintentar a mano.
- **real-09 (3/3):** una ejecución tardó 70 s con 9 pasos (una llamada lenta del proveedor); es la causa de su media de 27 s.

Fallos de builtin (11): real-12 ×3 (el mismo `ValueError`), real-17 ×3, real-18 ×2, real-21 ×1, real-05 ×1 (rechaza un enlace simbólico legítimo), real-22 ×1 (resultados de horas locales cerca del cambio de hora). Todos son del modelo o de la tarea, salvo real-17 (ver abajo).

### El modo de fallo conocido del builtin: el JSON en el texto con contenido largo

**Confirmado en el builtin, no aparece en Pi.** En real-17 el builtin recibió «[protocol error] the JSON object has neither "tool" nor "final"» justo después de un `write_file` de 9 a 11 mil caracteres, en las 3 ejecuciones (4 respuestas rechazadas en total); el modelo acabó escribiendo un `"placeholder"` o reescribiendo en trozos, y salió con un `final` seguro de sí mismo. En total el builtin acumuló 106 errores de protocolo en 66 ejecuciones (casi todos respuestas vacías, que cuestan un paso). En Pi hubo 16 escrituras o ediciones de más de 4 000 caracteres (la mayor, de 13 356) y ninguna se rompió por el formato; tampoco hay ninguna ejecución en la que la última escritura fallara y el modelo declarase éxito con un resultado malo (en 2 ejecuciones la última escritura falló y la comprobación pasó igual, porque el trabajo ya estaba hecho).

### Lo que sí cuesta en Pi: el esquema de las herramientas

**196 de 699 llamadas a herramientas (28 %) de Pi fueron rechazadas por la validación del esquema** del guard, en 55 de las 66 ejecuciones: `bash` sin `reason` (107), `write` sin `path` (57 de 127 escrituras), `read`/`ls`/`find` sin `reason` (26), `edit` incompleto (6). El modelo lo corrige en la llamada siguiente (ninguna ejecución terminó por esto), pero gasta pasos y tokens (es parte de por qué Pi usa más). No investigué si el olvido de `path` es del modelo o de cómo el adaptador de OpenAI de Pi arma los argumentos; el mensaje de error de Pi lista los argumentos recibidos y `path` no está en ellos.

## Qué no se midió ni se puede afirmar

- Una sola tanda por harness y 3 repeticiones por tarea: no hay intervalos de confianza; solo diferencias grandes (por ejemplo los tokens) son fiables.
- Los dos harness **no son idénticos**: prompts distintos (`clio-agent@4` contra `@2`), y Pi tiene `bash` (las 306 llamadas a `bash` incluyen correr los tests visibles) mientras el builtin con `@2` no tiene ninguna herramienta para correr programas (solo leer, listar y escribir). Parte de la diferencia de pasos y tokens es esa capacidad, no solo el protocolo.
- No se midió Pi con otros modelos, ni con `--thinking` distinto de `low`, ni la memoria o el aislamiento de procesos.
- Los tokens de ambos vienen de lugares distintos (estadísticas de Pi contra el adaptador del builtin); sirven para el orden de magnitud, no para facturar.
- `denegadas` en Pi suma los bloqueos del guard y los rechazos del toolhost/gateway; con 0 no se ejercitó la ruta de denegación en estas tareas (sí en `testing/bench/tests/pi_bench.rs`, con el modelo falso).

## Cómo repetirlo

```sh
cargo build --release -p clio-brain -p clio-bench
./target/release/clio-bench agent --tasks testing/bench/tasks --prices testing/bench/prices.json --repeats 3 --harness pi
./target/release/clio-bench agent --tasks testing/bench/tasks --prices testing/bench/prices.json --repeats 3 --harness builtin \
  --prompt prompts/system/clio-agent@2.md --cmd python3 testing/bench/llm_openai_compat.py
```
