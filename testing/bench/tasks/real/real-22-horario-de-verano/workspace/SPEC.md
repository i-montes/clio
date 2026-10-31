# Planificador diario

`next_run(after, hour, minute, tz_name)` devuelve la siguiente ejecución de una tarea que corre una vez al día a las `hour:minute` de la hora de pared de la zona `tz_name` (un nombre IANA como `America/New_York`).

- `after` es un `datetime` con zona horaria (en cualquier zona; se interpreta como el instante que es). Si no tiene zona, `ValueError`.
- El resultado es el primer instante ESTRICTAMENTE posterior a `after` en que la hora de pared de `tz_name` marca `hour:minute`. Se devuelve como `datetime` con `tzinfo=datetime.timezone.utc`, con segundos y microsegundos a cero.
- Hay como mucho una ejecución por día local (fecha del calendario de `tz_name`).
- Hora que no existe ese día (salto de primavera: por ejemplo 02:30 el 2024-03-10 en `America/New_York`): la ejecución ocurre en el instante que resulta de aplicar el desfase UTC vigente ANTES del salto (02:30 EST = 07:30 UTC, que en la hora local nueva es 03:30).
- Hora que ocurre dos veces ese día (retroceso de otoño: 01:30 el 2024-11-03 en `America/New_York`): se usa la PRIMERA vez (la anterior en el tiempo), solo una vez ese día.
- `hour` debe ser un entero de 0 a 23 y `minute` de 0 a 59; si no, `ValueError`. Una zona desconocida o inválida también lanza `ValueError`.

`runs(after, hour, minute, tz_name, count)` devuelve la lista de las siguientes `count` ejecuciones: la primera es `next_run(after, ...)` y cada una de las siguientes es `next_run` de la anterior.
