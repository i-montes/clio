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
- **El adaptador del modelo y `clio` mismo** no están en la jaula: corren como tu usuario, solo `run_tests` está enjaulado.
- **Un `kill -9` a `clio` durante `doctor`** puede dejar un archivo señuelo inofensivo en tu HOME.
- **`cargo test`** no se ofrece: haría falta montar parte de tu HOME.

### La pila con Pi: Pi + `clio-guard` + `clio-toolhost` (ADR 0007)

Pi corre el bucle del agente, los proveedores y las sesiones; Clio le quita todo lo que toca la máquina y le pone encima su propia guardia. Pi 1.4.0 arranca con `--no-builtin-tools --no-extensions -e clio-guard --no-mcp --no-skills --no-prompt-templates --no-themes --no-context-files --no-approve --offline`. Pi mismo dice que no es un límite de seguridad: la seguridad viene de aislar lo que puede alcanzar. Capa por capa:

| Capa | Qué defiende | Qué NO defiende |
|---|---|---|
| Banderas de arranque de Pi | Sin `bash`/`read`/`write`/`edit`, sin extensiones ajenas, sin MCP, sin skills, sin archivos de contexto; una extensión que no carga detiene a Pi (no queda ninguna herramienta). | No es un límite: corre dentro del proceso de Pi. |
| Extensión `clio-guard` (`adapters/pi/extensions/clio-guard/`) | Registra solo cinco herramientas, bloquea y reporta cualquier otra y las escrituras que el servicio rechazaría, niega siempre `project_trust` y el `bash` del cliente RPC, añade el prompt versionado (falla cerrado si falta), marca todo resultado de herramienta como datos, guarda la contaminación en la sesión de Pi y la restaura al reabrirla. | **Corre como la persona, dentro del proceso de Pi.** Un fallo de Pi o de una dependencia de Pi anula esta capa; por eso no decide nada que el servicio no decida también. |
| Servicio `clio-toolhost` | Lo único que ejecuta efectos. Socket Unix 0600 en una carpeta 0700; la pasarela decide cada llamada (niega por defecto, falla cerrada, cortacircuito a las 10 negadas seguidas); el ejecutor de `clio-node` trabaja en la copia (rutas canonizadas, enlaces hacia fuera rechazados, `extensions`, `skills`, `.pi`, `.claude`, `.git`, `.agents` sin escritura); toda salida se sanea (secuencias de escape), pasa por el limpiador del vault y se envuelve como datos; la contaminación de la sesión no se borra y va en cada respuesta. Si el servicio cae o calla, la herramienta falla y no se ejecuta nada. | Comparte usuario con Pi: quien controle a Pi puede hablar con su socket (es lo que hace la extensión) y puede intentar lo mismo que el modelo, con la misma pasarela delante. Un «preguntar» de la pasarela se niega (no hay diálogos en esta fase). |
| Jaula `bwrap` | Lo único que se ejecuta de verdad, `run_tests`, corre sin red, con solo la copia visible, y solo si la autoprueba pasa. | Lo que ya dice la sección anterior (sin seccomp, sin cuotas de disco). |
| Vault / limpiador | Valores conocidos y patrones de llaves salen tachados de toda salida, incluso partidos con caracteres invisibles (se sanea antes de limpiar). | Solo lo que conoce. Si se configura un vault y no contesta, la salida se retiene. |

**Límites honestos de esta pila:**

