# Prompts de sistema

Los prompts viven en el repositorio, versionados: `system/<nombre>@<versión>.md`, con la línea `version: <n>` al principio igual a la del nombre.

- **Una versión publicada no se edita.** Un cambio es una versión nueva (`clio@2.md`); la anterior se queda. Así cada traza puede decir qué versión usó (`clio.prompt.version` en clio-trace) y el banco compara versiones.
- `MANIFEST.json` guarda el sha256 de cada archivo. `scripts/check-prompts.py` falla si un prompt cambia sin que cambie el manifiesto, o si falta una entrada: el cambio queda explícito en el diff y lo revisa la persona (`CODEOWNERS`).
- **Nada cuya filtración importe (S-09).** El prompt de sistema se supone público: sin llaves, correos, rutas de tu máquina, direcciones IP ni cadenas largas de aspecto aleatorio. El escáner lo comprueba; la revisión humana comprueba lo que un patrón no ve (nombres de personas, detalles de tu infraestructura).
- Cambiar un prompt pasa por la misma compuerta que el código: tests, banco y ataques (`ci.yml` no filtra por rutas).

Para registrar un cambio: editar o añadir el archivo, correr `python3 scripts/check-prompts.py --update` y subir el archivo y el manifiesto juntos.
