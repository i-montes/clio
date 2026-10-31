## clio-day-zero · spec v0.1

> **Actualización 2026-10-31 ([ADR 0006](../decisions/0006-local-first-vps-optional.md)):** la base es el equipo de la persona. Donde este spec dice «VPS», léase «el equipo anfitrión (este PC por defecto)»; el VPS es opcional.

_Día cero. Fuente: pestaña «Spec: día cero» del artefacto «Clio · diseño del agente»._

Cómo nace Clio: lo que hay que hacer, en qué orden y qué no puede hacer hasta terminar.

### Orden

1. **Instalar el cerebro** en el VPS con la infraestructura como código; todo cerrado: sin puertos públicos, solo Tailscale.
2. **Registrar la passkey** desde la PWA en el teléfono. Sin passkey no hay nada más.
3. **Desbloquear el vault** por primera vez y cargar solo los secretos del primer uso (el proveedor del modelo).
4. **Crear el bloque de identidad** por conversación: Clio pregunta, tú contestas, y cada respuesta entra como recuerdo fijo con su presupuesto. Nada se infiere: solo lo que dices.
5. **Enrolar el primer nodo** (la laptop): generar su llave, aprobar su lista de permitidos y probar una orden firmada.
6. **Políticas iniciales**, muy estrechas y con vencimiento corto: leer en el sandbox, escribir en el sandbox. Nada hacia fuera.
7. **Correr el banco** por primera vez: esa es la línea base contra la que se mide todo lo demás.
8. **Primer trabajo programado**, si quieres uno, con tu passkey.

### Reglas

- **Nada fuera del sandbox antes de terminar:** Hasta que el día cero esté completo, la pasarela niega toda acción fuera del sandbox, sin excepción.
- **Lo que Clio no decide:** La identidad, las políticas, los nodos y los secretos los pones tú. Clio guía la conversación y propone, pero no activa nada.
- **Reproducible:** El día cero se puede repetir desde cero en una máquina limpia con el código de infraestructura y un respaldo. Es también la prueba de restauración.
- **Ámbitos desde el inicio:** En la identidad se crean al menos dos ámbitos de memoria (personal y trabajo) para que nada se mezcle después.

### Telemetría

Tiempo del día cero, pasos repetidos y la línea base del banco (éxito, costo, tiempo), que queda guardada como referencia.

### Reglas de seguridad que aplica

S-02, S-23, S-28, S-31, S-37, S-49, S-50.

### Pruebas de aceptación

1. Sin passkey registrada, ninguna otra operación del día cero está disponible.
2. Antes de completar el día cero, una acción fuera del sandbox se niega.
3. El bloque de identidad solo contiene lo que la persona escribió o confirmó.
4. El día cero completo en una máquina limpia deja a Clio idéntica a la original, salvo las llaves.
5. La línea base del banco queda guardada y aparece en clio-trace como referencia.

### Decisiones abiertas

| Decisión | Opciones |
|---|---|
| Guion de la identidad | preguntas fijas · conversación libre con un mínimo obligatorio |
| Primer nodo obligatorio | sí · se puede empezar solo con el VPS |

