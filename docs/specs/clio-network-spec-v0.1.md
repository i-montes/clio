## clio-network · spec v0.1

_Clio-network. Fuente: pestaña «Spec: clio-network» del artefacto «Clio · diseño del agente»._

El protocolo entre el cerebro y los nodos: cómo se saludan, cómo viajan las órdenes y qué pasa cuando algo se corta.

### Transporte

WebSocket sobre la tailnet (WireGuard ya cifra el canal). Lo abre siempre el nodo hacia el cerebro. Además del cifrado de Tailscale, cada mensaje va firmado (S-29): estar en la tailnet no basta.

### Saludo

1. El nodo envía `hello`: id del nodo, versión del protocolo, capacidades, un número de un solo uso y su firma con la llave del nodo (Ed25519).
2. El cerebro verifica la firma contra el registro y la identidad de Tailscale del dispositivo; si algo no coincide, cierra.
3. El cerebro responde `welcome` firmado, con la versión acordada y el último mensaje que el nodo confirmó.

### Sobre de cada mensaje

`{ id (ULID), type, ts, expires_at, traceparent, payload, sig }`. El nodo rechaza todo mensaje sin firma válida, vencido o con un id ya visto.

| Tipo | Dirección | Para qué |
|---|---|---|
| hello · welcome | nodo ↔ cerebro | saludo y versión |
| capabilities | nodo → cerebro | capacidades y estado (libre, ocupado, batería) |
| task.assign · task.cancel | cerebro → nodo | asignar o cancelar trabajo |
| task.progress · task.result | nodo → cerebro | avance y resultado, con evidencia |
| ack | ambos | confirmar recepción por id |
| event · error | ambos | eventos del sistema y errores tipados |

### Garantías

- **Ningún mensaje se pierde:** El cerebro guarda cada mensaje (outbox) hasta recibir su ack; se reenvía tras reconectar. Entrega al menos una vez.
- **Ninguno se ejecuta dos veces:** El nodo guarda los ids vistos y descarta duplicados: en la práctica, una sola vez.
- **Reconexión:** Espera creciente con azar; al volver, se reanuda desde el último ack y el nodo informa el estado de lo que tenía en curso.
- **Sin cerebro, quieto:** El nodo termina el paso atómico en curso, no empieza nada nuevo y, pasado un tiempo, cancela lo pendiente.

### Varias tareas a la vez: el orquestador es código

Cada contrato corre en su propia sesión del núcleo, con contexto independiente. Quién corre y cuándo lo decide el cerebro con reglas, no otro LLM: un orquestador LLM agregaría una capa de fallo a cada tarea (MAST: la desalineación entre agentes explica el 36,94 % de los fallos).

1. **Prioridad:** lo que la persona pide directamente va antes que lo programado; después, por plazo y por costo. La persona puede fijar prioridad a mano.
2. **Concurrencia:** tope configurable de sesiones a la vez; los recursos (repo, ruta, nodo) se arriendan, y dos tareas que quieren el mismo recurso se serializan.
3. **Interrupción:** solo la persona interrumpe una tarea en curso; una tarea nunca interrumpe a otra.
4. **Atención:** una sola tarea en foco en la PWA; el resto reporta por el resumen.
5. **Presupuesto global:** el tope por día se reparte entre las tareas activas; si se agota, las de menor prioridad se pausan primero.

### Capacidades

Cada capacidad tiene nombre, esquema de argumentos y la lista local de permitidos del nodo, que el cerebro no puede cambiar a distancia (S-30).

### Versionado

El protocolo lleva semver; se negocia en el saludo. Una versión mayor incompatible se rechaza con un error claro.

### Telemetría

Reconexiones por nodo, latencia hasta el ack, mensajes reenviados, rechazados por firma o por vencimiento, y tiempo con el nodo desconectado. El traceparent viaja en cada mensaje.

### Reglas de seguridad que aplica

S-28 a S-32.

### Pruebas de aceptación

1. Órdenes sin firma, vencidas o repetidas se rechazan.
2. Cortar la red a mitad de una tarea y reconectar no pierde ni duplica mensajes.
3. Un nodo con versión mayor incompatible recibe un error claro.
4. Con el cerebro caído, el nodo no empieza trabajo nuevo.
5. Un tramo que corre en un nodo aparece colgado de la traza del pedido original.
6. Una orden para ampliar la lista de permitidos del nodo se rechaza.
7. Dos tareas sobre el mismo repo se serializan; ninguna tarea puede interrumpir a otra.

### Decisiones abiertas

| Decisión | Opciones |
|---|---|
| Serialización | JSON · MessagePack · CBOR |
| Rotación de llaves de nodo | manual · automática con periodo fijo |

