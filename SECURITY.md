# Seguridad

## Cómo reportar una vulnerabilidad

No abras un issue público. Usa los avisos privados de seguridad de GitHub: pestaña **Security** del repositorio, **Report a vulnerability**. Solo los mantenedores ven el aviso.

Incluye qué regla de [`docs/security.md`](docs/security.md) (S-xx) crees que se rompe, los pasos para reproducirlo y qué daño permite. Un secreto real nunca va en el reporte: usa uno inventado.

## Qué esperar

Respondemos al aviso en privado y corregimos antes de publicar nada. Cada vulnerabilidad corregida agrega un caso a la batería de ataques (`testing/attacks`).

## Qué cubre

Clio, clio-memory, clio-network y clio-trace. El modelo de amenazas (qué se protege y de qué) está en [`docs/threat-model.md`](docs/threat-model.md) y las reglas obligatorias en [`docs/security.md`](docs/security.md).
