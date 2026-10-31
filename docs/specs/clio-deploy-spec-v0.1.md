## clio-deploy · spec v0.1

> **Actualización 2026-10-31 ([ADR 0006](../decisions/0006-local-first-vps-optional.md)):** la base es el equipo de la persona. Donde este spec dice «VPS», léase «el equipo anfitrión (este PC por defecto)»; el VPS es opcional.

_Despliegue y automodificación. Fuente: pestaña «Spec: despliegue y automodificación» del artefacto «Clio · diseño del agente»._

Cómo se actualiza Clio sin perder nada, y qué puede y qué no puede hacer cuando trabaja sobre su propio código.

### Despliegue

1. **Artefactos versionados y firmados:** binarios en Rust e imagen de contenedor.
2. **Lado a lado:** la versión nueva arranca junto a la vieja.
3. **Salud y humo:** comprobaciones de salud y un banco corto antes de cambiar el tráfico.
4. **Cambio y espera:** la vieja queda lista unas horas para volver con un comando.
5. **Migraciones:** copia previa de los datos, y cada migración con su vuelta atrás o su respaldo.

Los daemons de los nodos se actualizan de forma escalonada y siempre dentro de una versión de protocolo compatible. La infraestructura del VPS está escrita como código, para poder reconstruirla desde cero y no solo restaurar un respaldo.

### Clio sobre sí misma

| Puede | No puede |
|---|---|
| trabajar en su propio repositorio como en cualquier otro: rama y PR | fusionar o aprobar sus propios PRs |
| proponer cambios a prompts, skills, políticas y configuración | desplegarse a sí misma |
| explicar por qué propone un cambio, con datos de clio-trace | cambiar sus políticas, sus ascensos de confianza o sus reglas de seguridad |

Las rutas protegidas (pasarela, vault, firmas, reglas de seguridad, prompts de sistema, skills, configuración de Tailscale) exigen tu revisión explícita en cada PR: ninguna política permanente las cubre. El despliegue lo disparas tú, o la CI después de tu aprobación con passkey.

### Telemetría

Despliegues, vueltas atrás, tiempo de cambio, PRs propuestos por Clio y cuántos aceptaste.

### Reglas de seguridad que aplica

S-16, S-49 y la nueva S-53.

### Pruebas de aceptación

1. Un despliegue que no pasa salud o humo vuelve solo a la versión anterior.
2. Un push de Clio directo a la rama principal se rechaza.
3. Un PR de Clio que toca una ruta protegida no se puede fusionar sin tu aprobación.
4. Una migración se puede deshacer, o restaurar desde su copia previa.
5. El VPS se reconstruye desde cero con el código de infraestructura y un respaldo.

