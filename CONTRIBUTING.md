# Contribuir

Antes de abrir un PR: `just setup` (activa el hook que rechaza secretos), `just check` y `just test`.

## Cada PR nombra las reglas de seguridad que toca

En la descripción, la sección **Reglas** lista los S-xx de [`docs/security.md`](docs/security.md) que el cambio toca y las pruebas que los cubren. Si no toca ninguna, escribe «ninguna». Una regla sin prueba no está cumplida, y el CI lo exige para todas (S-04): cada regla de `docs/security.md` necesita una prueba que corra (un comentario `// S-xx` justo encima de un `#[test]`) o una línea en `scripts/rule-coverage.exempt` que diga, con motivo, dónde más se comprueba (un trabajo de CI, un script del equipo, una parte que aún no existe). Una regla nueva sin ninguna de las dos hace fallar el CI; una exención que ya tiene prueba también, para que no quede obsoleta.

## Estas rutas las revisa una persona, no un agente

Pasarela (`crates/clio-gateway`), vault (`crates/clio-vault`), firmas de nodos (`crates/clio-node`) y la PWA (`apps/pwa`). Lo declara `.github/CODEOWNERS`, y ningún agente aprueba ni fusiona cambios sobre ellas (S-53). Tampoco los Cargo.toml, `deny.toml` ni `.github/`, porque ahí vive `unsafe_code = "forbid"` (S-41).

## Dependencias y secretos

Ninguna dependencia nueva sin pasar `cargo-deny`. Nada que parezca un secreto entra al repositorio, ni siquiera como ejemplo: usa valores inventados que el detector no reconozca.

## Cada incidente agrega un caso a la batería

Si algo salió mal en producción o en un reporte, el mismo PR que lo corrige agrega el caso en `testing/attacks/cases/` y una línea en la «Batería de ataques» de `docs/security.md`. Una prueba falla si las dos listas no coinciden.

> La rama `main` está protegida: todo cambio entra por PR, con los 10 checks del CI en verde y una revisión de los dueños del código (`.github/CODEOWNERS`). Se trabaja siempre en una rama y un PR. La protección no se aplica a administradores, así que el dueño puede fusionar con `gh pr merge --admin` una vez el CI está en verde; subir directo a `main` salta la protección y no se hace.
