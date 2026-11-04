# Clio frente a Engram, punto por punto

Comparación hecha el 2026-11-04 contra `DOCS.md` y `README.md` de [Gentleman-Programming/engram](https://github.com/Gentleman-Programming/engram) (las 27 herramientas MCP y todos los subcomandos de la CLI, leídos completos), sobre las ramas `feat/memory-*` (PRs #29 a #39). «Persona» marca lo que en Clio solo hace la persona por decisión de seguridad (S-25, S-65): el modelo no puede borrar, fusionar sin permiso ni tocar lo que ella escribió. Cada fila nombra dónde está y qué lo prueba.

## Herramientas MCP de Engram

| Engram | Clio | Dónde / prueba |
|---|---|---|
| `mem_search` | `memory_search` (proyecto, personal y lecciones; vectores + texto) | `memory_port.rs search`; `tests/memory_port.rs`, `tests/memory_mcp.rs` |
| `mem_search_prompts` | No: la persona decidió no guardar sus mensajes (Pi ya los tiene) | diseño, decisión 3 |
| `mem_save` (topic_key, conflictos) | `memory_propose` (título, categoría, Jev decide nuevo/reemplazo/ya sabido; relaciones) | #30, #32 |
| `mem_update` | `memory_update` (Jev decide; lo de la persona o fijado solo con sus palabras) | #36; `an_update_*` |
| `mem_review` | Lo vencido sale marcado y en `memory_context`; reiniciar el plazo es de la persona (`clio memory reviewed`) | #35; `what_is_past_its_review_cycle_*` |
| `mem_pin` / `mem_unpin` | `memory_pin` (palabras de la persona + Jev); soltar: `clio memory unpin` (persona) | #32 |
| `mem_suggest_topic_key` | No hace falta: el reemplazo lo decide Jev entre las memorias parecidas | diseño, etapa 2 |
| `mem_delete` | `memory_forget` archiva (palabras + Jev); borrar para siempre es PURGE, solo de la persona | S-25, S-65 |
| `mem_save_prompt` | No: decisión de la persona | diseño, decisión 3 |
| `mem_context` | `memory_context` (resumen de la última sesión, fijado, reciente, vencido) | #30, #31, #35 |
| `mem_stats` | `clio memory stats`, `clio memory projects`, `clio doctor` (persona) | #34 |
| `mem_timeline` | `memory_timeline` | #30; `the_timeline_is_the_session_*` |
| `mem_get_observation` (+ versiones) | `memory_get`; versiones: `clio memory history ID` (persona) | #38 |
| `mem_session_summary` | `memory_session_summary` | #31 |
| `mem_session_start` / `_end` / `_finalize` | El cerebro abre y cierra la sesión: pide el resumen al compactar y al cerrar, y el servicio solo deja guardarlo en el cierre | #31; `closing_a_session_*` |
| `mem_capture_passive` | No: lo cubren el resumen de sesión y las propuestas con su forma `Qué · Por qué · Dónde · Aprendido` | — |
| `mem_merge_projects` | `clio memory projects merge DE EN` (persona): una operación `Move` del registro por memoria, deshacible con `revert` | #39; `a_person_moves_a_memory_*`, `the_person_merges_*` |
| `mem_current_project` | `memory_current_project` (MCP) y `clio memory project` | #33, #35 |
| `mem_list_projects` | `clio memory projects` (persona) | #34 |
| `mem_doctor` | `clio doctor` (incluye la memoria; nunca la escribe) | #34 |
| `mem_judge` / `mem_judge_many` / `mem_compare` | Jev juzga la relación al guardar (`contradice a mN`); `clio memory conflicts` las lista | #32, #38 |
| `mem_consolidate` | `memory_consolidate` (palabras + Jev confirma que la unión no añade nada) | #37 |

## Subcomandos de la CLI de Engram

| Engram | Clio |
|---|---|
| `setup [agent]` | `clio memory setup <agente>` (imprime, no escribe) |
| `serve` (HTTP) | No: la persona eligió MCP para otros agentes |
| `mcp` | `clio memory mcp` (leer y proponer) |
| `tui` | `clio` (pantalla de memoria: sitio, título, revisar, `y` copia) |
| `test` (autopruebas) | `cargo test` y `clio doctor`; no hay autoprueba en tiempo de ejecución |
| `init [name]` | `clio memory project set NOMBRE` (`.clio-project`) |
| `search` / `save` | `clio memory search` / `clio memory remember` |
| `delete <id>` | `clio memory archive` (reversible) y `clio memory purge` (para siempre) |
| `delete session` / `delete prompt` | No aplica: no hay sesiones ni mensajes guardados aparte |
| `delete project` | `clio memory projects archive NOMBRE` (archiva todo, reversible; borrar para siempre sigue siendo por recuerdo con `purge`) |
| `timeline` / `context` / `stats` | `clio memory timeline` / `clio memory context` / `clio memory stats` |
| `export` / `import` | `clio memory export` / `clio memory import` (a una memoria vacía) |
| `sync`, `cloud …`, `obsidian-export` | No: la persona decidió no sincronizar por ahora |
| `conflicts list/show/…` | `clio memory conflicts` (y `show` dice las relaciones) |
| `doctor` | `clio doctor` |
| `projects list/alias/merge/…` | `clio memory projects` y `projects merge` (también renombra: unir con un nombre nuevo); `alias` lo cubre `.clio-project` |
| `version` | `clio --version` |

## Lo que tiene Clio y Engram no

Vectores e híbrido, registro de operaciones con REVERT y reconstrucción, PURGE con lápida, cuarentena, cuotas por actor, detector de secretos y filtro del baúl, validez temporal, Jev como juez de cada escritura, lecciones entre proyectos, envoltura de datos no confiables y marca de sesión contaminada (S-05), modo cierre de sesión.

## Lo pendiente

Nada de lo que la persona no descartó. Lo descartado: sincronizar entre máquinas, la nube, Obsidian, guardar sus mensajes (`mem_save_prompt`, `mem_search_prompts`) y la API HTTP (eligió MCP). Distinto a propósito: borrar, fusionar y fijar desde el agente piden las palabras de la persona y a Jev; borrar para siempre (PURGE) es solo de la persona.
