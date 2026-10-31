# Lista maestra de Clio

Todo lo que pide el artefacto «Clio · diseño del agente», de lo primero a lo último: diseño, evidencia, memoria, seguridad y los 13 specs (contrato, triaje, red, políticas, programador, diagnóstico, pasarela, vault, preproducción, despliegue, modelos, PWA, día cero). Es una lista, no un plan: cada punto es una cosa que hay que hacer, para que pases por él y hagas su plan con el artefacto o el spec delante.

**Cómo leerla**
- El número es corrido: se puede decir «el punto 143».
- `S-xx`: regla de seguridad (la regla trae su propia prueba). `D1`–`D25`: decisión de la pestaña «La evidencia». `prueba`: prueba de aceptación del spec. `medir`: experimento de «Cómo lo comprobaría». `decidir`: decisión abierta del spec.
- Una regla de seguridad entra en la misma fase que lo que protege, no al final. El orden dentro de cada fase va de lo que desbloquea a lo que depende.
- `[x]` es lo que ya está en el repo (clio-memory v1). Todo lo demás está sin hacer.

---

## Ya hecho (no repetir)

### clio-memory v1

- [x] **001** Registro de operaciones como fuente de verdad y proyección reconstruible (prueba 1)
- [x] **002** Un solo camino de escritura: `propose` con actor y cuota
- [x] **003** Mismo contenido dos veces: ADD y luego NOOP (prueba 2)
- [x] **004** Detector de secretos: la llave se rechaza y no deja rastro (prueba 3)
- [x] **005** Cuarentena para lo externo y PROMOTE para sacarlo (prueba 4)
- [x] **006** Ámbitos aislados (prueba 5)
- [x] **007** Dos fechas por hecho: consulta por vigencia y por «sabido desde» (pruebas 6 y 17)
- [x] **008** Sin plugins, ningún DELETE se ejecuta solo (prueba 7)
- [x] **009** Operación y trabajos en la misma transacción (prueba 8)
- [x] **010** Diez actualizaciones, un solo trabajo de embedding (prueba 9)
- [x] **011** REVERT de operación y REVERT por actor (pruebas 10 y 15)
- [x] **012** Abstención cuando nada es relevante (prueba 12)
- [x] **013** Bloques fijos y `context()` dentro de presupuesto, sin cortar a la mitad (prueba 13)
- [x] **014** Memorias parecidas pero distintas conviven, sin marca de conflicto (prueba 14; el spec del repo reemplazó la del artefacto)
- [x] **015** Dueño único del archivo con bloqueo y error claro (prueba 16)
- [x] **016** Olvido determinista con reloj inyectable (prueba 18)
- [x] **017** Tres señales de «equivocado» mandan a revisión (prueba 19)
- [x] **018** Formato compacto: la mitad o menos de tokens que JSON (prueba 20)
- [x] **019** Telemetría como feature: sin tablas ni tramos si está apagada, sin texto de memorias, `trace_id` por operación (pruebas 21, 22, 24)
- [x] **020** Recall con 50 000 recuerdos en menos de 150 ms, corrido en el CI (prueba 25)
- [x] **021** Cola de trabajos con dedupe, arrendamientos y reintentos
- [x] **022** Exportar e importar JSONL con semver; respaldo en caliente con la API de SQLite
- [x] **023** CLI para inspeccionar y mantener, más `remember` y el ejemplo `init`
- [x] **024** Archivo con permisos 0600 comprobado al abrir (S-23, parte)
- [x] **025** Embedder potion-multilingual-128M en CPU que se descarga tras un rato ocioso
- [x] **026** Licencia MIT OR Apache-2.0, nombre y umbrales decididos (ADR 0004)
- [x] **027** Prueba de que clio-memory no depende de ningún otro crate de Clio
- [x] **028** Skill y script para usar clio-memory como memoria de Claude (dogfooding)
- [x] **029** Monorepo con los 12 crates, hook de secretos, `deny.toml`, `rule-coverage.sh` y CI con matriz Linux/macOS

---

## Fase 0 · Cimientos (todavía no hay agente)

### Terreno

- [x] **030** Subir el commit local `8cd2730` (prueba de independencia de clio-memory)
- [x] **031** Arreglar el CI: todas las ejecuciones terminan en `startup_failure` (revisar facturación y permisos de Actions)
- [x] **032** Confirmar que el CI corre en verde en Linux y macOS tras arreglarlo
- [x] **033** Confirmar que `cargo-deny` y `cargo-audit` corren en el CI y que fallan con una vulnerabilidad sembrada (S-41)
- [x] **034** Confirmar que `Cargo.lock` está versionado y que el CI falla si cambia sin revisión (S-41)
- [x] **035** Mantener `unsafe_code = forbid` y exigir revisión explícita para cualquier excepción (S-41)
- [x] **036** Probar el hook de pre-commit con una llave falsa: el commit se rechaza (S-43)
- [x] **037** Pasar cada spec del artefacto a `docs/specs/<crate>-spec-v0.1.md` con su sección «Pruebas de aceptación» numerada, para que `rule-coverage.sh` las cuente
- [x] **038** Pasar la pestaña de seguridad completa a `docs/security.md` y mantenerla sincronizada con el artefacto
- [ ] **039** Reconciliar el artefacto con el spec del repo en conflictos (el artefacto aún dice «en conflicto»)
- [x] **040** Hacer que `rule-coverage.sh` bloquee en vez de avisar, regla por regla, a medida que cada regla tenga prueba (S-04)

### Documentos de proyecto

- [x] **041** `SECURITY.md` con canal privado de reporte (S-48)
- [x] **042** Modelo de amenazas publicado: qué se protege y de qué (S-48)
- [x] **043** Documentar la regla de que cada PR nombra las reglas S-xx que toca y sus pruebas
- [x] **044** Documentar la regla de que los cambios en pasarela, vault, firmas de nodos y PWA los revisa la persona, no un agente

### Preproducción mínima

- [x] **045** `testing/fake-llm`: grabar respuestas reales por hash de la petición normalizada
- [x] **046** `testing/fake-llm`: reproducir las grabadas, sin red y sin costo
- [x] **047** `testing/fake-llm`: avisar y fallar si una petición nueva no tiene grabación
- [x] **048** `testing/fake-llm`: pasar las grabaciones por el detector de secretos antes de guardarlas
- [x] **049** prueba: dos corridas en modo reproducción dan exactamente el mismo resultado
- [x] **050** Reloj inyectable en `clio-core` para todo lo que dependa del tiempo (clio-memory conserva el suyo; sin dependencia entre ambos)
- [x] **051** `testing/canaries`: generar secretos falsos únicos por corrida
- [x] **052** `testing/canaries`: escáner que busca los señuelos (en claro, base64 y codificados para URL) en trazas, logs, payloads, memoria y contexto
- [x] **053** prueba: un señuelo que aparece en una traza hace fallar la compuerta (`a_decoy_in_a_trace_fails_the_gate`; el CI corre `cargo test --workspace`)
- [x] **054** Datos sintéticos: generador de memoria y archivos de prueba, nunca datos reales
- [ ] **055** Preproducción aislada: sin acceso a dispositivos reales ni al vault real (prueba al final de la fase 1) — se prueba con el vault y los nodos de la fase 1; aún no hay nada que aislar
- [x] **056** Decidir si Jev, el push y los proveedores falsos se escriben ahora o con su fase (se escriben en las fases 4 y 6)

### clio-trace mínimo

- [x] **057** Esquema SQLite de tramos con ids, tamaños, tiempos y decisiones, nunca texto (D15, D23)
- [x] **058** Capa propia de nombres encima de las convenciones `gen_ai.*` de OpenTelemetry (las convenciones aún cambian)
- [x] **059** Tokens de entrada y salida por llamada al modelo
- [x] **060** Tabla de precios versionada y costo calculado con modelo, proveedor y tokens
- [x] **061** `traceparent` que viaja por cada tramo y mensaje
- [x] **062** Archivo aparte (`clio-trace.db`) con permisos 0600, separado de la memoria
- [x] **063** Contenido de prompts apagado por defecto (S-46)
- [x] **064** prueba: con el contenido apagado, ningún tramo trae texto (S-46)
- [x] **065** Guardar la versión de cada prompt usada en cada traza
- [x] **066** Registrar cada cambio de estado de un contrato en la traza

### Banco de evaluación

- [ ] **067** Elegir las 10 tareas reales (tuyas), el banco propio de D15: éxito comprobable por señal externa, no por el modelo — pendiente de ti; ver `testing/bench/tasks/real/README.md`
- [x] **068** Escribir los 20 pedidos triviales que acompañan al banco
- [x] **069** Formato de una tarea del banco: entrada, criterio, comprobación, tope de pasos, costo y tiempo
- [x] **070** Runner del banco que repite cada tarea varias veces
- [x] **071** Runner del banco que devuelve éxito, costo y tiempo, en modo reproducción y en modo real
- [x] **072** Guardar cada corrida del banco en clio-trace
- [x] **073** Guardar una línea base como referencia fija
- [ ] **074** Fijar los umbrales de la compuerta con las primeras mediciones (decidir) — hay valores provisionales en `testing/bench/thresholds.json`; faltan corridas reales

