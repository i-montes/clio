# GEMINI.md

Go 1.23 service exposing a gRPC API.

## Workflow

1. Run `make generate` after you change anything under `proto/`.
2. Run `make test` and `make lint` before committing.
3. Never edit files under `gen/` by hand: they are generated.

## Conventions

- Errors are wrapped with `fmt.Errorf("...: %w", err)`.
- Context is the first parameter of every exported function.
- Log with `slog`; no `fmt.Println` in production code.
- Ask before you change a public API in `proto/`.
