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
- el vault y la pasarela son procesos aparte con sockets 0600, y ningún proceso de Clio escucha fuera de loopback ni de `tailscale0` (`infra/local/check-host.sh` lo vigila);
- el disco cifrado, el cortafuegos activo y SSH apagado.

Lo que aún falta y conviene saber: una **jaula a nivel de sistema** para ejecutar comandos (`bwrap` está instalado, no se usa todavía) y que pertenecer al grupo `docker` equivale a ser administrador, así que Docker debe quedar fuera del alcance del agente. Mientras no haya jaula, el agente no ejecuta comandos: solo lee y escribe dentro del sandbox.

### Fuera de alcance

Un atacante con control del dispositivo de la persona y de su passkey: ahí la firma ya no distingue a la persona del atacante. Lo limitan los topes de gasto y la revocación del cerebro en Tailscale.