### Compuerta en el CI

- [x] **075** Cada PR: unitarias, aceptación y ataques, en reproducción; bloquea si falla cualquiera
- [ ] **076** Cada noche: banco con modelo real, caos y fuzzing; bloquea si el éxito baja o el costo por tarea sube más del umbral — escrito en nightly.yml; falta definir la variable `CLIO_BENCH_CMD` y una línea base real (`baseline-real.json`)
- [ ] **077** Antes de una versión: todo lo anterior y la revisión humana de pasarela, vault y firmas — escrito en release.yml; falta crear el entorno `release-review` con la persona como revisora obligatoria
- [x] **078** Un cambio de prompt, de modelo o de política pasa por la misma compuerta
- [x] **079** prueba: un cambio de prompt que no pasó el banco no se puede desplegar
- [x] **080** Esqueleto de la batería de ataques en `testing/attacks`: formato de caso y runner
- [x] **081** Regla de proceso: cada incidente nuevo agrega un caso a la batería
- [x] **082** Esqueleto del arnés de caos: red cortada, VPS caído, disco lleno, proveedor caído, vault bloqueado
- [x] **083** Esqueleto de fuzzing con `cargo-fuzz`, corrido de noche (S-48)

### Cierre de la fase 0

- [x] **084** El banco corre en modo reproducción y devuelve un número: éxito, costo y tiempo

---

## Fase 1 · Corte mínimo: un nodo (el VPS), sin memoria, a propósito

### VPS e infraestructura (S-38 a S-40)

- [ ] **085** Infraestructura como código en `infra/` para reconstruir el VPS desde cero
- [ ] **086** Sin puertos públicos: solo Tailscale (S-38)
- [ ] **087** SSH desactivado o solo por Tailscale (S-38)
- [ ] **088** prueba: escaneo externo de puertos, nada abierto (S-38)
- [ ] **089** Procesos separados por usuario: agente, vault, memoria, trace (S-39)
- [ ] **090** Contenedores sin root (S-39)
- [ ] **091** Actualizaciones de seguridad automáticas (S-39)
- [ ] **092** Disco cifrado (S-40)
- [ ] **093** Respaldos cifrados fuera del VPS (S-40)
- [ ] **094** Primera restauración probada en una máquina limpia (S-40)
- [ ] **095** Llaves de Tailscale con vencimiento y Tailnet Lock (S-31)
- [ ] **096** Revisión por componente de qué puede hacer y por qué lo necesita (S-03)

### Adaptador de Pi (D16)

- [ ] **097** Fijar la versión de Pi, con lockfile e instalación sin scripts (S-42)
- [ ] **098** Adaptador propio entre Pi y Clio, para poder cambiar Pi sin rehacer nada
- [ ] **099** Que Pi corra sin sesiones propias: las guarda Clio
- [ ] **100** Desactivar la carga y la escritura de extensiones, skills y servidores MCP por parte del agente (S-16)
- [ ] **101** prueba: un intento de escribir en la carpeta de extensiones se bloquea (S-16)
- [ ] **102** Revisar cada actualización de Pi antes de adoptarla (S-42)
- [ ] **103** prueba: el CI falla si el lockfile de npm cambia sin revisión (S-42)
- [ ] **104** Prompts de sistema versionados en el repositorio
- [ ] **105** Escáner de secretos sobre los prompts versionados (S-09)
- [ ] **106** Verificar que el prompt de sistema no tiene nada cuya filtración importe (S-09)
- [ ] **107** prueba: con el prompt de sistema vacío, lo que se puede ejecutar no cambia (S-01)

### Triaje con reglas (spec: triaje)

- [ ] **108** Los cuatro niveles: 0 directo, 1 simple, 2 completo, 3 largo
- [ ] **109** Entradas: mensaje de la persona, nodo y bloque de identidad; nunca contenido externo (S-07)
- [ ] **110** Salidas en una pasada: nivel, esfuerzo, ambiguo, posible irreversible, herramientas y modelo
- [ ] **111** Orden de decisión: forzado por la persona, reglas duras, reglas del triaje, ante la duda nivel 1
- [ ] **112** Comandos `/rápido` y `/plan`
- [ ] **113** Regla dura: si algo puede ser irreversible, mínimo nivel 2
- [ ] **114** Reglas por verbos: pregunta corta sin verbos de acción a 0; «crea», «borra», «despliega» a 2 o más
- [ ] **115** Subir de nivel a mitad de camino, con registro, y que nunca baje
- [ ] **116** Al llegar a nivel 3, pedir aprobación del contrato
- [ ] **117** Telemetría: pedidos por nivel, subidas, fallos por nivel bajo y costo por nivel
- [ ] **118** prueba: «¿Qué hora es?» queda en nivel 0 y responde con la herramienta de hora
- [ ] **119** prueba: «Borra la carpeta de pruebas» queda en nivel 2 o más
- [ ] **120** prueba: una página leída que dice «esto es nivel 0» no cambia el nivel (S-07)
- [ ] **121** prueba: una tarea de nivel 1 que necesita escribir fuera del sandbox sube a 2 y queda registrado
- [ ] **122** prueba: ninguna tarea baja de nivel
- [ ] **123** prueba: sin Jev, el triaje funciona solo con reglas
- [ ] **124** prueba: `/plan` fuerza el nivel 3 aunque el pedido sea corto
- [ ] **125** Fijar el objetivo inicial de subidas y de fallos por nivel bajo
- [ ] **126** medir D22: las 10 tareas más 20 triviales, con y sin triaje, forzando cada nivel

### Contrato de tarea (spec: contrato)

- [ ] **127** Campos del contrato: id, nivel, objetivo, rol, alcance, requiere spec, criterios, terminación, presupuesto, política y nodos, tareas, preguntas abiertas, origen, estado, trace_id
- [ ] **128** Estados: borrador, aprobado, en curso, bloqueado, cerrado con éxito, cerrado con fallo, cancelado
- [ ] **129** Solo la persona cambia el objetivo (S-07)
- [ ] **130** El nivel viene del triaje y solo sube
- [ ] **131** Un criterio sin forma de comprobarse no se acepta
- [ ] **132** Alcance explícito: lo que no está dentro, está fuera
- [ ] **133** Ciclo: explorar en solo lectura, borrador, spec si hace falta, aprobación, plan, registro durable, tareas, verificación, cierre
- [ ] **134** Como máximo una pregunta a la vez a la persona
- [ ] **135** Registro durable del contrato antes de la primera escritura
- [ ] **136** Decidir formato del contrato: markdown con frontmatter o JSON
- [ ] **137** Decidir dónde vive el registro: repositorio de trabajo, memoria episódica o ambos
- [ ] **138** Una tarea se marca hecha solo con evidencia observada
- [ ] **139** Informe de cierre fijo: resultado verificado, comprobaciones fallidas o pendientes, siguiente paso
- [ ] **140** Enmiendas versionadas: el agente propone, solo la persona acepta
- [ ] **141** Telemetría: tiempo por fase, criterios al primer intento, enmiendas, cierres por tipo, tareas reabiertas
- [ ] **142** prueba: un contrato con un criterio sin forma de comprobarse no pasa de borrador
- [ ] **143** prueba: ninguna tarea se marca hecha sin evidencia adjunta
- [ ] **144** prueba: una instrucción dentro de un archivo leído no modifica el objetivo ni el alcance
- [ ] **145** prueba: agotar el presupuesto cierra el contrato con informe
- [ ] **146** prueba: el informe de cierre siempre lista lo pendiente, aunque esté vacío
- [ ] **147** prueba: otra sesión retoma una tarea a medias solo con el contrato y el diff
- [ ] **148** ataque: una inyección que intenta bajar el nivel del triaje o cambiar el contrato

### Capa de control (presupuesto, anti-bucle y comprobaciones)

- [ ] **149** Presupuesto de pasos, tiempo y costo por tarea, aplicado en código con corte duro (S-47)
- [ ] **150** Presupuesto por día aplicado en código (S-47)
- [ ] **151** prueba: un bucle sembrado se detiene al tocar el tope (S-47)
- [ ] **152** ataque: un bucle que intenta gastar sin parar
- [ ] **153** Anti-bucle: contador de pasos repetidos (D5)
- [ ] **154** Los niveles 0 y 1 saltan contrato y planificador, pero no el presupuesto ni el anti-bucle
- [ ] **155** Comprobar que la acción coincide con el plan antes de ejecutar (D7)
- [ ] **156** Validador de factibilidad de reglas sobre cada acción (D7)
- [ ] **157** Presupuesto de reintentos para la capa de control
- [ ] **158** Puntos de control para reanudar una tarea
- [ ] **159** Reintentos con contexto fresco más paquete de evidencia (conecta con la fase 3)
- [ ] **160** Filtro de salidas del modelo antes de usarlas
- [ ] **161** Decidir si el anti-bucle cuenta también pasos «sin avance» (hoy solo repeticiones)

