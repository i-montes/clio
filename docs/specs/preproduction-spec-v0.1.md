## preproduction · spec v0.1

_Preproducción. Fuente: pestaña «Spec: preproducción» del artefacto «Clio · diseño del agente»._

Dónde se prueba Clio antes de que toque nada real, y qué tiene que pasar para llegar a producción.

### Componentes

| Pieza | Qué simula |
|---|---|
| Red de prueba | una tailnet aparte, o contenedores en red local, sin acceso a tus dispositivos reales |
| Nodos falsos | capacidades, latencia, caídas, reconexiones y batería configurables |
| LLM con grabación y reproducción | graba respuestas reales una vez (por hash de la petición) y luego las reproduce: pruebas deterministas y sin costo; avisa si una petición nueva no tiene grabación |
| Jev, push y proveedores falsos | respuestas controladas, incluidas caídas y límites |
| Vault de prueba con señuelos | secretos falsos únicos que, si aparecen en una traza, un log o un payload, hacen fallar la prueba |
| Reloj inyectable | vigencias, olvido, programador y cambios de horario reproducibles |
| Datos sintéticos | nunca datos reales; memoria y archivos generados |

### Suites

| Suite | Qué cubre |
|---|---|
| Unitarias | cada crate |
| Aceptación | las pruebas de cada spec de esta página |
| Banco de evaluación | las 10 tareas reales y los pedidos triviales |
| Batería de ataques | los casos de la pestaña Seguridad |
| Caos | red cortada, VPS caído, disco lleno, proveedor caído, vault bloqueado |
| Fuzzing | mensajes del protocolo, tokenizador, importadores |

### Compuerta

| Cuándo | Qué corre | Bloquea si |
|---|---|---|
| Cada PR | unitarias, aceptación y ataques, en reproducción | falla cualquiera |
| Cada noche | banco con modelo real, caos, fuzzing | el éxito del banco baja o el costo por tarea sube más del umbral |
| Antes de una versión | todo lo anterior y tu revisión de pasarela, vault y firmas | un solo ataque pasa o un señuelo aparece |

Un cambio de prompt, de modelo o de política es un cambio de código: pasa por la misma compuerta.

### Pruebas de aceptación

1. Dos corridas en modo reproducción dan exactamente el mismo resultado.
2. Un señuelo que aparece en una traza hace fallar la compuerta.
3. Un cambio de prompt que no pasó el banco no se puede desplegar.
4. Los nodos falsos reproducen una caída a mitad de tarea de forma determinista.
5. Preproducción no puede alcanzar ningún dispositivo real ni el vault real.

### Decisiones abiertas

| Decisión | Opciones |
|---|---|
| Red de prueba | tailnet aparte · contenedores locales |
| Umbrales de la compuerta | por fijar con las primeras mediciones |

