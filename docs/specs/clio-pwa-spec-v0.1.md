## clio-pwa · spec v0.1

_PWA. Fuente: pestaña «Spec: PWA» del artefacto «Clio · diseño del agente»._

La superficie desde la que hablas con Clio, apruebas y revisas, en cualquier dispositivo.

### Pantallas

| Pantalla | Qué muestra y permite |
|---|---|
| Conversación | chat con el nivel del pedido, el nodo que trabaja y una marca visible si la sesión está contaminada; si Clio está ocupada, el mensaje se ve «en cola» y hay un botón de parar siempre visible |
| Foco | una sola tarea en primer plano; las demás en segundo plano con su estado; cambiar de foco no interrumpe ninguna |
| Tareas | en curso, en cola y programadas; contrato, progreso y evidencia; cancelar |
| Aprobaciones | lista y detalle con la acción exacta, el diff, el destino y el origen; aprobar con passkey (S-36) |
| Resumen | lo que pasó mientras no mirabas, agrupado |
| Trazas | la cascada de clio-trace, ordenable por tiempo o por costo |
| Memoria | buscar; revisar cuarentena, conflictos y revisión pendiente; promover, revertir o purgar con passkey |
| Políticas y trabajos | ver, proponer, renovar y revocar |
| Nodos | estado, últimas conexiones; revocar un dispositivo |
| Costos | por día, nivel y modelo; topes |

### Principios de diseño

Primero el teléfono; modo oscuro primero; sin scroll horizontal en ninguna pantalla; la información sensible nunca en la pantalla de bloqueo ni en el push.

### Técnica

Servida solo dentro de la tailnet con `tailscale serve`. Service worker para push y para guardar la interfaz, nunca datos sensibles. CSP estricta (S-33), markdown saneado (S-34), sesión con passkey (S-37). Sin conexión muestra la interfaz, pero no permite aprobar ni ver datos.

### Telemetría

Tiempo desde el push hasta la aprobación, aprobaciones rechazadas, errores del cliente (sin contenido).

### Reglas de seguridad que aplica

S-33 a S-37.

### Pruebas de aceptación

1. La pantalla de aprobación muestra la acción exacta y su hash.
2. Una imagen remota en una respuesta no genera ninguna petición.
3. Ningún push trae contenido.
4. Sin conexión no se puede aprobar.
5. Promover, revertir o purgar memoria pide passkey.
6. Ninguna pantalla tiene scroll horizontal a 360 px de ancho.
7. La marca de sesión contaminada aparece en la conversación y en la aprobación.
8. El botón de parar funciona aunque haya mensajes en cola y detiene solo la tarea en foco.

### Decisiones abiertas

| Decisión | Opciones |
|---|---|
| Framework | sin framework · uno liviano |
| Notificaciones en computadores | la misma PWA · avisos nativos del daemon |