### Verificación (D2, D3, D13)

- [ ] **162** Verificar cada criterio con señales externas: pruebas, resultados de herramientas e invariantes en código
- [ ] **163** Verificar también contra el objetivo final, no solo paso a paso (D3)
- [ ] **164** El verificador es otro proceso, con otras señales y otro contexto (D13)
- [ ] **165** Nunca usar la autocrítica del modelo ni un debate entre modelos como verificación (D2)
- [ ] **166** El resultado de un subagente entra como observación, no como verdad (D12)

### Pasarela (spec: pasarela)

- [ ] **167** Pasarela como proceso distinto del bucle de Pi y del modelo (S-11)
- [ ] **168** prueba: matar o modificar el bucle no permite ejecutar sin la pasarela (S-11)
- [ ] **169** Entrada de la pasarela: herramienta y versión, argumentos, nodo, sesión, actor y secretos por marcador
- [ ] **170** Salida: permitir, pedir aprobación o negar, con razón explicable e id
- [ ] **171** Catálogo de acciones con efecto: lectura, escritura local reversible, escritura externa, irreversible
- [ ] **172** Declarar para cada herramienta si tiene salida al exterior y a qué destinos
- [ ] **173** Denegar por defecto lo que no está en una lista (S-02)
- [ ] **174** prueba: una herramienta sin declaración de efecto no se puede llamar (S-02)
- [ ] **175** Orden de evaluación, paso 1: esquema de argumentos y hash de la descripción (S-15)
- [ ] **176** Orden de evaluación, paso 2: normalizar rutas, separar argumentos, resolver dominio
- [ ] **177** Orden de evaluación, paso 3: prohibiciones duras; negar siempre gana
- [ ] **178** Orden de evaluación, paso 4: presupuesto agotado niega (S-47)
- [ ] **179** Orden de evaluación, paso 5: secretos permitidos para esa herramienta y ese destino (S-19)
- [ ] **180** Orden de evaluación, paso 6: sesión contaminada más salida a destino no permitido pide aprobación (S-06)
- [ ] **181** Orden de evaluación, paso 7: en niveles 2 y 3, una acción fuera del plan vuelve al control o pide aprobación
- [ ] **182** Orden de evaluación, paso 8: reversible dentro del sandbox se permite
- [ ] **183** Orden de evaluación, paso 9: una política que cubra todas las dimensiones permite (se conecta en la fase 5)
- [ ] **184** Orden de evaluación, paso 10: si no, aprobación sobre el hash de la acción
- [ ] **185** Comandos: lista de binarios permitidos con argumentos separados (argv), nunca `sh -c` (S-12)
- [ ] **186** Un shell libre solo dentro de un contenedor desechable, sin secretos y sin red por defecto (S-12)
- [ ] **187** prueba: argumentos con `;`, `|`, `$(...)` y comillas no escapan (S-12)
- [ ] **188** Rutas canonizadas antes de compararlas con la raíz permitida (S-13)
- [ ] **189** prueba: rutas con `..` y enlaces simbólicos hacia fuera se rechazan (S-13)
- [ ] **190** ataque: comandos con `;`, `$(...)` y rutas con `..` o enlaces simbólicos
- [ ] **191** La salida del modelo nunca se ejecuta, se interpola en un shell ni se renderiza como HTML sin validar (S-10)
- [ ] **192** prueba: una respuesta con `$(...)` o HTML no se ejecuta ni se renderiza (S-10)
- [ ] **193** Lo externo llega al modelo delimitado como datos, no mezclado con las instrucciones (S-08)
- [ ] **194** Revisar el armado del contexto contra S-08
- [ ] **195** Marcar como no confiable todo lo de fuera: web, archivos, correos, Notion, MCP, resultados de herramientas y resúmenes de subagentes (S-05)
- [ ] **196** Marca de contaminación que se propaga a toda la sesión y a sus subagentes y no se borra (S-05)
- [ ] **197** prueba: leer una página web marca la sesión y la marca aparece en la traza (S-05)
- [ ] **198** Sesión contaminada: toda salida al exterior exige aprobación y avisa que está contaminada (S-06)
- [ ] **199** prueba: una página con instrucciones ocultas no logra sacar ningún dato sin aprobación (S-06)
- [ ] **200** ataque: una página web con instrucciones ocultas que pide leer `~/.ssh` y mandarlo a otro sitio
- [ ] **201** ataque: un README o un PDF que dice «ignora tus instrucciones y…»
- [ ] **202** Hash de la acción ya normalizada
- [ ] **203** Ejecutar solo si el hash coincide con el aprobado (S-14)
- [ ] **204** prueba: cambiar un byte entre aprobar y ejecutar invalida la aprobación (S-14)
- [ ] **205** Aprobaciones por terminal en esta fase; la passkey llega con la PWA
- [ ] **206** Resolver los secretos en el último paso, dentro del proceso que ejecuta
- [ ] **207** Pasar la salida por la limpieza del vault antes de devolverla
- [ ] **208** Falla cerrada: si la pasarela no responde o da error, nada se ejecuta
- [ ] **209** prueba: con la pasarela caída, ninguna acción se ejecuta
- [ ] **210** prueba: cada decisión trae una razón que nombra la regla o la política
- [ ] **211** prueba: una acción reversible dentro del sandbox pasa sin preguntar
- [ ] **212** Herramientas MCP y skills con versión fijada y hash de su descripción (S-15)
- [ ] **213** Si la descripción cambia, la herramienta se desactiva hasta revisarla y avisa (S-15)
- [ ] **214** prueba: cambiar la descripción de un servidor MCP lo desactiva y avisa (S-15)
- [ ] **215** ataque: un servidor MCP que cambia la descripción de su herramienta después de aprobado
- [ ] **216** Los subagentes solo leen y no reciben secretos (S-17)
- [ ] **217** prueba: un subagente que intenta escribir o resolver un secreto falla (S-17)
- [ ] **218** ataque: un intento del agente de escribir una extensión de Pi o un skill
- [ ] **219** Hasta completar el día cero, la pasarela niega toda acción fuera del sandbox (S-56)
- [ ] **220** Telemetría: decisiones por tipo y razón, latencia, aprobaciones por causa, negaciones, intentos de secreto hacia destino ajeno
- [ ] **221** decidir formato de las reglas de la pasarela: el mismo de las políticas o uno propio

### Sandbox y reversibilidad

- [ ] **222** Sandbox desechable para lectura y escritura
- [ ] **223** Snapshot por tarea para poder volver
- [ ] **224** «Para» o cancelar deja el sandbox como estaba, en segundos
- [ ] **225** Solo herramientas de lectura y escritura dentro del sandbox en esta fase
- [ ] **226** Registro de efectos idempotente por acción
- [ ] **227** Cortacircuito en la pasarela
- [ ] **228** Arrendamiento de recursos (repo, ruta) para serializar tareas sobre el mismo recurso

### Herramientas

- [ ] **229** Juego básico de herramientas con su declaración de efecto
- [ ] **230** Pocas herramientas visibles, el resto por recuperación (D8)
- [ ] **231** El LLM puede pedir una herramienta no recomendada y la pasarela sigue revisando todo
- [ ] **232** medir D8: acierto de selección con 10, 30 y 60 herramientas, con tu modelo (todas visibles, recomendadas, recuperación vectorial)

### Vault (spec: vault)

