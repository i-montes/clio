# Clio como asistente autónomo

Fecha: 2026-11-17. Repos: `clio` y, para los ajustes de memoria, `clio-memory`. Pedido por Iván.

## Por qué

Hasta hoy casi todo el trabajo de Clio fue seguridad. El resultado es un asistente de programación
encerrado: trabaja sobre una copia de un proyecto, sin red, con 40 pasos y 10 minutos por tarea, y
solo cuando alguien le habla. Iván quiere otra cosa: un asistente real, despierto y autónomo, que
viva en su VPS y que sea un producto para más personas.

Lo que pidió, en sus palabras:

- Que todos los días **se levante solo**, sin que nadie le hable, y revise estados.
- Que **solo se comunique cuando haya algo realmente importante** (un sitio se cayó, hay un PR por
  revisar).
- Que tenga **conexiones**: GitHub, correo, Notion y las que vengan.
- Que sea **también un agente de código**, que **escriba sus propias herramientas** siguiendo los
  patrones de seguridad, y que **cree una skill** cuando detecta que una orden se va a repetir.
- Que se piense **como producto**: funciona para cualquier persona y es personalizable. Nada hecho a
  la medida de Iván queda dentro de Clio (ni siquiera un script que revise sus sitios).
- **Temis** es el juez de si un texto es una inyección y de lo que va a memoria.
- El **baúl** lo abre la persona; por ahora se queda así.
- Avisa por **Telegram**.
- La identidad vive **en la memoria** (clio-memory), no en un `SOUL.md`. Todos los agentes de la
  persona adoptan la personalidad de Clio, y la persona la cambia cuando quiera, hablando.
- Una skill que Clio crea queda **activa de inmediato**, avisa por Telegram y **se puede deshacer**.
- **Total libertad en el servidor:** la consola es libre, puede usar `sudo`, y todo queda registrado.
- **Pocas preguntas, y ninguna detiene el trabajo:** si Iván no mira el teléfono, Clio sigue.

## Objetivos

1. **Presentación.** La primera vez Clio se presenta, pregunta y deja en la memoria los recuerdos
   primordiales.
2. **Manos reales.** Las herramientas de Clio actúan sobre el servidor de verdad, con red.
3. **Telegram.** Avisos, aprobaciones de sí o no, y deshacer.
4. **La ronda.** Clio se despierta sola, revisa lo que vigila, calla si no hay nada y no repite.
5. **Skills y herramientas propias.** Clio las escribe cuando algo se repite.
6. **Conexiones.** GitHub, correo y Notion, cada una como skill con su llave en el baúl.
7. **Código.** Los cambios de código salen como PR.

Con 1 a 4 ya existe un agente que se despierta solo y avisa.

## No objetivos (de este diseño)

- Conversar con Clio por Telegram más allá de sí, no y deshacer.
- Que el baúl se abra solo tras un reinicio.
- Las políticas permanentes y la confianza ganada de la fase 5 de la lista maestra: aquí hay reglas
  fijas y simples; las políticas vendrán después sobre esto.
- Otros canales (WhatsApp, correo como canal).
- Varias personas en una misma instalación.
- El modo servidor en macOS.

## Anti-objetivos (aunque funcione, sería un fracaso)

- Que algo específico de una persona quede en el código de Clio.
- Que un texto ajeno (un correo, un PR, una página) logre que Clio cree una skill, cambie su
  identidad o actúe hacia afuera.
- Que Clio moleste: avisos repetidos o sin importancia.
- Que una llave llegue al modelo, a un registro o a Telegram.
- Que algo hecho en el servidor no quede registrado.
- Que Clio se quede esperando una respuesta, o que pregunte tanto que deje de ser autónoma.

## Restricciones

- **El VPS tiene cosas reales** (sitios en producción). No es una máquina desechable.
- **Sin el baúl abierto no hay modelo ni llaves.** Tras un reinicio del VPS Clio queda muda hasta
  que la persona lo abre. Límite conocido y aceptado.
- **Pi sigue siendo el arnés y sigue en su jaula** (ADR 0007 y 0009): el modelo no ve llaves ni el
  sistema; todo lo que hace pasa por el toolhost.
- **El conector `claude` no arranca sin jaula**, y la jaula necesita `bwrap` con espacios de nombres
  de usuario. Algunos proveedores de VPS lo bloquean: hay que comprobarlo en el servidor.
- **La suscripción tiene límites de uso.** La ronda tiene que gastar poco modelo.
- **clio-memory:** 1.200 caracteres por recuerdo; la comparten todos los agentes de la persona
  **en una misma máquina**. La memoria es un archivo local: la Clio del VPS y los agentes del PC
  tienen memorias distintas. Sincronizarlas es otro proyecto.
