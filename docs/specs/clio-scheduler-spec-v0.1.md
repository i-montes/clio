## clio-scheduler · spec v0.1

_Programador. Fuente: pestaña «Spec: programador» del artefacto «Clio · diseño del agente»._

Los trabajos que corren solos: cuándo, con qué permiso y qué pasa cuando algo falla.

### Modelo de un trabajo

| Campo | Qué es |
|---|---|
| disparador | cron con zona horaria, intervalo o evento (fuente y condición) |
| plantilla de contrato | el contrato que se instancia en cada corrida |
| nivel | fijo; no pasa por el triaje |
| política | la que lo autoriza; sin política activa, no corre |
| destino | un nodo concreto o «el enrutador decide» |
| concurrencia | si la corrida anterior sigue: omitir o encolar |
| recuperación | si el cerebro estuvo caído en el disparo: correr una vez u omitir |
| ventana | horas permitidas (por ejemplo, no de madrugada) |
| tope | costo máximo por corrida |
| aviso | resumen, inmediato o en silencio |
| estado | propuesto, activo, pausado, vencido |

### Comportamiento

1. En el disparo, instancia el contrato y crea una tarea en la cola. No pasa por el triaje: su nivel ya está fijado.
2. La tarea pasa por la pasarela con la política del trabajo, como cualquier otra.
3. Los eventos se detectan por sondeo con caché (ETag) o por un webhook que solo llega por la tailnet o va firmado. Lo que trae el evento es contenido externo: la sesión queda contaminada (S-05).
4. Al arrancar, el programador calcula las corridas perdidas y aplica su política de recuperación.
5. Tras N fallos seguidos, el trabajo se pausa y avisa.
6. Los resultados van al notificador según el aviso configurado.

Usa el mismo motor de colas que clio-memory (código compartido, archivo propio) y el reloj inyectable, así que los cambios de horario se pueden probar.

### Telemetría

Corridas, duración, costo, retraso respecto al disparo, fallos seguidos y corridas omitidas.

### Reglas de seguridad que aplica

S-05 (lo que trae un evento es externo), S-49 (crear un trabajo exige passkey), S-52.

### Pruebas de aceptación

1. Con el cerebro caído en el disparo, se aplica la recuperación configurada.
2. Dos corridas del mismo trabajo no se superponen.
3. Con la política vencida, el trabajo no corre y avisa.
4. Un cambio de horario no duplica ni salta corridas.
5. Un evento con instrucciones inyectadas no puede crear otro trabajo.
6. N fallos seguidos pausan el trabajo y avisan.
7. Un trabajo propuesto por el agente no corre hasta la aprobación con passkey.

### Decisiones abiertas

| Decisión | Opciones |
|---|---|
| Cómo se escribe el disparador | cron · lenguaje natural traducido a cron y confirmado |
| Máximo de trabajos activos | sin tope · tope configurable |