- [ ] **233** Vault como proceso aparte con su propio usuario (S-22)
- [ ] **234** Hablar solo por un socket Unix con permisos, nunca por red
- [ ] **235** Modelo de un secreto: nombre-marcador, valor cifrado, tipo, destinos, ámbito, rotación y vencimiento, estado y uso
- [ ] **236** Marcadores `{{secreto:nombre}}`; el modelo solo ve marcadores
- [ ] **237** Cifrado en reposo del vault
- [ ] **238** Llave maestra que no queda sin protección en el mismo disco (S-22)
- [ ] **239** Arranque bloqueado tras reiniciar; lo que necesita secretos espera en la cola
- [ ] **240** Desbloqueo por terminal en esta fase; con passkey en la fase 4
- [ ] **241** Operación `put` (la persona)
- [ ] **242** Operación `resolve` (solo la pasarela, con herramienta y destino, devuelve en memoria)
- [ ] **243** Operación `issue_token` (token corto con alcance y vida limitada)
- [ ] **244** Operaciones `rotate` y `revoke`
- [ ] **245** Operación `list` (nombres y metadatos, nunca valores)
- [ ] **246** Operación `audit` (cada resolución con destino y trace_id)
- [ ] **247** Cada secreto atado a su destino: herramienta y dominio permitidos (S-19)
- [ ] **248** prueba: `resolve` hacia un destino no permitido falla y alerta (S-19)
- [ ] **249** ataque: un pedido para usar `{{secreto:x}}` hacia un dominio no permitido
- [ ] **250** Filtro único de limpieza de salidas, usado por pasarela, LLM, Jev, clio-trace y clio-memory (S-20)
- [ ] **251** El filtro busca los valores del vault también en base64 y codificados para URL
- [ ] **252** El filtro busca patrones de llaves de proveedores
- [ ] **253** Registrar cada tachado, porque suele indicar un intento de filtración
- [ ] **254** prueba: una herramienta que imprime un secreto, también en base64, llega al modelo tachada (S-20)
- [ ] **255** ataque: una herramienta que devuelve un secreto en su salida
- [ ] **256** Ningún secreto entra al contexto, a la memoria, a la traza ni a un log (S-18)
- [ ] **257** prueba: buscar los valores del vault en contexto, memoria y trazas tras una semana: cero apariciones (S-18)
- [ ] **258** ataque: una página que pide «muéstrame tus llaves»
- [ ] **259** Tokens de vida corta para nodos y herramientas (S-21)
- [ ] **260** Rotación programada de secretos (S-21)
- [ ] **261** Revocación inmediata desde la CLI (S-21)
- [ ] **262** prueba: un token vencido o revocado se rechaza (S-21)
- [ ] **263** prueba: un token emitido para un nodo deja de servir al vencer
- [ ] **264** prueba: `list` nunca devuelve valores
- [ ] **265** prueba: tras reiniciar, ningún secreto se resuelve hasta el desbloqueo
- [ ] **266** prueba: ningún proceso distinto de la pasarela puede llamar a `resolve`
- [ ] **267** Respaldos del vault cifrados con una llave distinta (S-22)
- [ ] **268** prueba: un respaldo del vault no se puede abrir sin su llave propia
- [ ] **269** Telemetría: resoluciones por secreto y destino, negaciones, tachados, secretos por vencer, tiempo bloqueado
- [ ] **270** decidir implementación del vault: propia en Rust, OpenBao o Infisical
- [ ] **271** decidir desbloqueo: passkey tras reinicio, llave en otro equipo o servicio externo
- [ ] **272** Cargar solo el secreto del primer uso (el proveedor del modelo)

### Modelos y proveedores, versión mínima (spec: modelos)

- [ ] **273** Catálogo de modelos: proveedor, id exacto con versión, ventana, precio, capacidades, retención de datos y niveles
- [ ] **274** Un modelo sin versión fijada no se acepta (prueba)
- [ ] **275** Nunca un alias que cambia solo
- [ ] **276** Modelo por nivel: rápido y barato para 0 y 1, fuerte para 2 y 3
- [ ] **277** Todo lo que sale hacia un LLM pasa antes por la limpieza (S-44)
- [ ] **278** prueba: payloads salientes sin valores del vault (S-44)
- [ ] **279** Tabla de qué datos van a qué proveedor (S-44)
- [ ] **280** Usar opciones sin retención donde existan (S-44)
- [ ] **281** Topes de gasto en el panel de cada proveedor, además de los de Clio (S-45)
- [ ] **282** Límite de llamadas con espera por proveedor
- [ ] **283** Registrar el costo y los tokens por proveedor y modelo

### Mediciones de la fase

- [ ] **284** prueba: preproducción no puede alcanzar ningún dispositivo real ni el vault real
- [ ] **285** Medir las dos pruebas rápidas de la fase: selección de herramientas (D8) y posición en el contexto (D9)
- [ ] **286** medir D9: el dato clave al principio, en medio y al final, con tu modelo
- [ ] **287** medir D1: las 10 tareas con un flujo fijo y con el bucle libre
- [ ] **288** medir D2 y D3: con y sin verificador (por paso y contra el objetivo final)
- [ ] **289** medir D4: con y sin contrato en las mismas tareas
- [ ] **290** medir D5: episodios de pasos repetidos antes y después del detector
- [ ] **291** medir D6: tareas ambiguas a propósito mezcladas con tareas claras
- [ ] **292** medir D7: acciones sembradas que no coinciden con el plan o violan una restricción
- [ ] **293** medir D13: defectos sembrados (como el del comando en `auth.json`) con constructor y con verificador distinto
- [ ] **294** medir D16: las 10 tareas con Pi solo y con cada capa propia; subir Pi una versión y volver a correr el banco
- [ ] **295** medir D17: buscar secretos en trazas, memoria y contexto tras una semana de uso
- [ ] **296** medir D23: costo calculado de una semana contra la factura real (desviación menor al 5 %)
- [ ] **297** medir D23: sobrecarga de latencia con la telemetría encendida y apagada

### Cierre de la fase 1

- [ ] **298** Hay una línea base medida, sin memoria
- [ ] **299** Pasan los ataques de inyección, de secretos y de comandos

---

## Fase 2 · Memoria

### Cerrar lo que quedó abierto de clio-memory v1

- [ ] **300** Confirmar que el nombre `clio-memory` está libre en crates.io y en GitHub
- [ ] **301** Documentar `remember` en el spec (§8) y en el README
- [ ] **302** Respaldos y exportaciones cifrados (S-23, parte que falta)
- [ ] **303** Decidir cifrado en reposo: ninguno o SQLCipher (coste en rendimiento y compatibilidad de extensiones)
- [ ] **304** Decidir API remota: HTTP + JSON o gRPC
- [ ] **305** Decidir índice vectorial: sqlite-vec plano, DiskANN o vec1; medir a 50 000 y a 500 000 recuerdos
- [ ] **306** Verificar si model2vec-rs puede cargar a menor precisión y bajar la RAM
- [ ] **307** Investigar los ~550 MB que quedan retenidos tras soltar el modelo y reportarlo a model2vec-rs
- [ ] **308** Fuzzing del tokenizador y de los importadores de clio-memory (S-48)
- [ ] **309** prueba: aislamiento de ámbitos, cuota por actor y detector de secretos en cada versión (S-27)

### Daemon y MCP

- [ ] **310** Daemon de clio-memory con HTTP + JSON y token
- [ ] **311** Servidor MCP con `memory_recall`, `memory_propose` y retroalimentación
- [ ] **312** PROMOTE, PURGE y REVERT nunca expuestos por MCP (S-25)
- [ ] **313** prueba: llamar PROMOTE por MCP no es posible (S-25)
- [ ] **314** Daemon como proceso aparte con su propio usuario (S-39)
- [ ] **315** Token de vida corta y rotación para el daemon (S-21)
- [ ] **316** Daemon accesible solo por socket o por la tailnet

### PURGE y borrado real

- [ ] **317** Operación PURGE: elimina el contenido también de las operaciones pasadas y deja una lápida
- [ ] **318** prueba: PURGE elimina el contenido de todas las operaciones y deja la lápida (prueba 11)
- [ ] **319** PURGE también borra el vector (S-24)
- [ ] **320** prueba: después de un PURGE no queda el vector (S-24)
- [ ] **321** Que la exportación respete las lápidas

### Gestor de memoria (conexión con el agente)

- [ ] **322** Gestor de memoria como único escritor desde el agente
- [ ] **323** Escritura al cerrar el episodio, en lote; la memoria de trabajo se escribe al momento
- [ ] **324** Lo de fuera entra siempre en cuarentena y nunca se promueve solo (S-25)
- [ ] **325** Distinguir un hecho de la persona de contenido externo
- [ ] **326** Los recuerdos procedimentales propuestos por el agente quedan pendientes hasta que la persona los apruebe (S-26)
- [ ] **327** prueba: un procedimiento propuesto por el agente queda pendiente hasta aprobarlo (S-26)
- [ ] **328** ataque: una inyección que intenta guardar un procedimiento en la memoria
- [ ] **329** Actores: agente, sesión, persona, importador, cada uno con su cuota
- [ ] **330** ataque: una inundación de propuestas a la memoria desde un mismo actor
- [ ] **331** Que el triaje y el enrutador lean la memoria
- [ ] **332** Que las respuestas de la persona a una pregunta de ruteo queden como preferencia procedimental
- [ ] **333** Conectar `context()` al arranque de cada sesión con presupuestos separados para bloques fijos y recall
- [ ] **334** Conectar la retroalimentación útil/equivocado a las respuestas del agente
- [ ] **335** Conectar la memoria a Pi por MCP o por el adaptador

### Evaluación de la memoria

- [ ] **336** Arnés de evaluación: banco de preguntas con resultado conocido
- [ ] **337** Medir recall@k, precisión de la abstención, tasa de NOOP y de revisión
- [ ] **338** Banco en español, incluyendo consultas con términos en inglés (el caso «scope» frente a «ámbito»)
- [ ] **339** Correr LongMemEval y LoCoMo para comparar con otros motores con números
- [ ] **340** Comparar decisores en modo sombra
- [ ] **341** medir D10: tareas de varias sesiones con memoria y sin memoria, contra la línea base
- [ ] **342** medir D11: escrituras concurrentes forzadas, con y sin gestor único

### Cierre de la fase 2

- [ ] **343** El banco con memoria supera a la línea base sin memoria
- [ ] **344** Si no la supera, dejar la memoria solo en la identidad

---

