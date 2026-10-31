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

### Fuera de alcance

Un atacante con control del dispositivo de la persona y de su passkey: ahí la firma ya no distingue a la persona del atacante. Lo limitan los topes de gasto y la revocación del cerebro en Tailscale.
