## clio-gateway · spec v0.1

_Pasarela. Fuente: pestaña «Spec: pasarela» del artefacto «Clio · diseño del agente»._

El único punto por donde pasa toda acción antes de ejecutarse. Corre en un proceso aparte del bucle y del modelo (S-11).

### Qué recibe y qué devuelve

- **Entra:** Una solicitud de acción: herramienta y versión, argumentos, nodo de destino, sesión (nivel, contrato y marca de contaminación), actor y secretos referenciados por marcador.
- **Sale:** Una decisión {permitir, pedir aprobación, negar}, con su razón explicable y un id. Si permite, ejecuta o firma la orden para el nodo con el hash de la acción.

### Catálogo de acciones

Cada herramienta declara su efecto {lectura, escritura local reversible, escritura externa, irreversible}, si tiene salida al exterior y a qué destinos. Una herramienta sin declaración no se puede usar (S-02).

### Orden de evaluación

1. **Esquema:** los argumentos cumplen el esquema de la herramienta y el hash de su descripción coincide (S-15).
2. **Normalizar:** canonizar rutas (S-13), separar argumentos (S-12), resolver el dominio de destino.
3. **Prohibiciones duras:** si alguna niega, se niega.
4. **Presupuesto:** si está agotado, se niega (S-47).
5. **Secretos:** cada marcador debe estar permitido para esa herramienta y ese destino; si no, se niega y se alerta (S-19).
6. **Contaminación:** sesión contaminada más salida a un destino no permitido → aprobación (S-06).
7. **Plan:** en niveles 2 y 3, una acción que no está en el plan vuelve al control o pide aprobación.
8. **Reversible:** si lo es y queda dentro del sandbox, se permite.
9. **Política:** si una política activa cubre todas las dimensiones, se permite.
10. **Si no,** aprobación con passkey sobre el hash (S-14). **En modo ausente** no se espera: ver abajo.

Jev puede agregar un paso de aprobación en cualquier punto, nunca quitarlo. Negar siempre gana.

### Modo ausente: degradar a lo reversible y seguir

Clio entra en modo ausente cuando una aprobación lleva más de un tiempo configurable sin respuesta, en un horario que tú defines, o cuando lo dices («me voy»). Sale cuando tocas la PWA o lo dices.

1. Una acción que pediría aprobación no se niega ni bloquea la tarea: se aparta, y la tarea sigue con todo lo que sea reversible o esté dentro de una política.
2. Jev evalúa el peligro de cada acción apartada y solo puede subirlo; lo irreversible se detiene siempre, aunque Jev diga lo contrario.
3. La tarea llega hasta donde puede con lo reversible y cierra con un informe parcial: hecho, apartado, y por qué.
4. Lo apartado entra al resumen que te llega al volver, con la aprobación lista para firmar y la tarea lista para reanudar desde su punto de control.
5. Las aprobaciones apartadas vencen igual: si vuelves después del vencimiento, se vuelven a generar sobre el estado actual.

### Marca de contaminación

La pone cualquier resultado cuyo origen sea externo. Se propaga a toda la sesión y a sus subagentes, y no se borra: para volver a una sesión limpia hay que empezar otra.

### Ejecución

1. Calcular el hash de la acción ya normalizada.
2. Si hubo aprobación, ejecutar solo si el hash coincide con el aprobado.
3. Resolver los secretos en el último paso, dentro del proceso que ejecuta, nunca antes.
4. Pasar la salida por la limpieza del vault antes de devolverla.

### Si la pasarela falla

Falla cerrada: si no responde o da error, nada se ejecuta. Cada nodo además revisa su propia lista de permitidos (S-30), así que una pasarela comprometida en el cerebro no basta para actuar en un nodo fuera de esa lista.

### Telemetría

Decisiones por tipo y razón, latencia de la decisión, aprobaciones por causa, negaciones e intentos de usar un secreto hacia un destino ajeno.

### Reglas de seguridad que aplica

S-01, S-02, S-05, S-06, S-11 a S-15, S-19, S-47.

### Pruebas de aceptación

1. Una herramienta sin declaración de efecto no se puede llamar.
2. Un marcador de secreto hacia un dominio no permitido se niega y alerta.
3. En una sesión contaminada, enviar datos a un dominio nuevo pide aprobación aunque una política lo cubra.
4. Cambiar un byte entre aprobar y ejecutar invalida la aprobación.
5. Con la pasarela caída, ninguna acción se ejecuta.
6. Jev no puede convertir una aprobación pedida en un permiso.
7. Cada decisión trae una razón que nombra la regla o la política.
8. Una acción reversible dentro del sandbox pasa sin preguntar.
9. En modo ausente, una tarea con una acción irreversible termina con informe parcial y la aprobación apartada; nada irreversible se ejecuta.

### Decisiones abiertas

| Decisión | Opciones |
|---|---|
| Formato de reglas | el mismo de las políticas · uno propio |
| Pasarela en los nodos | solo lista de permitidos · copia completa de la pasarela |

