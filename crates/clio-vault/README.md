# clio-vault

Secret vault (S-18 to S-22, S-44). The model only sees `{{secreto:name}}` markers.

- Values are encrypted at rest (XChaCha20-Poly1305). The master key comes from a passphrase
  (Argon2id) and is never stored: a fresh or restarted vault is locked until `unlock`.
- `resolve` needs a `GatewayCap`, takes the tool and the destination domain, returns the value
  in memory only, and fails and raises an alert if the destination is not allowed.
- `put`, `rotate`, `revoke`, `list`, `audit` and `backup` need a `PersonCap`, which `init` and
  `unlock` hand out. `list` never returns values.
- `issue_token`: short-lived token bound to one secret, one tool and one node.
- `Scrubber` (`vault.scrubber()`): the single output filter for the gateway, LLM and Jev
  payloads, clio-trace and clio-memory. Removes known values (plain, base64 at any alignment,
  URL-encoded) and provider key patterns, and logs every redaction.
- Backups are sealed with a random `BackupKey`, different from the master key.
- Time comes from `clio_core::Clock`.

Not built yet: the separate process with its own user and the 0600 Unix socket (tasks 233, 234),
which is what makes `GatewayCap` a real boundary instead of a convention.

Spec: [`docs/specs/clio-vault-spec-v0.1.md`](../../docs/specs/clio-vault-spec-v0.1.md).