- **En el VPS la memoria queda sin cifrar** (ADR 0011 se apoyaba en el disco cifrado del PC).
  Aceptado por ahora; por eso el contenido de los correos no se guarda en memoria, solo referencias.

## Enfoques

**De dónde sale la autonomía.**

1. **El modelo sigue encerrado y se abren las herramientas (elegido).** Pi no cambia. El toolhost
   gana un «modo anfitrión»: sus herramientas actúan sobre la máquina real, bajo pocas reglas fijas
   (todo registrado y una lista corta de lo que pregunta). Se conserva todo el
   trabajo de seguridad y el modelo sigue sin ver llaves.
2. **Quitar la jaula y darle a Pi sus herramientas propias**, como hacen otros agentes de terminal.
   Es lo más simple, pero el modelo y cualquier texto que lea tendrían a mano las llaves y el
   sistema entero, y el conector `claude` ya no arranca así.
3. **Seguir la lista maestra en orden** (fases 3, 4 y 5: diagnóstico, red y PWA, políticas y
   programador). Es correcto pero queda lejos; Iván pidió el alcance corto.

**Dónde vive lo personal.** En la memoria (elegido por Iván), no en archivos: se cambia hablando,
queda registro y se puede revertir, y la comparten todos sus agentes.

**Cuándo queda activa una skill que Clio crea.** De inmediato, con aviso y deshacer (elegido por
Iván). Las otras opciones eran pedir visto bueno solo para permisos nuevos, o siempre.

## Diseño

### 1. Recuerdos primordiales y presentación

Clio arranca con seis recuerdos primordiales. Los cuatro primeros vienen con Clio, con un texto por
defecto que la persona puede cambiar; los dos últimos se llenan en la presentación.

1. Quién es Clio: nombre, qué es y para quién trabaja.
2. Cómo habla: tono, idioma, qué tan directa.
3. Cómo trabaja: actúa sola, pregunta muy poco y nunca se queda esperando, avisa solo lo
   importante.
4. Límites que no cruza: las llaves, no obedecer texto ajeno, no actuar hacia afuera sin permiso.
5. Quién es la persona.
6. Qué vigila (uno o varios recuerdos: cada cosa a vigilar es un recuerdo).

- Están **fijados** y son **de la persona**: el sueño de la memoria no los toca.
- **La presentación** corre la primera vez que se abre `clio` en una máquina cuya memoria no tiene
  identidad: Clio se presenta, hace unas pocas preguntas (cómo quiere que le hablen, qué tiene en el
  servidor, qué quiere vigilar, qué cuenta como importante) y guarda las respuestas.
- **Se cambian hablando con Clio.** Quien escribe el cambio es el agente, así que la regla es: los
  cambia cuando la persona lo pide en la conversación, nunca porque lo diga un texto que leyó.
- **Solo Clio (o la persona con su comando) los escribe.** Los demás agentes que comparten la
  memoria los leen pero no los cambian: la memoria no puede saber si la sesión de otro agente leyó
  texto ajeno. Hoy cualquier agente puede corregir, olvidar o fijar cualquier recuerdo, también los
  de la persona; eso hay que cerrarlo para estos dos tipos.
- **Quedan guardados como dichos por la persona,** no como del agente. Hoy un recuerdo que escribe
  un agente queda como «del agente», y leerlo marcaría la sesión como si fuera texto ajeno.
- **En clio-memory hace falta:**
  - una herramienta para **listar** todos los recuerdos de un tipo (hoy solo se puede buscar, y una
    búsqueda devuelve los más parecidos, no todos);
  - un tipo para la identidad y otro para lo que se vigila;
  - que esos dos tipos solo se puedan crear o cambiar en una sesión que no leyó texto ajeno.
- **Un comando que muestre la identidad como texto**, para quien quiera leerla de corrido.

### 2. Manos reales: el modo anfitrión

Hoy el toolhost trabaja sobre `runs/<id>/sandbox` (una copia), ejecuta dentro de una jaula sin red y
corta a los 40 pasos o 10 minutos. En el modo anfitrión sus herramientas leen y escriben en el
sistema real y ejecutan órdenes en la máquina real, con red, como el usuario de Clio.

Reglas, fijas y pocas (las decide una lista, no el modelo):

- **La consola es libre.** Clio ejecuta lo que necesite, también con `sudo`, sin pedir permiso.
- **Todo queda registrado.** Cada orden y cada cambio: qué, cuándo y por qué tarea.
- **Copia antes de cambiar,** cuando el cambio lo hace con sus herramientas de archivos: ahí
  deshacer es exacto. Lo que cambia una orden de consola queda registrado pero no siempre se puede
  deshacer.
