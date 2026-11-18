# clio-jev

Client of Jev (TypeSafe System One): closed questions (yes/no or pick an option) and probabilities, never text. **Optional**: without a key, without a network, without budget or with the breaker open there is no answer and the caller goes on with its own rules. Design: `docs/specs/2026-11-03-jev-design.md`; rule S-62.

- `wire`: the questions and the answers once checked (nothing missing, nothing of another type, numbers in range, options that were offered).
- `client`: the call with a time limit (3 s), a circuit breaker (3 failures in a row, 60 s) and a daily token budget. The model is `jev-latest`; the version that answered comes back in every answer and `model_changed()` warns if it changed. The HTTP is done by `adapters/jev/jev_systemone.py` (Python standard library, like the model adapter): the key arrives on standard input from the vault and only goes out to `api.typesafe.ai` over TLS, without following redirects.
- `catalog`: one yes/no question per tool, generated from the catalog that the guard (`adapters/pi/extensions/clio-guard`, `USE_WHEN`) reports when the session starts. Adding a tool to the guard adds its question; there is no other list. The question names the state key `mensaje_de_la_persona` (the person's message), which the brain fills.

Nothing calls this crate yet: delivery 2 connects it to the «before the prompt» step.

    cargo test -p clio-jev
