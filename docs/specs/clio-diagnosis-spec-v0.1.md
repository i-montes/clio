## clio-diagnosis · spec v0.1

_Diagnóstico. Fuente: pestaña «Spec: diagnóstico» del artefacto «Clio · diseño del agente»._

Qué pasa cuando algo falla: juntar evidencia, clasificar el fallo y mandarlo al lugar correcto antes de volver a intentar.

### Cuándo se activa

Cuando la verificación falla, cuando una herramienta devuelve un error que no es trivial o cuando el anti-bucle detecta que no hay avance. Nunca se reintenta antes de terminar el diagnóstico.

### Fase 1 · Juntar evidencia (código, sin LLM)

1. **Reproducir** el fallo con el mismo comando o prueba.
2. **Repetir una vez** para saber si es intermitente.
3. **Juntar:** salida exacta del error, tramos de clio-trace del pedido, diff desde el último estado verde, versión de dependencias y estado del nodo.
4. **Acotar** cuando se pueda: bisección sobre los commits o pasos desde el último verde.

Esta fase es determinista y no gasta tokens: qué evidencia se junta y cuánta lo fija el código, no el modelo, para que sea siempre la misma y de tamaño acotado. Su resultado es el paquete de evidencia, lo único que ve el modelo después.

### Fase 2 · Clasificar y enrutar (Jev o reglas)

| Tipo | Señal | A dónde va |
|---|---|---|
| Especificación | el criterio o el spec se contradicen, o piden algo imposible | a la persona, con una propuesta de enmienda |
| Entorno | red, dependencia, permisos o nodo caído | al enrutador (otro nodo) o a la cola para más tarde |
| Intermitente | pasa al repetir | un reintento, y queda marcado en clio-trace |
| Código | falla siempre igual y la evidencia apunta al cambio | al ciclo de corrección |

### Fase 3 · Hipótesis y corrección (LLM, en contexto fresco)

Aquí sí trabaja el LLM, pero en una sesión nueva del núcleo: su contexto es el contrato más el paquete de evidencia, nunca la trayectoria que falló. Es la respuesta al «self-conditioning» medido por Sinha et al.: un modelo que ve sus propios errores en el contexto se equivoca más.

1. **Hipótesis** escrita en una frase, a partir de la evidencia.
2. **Prueba mínima** que falle por esa causa y confirme la hipótesis.
3. **Corrección** que haga pasar esa prueba sin romper las demás.
4. **Verificación** completa otra vez.

Cada vuelta gasta del presupuesto de reintentos de la capa de control. Si se agota, el contrato se cierra con informe: qué se probó, qué se descartó y cuál es la mejor hipótesis pendiente.

### La skill

El procedimiento vive como memoria procedimental revisada por la persona (S-26) y se carga como skill de Pi solo cuando se activa el diagnóstico. El agente puede proponer mejoras al procedimiento, pero no cambiarlo solo.

### Telemetría

Fallos por tipo, vueltas del ciclo por fallo, porcentaje resuelto en la primera hipótesis, intermitentes por prueba y costo del diagnóstico frente al costo de reintentar sin él.

### Reglas de seguridad que aplica

S-20 (la evidencia pasa por la limpieza antes de llegar al modelo), S-26 (el procedimiento lo revisa la persona), S-47 (presupuesto).

### Pruebas de aceptación

1. Un fallo nunca dispara un reintento sin un paquete de evidencia previo.
2. La sesión de corrección no contiene ningún mensaje de la sesión que falló: solo el contrato y el paquete de evidencia.
3. Una prueba intermitente sembrada se clasifica como intermitente y se reintenta una sola vez.
4. Un error de red se manda al enrutador o a la cola, no al ciclo de corrección.
5. Un criterio contradictorio sembrado llega a la persona con una propuesta de enmienda.
6. En un fallo de código, la corrección viene precedida de una prueba que fallaba por esa causa.
7. Agotar los reintentos cierra con un informe que lista hipótesis probadas y descartadas.
8. El paquete de evidencia no trae secretos.

### Decisiones abiertas

| Decisión | Opciones |
|---|---|
| Tamaño del paquete de evidencia | fijo · según nivel |
| Bisección automática | siempre · solo con permiso de política |

