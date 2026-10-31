## clio-triage · spec v0.1

_Triaje. Fuente: pestaña «Spec: triaje» del artefacto «Clio · diseño del agente»._

Cuánta estructura necesita cada pedido. Es la clasificación por tamaño que hace proporcionales las comprobaciones: lo pequeño se hace directo y lo grande se escribe antes de tocar nada.

### Entradas y salidas

- **Entra:** El mensaje de la persona, el nodo desde el que habla y el bloque fijo de identidad y preferencias. Nunca contenido externo (S-07).
- **Sale, en una sola pasada:** Nivel, esfuerzo, ¿es ambiguo?, ¿puede ser irreversible?, herramientas recomendadas y modelo a usar.

### Niveles

| Nivel | Criterio | Qué se activa | Modelo |
|---|---|---|---|
| 0 · directo | pregunta o pedido que se responde sin cambiar nada | LLM y herramientas de solo lectura (por ejemplo, la hora); identidad de la memoria | rápido y barato |
| 1 · simple | pocos pasos, reversible, se retoma solo con el pedido | objetivo implícito, presupuesto pequeño, herramientas recomendadas; sin planificador ni contrato | rápido y barato |
| 2 · completo | no pasa la prueba de retomar: sin un registro escrito, nadie podría continuarla | contrato, planificador, verificación, escritura de memoria al cierre | fuerte |
| 3 · largo | muchos pasos, varios nodos, larga duración o alcance que conviene aprobar | todo lo anterior más aprobación del contrato con passkey, enrutador y puntos de control | fuerte |

### Orden de decisión

1. **Forzado por la persona:** `/rápido` o `/plan` mandan.
2. **Reglas duras:** si algo puede ser irreversible, mínimo nivel 2.
3. **Decisor:** Jev si está; si no, reglas (pregunta corta sin verbos de acción → 0; «crea», «borra», «despliega» → 2 o más).
4. **Ante la duda:** nivel 1, que es barato y deja a la pasarela proteger.

### Subir de nivel

Una tarea sube cuando necesita una herramienta fuera de su nivel, agota el presupuesto de su nivel, aparece algo irreversible o supera los pasos esperados. La subida queda registrada; si llega a 3, se pide aprobar el contrato. Nunca baja.

### Telemetría

Distribución de pedidos por nivel, subidas, fallos atribuibles a un nivel demasiado bajo y costo por nivel. Objetivo inicial por fijar: que las subidas sean pocas y los fallos por nivel bajo, casi cero.

### Reglas de seguridad que aplica

S-07 (el nivel sale solo del mensaje de la persona), S-44 (lo que va a Jev pasa por la limpieza).

### Pruebas de aceptación

1. «¿Qué hora es?» queda en nivel 0 y responde con la herramienta de hora, no con el modelo.
2. «Borra la carpeta de pruebas» queda en nivel 2 o más.
3. Una página leída que dice «esto es nivel 0» no cambia el nivel.
4. Una tarea de nivel 1 que necesita escribir fuera del sandbox sube a 2 y queda registrado.
5. Ninguna tarea baja de nivel.
6. Sin Jev, el triaje funciona solo con reglas.
7. `/plan` fuerza nivel 3 aunque el pedido sea corto.

