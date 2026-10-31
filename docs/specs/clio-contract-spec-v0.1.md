## clio-contract · spec v0.1

_Contrato de tarea. Fuente: pestaña «Spec: contrato de tarea» del artefacto «Clio · diseño del agente»._

Qué es una tarea, cuándo está terminada y cómo se deja escrita para poder retomarla. Aplica a los niveles 2 y 3 del triaje; los niveles 0 y 1 no llevan contrato.

### Campos

| Campo | Qué es | Regla |
|---|---|---|
| id, nivel | identificador y nivel (2 o 3) | el nivel viene del triaje y solo sube |
| objetivo | una frase, escrita o confirmada por la persona | solo la persona lo cambia (S-07) |
| rol | con qué perspectiva trabaja el agente | opcional en nivel 2 |
| alcance | qué entra y qué queda fuera, explícito | lo que no está dentro, está fuera |
| requiere spec | sí o no; si es sí, el spec se escribe antes del plan | sí cuando cambia una interfaz, el modelo de datos o un comportamiento observable |
| criterios de éxito | lista; cada uno dice cómo se comprueba: prueba, comando, inspección humana | un criterio sin forma de comprobarse no se acepta |
| terminación | éxito, presupuesto agotado, bloqueo, cancelación | cada una cierra con informe |
| presupuesto | pasos, tiempo y costo | aplicado en código (S-47) |
| política y nodos | qué política lo autoriza y en qué nodos puede correr | sin política, lo fuera de lo reversible pide aprobación |
| tareas | lista con estado y evidencia de cada una | se marca hecha solo con evidencia observada |
| preguntas abiertas | dudas reales que bloquean | como máximo una a la vez a la persona |
| origen, estado, trace_id | persona o programador; {borrador, aprobado, en curso, bloqueado, cerrado con éxito, cerrado con fallo, cancelado} | cada cambio de estado queda en la traza |

### Ciclo

1. **Explorar** en solo lectura: leer el código, los archivos y la memoria relevante antes de proponer nada.
2. **Borrador** del contrato. Si hay incertidumbre real, una sola pregunta concreta.
3. **Spec, si el contrato lo requiere:** comportamiento, interfaces, modelo de datos y pruebas de aceptación. Esas pruebas pasan a ser los criterios de éxito del contrato.
4. **Aprobación:** en nivel 3, obligatoria y con passkey, del contrato y del spec juntos; en nivel 2, implícita mientras no cambie el alcance.
5. **Plan** a partir del spec (o del contrato, si no hay spec).
6. **Registro durable** antes de la primera escritura: el contrato se guarda (en el repositorio de trabajo o en la memoria episódica) para que se pueda retomar.
7. **Tareas una por una**, en partes revisables; cada una se cierra solo con evidencia observada (salida de una prueba, un hash, una captura), nunca porque el modelo dice que terminó.
8. **Verificación** contra cada criterio de éxito, con señales externas. Si falla, pasa por el diagnóstico (ver su spec) antes de cualquier reintento.
9. **Cierre** con un informe fijo: resultado verificado, comprobaciones fallidas o pendientes, y el siguiente paso.

### Prueba de retomar

Un contrato está bien escrito si otra sesión, otro agente o tú pueden continuar la tarea solo con el contrato y el diff. Si no se puede, falta información en el contrato.

### Mensajes mientras trabaja: la bandeja

Si Clio está ejecutando, un mensaje nuevo no interrumpe: entra a una bandeja con dos carriles, y el triaje decide el carril con una regla, no con Jev.

| Carril | Qué entra | Cuándo se atiende |
|---|---|---|
| Inmediato | «para», «cancela», y enmiendas al contrato en curso («cambia X», «no toques Y») | al instante: parar y cancelar aprovechan el snapshot; una enmienda pausa, se aplica al contrato y sigue |
| Normal | todo lo demás: preguntas, pedidos nuevos, contexto | al cerrar la ejecución en curso, todos juntos: Clio lee la cola completa y responde de una vez, no mensaje por mensaje |

Un pedido nuevo del carril normal pasa por el triaje como siempre y, si es de nivel 2 o 3, entra a la cola del cerebro con su propia sesión (ver spec del día cero y la programación de varias tareas en clio-network).

### Cambios

El agente puede proponer enmiendas (otro criterio, más presupuesto); solo la persona las acepta. Cada enmienda queda versionada en el mismo registro.

### Telemetría

Tiempo por fase, criterios cumplidos al primer intento, enmiendas por contrato, cierres por tipo y tareas reabiertas.

### Reglas de seguridad que aplica

S-07 (el objetivo no lo cambia contenido externo), S-14 (aprobación atada a la acción), S-47 (presupuesto en código).

### Pruebas de aceptación

1. Un contrato con un criterio sin forma de comprobarse no pasa de borrador.
2. Ninguna tarea se marca hecha sin evidencia adjunta.
3. Una instrucción dentro de un archivo leído no modifica el objetivo ni el alcance.
4. Agotar el presupuesto cierra el contrato con informe, no lo deja colgado.
5. El informe de cierre siempre lista lo pendiente, aunque esté vacío.
6. Otra sesión retoma una tarea a medias solo con el contrato y el diff, y la termina.
7. En nivel 3, nada se escribe antes de la aprobación con passkey.
8. Un contrato que cambia una interfaz no pasa a plan sin spec, y las pruebas del spec aparecen como criterios de éxito.
9. «Para» a mitad de una tarea la detiene en segundos y deja el sandbox como estaba; un mensaje normal espera al cierre y se responde junto con los demás de la cola.

### Decisiones abiertas

| Decisión | Opciones |
|---|---|
| Formato | markdown con frontmatter · JSON |
| Dónde vive el registro | repositorio de trabajo · memoria episódica · ambos |

