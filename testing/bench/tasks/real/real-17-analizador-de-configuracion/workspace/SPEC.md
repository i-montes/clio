# Formato `confmini`

Un archivo de configuración pequeño. `parser.parse(text)` lo convierte en un `dict`; `writer.dumps(data)` hace el camino contrario (ya funciona bien: no lo cambies).

## Líneas

- El texto se separa en líneas SOLO por `\n`. Un `\r` justo antes del `\n` se ignora (`\r\n`). Cualquier otro carácter, incluidos U+2028, U+2029, U+0085, `\x0b` y `\x0c`, es un carácter normal de la línea. Los números de línea empiezan en 1.
- Las líneas en blanco y las que empiezan (tras espacios o tabuladores) por `#` se ignoran. Se ignoran espacios y tabuladores al principio y al final de la línea y alrededor de `=`, de las comas y de los corchetes de una lista.
- Un comentario `# ...` puede seguir a un valor o a una cabecera de sección, pero un `#` dentro de una cadena entre comillas es texto.

## Pares clave = valor

`clave = valor`. Una clave cumple `[A-Za-z_][A-Za-z0-9_-]*`. Una clave repetida dentro de la misma sección es un error.

Valores:

| Valor | Ejemplo | Resultado |
|---|---|---|
| cadena entre comillas dobles | `"a = b # no es comentario"` | `str` |
| entero | `0`, `42`, `-7` | `int` (sin `+`; sin ceros a la izquierda: `007` es un error) |
| booleano | `true`, `false` | `bool` |
| palabra sin comillas | `hello-world.v2` | `str` (`[A-Za-z_][A-Za-z0-9_.-]*`, y no es `true` ni `false`) |
| lista | `[1, "a, b", [2, 3], []]` | `list`, con cualquier valor dentro, anidada hasta 100 niveles; se admite una coma final (`[1, 2,]`) |

Escapes dentro de una cadena: `\"`, `\\`, `\n`, `\t`, `\r` y `\uXXXX` (exactamente 4 dígitos hexadecimales, y no un sustituto D800 a DFFF). Cualquier otro escape, una cadena sin cerrar o texto sobrante tras el valor (`a = 1 2`) es un error. Una lista sin cerrar, sin coma entre elementos (`[1 2]`) o con comas dobles (`[1,,2]`, `[,]`) es un error. Más de 100 niveles de listas es un error (no un fallo del intérprete).

## Secciones

`[nombre]` o `[a.b.c]` abre una sección: las claves siguientes van dentro de ese diccionario anidado (`[a.b]` crea `{"a": {"b": {...}}}`). Las partes siguen la sintaxis de las claves, sin espacios: dentro de los corchetes no se admiten espacios (`[ a ]` y `[a. b]` son errores), pero sí antes de la cabecera y entre ella y un comentario. Abrir `[a.b]` crea `a` si no existía; abrir después `[a]` es válido. Es un error: abrir dos veces la misma sección, abrir una sección cuyo nombre ya es una clave con valor (`a = 1` y luego `[a]`) o repetir una clave que ya existe como sección dentro de una sección.

## Errores

Todo error es `parser.ParseError`, con el atributo `.line` (la línea, desde 1, donde está el problema). Ninguna entrada puede provocar otra excepción.
