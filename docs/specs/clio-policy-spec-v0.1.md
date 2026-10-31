## clio-policy · spec v0.1

_Políticas permanentes. Fuente: pestaña «Spec: políticas permanentes» del artefacto «Clio · diseño del agente»._

Cómo se autoriza una clase de acciones una sola vez para que Clio trabaje sin preguntar, sin perder el control.

### Modelo

| Campo | Qué es |
|---|---|
| alcance | herramientas, acciones, rutas (canonizadas), dominios, nodos, ámbitos de memoria y secretos permitidos |
| límites | costo por día y por ejecución, número de acciones |
| vigencia | desde y hasta; el vencimiento es obligatorio (S-50) |
| condiciones | por ejemplo, «solo si la sesión no está contaminada» o «solo acciones reversibles» |
| origen y firma | quién la propuso y la passkey que la aprobó |
| estado | propuesta, activa, suspendida, vencida, revocada |
| historial | cada uso, cada cambio y cada degradación |

### Evaluación en la pasarela

1. **Prohibiciones duras** primero: si una regla niega, se niega. Negar siempre gana.
2. **¿Es reversible?** Si lo es y está dentro del sandbox, pasa sin política.
3. **¿Hay una política activa que cubra todas las dimensiones** (herramienta, ruta, dominio, nodo, secreto, límites)? Si cubre todas, pasa; si falta una sola, no cubre.
4. **Si no,** aprobación con passkey.

Las políticas se escriben en un formato declarativo (no código) y se evalúan de forma determinista. Cada decisión se puede explicar: «permitido por la política X, dimensión Y».

### Confianza ganada

Para cada clase de acción, clio-trace cuenta ejecuciones y reversiones. Pasadas N ejecuciones sin reversión, Clio propone una política que la cubra; la persona la aprueba. Una reversión dentro de una política la suspende y vuelve a pedir aprobación (S-51).

### Políticas y modo ausente

Una política puede declarar «solo con la persona presente». Las demás siguen valiendo en modo ausente. Al crear una política, el valor por defecto es «vale también ausente» para lo reversible y «solo presente» para todo lo que salga al exterior.

### Vencimiento

Aviso una semana antes; renovar exige passkey otra vez. Las políticas sin uso en su vigencia se proponen para borrar.

### Telemetría

Decisiones por política, cercanía a los topes, aprobaciones pedidas por falta de política y políticas sin uso.

### Reglas de seguridad que aplica

S-19 (secretos atados a destino), S-49 a S-51.

### Pruebas de aceptación

1. Una política sin vencimiento o sin tope se rechaza al crearla.
2. Una acción que cumple cuatro de cinco dimensiones pide aprobación.
3. Una prohibición dura gana sobre cualquier política.
4. Una política vencida no autoriza nada.
5. Una reversión dentro de una política la suspende.
6. Cada decisión trae su explicación.
7. El agente puede proponer una política, pero no activarla.

### Decisiones abiertas

| Decisión | Opciones |
|---|---|
| Formato | TOML · JSON · un lenguaje de políticas existente |
| Vigencia máxima | 30 · 90 días |