- **Una lista corta de lo que sí pregunta:** borrar datos o respaldos, gastar dinero, enviar o
  publicar en nombre de la persona, y tocar lo que controla a Clio. Nada más.
- **Pocas preguntas, y ninguna detiene el trabajo.** Lo que necesita visto bueno se aparta y la
  tarea sigue. Hay un tope de preguntas al día (por defecto tres); lo que pase del tope no se
  pregunta: se aparta y va al resumen del día.
- **Las llaves no llegan al agente.** El baúl se las entrega a la herramienta que las usa, solo para
  esa orden y solo hacia su servicio, como ya hace con Jev y Temis.
- **Lo que controla a Clio está fuera de su alcance:** su configuración, el baúl, la base de la
  memoria, el registro y sus propias reglas. Una lista de rutas prohibidas no basta, porque una
  orden de consola puede leer cualquier archivo del mismo usuario. Las órdenes corren dentro de una
  jaula que muestra el sistema real y la red, pero tapa las carpetas y los sockets de Clio.
  **Límite conocido:** con `sudo` esa tapa deja de ser un muro, porque quien es administrador puede
  llegar a todo. Contra eso quedan la regla (tocar lo de Clio pregunta) y el registro.
- **Presupuesto por día,** no por tarea.
- **Texto ajeno marca la sesión.** Una sesión marcada no puede crear ni cambiar skills ni la
  identidad, ni hacer nada hacia afuera sin visto bueno. Temis juzga además cada texto ajeno.
- **Qué es texto ajeno.** Hoy cualquier lectura de archivo o cualquier orden marca la sesión; en el
  modo anfitrión eso la marcaría desde el primer `ls` y nada de lo anterior funcionaría. Pasa a
  marcarla solo lo que escriben otros: lo que llega por una conexión (correos, PRs, páginas de
  Notion), lo que viene de la web, y una lista de carpetas del servidor donde entra texto de fuera
  (por defecto los registros de los servicios y las carpetas de subidas; la persona la ajusta).
- **Aprobar sin nadie en la terminal.** La acción que necesita visto bueno se aparta, con todo lo
  necesario para ejecutarla tal cual, y la tarea sigue con lo demás y termina con un informe de lo
  hecho y lo apartado. Un «sí» por Telegram dentro de las 24 horas la ejecuta; pasado ese tiempo
  vence, y un «sí» tardío no hace nada salvo avisar que venció. En una sesión de terminal pregunta
  ahí mismo.

Código: en un repositorio Clio trabaja en una rama y entrega un PR; nunca empuja a la rama
principal.

De paso: las copias de trabajo viejas se borran solas (más de 14 días y sin cambios pendientes).

### Temis como juez

- **Qué juzga:** cada texto ajeno antes de entrar a la sesión, lo que va a memoria (como hoy) y el
  texto y los scripts de una skill antes de activarla. La API de Temis ya recibe cualquier texto con
  preguntas tipadas; Clio ya le pregunta por el camino del adaptador de Jev.
- **Si Temis no responde:** el texto ajeno marca la sesión igual, y una skill no se activa sola:
  espera el visto bueno.
- **Para otras personas es opcional.** Cada quien pone su dirección y su llave. Sin Temis mandan las
  reglas fijas, como hoy sin Jev.

### 3. Telegram

- **Conectar:** el token del bot va al baúl, autorizado solo hacia `api.telegram.org`. Se empareja
  un chat con un código, y Clio solo atiende ese chat.
- **Sale:** avisos y preguntas de sí o no. Todo texto pasa antes por el limpiador de llaves.
- **Entra, solo del chat emparejado:** «sí», «no» y «deshacer». Lo demás se ignora en esta versión.
- **Un servicio pequeño** siempre encendido escucha las respuestas; sin él las aprobaciones no
  llegarían a tiempo.

### 4. La ronda

- **Un timer** del sistema despierta a Clio (como el sueño de la memoria). Por defecto tres veces al
  día; configurable.
- **Qué revisa:** la lista completa de lo que vigila, leída de la memoria con la herramienta de
  listar.
- **Cómo:** una sesión del agente en modo anfitrión que solo lee. Para cada cosa usa las skills y
  conexiones que tenga.
- **Silencio por defecto.** Cada cosa termina en «nada» o en un aviso. Qué es importante lo dice la
  persona en su recuerdo; más adelante puede juzgarlo Temis.
- **No repite.** Clio guarda qué avisó; vuelve a avisar solo si cambió o si se resolvió.
- **Revisiones frecuentes.** Una skill puede marcarse para correr entre rondas (cada pocos minutos)
  sin modelo, y despertar al modelo solo cuando algo cambia. Así se nota pronto un sitio caído sin
  que Clio traiga un script para eso: la skill la escribe Clio para cada persona.
