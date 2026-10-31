## clio-models · spec v0.1

_Modelos y proveedores. Fuente: pestaña «Spec: modelos y proveedores» del artefacto «Clio · diseño del agente»._

Qué modelos usa Clio, qué pasa cuando un proveedor falla y cómo se cambia de modelo sin romper nada.

### Catálogo

Por cada modelo: proveedor, id exacto con versión fijada, ventana de contexto, precio (en la tabla versionada de clio-trace), capacidades (herramientas, visión), política de retención de datos del proveedor y niveles en los que se puede usar.

### Ruteo

| Nivel | Primario | Alternativo | Si ambos fallan |
|---|---|---|---|
| 0 y 1 | rápido y barato | otro rápido de otro proveedor | responder que no se puede ahora |
| 2 y 3 | fuerte | fuerte de otro proveedor | a la cola, con aviso |
| Triaje | Jev | reglas | — |

El alternativo nunca puede tener menos garantías de privacidad que el primario si la sesión maneja datos personales; en ese caso, mejor la cola. Cada proveedor tiene un límite de llamadas con espera, y un cortacircuito que deja de llamarlo tras varios errores seguidos.

### Prompts como código

Los prompts viven en el repositorio, con versión; cada traza registra qué versión usó. Cambiar un prompt es un PR que pasa por el banco.

### Cambiar de modelo

1. Correr el banco completo con el modelo nuevo.
2. Comparar éxito, costo y latencia con el actual.
3. Probarlo un tiempo en niveles 0 y 1 antes de llevarlo a 2 y 3.
4. Fijar la versión exacta: nunca un alias que cambia solo.

### Telemetría

Errores, latencia, tokens y costo por proveedor y modelo; veces que se usó el alternativo; tareas que fueron a la cola por caída.

### Reglas de seguridad que aplica

S-44, S-45.

### Pruebas de aceptación

1. Con el proveedor primario caído (falso), se usa el alternativo y queda registrado.
2. Con ambos caídos, la tarea va a la cola y avisa.
3. Un modelo sin versión fijada no se acepta en el catálogo.
4. Un cambio de modelo sin banco no se puede desplegar.
5. Una sesión con datos personales no cae a un alternativo con menos garantías.

