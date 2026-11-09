# Plan de la fase 2 · Memoria

Escrito el 2026-11-09 y precisado el mismo día después de revisar el código. La memoria vive desde el 2026-11-04 en su propio repositorio, [clio-memory](https://github.com/i-montes/clio-memory) (hoy v0.7.1), y Clio la usa fijada por el commit de un tag. Este plan dice qué queda de la fase 2 de [`lista-maestra.md`](../lista-maestra.md) (tareas 300 a 344), qué ya está cubierto, qué se aplaza y cómo se hace lo demás. Se ejecuta con la persona desconectada; lo que solo ella puede hacer o decidir queda al final.

## Reglas de trabajo

- Cada paso va en su rama y su PR, en el repositorio que toca. **Ningún PR se fusiona sin la persona** (CONTRIBUTING: ningún agente aprueba ni fusiona; los `Cargo.toml` y `.github/` los revisa una persona).
- Antes de cada PR: `just check`, `just test` y `just audit` en Clio; `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings` y `cargo test` en clio-memory.
- Los datos de los bancos son inventados. La memoria real de la persona (`~/.local/share/clio/memory.db`) no se lee ni se toca.
- Los números se miden con el modelo de vectores que usa la persona (Granite 311M, recortado a 384), bajado con `clio-memory setup model`, que lo fija y comprueba su sha256. Donde no hay modelo (el CI), las pruebas usan un vectorizador falso y solo comprueban que el arnés funciona, no los números.
- Commits en inglés, con el estilo del repositorio (`docs:`, `feat(memory):`, `test:`…).

## Estado de partida (verificado contra el código el 2026-11-09)

18 de 45 tareas marcadas. La lista se escribió antes del rediseño del 2026-11-04 («la memoria es del agente», S-25 y S-61 reescritas), así que varias tareas dicen algo que ya no es el diseño.

| Tareas | Situación real | Evidencia |
|---|---|---|
| 311 | Hecha, con otro alcance: el MCP sirve leer, guardar, corregir, olvidar, fijar, unir, el resumen de sesión y deshacer lo propio; no hay herramienta de retroalimentación (las señales de uso salen de leer) | `clio-memory/src/tools.rs` (13 herramientas) |
| 312, 313 | El texto de la lista es el S-25 viejo. El S-25 de hoy: PROMOTE, PURGE y el REVERT **de lo ajeno** nunca se exponen; `memory_revert` sí existe, solo para los cambios del propio agente | `docs/security.md` (S-25, S-60, S-65); pruebas `// S-25` en `crates/clio-brain/tests/memory_mcp.rs` y `pi_stack/memory.rs` |
| 335 | Hecha | ADR 0010 y sus tres etapas; Pi tiene 12 herramientas de memoria por el tool host |
| 323 | El lote al cerrar existe; la memoria de trabajo inmediata la cubren `memory_save` y `memory_session_summary` durante la sesión | S-61 |
| 326, 327 | Marcadas como hechas con el S-26 viejo («pendiente hasta aprobar»). Hoy el procedimiento entra activo, marcado con su origen | `docs/security.md` (S-26) |
| 310, 314, 315, 316, 304 | Se aplazan a la fase 4 | El daemon HTTP no hace falta mientras Clio use la librería dentro del servicio y los demás agentes usen MCP por stdio |
| 303 | Sin decidir | Ver paso A |
| 305 | Sin medir | `src/vector.rs`: búsqueda plana en memoria, sin índice |

## Paso A · Poner la lista al día (repositorio clio, rama `docs/fase-2-plan`)

**Entrega:**
1. Este plan.
2. `lista-maestra.md`, tareas 300 a 344:
   - Marcar 311 y 335 con la nota de qué se hizo.
   - Reescribir 312 y 313 con el S-25 de hoy y marcarlas solo si sus pruebas existen y pasan (verificar cada una).
   - Anotar en 326 y 327 que el S-26 cambió.
   - Dar por cerrada 323 con su motivo.
   - Aplazar 304, 310 y 314 a 316 a la fase 4: se quedan en su sitio con la nota «aplazada» y su motivo, como hace la fase 1, porque el número es su identidad.
   - Corregir 324, que también dice el S-25 viejo.
3. ADR 0011, cifrado en reposo (tarea 303), con la decisión por defecto: sin SQLCipher. El archivo ya es 0600 en una carpeta 0700 (S-23), el disco del equipo es LUKS (`infra/local/check-host.sh`) y las copias van cifradas por restic (tarea 302, con 093 y 094). Se reabre si la memoria sale de este equipo (un VPS, una sincronización). La decisión también va a `decisiones-pendientes.md`.

**Hecho cuando:** el recuento de la fase 2 refleja el código, `just check` pasa (la cobertura de reglas lee `security.md`) y el PR está abierto.

## Paso B · Banco de evaluación de la memoria (repositorio clio-memory, rama `feat/recall-eval`) · tareas 336, 337 y 338

Hoy los umbrales de `recall` (`min_vector_score` 0,30 y `min_text_coverage` 0,5, en `src/model.rs`) se fijaron con 12 recuerdos y un modelo que ya no es el principal. El propio código dice que el arnés debe afinarlos.

**Entrega:**
1. Datos en `bench/recall/`:
   - `memories.jsonl`: unos 150 recuerdos inventados en español, del tipo que guarda un agente de verdad (perfil, preferencias, decisiones, arquitectura, errores y su causa, lecciones, configuración), con título, categoría y ámbito (lo personal, dos proyectos y las lecciones).
   - `queries.jsonl`: unas 150 preguntas.
     - Paráfrasis con respuesta conocida (una o dos memorias correctas).
     - Preguntas que mezclan términos en inglés con recuerdos en español y al revés («scope» frente a «ámbito»).
     - Preguntas sin respuesta en el banco, unas del mismo dominio y otras de fuera, para la abstención.
     - Preguntas que solo se responden desde otro proyecto.
   - `duplicates.jsonl`: unas 30 reformulaciones de recuerdos que ya están, para medir cuántas veces el motor contesta NOOP o «ya lo sé» en vez de duplicar.
2. Un ejemplo, `examples/recall_eval.rs`, que carga el banco en una memoria temporal con el modelo (Granite, o potion, o el falso) y calcula:
   - acierto en el primer puesto, recall@5 y MRR;
   - precisión y cobertura de la abstención;
   - tasa de NOOP y tasa de revisión (lo que no queda activo al guardar);
   - un barrido de `min_vector_score` y de `min_text_coverage` que muestra dónde está el mejor equilibrio entre responder y abstenerse.

   Imprime una tabla y escribe el JSON del informe.
3. Una prueba que corre el arnés con el vectorizador falso sobre una parte del banco, para que no se rompa sin que nadie lo vea.
4. `bench/recall/results.md`: los números con Granite en esta máquina y la propuesta de umbrales.

Si los números lo justifican, el cambio de los valores por defecto va en un commit aparte del mismo PR, con su motivo. Si no lo justifican, se dice por qué.

**Hecho cuando:** el ejemplo corre con Granite y con el falso, la prueba pasa en el CI y `results.md` tiene los números.

## Paso C · Mediciones

### C1 · Escala del índice vectorial (tarea 305; clio-memory, misma rama o `bench/vector-scale`)
Medir con vectores sintéticos de 384 dimensiones a 5 000, 50 000 y 500 000 recuerdos:
- la latencia de `recall` de extremo a extremo;
- la memoria que ocupa la copia de vectores;
- el tiempo de abrir la base y cargar la copia.

**Decide:** si la búsqueda plana alcanza (por ejemplo, menos de 100 ms a 50 000) o si hace falta un índice. El resultado y la decisión van en `bench/vector-scale/results.md` y en la tarea 305.

### C2 · Escrituras concurrentes (tarea 342, D11; clio-memory, rama `test/concurrent-writes`)
Varios procesos que escriben a la vez sobre el mismo archivo: sesiones MCP, el CLI y el sueño. Se mide:
- si se pierde alguna escritura;
- si se duplica un mismo contenido;
- la latencia de escribir con el candado ocupado (p50 y p99);
- las escrituras que fallan por tiempo de espera.

El brazo «sin gestor único» son escritores que abren el SQLite sin el candado del motor, para ver qué rompe el candado cuando falta.

**Supuesto:** el artefacto de diseño que define D11 no está en el repositorio. Se mide esta interpretación y se dice así en el informe.

### C3 · Tareas de varias sesiones (tarea 341, D10; repositorio clio, rama `feat/bench-memory-sessions`)
Hoy el modo `agent --harness pi` del banco corre sin memoria a propósito (`testing/bench/src/pi_bench.rs` pasa `None`).

**Entrega:**
- Una forma de tarea con varias sesiones seguidas sobre la misma memoria temporal. La primera sesión enseña algo (una convención, una preferencia o la causa de un error); la segunda tiene que aplicarlo sin que se lo repitan.
- Una bandera `--memory` que conecta la memoria al tool host del banco.
- Unas 8 tareas con su comprobación oculta.
- Prueba de la mecánica con el proveedor falso.

**La corrida real necesita la clave del proveedor**, que esta máquina no tiene: queda para la persona (ver al final).

## Paso D · Conectar el triaje y el enrutador (331, 332), solo si C3 da positivo
Depende del control sobre el camino Pi (ADR 0008), que todavía no existe. No entra en esta tanda.

## Paso E · Aplazadas
| Tarea | Qué | Cuándo |
|---|---|---|
| 302 | Respaldos y exportaciones cifrados de la memoria | Con 093 y 094 (restic): necesita un destino fuera del equipo |
| 306, 307 | Bajar la RAM del modelo estático (potion) | Granite es el principal; solo si potion vuelve a importar |
| 308 | Fuzzing del tokenizador y de los importadores (S-48) | Cuando clio-memory tenga su `fuzz/` y su trabajo nocturno |
| 339 | LongMemEval y LoCoMo | Solo para comparar con otros motores; son en inglés |
| 340 | Decisores en modo sombra | Después de C3 |

## Orden

```
A ─► B ─► C1 ─► C2 ─► C3 (mecánica) ─► [persona: corrida real de C3] ─► 343/344 ─► D
```

A, B, C1 y C2 no necesitan a la persona. C3 se construye y se prueba sin ella, pero el número que cierra la fase sí la necesita.

## Lo que queda para la persona al final

1. Revisar y fusionar los PRs (A en clio; B, C1 y C2 en clio-memory; C3 en clio).
2. La clave del proveedor en `~/.config/clio/opencode-api-key` (0600) de esta máquina, o correr C3 en su PC: `cargo run --release -p clio-bench -- agent --harness pi --memory …`.
3. Confirmar o cambiar:
   - el cifrado en reposo (ADR 0011);
   - los umbrales nuevos de `recall`, si B propone moverlos;
   - el aplazamiento del daemon a la fase 4.
4. Si sube la versión de clio-memory, subir el pin de Clio al commit del tag nuevo (un `Cargo.toml`: lo revisa la persona).