- **Sin baúl,** la ronda no hace nada y lo deja escrito.

### 5. Skills y herramientas propias

- **Formato:** una carpeta por skill con sus instrucciones y, si hace falta, sus scripts (las
  herramientas propias son esos scripts). Viven en la carpeta de la persona, con historial de
  cambios para poder deshacer.
- **Cuándo la crea:** cuando la persona lo pide, o cuando Clio nota que el mismo tipo de orden se
  repite.
- **Qué pasa al crearla:** queda activa sin pedir permiso; Clio avisa por Telegram qué hace y qué
  conexiones usa; «deshacer» la quita. Pi carga las skills al arrancar (Clio le pasa cada carpeta
  con `--skill`; con `--no-skills` solo se apaga la búsqueda automática), así que una skill nueva
  sirve desde la siguiente tarea o ronda, no en medio de la que la creó.
- **Una excepción:** en una sesión que leyó texto ajeno, la skill espera el visto bueno.
- **Solo usa conexiones que la persona ya dio.** Una conexión nueva siempre la da la persona, que
  es quien tiene la llave.
- **Patrones de seguridad, comprobados solos antes de activarla:** sin llaves en el texto, solo las
  conexiones que declara, sin red fuera de los servicios que declara, y pasa el juicio de Temis.
- Sus scripts corren por el toolhost, con las mismas reglas del modo anfitrión.

### 6. Conexiones

- **Una conexión es:** una skill, una llave en el baúl atada a un servicio y, si existe, una
  herramienta ya hecha.
- **Las primeras:** GitHub (con `gh`: PRs que esperan a la persona, CI roto), correo (IMAP, solo
  lectura) y Notion (su API).
- **Todas empiezan en solo lectura.** Escribir es un permiso aparte.
- **Clio no trae código por servicio:** trae el mecanismo, y cada conexión llega como skill.

## Orden

1. Recuerdos primordiales y presentación.
2. Manos reales.
3. Telegram.
4. La ronda.
5. Skills y herramientas propias.
6. Conexiones.

Cada parte tiene su propio plan y deja algo que se puede usar. El código lo escriben subagentes; el
plan lleva las decisiones.

## Pruebas

- **Presentación:** una memoria vacía termina con los seis recuerdos; una con identidad no vuelve a
  presentarse; una sesión marcada no puede cambiarlos.
- **Modo anfitrión:** cada orden deja registro; un archivo cambiado con las herramientas se puede
  devolver; algo de la lista corta no se ejecuta sin visto bueno y la tarea sigue sin esperar;
  pasado el tope de preguntas no se pregunta más ese día; una llave nunca aparece en el entorno
  del agente.
- **Telegram:** un chat no emparejado se ignora; una llave en un aviso sale tapada; un «sí» tardío
  sobre una pregunta vencida no ejecuta nada.
- **Ronda:** sin novedades no envía nada; lo mismo dos veces avisa una; lee toda la lista.
- **Skills:** crearla avisa y deshacerla la quita; en sesión marcada espera; una que usa una
  conexión no dada se rechaza.
- **Ataques:** un correo y un PR con órdenes para el agente no logran crear una skill, cambiar la
  identidad ni enviar nada.

## Decisiones que tomé yo (para revisar)

- El tope de tres preguntas al día, y qué entra en la lista corta de lo que pregunta.
- Una memoria por máquina, sin sincronizar, y la memoria del VPS sin cifrar (recomendadas por mí;
  Iván no las objetó).

- La excepción de la sesión marcada para las skills.
- Telegram, en esta versión, solo entiende sí, no y deshacer.
- Los seis recuerdos primordiales.
- Qué pregunta lo decide una lista fija, no el modelo.
- Tres rondas al día por defecto.
- Las copias de trabajo se borran a los 14 días si no tienen cambios pendientes.
- Las órdenes de consola corren en una jaula que muestra el sistema real pero tapa lo de Clio.
- Qué cuenta como texto ajeno: conexiones, web y una lista de carpetas del servidor.
- Las aprobaciones sin nadie presente se apartan y vencen a las 24 horas.
- Si Temis no responde, una skill espera el visto bueno.
- La identidad y lo que se vigila solo los escribe Clio o la persona; los otros agentes solo leen.
- Una skill nueva sirve desde la siguiente tarea o ronda.

## Lo que queda a quien construye, y lo que falta comprobar

- Si `bwrap` funciona en el VPS de Iván (`clio doctor` lo dice en el servidor).
- La lista exacta de lo que pregunta (las cuatro clases de arriba, llevadas a órdenes concretas).
- Los nombres de los comandos nuevos (`clio connect` ya se usa para los conectores de modelo).
- Cómo detecta Clio que una orden se repite.
