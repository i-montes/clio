## clio-vault · spec v0.1

_Vault. Fuente: pestaña «Spec: vault» del artefacto «Clio · diseño del agente»._

Dónde viven las llaves y cómo se usan sin que el modelo las vea nunca.

### Modelo de un secreto

| Campo | Qué es |
|---|---|
| nombre | el marcador que ve el modelo: `{{secreto:notion}}` |
| valor | cifrado; nunca sale del proceso del vault salvo hacia quien ejecuta |
| tipo | llave de API, token OAuth, contraseña, llave SSH |
| destinos | herramientas, dominios o hosts y métodos permitidos |
| ámbito | personal, organización o proyecto |
| rotación y vencimiento | cada cuánto se rota y cómo; cuándo vence |
| estado y uso | activo, revocado; último uso |

### Operaciones

| Operación | Quién | Qué hace |
|---|---|---|
| put | la persona, con passkey | guardar o reemplazar un secreto |
| resolve | solo la pasarela, con herramienta y destino | devuelve el valor al proceso que ejecuta, en memoria |
| issue_token | la pasarela | token corto con alcance y vida limitada para un nodo o una herramienta |
| rotate · revoke | la persona o el programador con política | rota o anula |
| list | la persona | nombres y metadatos, nunca valores |
| audit | la persona | cada resolución, con destino y trace_id |

### Limpieza de salidas

Un filtro único que usan la pasarela, los envíos a LLM y Jev, clio-trace y clio-memory. Busca los valores conocidos del vault (también en base64 y codificados para URL) y patrones de llaves de proveedores, y los reemplaza por `[secreto:nombre tachado]`. Cada tachado se registra: suele indicar un intento de filtración.

### Arranque y llave maestra

Tras un reinicio, el vault arranca bloqueado y se desbloquea con tu passkey desde la PWA. Mientras está bloqueado, lo que necesita secretos espera en la cola y te llega un aviso. Es menos cómodo que dejar la llave en el disco, pero un disco robado o un respaldo filtrado no sirven solos.

### Almacenamiento

Proceso aparte con usuario propio; habla solo por un socket Unix con permisos, nunca por red. Datos cifrados en reposo; respaldos cifrados con una llave distinta.

### Telemetría

Resoluciones por secreto y destino, negaciones, tachados del filtro, secretos próximos a vencer o a rotar y tiempo bloqueado tras un reinicio.

### Reglas de seguridad que aplica

S-18 a S-22, S-44.

### Pruebas de aceptación

1. `resolve` hacia un destino no permitido falla y alerta.
2. Una herramienta que imprime un secreto (también en base64) llega al modelo con el valor tachado.
3. `list` nunca devuelve valores.
4. Un token emitido para un nodo deja de servir al vencer.
5. Tras reiniciar, ningún secreto se resuelve hasta el desbloqueo con passkey.
6. Ningún proceso distinto de la pasarela puede llamar a `resolve`.
7. Un respaldo del vault no se puede abrir sin su llave propia.

### Decisiones abiertas

| Decisión | Opciones |
|---|---|
| Implementación | propia en Rust · OpenBao · Infisical |
| Desbloqueo | passkey tras reinicio · llave en otro equipo · servicio de llaves externo |

