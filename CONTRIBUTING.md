# Contribuir

Antes de abrir un PR: `just setup` (activa el hook que rechaza secretos), `just check` y `just test`.

## Cada PR nombra las reglas de seguridad que toca

En la descripción, la sección **Reglas** lista los S-xx de [`docs/security.md`](docs/security.md) que el cambio toca y las pruebas que los cubren. Si no toca ninguna, escribe «ninguna». Una regla sin prueba no está cumplida; las que ya la tienen están en `scripts/rule-coverage.enforced` y el CI falla si la pierden (S-04).

## Estas rutas las revisa una persona, no un agente

Pasarela (`crates/clio-gateway`), vault (`crates/clio-vault`), firmas de nodos (`crates/clio-node`) y la PWA (`apps/pwa`). Lo declara `.github/CODEOWNERS`, y ningún agente aprueba ni fusiona cambios sobre ellas (S-53). Tampoco los Cargo.toml, `deny.toml` ni `.github/`, porque ahí vive `unsafe_code = "forbid"` (S-41).

## Dependencias y secretos

Ninguna dependencia nueva sin pasar `cargo-deny`. Nada que parezca un secreto entra al repositorio, ni siquiera como ejemplo: usa valores inventados que el detector no reconozca.

## Cada incidente agrega un caso a la batería

Si algo salió mal en producción o en un reporte, el mismo PR que lo corrige agrega el caso en `testing/attacks/cases/` y una línea en la «Batería de ataques» de `docs/security.md`. Una prueba falla si las dos listas no coinciden.

> `CODEOWNERS` solo obliga a revisar cuando la rama `main` tiene protección con «Require review from Code Owners». Hasta que se active, la regla se cumple por disciplina y por los checks del CI.
