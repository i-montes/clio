# clio-jev

Cliente de Jev (TypeSafe System One): preguntas cerradas (sí/no o elegir una opción) y probabilidades, nunca texto. **Opcional**: sin clave, sin red, sin presupuesto o con el disyuntor abierto no hay respuesta y quien llama sigue con sus reglas. Diseño: `docs/specs/2026-11-03-jev-design.md`; regla S-62.

- `wire`: las preguntas y las respuestas ya comprobadas (nada que falte, nada de otro tipo, números en rango, opciones que se ofrecieron).
- `client`: la llamada con tiempo máximo (3 s), disyuntor (3 fallos seguidos, 60 s) y presupuesto diario de tokens. El modelo es `jev-latest`; la versión que contestó vuelve en cada respuesta y `model_changed()` avisa si cambió. El HTTP lo hace `adapters/jev/jev_systemone.py` (biblioteca estándar de Python, como el adaptador de modelos): la clave llega por la entrada estándar desde el vault y solo sale hacia `api.typesafe.ai` por TLS, sin seguir redirecciones.
- `catalog`: una pregunta sí/no por herramienta, generada desde el catálogo que la guardia (`adapters/pi/extensions/clio-guard`, `USE_WHEN`) informa al empezar la sesión. Agregar una herramienta a la guardia agrega su pregunta; no hay otra lista.

Todavía nada llama a este crate: la entrega 2 lo conecta al «antes del prompt».

    cargo test -p clio-jev