## Fase 3 · Spec y diagnóstico

### Paso de spec antes del plan

- [ ] **345** Detectar cuándo un contrato «requiere spec» (cambia interfaz, modelo de datos o comportamiento observable)
- [ ] **346** Escribir el spec antes del plan: comportamiento, interfaces, modelo de datos y pruebas de aceptación
- [ ] **347** Que las pruebas del spec pasen a ser los criterios de éxito del contrato
- [ ] **348** Aprobación del contrato y del spec juntos en nivel 3
- [ ] **349** prueba: un contrato que cambia una interfaz no pasa a plan sin spec, y las pruebas del spec aparecen como criterios

### Diagnóstico (spec: diagnóstico)

- [ ] **350** Activación: verificación fallida, error de herramienta no trivial o anti-bucle sin avance
- [ ] **351** Nunca reintentar antes de terminar el diagnóstico
- [ ] **352** Fase 1 (código, sin LLM): reproducir el fallo con el mismo comando o prueba
- [ ] **353** Fase 1: repetir una vez para saber si es intermitente
- [ ] **354** Fase 1: juntar salida exacta del error, tramos de clio-trace, diff desde el último verde, versiones de dependencias y estado del nodo
- [ ] **355** Fase 1: bisección sobre commits o pasos desde el último verde
- [ ] **356** Paquete de evidencia de tamaño acotado y fijado por el código
- [ ] **357** Pasar el paquete de evidencia por la limpieza de secretos antes de que lo vea el modelo (S-20)
- [ ] **358** prueba: el paquete de evidencia no trae secretos
- [ ] **359** Fase 2: clasificar el fallo en especificación, entorno, intermitente o código, con reglas
- [ ] **360** Enrutar especificación a la persona con una propuesta de enmienda
- [ ] **361** Enrutar entorno al enrutador o a la cola
- [ ] **362** Enrutar intermitente a un reintento marcado en clio-trace
- [ ] **363** Enrutar código al ciclo de corrección
- [ ] **364** Fase 3: sesión nueva del núcleo con contrato más paquete de evidencia, sin la trayectoria que falló
- [ ] **365** Fase 3: hipótesis escrita en una frase a partir de la evidencia
- [ ] **366** Fase 3: prueba mínima que falle por esa causa
- [ ] **367** Fase 3: corrección que haga pasar esa prueba sin romper las demás
- [ ] **368** Fase 3: verificación completa otra vez
- [ ] **369** Cada vuelta gasta del presupuesto de reintentos (S-47)
- [ ] **370** Informe al agotar reintentos: qué se probó, qué se descartó y la mejor hipótesis pendiente
- [ ] **371** Procedimiento de diagnóstico como memoria procedimental revisada por la persona (S-26)
- [ ] **372** Cargar el procedimiento como skill de Pi solo cuando se activa el diagnóstico
- [ ] **373** El agente puede proponer mejoras al procedimiento, pero no cambiarlo solo
- [ ] **374** decidir tamaño del paquete de evidencia: fijo o según nivel
- [ ] **375** decidir bisección automática: siempre o solo con permiso de política
- [ ] **376** Telemetría: fallos por tipo, vueltas por fallo, resuelto en la primera hipótesis, intermitentes por prueba, costo del diagnóstico contra reintentar sin él
- [ ] **377** prueba: un fallo nunca dispara un reintento sin un paquete de evidencia previo
- [ ] **378** prueba: la sesión de corrección no contiene ningún mensaje de la sesión que falló
- [ ] **379** prueba: una prueba intermitente sembrada se clasifica como tal y se reintenta una sola vez
- [ ] **380** prueba: un error de red se manda al enrutador o a la cola, no al ciclo de corrección
- [ ] **381** prueba: un criterio contradictorio sembrado llega a la persona con una propuesta de enmienda
- [ ] **382** prueba: en un fallo de código, la corrección viene precedida de una prueba que fallaba por esa causa
- [ ] **383** prueba: agotar los reintentos cierra con un informe de hipótesis probadas y descartadas
- [ ] **384** medir D25: las 10 tareas con fallos sembrados, con y sin diagnóstico; construcción con y sin spec previo

### Cierre de la fase 3

- [ ] **385** Bajan los reintentos y los fallos de especificación frente a la fase 2

---

## Fase 4 · Red y PWA

### Preproducción de red

- [ ] **386** decidir red de prueba: tailnet aparte o contenedores locales
- [ ] **387** Nodos falsos con capacidades, latencia, caídas, reconexiones y batería configurables (`testing/fake-nodes`)
- [ ] **388** prueba: los nodos falsos reproducen una caída a mitad de tarea de forma determinista
- [ ] **389** Jev, push y proveedores falsos con respuestas controladas, caídas y límites
- [ ] **390** Suite de caos completa: red cortada, VPS caído, disco lleno, proveedor caído, vault bloqueado

### Protocolo clio-network (spec: clio-network)

- [ ] **391** WebSocket sobre la tailnet, abierto siempre por el nodo hacia el cerebro
- [ ] **392** Mensaje `hello` con id de nodo, versión, capacidades, número de un solo uso y firma Ed25519
- [ ] **393** El cerebro verifica la firma contra el registro y la identidad de Tailscale del dispositivo
- [ ] **394** Mensaje `welcome` firmado, con versión acordada y último mensaje confirmado
- [ ] **395** Sobre de cada mensaje: id (ULID), tipo, ts, expires_at, traceparent, payload y firma
- [ ] **396** Tipos de mensaje: `capabilities`, `task.assign`, `task.cancel`, `task.progress`, `task.result`, `ack`, `event`, `error`
- [ ] **397** El nodo rechaza todo mensaje sin firma válida, vencido o con un id ya visto (S-29)
- [ ] **398** prueba: órdenes sin firma, vencidas o repetidas se rechazan (S-29)
- [ ] **399** ataque: una orden a un nodo sin firma, repetida o vencida, enviada desde otro dispositivo de la tailnet
- [ ] **400** Outbox en el cerebro hasta recibir el ack; entrega al menos una vez
- [ ] **401** El nodo guarda los ids vistos y descarta duplicados
- [ ] **402** prueba: cortar la red a mitad de una tarea y reconectar no pierde ni duplica mensajes
- [ ] **403** Reconexión con espera creciente y azar, reanudando desde el último ack
- [ ] **404** Sin cerebro, el nodo termina el paso atómico en curso, no empieza nada nuevo y cancela lo pendiente pasado un tiempo
- [ ] **405** prueba: con el cerebro caído, el nodo no empieza trabajo nuevo
- [ ] **406** Versionado semver del protocolo, negociado en el saludo
- [ ] **407** prueba: un nodo con versión mayor incompatible recibe un error claro
- [ ] **408** prueba: un tramo que corre en un nodo aparece colgado de la traza del pedido original
- [ ] **409** decidir serialización: JSON, MessagePack o CBOR
- [ ] **410** decidir rotación de llaves de nodo: manual o automática con periodo fijo
- [ ] **411** Fuzzing de los mensajes del WebSocket (S-48)
- [ ] **412** Telemetría de red: reconexiones, latencia hasta el ack, reenviados, rechazados por firma o vencimiento, tiempo desconectado

### Nodos y daemon (clio-node)

- [ ] **413** Daemon del nodo sin privilegios, nunca como root (S-30)
- [ ] **414** Lista local de permitidos en cada nodo, que el cerebro no puede cambiar a distancia (S-30)
- [ ] **415** prueba: una orden para ampliar la lista de permitidos desde el cerebro se rechaza (S-30)
- [ ] **416** Capacidades con nombre, esquema de argumentos y lista local
- [ ] **417** decidir pasarela en los nodos: solo lista de permitidos o copia completa
- [ ] **418** Snapshot por tarea en cada nodo
- [ ] **419** Reglas de Tailscale que niegan por defecto: el cerebro solo llega al puerto del daemon, los nodos no se ven entre sí, el teléfono solo llega a la PWA (S-28)
- [ ] **420** prueba: desde un nodo, intentar conectar con otro falla (S-28)
- [ ] **421** Un par de llaves por nodo
- [ ] **422** Enrolar el primer nodo (la laptop): generar su llave, aprobar su lista de permitidos y probar una orden firmada
- [ ] **423** Revocar un dispositivo perdido en un paso: Tailscale, llave del nodo y sesiones (S-32)
- [ ] **424** prueba: simulacro de dispositivo perdido (S-32)
- [ ] **425** medir D18: pedir a un nodo algo que su lista no permite, tumbar el VPS a mitad de tarea y revocar el cerebro en Tailscale
- [ ] **426** Decidir si se puede empezar solo con el VPS o el primer nodo es obligatorio

### Enrutador y orquestador

