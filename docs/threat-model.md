## Modelo de amenazas

Qué protege Clio y de qué. Las reglas que lo cierran (S-xx), con su prueba, están en [`security.md`](security.md); si algo de aquí cambia, cambia allí también.

**Supuesto de partida.** Cualquier contenido de fuera puede traer instrucciones y el modelo a veces las va a seguir. La seguridad no depende de que el modelo se porte bien: está en código, fuera de él, y asume que el modelo ya fue engañado.

### Lo que se protege

- Secretos y llaves.
- Los dispositivos de la persona (los nodos tienen shell).
- La memoria y los datos personales.
- El dinero (APIs).
- La integridad de lo que se ejecuta, sobre todo lo irreversible.
- La disponibilidad del cerebro.

### De qué

- Inyección por contenido externo (lo más probable).
- Dependencias, extensiones, skills o servidores MCP comprometidos.
- Un dispositivo robado o perdido.
- Un dispositivo comprometido dentro de la tailnet.
- El equipo anfitrión comprometido (este PC o, si se usa, un VPS).
- Proveedores que ven los datos.
- Errores del propio modelo, sin malicia.

### Lo más grave: la trifecta letal

Clio junta datos privados (memoria, archivos), contenido no confiable (web, Notion, correos) y salida al exterior (red, mensajes). Por eso la pasarela decide por sesión, no solo por acción (S-05, S-06). Los demás huecos conocidos y las reglas que los cierran están en [`security.md`](security.md#huecos-en-el-diseño-actual).

### Si todo corre en tu propio PC (la base, ADR 0006)

Sin servidor aparte, el agente corre como tu usuario. Eso trae un riesgo que un VPS no tenía: tus archivos personales (`~/.ssh`, el perfil del navegador, otros proyectos) están al alcance de cualquier cosa que el agente pueda ejecutar. Lo que lo limita hoy:
- el sandbox de `clio-node` (carpeta de trabajo, rutas canonizadas, enlaces simbólicos hacia fuera rechazados) y la pasarela, que niega por defecto y no deja ejecutar comandos libres;
- **la jaula `bwrap`** (`clio-node/src/jail.rs`) para lo único que el agente puede ejecutar, `run_tests`: espacios de nombres nuevos (red, procesos, IPC, UTS, usuario, cgroup), sistema de archivos de solo lectura con solo la copia visible en `/work`, entorno vacío, límites de procesos, memoria, CPU, archivo y tiempo, y una autoprueba en `clio doctor` que intenta escapar y debe fallar;
- **lo que se muestra y lo que se guarda de un run**: el modo debug (`--debug`, `CLIO_DEBUG=1`, `Ctrl-D`) y las vistas de contexto y uso pasan todo por el limpiador de secretos (que conoce la clave del modelo; la reconoce en claro, en base64 y en URL) y por el saneado de terminal antes de dibujar o escribir; `debug.log` y `usage.json` son 0600 y sin debug no se escribe `debug.log` (`docs/uso-local.md`, «Modo debug»). Solo se oculta lo que el limpiador conoce;
- el vault y la pasarela son procesos aparte con sockets 0600, y ningún proceso de Clio escucha fuera de loopback ni de `tailscale0` (`infra/local/check-host.sh` lo vigila);
- el disco cifrado y el cortafuegos activo. OpenSSH está apagado, pero **Tailscale SSH está activado**: se puede entrar por la tailnet, así que quien controle un dispositivo de tu tailnet o tu cuenta de Tailscale puede abrir una terminal aquí si tu ACL lo permite. Si no lo usas, apágalo (`tailscale set --ssh=false`).

Pertenecer al grupo `docker` equivale a ser administrador, así que Docker debe quedar fuera del alcance del agente. Sin jaula probada (otro sistema operativo, sin `bwrap` o `prlimit`, núcleo sin espacios de nombres sin privilegios, autoprueba fallida) el agente no ejecuta nada: solo lee y escribe dentro del sandbox.

**Lo que la jaula NO cubre** (lista honesta; ver también `docs/uso-local.md`):

- **Fallos del núcleo.** Es un aislamiento del propio Linux, no una máquina virtual: no hay filtro seccomp, así que toda la superficie de llamadas al sistema sigue al alcance del código jaulado, y un fallo del núcleo (o de `bwrap`) permitiría salir. Mantén el sistema al día.
- **Canales laterales y encubiertos** (tiempos, caché, memoria compartida del procesador): no se intentan cerrar.
- **Tiempo de CPU y recursos del equipo.** Un tope de 120 s de reloj, 120 s de CPU, 2 GiB de memoria virtual y 128 procesos acota el daño, no lo elimina: un comando puede usar varios núcleos durante ese tiempo. No hay cuotas de disco: un programa puede llenar la copia con archivos (cada uno de hasta 64 MB) o `/tmp` hasta 256 MB (el tope de `/tmp` exige `bwrap` 0.9 o más; en 0.8 no existe). La memoria virtual (`RLIMIT_AS`) no es la memoria real.
- **Lo que está dentro de `/work`.** Es la copia: el código jaulado puede leerla entera y modificarla. Lo que no quieras que lea, no debe estar en la copia (la copia ya excluye lo que parece secreto). Lo que cambia sigue pasando por tu `diff` y tu `apply`, que rechaza rutas protegidas y enlaces simbólicos.
- **Lo que imprime.** La salida es contenido externo: se limpia, se envuelve como datos y marca la sesión, pero un modelo débil puede dejarse influir por ella; la defensa es la pasarela, no el prompt.
- **`/usr` es legible** (versiones de programas, qué paquetes hay instalados) y `/proc` muestra datos del equipo (CPU, memoria, núcleo).
- **El adaptador del modelo y `clio` mismo** no están en la jaula: corren como tu usuario. En el camino `run` (bucle propio) solo `run_tests` está enjaulado; en el chat, Pi sí corre enjaulado (ADR 0009, sección siguiente).
- **Un `kill -9` a `clio` durante `doctor`** puede dejar un archivo señuelo inofensivo en tu HOME.
- **`cargo test`** no se ofrece: haría falta montar parte de tu HOME.
- **El banco de pruebas (`clio-bench`)** ejecuta el `check` de cada tarea sobre lo que escribió el modelo: ese `check` corre en esta misma jaula (sin red, sin tu HOME ni tu entorno). Sin jaula no corre, salvo `CLIO_BENCH_CHECK_JAIL=off`.

### La pila con Pi: Pi + `clio-guard` + `clio-toolhost` (ADR 0007)

Pi corre el bucle del agente, los proveedores y las sesiones; Clio no reconstruye sus herramientas, solo cambia dónde corre cada operación primitiva. Pi 1.4.0 arranca con `--no-builtin-tools --no-extensions -e clio-guard --no-mcp --no-skills --no-prompt-templates --no-themes --no-context-files --no-approve --offline`; la extensión registra las herramientas de Pi (`read`, `write`, `edit`, `bash`, `find`, `ls`; `grep` no, porque siempre arranca `ripgrep` en la máquina) con operaciones propias que viajan al servicio, y las once herramientas de Clio para la memoria (cuatro leen; `memory_save` y las demás escriben en el acto, porque la memoria es del agente; ADR 0010). Pi mismo dice que no es un límite de seguridad: la seguridad viene de aislar lo que puede alcanzar. Capa por capa:

| Capa | Qué defiende | Qué NO defiende |
|---|---|---|
| Banderas de arranque de Pi | Sin herramientas incorporadas, sin extensiones ajenas, sin MCP, sin skills, sin archivos de contexto; una extensión que no carga detiene a Pi (sin ella no queda ninguna herramienta). | No es un límite: corre dentro del proceso de Pi. |
| Extensión `clio-guard` (`adapters/pi/extensions/clio-guard/`) | Registra solo las nueve herramientas (las seis de Pi y las tres de memoria), bloquea y reporta cualquier otra y las escrituras fuera de la copia o en carpetas protegidas, niega siempre `project_trust`, manda el `bash` de la persona por la jaula, añade el prompt versionado (falla cerrado si falta), marca todo resultado de herramienta como datos y lo pasa por el filtro del servicio, guarda la contaminación en la sesión de Pi y la restaura al reabrirla. | **Corre como la persona, dentro del proceso de Pi.** Un fallo de Pi o de una dependencia de Pi anula esta capa; por eso no decide nada que el servicio no decida también. |
| Servicio `clio-toolhost` | Lo único que ejecuta efectos. Socket Unix 0600 en una carpeta 0700; la pasarela decide cada operación normalizada (niega por defecto, falla cerrada, cortacircuito a las 10 negadas seguidas); el ejecutor de `clio-node` trabaja en la copia (rutas canonizadas, enlaces hacia fuera rechazados, `extensions`, `skills`, `.pi`, `.claude`, `.git`, `.agents`, `.vscode`, `.husky`, `.githooks`, `.cargo` y `.devcontainer` sin escritura); toda lectura y salida de comando se sanea y pasa por el limpiador del vault, y el filtro final limpia y envuelve lo que Pi le haya hecho; la contaminación no se borra y va en cada respuesta. Atiende `memory_search` y `memory_get` (solo lectura, solo recuerdos activos del ámbito `clio`, en una línea por recuerdo, limpios, envueltos como datos, 20 llamadas y 24 KiB por sesión con corte duro, registro de hechos sin texto: S-60, S-47). Si cae o calla, la herramienta falla y no se ejecuta nada. | Comparte usuario con Pi: quien controle a Pi puede hablar con su socket y hacer lo mismo que el modelo, con la misma pasarela delante. Un «preguntar» de la pasarela se niega (no hay diálogos en esta fase). Los nombres de archivo salen tal cual del listado (Pi tiene que poder consultar cada uno) y se limpian en el filtro final. |
| Jaula `bwrap` | **`bash` corre siempre aquí** (S-12, único shell libre): sin red, `HOME` oculto, solo la copia visible y escribible, `.pi`, `extensions`, `skills`, `.claude`, `.git`, `.agents`, `.vscode`, `.husky`, `.githooks`, `.cargo` y `.devcontainer` de solo lectura (las que no existen se crean vacías durante el comando para que no se puedan crear desde dentro, y se quitan después), límites de tiempo, procesos, memoria, archivo y salida. Solo se ofrece si la autoprueba pasa; si no, `bash` falla cerrado. La regla `S-12/bash` del servicio añade lo mínimo sobre el texto del comando: no vacío, sin NUL, hasta 8 KiB. | Lo que ya dice la sección anterior (sin seccomp, sin cuotas de disco). Lo que el comando hace dentro de la copia es legítimo: puede leerla, cambiarla y borrarla; lo que cambie sigue pasando por la revisión de la persona. |
| Vault / limpiador | Valores conocidos y patrones de llaves salen tachados de toda lectura, salida de comando y resultado final, incluso partidos con caracteres invisibles (se sanea antes de limpiar). | Solo lo que conoce. Si se configura un vault y no contesta, la salida se retiene. Los bytes que lee `edit` para reescribir un archivo no se limpian (dañarían el archivo); su resultado sí. |

**Límites honestos de esta pila:**

- El **proceso de Pi y la extensión corren como la persona, pero dentro de una jaula `bwrap`** (ADR 0009): no ven el vault ni tus archivos, la copia de trabajo les es de solo lectura (todo cambio pasa por el servicio de herramientas) y su red es propia: solo alcanzan la dirección del proveedor del modelo, a través de un proxy del anfitrión que niega todo lo demás, y no alcanzan el loopback ni los sockets abstractos del anfitrión. Lo que queda: Pi conserva la llave del proveedor y puede enviar lo que leyó a esa única dirección (que ya recibe todo el contexto del modelo). Si la jaula o el proxy no arrancan, o la autoprueba falla, Pi no arranca. Lo que niega la jaula se muestra en el chat.
- **La regla `S-12/bash` no lee el comando**: lo que un comando puede hacer lo limita la jaula, no una lista de palabras. Una denegación `S-12/bash` no cuenta para el cortacircuito (no pasa por la cuenta de la pasarela).
- **La contaminación no cierra aún ninguna salida**: ninguna herramienta tiene red, así que S-06 todavía no se ejercita; la marca existe, persiste y se informa, para cuando haya herramientas con salida.
- **El aviso de contaminación en el prompt de sistema** se arma al empezar cada ejecución, no entre turnos de una misma ejecución; dentro de una, lo que protege es el envoltorio de datos en cada resultado.
- **Una vez abierto el cortacircuito** (10 negadas seguidas) la sesión no vuelve a ejecutar nada por sí sola: hace falta que la persona decida (no hay diálogo todavía); el remedio actual es empezar otra sesión.
- **Lo que las herramientas de memoria leen sale del equipo** (S-60, ADR 0010):
  - El proveedor del modelo ve lo que la herramienta devuelve, igual que ya ve el contexto (S-44). El presupuesto (20 llamadas, 24 KiB, por sesión) limita cuánto puede salir, no lo elimina: lo que no deba salir del equipo no debe estar en la memoria.
  - Comparten usuario con Pi: un Pi comprometido puede hablar con el socket del servicio y hacer las mismas llamadas que el modelo, con la misma pasarela y el mismo presupuesto delante. No puede abrir `memory.db`: la jaula no la ve.
  - Un recuerdo **activo** de origen `agente` puede traer una inyección vieja (un run limpio que la escribió sin saberlo). Por eso llega envuelto como datos y marca la sesión; la defensa no es un filtro de texto.
  - No hay clasificación de sensibilidad: todo lo activo se puede consultar. La cuarentena (lo olvidado), lo archivado y lo de otro ámbito no se pueden leer, y los errores no dicen cuál de los cuatro casos es.
  - Al abrir la memoria en solo lectura no se anotan señales de uso: las estadísticas no reflejan lo que el agente consultó por herramienta.
  - El agente escribe en su memoria en el acto (`memory_save`, `memory_update`, `memory_forget`, `memory_pin`, `memory_consolidate`, `memory_revert` de lo suyo; ver «La memoria como superficie»); no hay herramienta ni operación que promueva, archive ni purgue (pedirlas es una operación no declarada, S-02).
- La comprobación de herramientas al arrancar se apoya en lo que la extensión informa (`pi.getActiveTools()` / `pi.getAllTools()` con su origen) y en la lista de herramientas que recibe el modelo; el RPC de Pi no tiene un comando que las liste.
- El enlace entre la extensión y el servicio no está autenticado más allá de los permisos del socket (carpeta 0700, socket 0600, mismo usuario).
- `grep` queda fuera mientras Pi no permita confinar su búsqueda; `bash grep` dentro de la jaula lo reemplaza.

**Hecho (ADR 0009):** Pi corre dentro de `bwrap` con el sistema de archivos mínimo (fase 3a) y red propia con el proveedor como único destino (fase 3b).

### La memoria como superficie (persistencia de inyecciones)

Una inyección normal muere con el run: el contenido hostil solo vive en el contexto de esa sesión. La memoria entre runs cambia eso: si un texto de un archivo, una web o una salida de pruebas logra quedar guardado como «recuerdo», vuelve al contexto de cada run futuro, ya sin el aviso de que vino de fuera. Es una inyección que persiste, y también una forma de envenenar lo que Clio cree de ti (por ejemplo «la persona autorizó X»). Lo que la acota (`crates/clio-brain/src/memory_link.rs`):

- **Quién escribe** (S-25, S-27). La memoria es del agente (spec `2026-11-04-clio-memory-standalone-design.md`): en el bucle de `clio run` lo aprendido se escribe de una vez al cerrar; en la pila con Pi el agente guarda, corrige, olvida, fija, une y deshace lo suyo con sus herramientas, todo activo en el acto, con actores propios y cuota por hora cada uno (500 la del agente, un freno a un bucle). Nada de eso promueve, archiva ni purga (S-60).
- **El origen `persona` solo lo escribe la persona** con `clio memory remember`. Lo que el agente guarda después de leer algo de fuera, y lo que el modelo pide recordar al cerrar un run, entra como `externo`; el actor de agente ni siquiera puede declarar origen de persona (la biblioteca lo degrada).
- **Origen a la vista, no cuarentena** (S-25, S-27): nada de lo que escribe el agente espera revisión; lo que vino tras leer algo de fuera queda activo marcado `externo`, leerlo contamina la sesión, el bucle de `clio run` no lo ve (salvo que la persona lo tenga en un bloque) y un bloque fijo no sigue una corrección `externo`. La cuarentena es solo lo olvidado (`memory_forget`, los originales de una unión): no sale en `recall` ni en el contexto y conserva su registro. Todo cambio queda en el registro de operaciones y la persona lo revierte.
- **Procedimientos** (S-26): los «cómo se hace» entran activos como cualquier recuerdo, con su origen; los que el modelo pide al cerrar un run son siempre `externo`.
- **Lo recordado vuelve como datos**: en una capa propia `memoria`, envuelta como contenido no confiable, con el id de cada recuerdo, tras el limpiador de secretos y el filtro de lo que sale hacia el modelo (S-18, S-44).
- **Lectura por herramienta** (S-60, ADR 0010): `memory_search` y `memory_get` las atiende el servicio de herramientas, que abre la memoria en solo lectura. Solo salen recuerdos activos del ámbito `clio`; los de cuarentena, los archivados y los de otro ámbito se contestan con el mismo error que uno que no existe. Lo que sale va limpio, en una línea por recuerdo, envuelto como datos con la fuente `memory`, y marca la sesión como contaminada solo si algún recuerdo no es de la persona (origen `agente` o `externo`). Cada sesión tiene un tope duro de 20 llamadas y 24 KiB de texto devuelto (`S-47/memory`) y un registro de hechos (ids, cuentas, un hash corto de la consulta; nunca texto). Una orden escrita dentro de la copia se cumple aunque venga de un recuerdo: la defensa es que llega como datos, la sesión queda marcada y la persona revisa la copia con `diff` y `apply`; solo se rechazan las que apuntan fuera de la carpeta o a una protegida (S-13, S-16). Casos 56 a 60 de la batería.
- **Escritura por herramienta** (S-61, S-64, S-65, S-27): `memory_save` y las demás no tienen argumentos para el actor, el origen ni el ámbito: el servicio los fija (actor `agente`, origen `agente` o `externo` según la sesión, los sitios de la sesión), así que una orden dentro de un archivo no consigue un recuerdo con origen de persona. El texto pasa antes por el limpiador del vault y, si tenía un secreto, no se guarda; el motor aplica su detector y la cuota por hora del actor `agente`. Con Jev encendido, Jev pregunta antes de escribir si el texto es inseguro o no vale la pena (un «no» claro no guarda nada). Cada sesión puede guardar cinco si leyó algo hostil, treinta si no, y hacer diez cambios (`S-47/memory`; cuentan aunque fallen y sobreviven a un reinicio del servicio). `memory_revert` del agente solo deshace lo que firmó el agente. La memoria se abre para escribir solo durante la llamada. Lo que queda: un modelo engañado puede guardar texto dañino (activo, marcado `externo`) o olvidar o corregir recuerdos buenos; la defensa es el origen marcado, Jev cuando está, el registro de operaciones y `clio memory revert`. Caso 61 de la batería.
- **Texto limpio**: al guardar se sustituyen los caracteres de control, y al mostrar (TUI, CLI) se sanea otra vez, aunque otra herramienta haya escrito el archivo.
- **Secretos**: el limpiador tacha lo que conoce (y los patrones de claves) y el detector de la biblioteca rechaza el resto antes de escribir; un rechazo se anota en una línea y no rompe el cierre del run.

Lo que **no** acota: el pedido que escribes tú se guarda como parte del registro del run (activo en un run limpio); si pegas en él texto de una web hostil, ese texto queda como recuerdo activo y vuelve en runs futuros, marcado como dato pero sin cuarentena. Lo que el agente guardó tras leer algo hostil está activo: la defensa es su marca `externo` (y Jev), no tu revisión; si lo ves, `clio memory revert` lo deshace. Un recuerdo activo de origen `agente` (de un run limpio) puede traer una inyección que nadie vio; la lectura por herramienta lo entrega envuelto y marca la sesión, pero no lo detecta. Archivar no borra (hasta que llegue PURGE, el texto sigue en el archivo y en el registro, sin cifrar). El detector de secretos no ve lo que no parezca una clave (un dato personal sin forma de clave sí se guarda si tú lo escribes).

### Fuera de alcance

Un atacante con control del dispositivo de la persona y de su passkey: ahí la firma ya no distingue a la persona del atacante. Lo limitan los topes de gasto y la revocación del cerebro en Tailscale.