- El **proceso de Pi y la extensión corren como la persona**, con sus archivos y su red; solo las herramientas están confinadas. Pi lee su `models.json` y la llave del proveedor, y habla con el proveedor por la red. Una vulnerabilidad en Pi o en una de sus ~120 dependencias de Node no la detiene nada de esto.
- **La contaminación no cierra aún ninguna salida**: las cinco herramientas no tienen red, así que S-06 todavía no se ejercita; la marca existe, persiste y se informa, para cuando haya herramientas con salida.
- **El aviso de contaminación en el prompt de sistema** se arma al empezar cada ejecución, no entre turnos de una misma ejecución; dentro de una, lo que protege es el envoltorio de datos en cada resultado.
- **Una vez abierto el cortacircuito** (10 negadas seguidas) la sesión no vuelve a ejecutar nada por sí sola: hace falta que la persona decida (no hay diálogo todavía); el remedio actual es empezar otra sesión.
- La comprobación de herramientas al arrancar se apoya en lo que la extensión informa (`pi.getActiveTools()` / `pi.getAllTools()` con su origen) y en la lista de herramientas que recibe el modelo; el RPC de Pi no tiene un comando que las liste.
- El enlace entre la extensión y el servicio no está autenticado más allá de los permisos del socket (carpeta 0700, socket 0600, mismo usuario).

**Trabajo futuro:** correr Pi mismo dentro de `bwrap`, con red limitada al proveedor del modelo y solo la copia, el socket del servicio y su carpeta de datos visibles; es lo que cambia el límite de «solo las herramientas» a «todo el proceso».

### La memoria como superficie (persistencia de inyecciones)

Una inyección normal muere con el run: el contenido hostil solo vive en el contexto de esa sesión. La memoria entre runs cambia eso: si un texto de un archivo, una web o una salida de pruebas logra quedar guardado como «recuerdo», vuelve al contexto de cada run futuro, ya sin el aviso de que vino de fuera. Es una inyección que persiste, y también una forma de envenenar lo que Clio cree de ti (por ejemplo «la persona autorizó X»). Lo que la acota (`crates/clio-brain/src/memory_link.rs`):

- **Un solo escritor y solo al cerrar** (S-25, S-27). El bucle del modelo no tiene herramienta de memoria (pedirla es una herramienta no declarada: S-02); lo aprendido se escribe de una vez cuando el run termina, con actores propios y cuota por hora cada uno.
- **El origen `persona` solo lo escribe la persona** con `clio memory remember`. Lo que el modelo dice o lo que vino de un archivo entra como externo; el actor de agente ni siquiera puede declarar origen de persona (la biblioteca lo degrada).
- **Cuarentena** (S-25): todo lo externo, y todo lo que el modelo pide recordar, espera; no sale en `recall` ni en el contexto hasta que la persona promueve. El registro del run queda activo solo si el run no tocó contenido de fuera (si leyó cualquier archivo o salida de pruebas, queda en cuarentena).
- **Procedimientos** (S-26): los «cómo se hace» propuestos por el agente esperan aprobación de la persona aunque el run haya sido limpio.
- **Lo recordado vuelve como datos**: en una capa propia `memoria`, envuelta como contenido no confiable, con el id de cada recuerdo, tras el limpiador de secretos y el filtro de lo que sale hacia el modelo (S-18, S-44).
- **Texto limpio**: al guardar se sustituyen los caracteres de control, y al mostrar (TUI, CLI) se sanea otra vez, aunque otra herramienta haya escrito el archivo.
- **Secretos**: el limpiador tacha lo que conoce (y los patrones de claves) y el detector de la biblioteca rechaza el resto antes de escribir; un rechazo se anota en una línea y no rompe el cierre del run.

Lo que **no** acota: el pedido que escribes tú se guarda como parte del registro del run (activo en un run limpio); si pegas en él texto de una web hostil, ese texto queda como recuerdo activo y vuelve en runs futuros, marcado como dato pero sin cuarentena. Un recuerdo que apruebes tú es tuyo: aprobar sin leer anula la defensa (el panel muestra de dónde vino cada uno). Archivar no borra (hasta que llegue PURGE, el texto sigue en el archivo y en el registro, sin cifrar). El detector de secretos no ve lo que no parezca una clave (un dato personal sin forma de clave sí se guarda si tú lo escribes).

### Fuera de alcance

Un atacante con control del dispositivo de la persona y de su passkey: ahí la firma ya no distingue a la persona del atacante. Lo limitan los topes de gasto y la revocación del cerebro en Tailscale.