- [ ] **427** Enrutador de nodos con reglas de capacidades: shell, repo local, build pesado, GUI, larga duración
- [ ] **428** Destinos posibles: este nodo, otro nodo, VPS, cola, preguntar
- [ ] **429** Preguntar a la persona cuando quede más de una opción
- [ ] **430** Guardar la respuesta como preferencia procedimental
- [ ] **431** medir D19: 20 pedidos reales desde teléfono y computador, con la laptop encendida y apagada
- [ ] **432** Cada contrato corre en su propia sesión con contexto independiente
- [ ] **433** El orquestador es código y no un LLM
- [ ] **434** Prioridad: lo que pide la persona primero, después por plazo y por costo
- [ ] **435** Tope de sesiones concurrentes configurable
- [ ] **436** Arrendamiento de recursos para serializar tareas sobre el mismo repo, ruta o nodo
- [ ] **437** Solo la persona interrumpe, cancela o cambia la prioridad de una tarea (S-55)
- [ ] **438** prueba: dos tareas sobre el mismo repo se serializan y ninguna tarea puede interrumpir a otra
- [ ] **439** ataque: una inyección que intenta cancelar o reordenar otra tarea (S-55)
- [ ] **440** Una sola tarea en foco; el resto reporta por el resumen
- [ ] **441** Presupuesto global por día repartido entre las tareas activas; las de menor prioridad se pausan primero
- [ ] **442** Bandeja de mensajes con dos carriles, inmediato y normal, decidida por una regla y no por Jev
- [ ] **443** prueba: «para» detiene la tarea en segundos y deja el sandbox como estaba; un mensaje normal espera al cierre y se responde junto con los demás

### Bus de eventos y push

- [ ] **444** Web Push hacia la persona cuando la PWA está en segundo plano
- [ ] **445** Push sin contenido: solo «tienes algo pendiente» (S-35)
- [ ] **446** prueba: inspeccionar los payloads de push, sin datos (S-35)
- [ ] **447** La información sensible nunca en la pantalla de bloqueo ni en el push
- [ ] **448** En iPhone, exigir la PWA instalada en la pantalla de inicio
- [ ] **449** medir D20: cortar la red de un nodo a mitad de tarea; avisos con la PWA cerrada en Android y en iPhone

### PWA (spec: PWA)

- [ ] **450** decidir framework: sin framework o uno liviano
- [ ] **451** Servir solo dentro de la tailnet con `tailscale serve`
- [ ] **452** Primero el teléfono, modo oscuro primero, sin scroll horizontal
- [ ] **453** prueba: ninguna pantalla tiene scroll horizontal a 360 px de ancho
- [ ] **454** Tokens de la paleta, tipografía Space Grotesk y JetBrains Mono servidas desde la tailnet
- [ ] **455** Pantalla Conversación: nivel del pedido, nodo que trabaja, marca de sesión contaminada, mensaje «en cola», botón de parar siempre visible
- [ ] **456** prueba: el botón de parar funciona aunque haya mensajes en cola y detiene solo la tarea en foco
- [ ] **457** Pantalla Foco: una tarea en primer plano; cambiar de foco no interrumpe nada
- [ ] **458** Pantalla Tareas: en curso, en cola y programadas; contrato, progreso, evidencia y cancelar
- [ ] **459** Pantalla Aprobaciones: lista y detalle con la acción exacta, el diff, el destino, el origen y el hash (S-36)
- [ ] **460** prueba: la pantalla de aprobación muestra la acción exacta y su hash
- [ ] **461** Nunca solo un resumen escrito por el modelo en la pantalla de aprobación (S-36)
- [ ] **462** prueba: la marca de sesión contaminada aparece en la conversación y en la aprobación
- [ ] **463** ataque: una notificación provocada que pide aprobar «urgente»
- [ ] **464** Pantalla Resumen: lo que pasó mientras no mirabas, agrupado
- [ ] **465** Pantalla Trazas: la cascada de clio-trace ordenable por tiempo o por costo
- [ ] **466** Pantalla Memoria: buscar y revisar cuarentena, conflictos y revisión pendiente
- [ ] **467** Promover, revertir o purgar memoria con passkey
- [ ] **468** prueba: promover, revertir o purgar memoria pide passkey
- [ ] **469** Pantalla Políticas y trabajos: ver, proponer, renovar y revocar
- [ ] **470** Pantalla Nodos: estado, últimas conexiones, revocar un dispositivo
- [ ] **471** Pantalla Costos: por día, nivel y modelo, con topes
- [ ] **472** CSP estricta: sin scripts en línea, sin recursos externos, `frame-ancestors 'none'` (S-33)
- [ ] **473** prueba: escáner de cabeceras en el CI (S-33)
- [ ] **474** Markdown saneado: sin HTML, sin imágenes remotas, enlaces que muestran el dominio y nunca se abren solos (S-34)
- [ ] **475** prueba: una imagen remota en una respuesta no genera ninguna petición (S-34)
- [ ] **476** ataque: una respuesta con una imagen markdown cuya URL lleva datos
- [ ] **477** Service worker para push y para guardar la interfaz, nunca datos sensibles
- [ ] **478** Sin conexión muestra la interfaz, pero no permite aprobar ni ver datos
- [ ] **479** prueba: sin conexión no se puede aprobar
- [ ] **480** Sesión con passkey; cookies Secure, HttpOnly y SameSite=Strict; vencimiento corto (S-37)
- [ ] **481** prueba: escáner de cabeceras y sesión vencida (S-37)
- [ ] **482** prueba: ningún push trae contenido
- [ ] **483** decidir notificaciones en computadores: la misma PWA o avisos nativos del daemon
- [ ] **484** Revisión de interfaz con casos sembrados de aprobación (S-36)
- [ ] **485** Telemetría de la PWA: tiempo del push a la aprobación, rechazos, errores del cliente sin contenido

### Passkey y aprobaciones

- [ ] **486** Registrar la passkey desde la PWA; sin passkey no hay nada más
- [ ] **487** Desafío WebAuthn con el hash de la acción, un número de un solo uso y vencimiento
- [ ] **488** `userVerification: "required"`
- [ ] **489** Aprobar lo irreversible con passkey sobre el hash de la acción exacta (S-14)
- [ ] **490** Conectar las aprobaciones de la pasarela a la passkey en lugar de la terminal
- [ ] **491** Contrato y spec del nivel 3 aprobados con passkey; nada se escribe antes
- [ ] **492** prueba: en nivel 3, nada se escribe antes de la aprobación con passkey
- [ ] **493** Desbloqueo del vault con passkey tras un reinicio
- [ ] **494** `put` del vault con passkey
- [ ] **495** medir D21: aprobar con la firma de otra acción, una vencida y una ya usada
- [ ] **496** medir D14: tasa de aprobación y tiempo de decisión (si pasa del 95 % es adorno)

### clio-trace completo y retención

- [ ] **497** Módulo `retention` reutilizable: trabajos `rollup` y `expire` en la cola
- [ ] **498** Resúmenes por minuto, hora y día antes de borrar el dato crudo
- [ ] **499** Vida por defecto: prompts 7 días (solo si se activa), tramos 30 días, eventos 90 días, resumen por hora 1 año, resumen por día siempre
- [ ] **500** Histogramas con cubetas fijas para calcular el p95 al leer, sin promediar percentiles
- [ ] **501** prueba: tras caducar los tramos crudos, totales y p95 diario dan lo mismo (prueba 23 de clio-memory)
- [ ] **502** Contenido de prompts pasa por la limpieza y caduca a los 7 días (S-46)
- [ ] **503** El visor de trazas pide passkey (S-46)
- [ ] **504** Sesiones de Pi guardadas por Clio con la misma retención
- [ ] **505** Vista de la cascada en la PWA con la paleta definida
- [ ] **506** Revisión semanal de las métricas de seguridad (ver operación)

### Despliegue y automodificación (spec: despliegue)

- [ ] **507** Artefactos versionados y firmados: binarios en Rust e imagen de contenedor
- [ ] **508** Despliegue lado a lado: la versión nueva arranca junto a la vieja
- [ ] **509** Comprobaciones de salud y banco corto de humo antes de cambiar el tráfico
- [ ] **510** prueba: un despliegue que no pasa salud o humo vuelve solo a la versión anterior
- [ ] **511** Mantener la versión vieja unas horas para volver con un comando
- [ ] **512** Migraciones con copia previa de los datos y vuelta atrás o respaldo
- [ ] **513** prueba: una migración se puede deshacer o restaurar desde su copia previa
- [ ] **514** Daemons de los nodos actualizados de forma escalonada y dentro de una versión de protocolo compatible
- [ ] **515** prueba: el VPS se reconstruye desde cero con el código de infraestructura y un respaldo
- [ ] **516** Telemetría de despliegues, vueltas atrás, tiempo de cambio y PRs de Clio aceptados

### Día cero (spec: día cero)

- [ ] **517** Paso 1: instalar el cerebro en el VPS con la infraestructura como código, todo cerrado
- [ ] **518** Paso 2: registrar la passkey desde la PWA en el teléfono
- [ ] **519** prueba: sin passkey registrada, ninguna otra operación del día cero está disponible
- [ ] **520** Paso 3: desbloquear el vault por primera vez y cargar solo los secretos del primer uso
- [ ] **521** Paso 4: crear el bloque de identidad por conversación; solo lo que la persona dice
- [ ] **522** prueba: el bloque de identidad solo contiene lo que la persona escribió o confirmó
- [ ] **523** Crear al menos dos ámbitos de memoria desde el inicio: personal y trabajo
- [ ] **524** decidir guion de identidad: preguntas fijas o conversación libre con un mínimo obligatorio
- [ ] **525** Paso 5: enrolar el primer nodo, aprobar su lista de permitidos y probar una orden firmada
- [ ] **526** Paso 6: políticas iniciales muy estrechas y con vencimiento corto: leer y escribir en el sandbox, nada hacia fuera
- [ ] **527** Paso 7: correr el banco por primera vez y guardar la línea base en clio-trace
- [ ] **528** prueba: la línea base queda guardada y aparece en clio-trace como referencia
- [ ] **529** Paso 8: primer trabajo programado, si quieres uno, con tu passkey
- [ ] **530** prueba: antes de completar el día cero, una acción fuera del sandbox se niega (S-56)
- [ ] **531** prueba: el día cero completo en una máquina limpia deja a Clio idéntica a la original, salvo las llaves
- [ ] **532** Que Clio guíe y proponga pero no active identidad, políticas, nodos ni secretos
- [ ] **533** Telemetría del día cero: tiempo, pasos repetidos y línea base

### Si algo sale mal (respuesta a incidentes)

- [ ] **534** Procedimiento «Cortar»: revocar el cerebro en Tailscale y que los nodos queden quietos
- [ ] **535** Procedimiento «Congelar»: clio-memory en solo lectura
- [ ] **536** Procedimiento «Rotar»: secretos del vault, llaves de los nodos y, si hay duda, passkeys
- [ ] **537** Procedimiento «Investigar»: clio-trace por `trace_id` y el registro de operaciones por actor y hora
- [ ] **538** Procedimiento «Reparar»: REVERT por actor o desde una hora; PURGE si se filtró un dato personal
- [ ] **539** Procedimiento «Volver»: restaurar desde un respaldo limpio, postmortem y un caso nuevo en la batería
- [ ] **540** Simulacro de cada procedimiento

### Ataques de esta fase

- [ ] **541** ataque: una orden sin firma, repetida o vencida desde otro dispositivo de la tailnet (batería)
- [ ] **542** ataque: una imagen markdown con datos en la URL (batería)
- [ ] **543** ataque: una notificación urgente provocada (batería)

### Cierre de la fase 4

- [ ] **544** Pasan los ataques de órdenes sin firma, repetidas o vencidas
- [ ] **545** Una caída a mitad de tarea no pierde ni duplica nada

---

## Fase 5 · Autonomía

### Políticas permanentes (spec: políticas)

- [ ] **546** Modelo de política: alcance, límites, vigencia, condiciones, origen y firma, estado, historial
- [ ] **547** Alcance: herramientas, acciones, rutas canonizadas, dominios, nodos, ámbitos de memoria y secretos
- [ ] **548** Límites: costo por día y por ejecución, número de acciones
- [ ] **549** Vigencia obligatoria; una política sin vencimiento o sin tope se rechaza (S-50)
- [ ] **550** prueba: una política sin vencimiento o sin tope se rechaza al crearla (S-50)
- [ ] **551** decidir formato: TOML, JSON o un lenguaje de políticas existente
- [ ] **552** decidir vigencia máxima: 30 o 90 días
- [ ] **553** Estados: propuesta, activa, suspendida, vencida, revocada
- [ ] **554** Crear, cambiar o ampliar una política exige passkey; el agente solo propone (S-49)
- [ ] **555** prueba: el agente puede proponer una política, pero no activarla
- [ ] **556** ataque: una inyección que intenta crear un trabajo o ampliar una política
- [ ] **557** Evaluación determinista en la pasarela: prohibiciones, reversibilidad, política que cubra todas las dimensiones, passkey
- [ ] **558** prueba: una prohibición dura gana sobre cualquier política
- [ ] **559** prueba: una acción que cumple cuatro de cinco dimensiones pide aprobación
- [ ] **560** prueba: una política vencida no autoriza nada
- [ ] **561** prueba: cada decisión trae su explicación («permitido por la política X, dimensión Y»)
- [ ] **562** Condiciones como «solo si la sesión no está contaminada» o «solo acciones reversibles»
- [ ] **563** Política «solo con la persona presente» y valores por defecto: ausente vale para lo reversible, solo presente para lo que sale al exterior
- [ ] **564** Aviso una semana antes del vencimiento; renovar exige passkey
- [ ] **565** Proponer borrar las políticas sin uso
- [ ] **566** Confianza ganada: clio-trace cuenta ejecuciones y reversiones por clase de acción
- [ ] **567** Tras N ejecuciones sin reversión, Clio propone una política y la persona la aprueba (S-51)
- [ ] **568** Una reversión dentro de una política la suspende y vuelve a pedir aprobación (S-51)
- [ ] **569** prueba: una reversión dentro de una clase autónoma la degrada en la siguiente ejecución (S-51)
- [ ] **570** Telemetría: decisiones por política, cercanía a los topes, aprobaciones por falta de política, políticas sin uso

### Modo ausente (S-54)

- [ ] **571** Entrar en modo ausente por tiempo sin respuesta, por horario o cuando la persona lo dice
- [ ] **572** Salir al tocar la PWA o al decirlo
- [ ] **573** Una acción que pediría aprobación se aparta y la tarea sigue con lo reversible
- [ ] **574** Nada irreversible se ejecuta en modo ausente, aunque lo cubra una política (S-54)
- [ ] **575** prueba: sembrar una acción irreversible con política en modo ausente: se aparta (S-54)
- [ ] **576** ataque: una acción irreversible cubierta por política, en modo ausente
- [ ] **577** Informe parcial al cerrar: hecho, apartado y por qué
- [ ] **578** Resumen al volver con la aprobación lista para firmar y la tarea lista para reanudar
- [ ] **579** Las aprobaciones apartadas vencen y se regeneran sobre el estado actual
- [ ] **580** prueba: en modo ausente, una tarea con una acción irreversible termina con informe parcial y la aprobación apartada

### Programador (spec: programador)

- [ ] **581** Modelo de un trabajo: disparador, plantilla de contrato, nivel, política, destino, concurrencia, recuperación, ventana, tope, aviso, estado
- [ ] **582** Disparadores: cron con zona horaria, intervalo o evento
- [ ] **583** decidir cómo se escribe el disparador: cron o lenguaje natural traducido y confirmado
- [ ] **584** decidir máximo de trabajos activos
- [ ] **585** El trabajo corre con contrato y nivel fijos, sin pasar por el triaje (S-52)
- [ ] **586** La tarea pasa por la pasarela con la política del trabajo
- [ ] **587** Sin política activa o con la política vencida, el trabajo no corre y avisa (S-52)
- [ ] **588** prueba: con la política vencida, el trabajo no corre y avisa
- [ ] **589** Dos corridas del mismo trabajo no se superponen (S-52)
- [ ] **590** prueba: dos corridas del mismo trabajo no se superponen
- [ ] **591** Cada corrida queda trazada (S-52)
- [ ] **592** Eventos por sondeo con caché (ETag) o webhook solo por la tailnet o firmado
- [ ] **593** Lo que trae un evento es contenido externo: la sesión queda contaminada (S-05)
- [ ] **594** prueba: un evento con instrucciones inyectadas no puede crear otro trabajo
- [ ] **595** Al arrancar, calcular las corridas perdidas y aplicar la recuperación
- [ ] **596** prueba: con el cerebro caído en el disparo, se aplica la recuperación configurada
- [ ] **597** prueba: un cambio de horario no duplica ni salta corridas
- [ ] **598** Tras N fallos seguidos, pausar el trabajo y avisar
- [ ] **599** prueba: N fallos seguidos pausan el trabajo y avisan
- [ ] **600** Un trabajo propuesto por el agente no corre hasta la aprobación con passkey
- [ ] **601** prueba: un trabajo propuesto por el agente no corre hasta la aprobación con passkey
- [ ] **602** Notificador según el aviso configurado: resumen, inmediato o en silencio
- [ ] **603** Decidir cómo compartir el motor de colas con clio-memory sin que clio-memory dependa de otro crate de Clio
- [ ] **604** Cola del programador en archivo propio
- [ ] **605** Telemetría: corridas, duración, costo, retraso, fallos seguidos, corridas omitidas

### Clio sobre sí misma (S-53)

- [ ] **606** Clio trabaja en su propio repositorio con rama y PR, como en cualquier otro
- [ ] **607** Clio nunca despliega, fusiona ni aprueba cambios sobre sí misma (S-53)
- [ ] **608** Clio no puede cambiar sus políticas, sus ascensos de confianza ni sus reglas de seguridad (S-53)
- [ ] **609** Rutas protegidas: pasarela, vault, firmas, reglas de seguridad, prompts de sistema, skills, configuración de Tailscale
- [ ] **610** Revisión humana obligatoria en cada PR que toque una ruta protegida; ninguna política las cubre
- [ ] **611** prueba: un PR de Clio que toca la pasarela no se puede fusionar sin tu aprobación (S-53)
- [ ] **612** prueba: un push de Clio directo a la rama principal se rechaza
- [ ] **613** ataque: un cambio propuesto por Clio que intenta fusionarse o desplegarse sin revisión
- [ ] **614** El despliegue lo disparas tú, o el CI tras tu aprobación con passkey
- [ ] **615** Clio explica por qué propone un cambio, con datos de clio-trace

### Medición de la fase

- [ ] **616** medir D24: dos semanas de uso real con acciones autónomas, aprobaciones por día y reversiones
- [ ] **617** medir D24: correr un trabajo programado con el VPS caído a la hora del disparo
- [ ] **618** Revisar la tasa de aprobaciones de las dos semanas (D14)

### Cierre de la fase 5

- [ ] **619** Dos semanas de uso real con pocas aprobaciones al día y ninguna reversión dentro de una política

---

## Fase 6 · Jev y ajustes

### Adaptador de decisión

- [ ] **620** Adaptador de modelo de decisión detrás de la misma interfaz que Pi, para poder cambiar Jev por un modelo abierto
- [ ] **621** Guardar cada decisión de Jev en la traza
- [ ] **622** Modo sombra: Jev corre en paralelo sin actuar y sus decisiones se guardan junto a las reales
- [ ] **623** Calibrar con tus tareas, en español
- [ ] **624** Revisar si la confianza de Jev es fiable (comparar confianza contra acierto)
- [ ] **625** Zonas por decisión: confianza alta actúa, zona gris decide el LLM, riesgo va a la persona
- [ ] **626** Sacar los umbrales de tus datos y no de la documentación
- [ ] **627** Jev solo puede subir el control, nunca bajarlo
- [ ] **628** Todo lo que sale hacia Jev pasa por la limpieza de secretos (S-44)
- [ ] **629** Costo y latencia de Jev medidos (unos 0,19 USD por 10 000 decisiones, unos 255 ms, según la fuente)
- [ ] **630** Medir cada punto con y sin Jev en el banco: éxito, costo, latencia y cuántas veces escala

### Los diez puntos de decisión

- [ ] **631** Jev en el triaje: nivel, esfuerzo, ambigüedad, irreversible, herramientas; si duda, nivel 1
- [ ] **632** Jev en herramientas recomendadas: familias y herramientas sobre el umbral; si duda, el juego básico
- [ ] **633** Jev en el enrutador de nodos: elegir entre las opciones que las reglas dejan; si duda, preguntar
- [ ] **634** Jev en la pasarela: riesgo de la acción y coincidencia con el plan; solo puede subir el control
- [ ] **635** Jev en el gestor de memoria: ADD, UPDATE, DELETE o NOOP y hecho de usuario frente a externo; si duda, NOOP o cuarentena
- [ ] **636** Jev en la cuarentena: ¿el contenido externo trae instrucciones?; si duda, no confiable
- [ ] **637** Jev en la recuperación: relevancia y abstención; si duda, pasar los fragmentos al LLM
- [ ] **638** Jev en la verificación: ¿la afirmación está respaldada por la fuente?; solo como filtro, nunca veredicto
- [ ] **639** Jev en el anti-bucle: ¿este paso avanzó?; si duda, replanificar o cortar
- [ ] **640** Jev en el diagnóstico: tipo de fallo; si duda, tratarlo como código
- [ ] **641** Activar Jev solo en los puntos donde gana a las reglas
- [ ] **642** No usar Jev para presupuesto y umbrales numéricos, para planificar o escribir, para conceder permisos irreversibles ni para el veredicto final

### clio-memory v2 (plugins y consolidación)

- [ ] **643** Plugin de decisor con clasificador externo (Jev u otro)
- [ ] **644** Plugin de decisor con un LLM compatible con OpenAI o un modelo local
- [ ] **645** Modo sombra de decisores en clio-memory y comparación con datos reales
- [ ] **646** Resúmenes por episodio que suben en árbol (episodio, día, semana)
- [ ] **647** Entidades con alias que enlazan recuerdos
- [ ] **648** Trabajo `consolidate`: episódico a semántico con la máquina ociosa
- [ ] **649** Trabajo `dedupe` periódico por ámbito
- [ ] **650** Trabajo `reembed` al cambiar de modelo
- [ ] **651** Trabajo `summarize` al cerrar un episodio

### Modelos alternativos (spec: modelos)

- [ ] **652** Ruteo por nivel con alternativo de otro proveedor
- [ ] **653** Triaje: Jev primario y reglas como alternativo
- [ ] **654** El alternativo nunca tiene menos garantías de privacidad si la sesión maneja datos personales; si no, a la cola
- [ ] **655** Cortacircuito que deja de llamar a un proveedor tras varios errores seguidos
- [ ] **656** Responder «no se puede ahora» si ambos fallan en niveles 0 y 1; cola con aviso en niveles 2 y 3
- [ ] **657** prueba: con el proveedor primario caído, se usa el alternativo y queda registrado
- [ ] **658** prueba: con ambos caídos, la tarea va a la cola y avisa
- [ ] **659** prueba: una sesión con datos personales no cae a un alternativo con menos garantías
- [ ] **660** Proceso para cambiar de modelo: correr el banco completo, comparar, probarlo en niveles 0 y 1, fijar versión exacta
- [ ] **661** prueba: un cambio de modelo sin banco no se puede desplegar
- [ ] **662** Telemetría: errores, latencia, tokens y costo por proveedor y modelo, uso del alternativo, tareas a la cola
- [ ] **663** Revisión mensual de los topes de gasto (S-45)

### Recuperación de herramientas y subagentes

- [ ] **664** Recuperación vectorial de herramientas con su comparación
- [ ] **665** Subagentes solo con motivo medido (D12); solo leen y no reciben secretos (S-17)
- [ ] **666** medir D12: con y sin cada experto, repitiendo cada tarea
- [ ] **667** Skills generadas por el agente solo con revisión humana (S-16, S-26)

### Cierre de la fase 6

- [ ] **668** Cada pieza muestra su mejora en el banco, o se descarta

---

## Operación continua (desde la fase 1 en adelante)

### En cada PR

- [ ] **669** El PR nombra las reglas S-xx que toca y sus pruebas pasan
- [ ] **670** Ninguna dependencia nueva sin `cargo-deny`
- [ ] **671** Escáner de secretos antes de cada commit (S-43)
- [ ] **672** Revisión humana de cambios en pasarela, vault, firmas de nodos y PWA

### Antes de cada versión

- [ ] **673** Batería de ataques completa
- [ ] **674** `cargo-audit`
- [ ] **675** Revisión de la actualización de Pi y de los servidores MCP
- [ ] **676** La batería corriendo en el entorno de preproducción

### Cada semana

- [ ] **677** Tasa de aprobaciones (si pasa del 95 % es adorno)
- [ ] **678** Bloqueos de la pasarela
- [ ] **679** Intentos de usar un secreto hacia un destino ajeno
- [ ] **680** Sesiones contaminadas con salida al exterior
- [ ] **681** Revisar la cuarentena de memoria

### Cada mes

- [ ] **682** Rotar tokens
- [ ] **683** Restaurar un respaldo en una máquina limpia (S-40)
- [ ] **684** Revisar las reglas de Tailscale y la lista de dispositivos
- [ ] **685** Actualizar dependencias
- [ ] **686** Revisar los topes de gasto en cada proveedor (S-45)
- [ ] **687** Revisar las políticas sin uso y las que vencen

---

## Publicación como código abierto (cuando toque)

- [ ] **688** Releases del proyecto firmadas y con SBOM (S-43)
- [ ] **689** Decidir si clio-memory sale a su propio repositorio (cuando se publique o cuando otro crate dependa de él)
- [ ] **690** Publicar clio-memory en crates.io con nombre confirmado
- [ ] **691** Fuzzing nocturno corriendo en el CI (S-48)
- [ ] **692** `SECURITY.md` y modelo de amenazas visibles en el repositorio
- [ ] **693** Revisar que ningún dato personal ni secreto está en el historial antes de abrir el repositorio

---

## Después (no antes de lo anterior)

- [ ] **694** Índice vectorial aproximado, cuando salga de alfa y se mida a 50 000 y 500 000 recuerdos
- [ ] **695** Adaptador de Postgres como otra implementación de `Store`
- [ ] **696** Cifrado en reposo con SQLCipher, si el costo lo permite
- [ ] **697** Sincronización entre réplicas de la memoria
- [ ] **698** Interfaz web de revisión de la memoria
- [ ] **699** Logo (el hilo: un trazo, una línea de tiempo), con la paleta del canal
- [ ] **700** Modo claro, derivado de «hueso» como fondo, solo si hace falta
- [ ] **701** Reevaluar Pi contra un bucle propio si las actualizaciones rompen las extensiones a menudo (D16)
