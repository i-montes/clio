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

## The process (`clio-vault` binary, tasks 233, 234, 260, 261, 266, 272, 257)

`clio-vault serve --vault FILE --gateway-sock P --person-sock Q` listens ONLY on two Unix
sockets (mode 0600, never the network), JSON lines (`proto.rs`). It starts locked. Who is calling
is decided by the socket, not by the message:

- gateway socket: `resolve`, `check`, `issue_token`, `scrub`, `status`.
- person socket: `unlock`, `lock`, `put`, `rotate`, `revoke`, `list`, `audit`, `alerts`, `due`,
  `scan_dir`, `status`. A `resolve` here is refused.

The CLI talks to the person socket (`--sock` or `$CLIO_VAULT_PERSON_SOCK`); passphrases and values
are read as one line from stdin:

```text
clio-vault init --vault FILE
clio-vault serve --vault FILE --gateway-sock G --person-sock P
clio-vault --sock P unlock [--eager NAME]...
clio-vault --sock P put NAME --dest TOOL:DOMAIN [--kind api_key] [--rotate-days N]
clio-vault --sock P revoke NAME        # works while locked, effective at once
clio-vault --sock P list | audit | alerts | due | lock | status
clio-vault --sock P scan DIR           # vault values found in files; exit 1 on any hit
```

`VaultClient` (library) is the gateway's side: `resolve`, `check`, `issue_token`, `scrub`. With no
server it errors, so nothing resolves (fail closed). The gateway integration (`SecretPolicy`,
resolving at the last step, scrubbing outputs) is in `clio-brain::vault_link`, so the vault does
not depend on the gateway.

Rotation (260): `Vault::due_for_rotation(now_ms)` and `Vault::rotate_due(cap, new_value)` use the
injected clock; the scheduler that calls them is another phase.

Lazy loading (272): `Vault::unlock_lazy(pass, eager)` decrypts only the `eager` secrets. The filter
must know every value to cross it out, so for the rest it holds keyed digests (SHA-256 of each
form's text, re-keyed with a random per-process key) and slides a window of each known length over
the output; the digests are made at `put` time and stored sealed, so no value is decrypted at
unlock. A secret's first `resolve` promotes it to a full needle. Limits: one hash per byte offset
and distinct length (slow on very large outputs); a vault file written before this change is
decrypted once to make the digests; the master key is in memory after any unlock, so this reduces
what plaintext sits in the filter, it does not stop a memory dump.

Real limits:
- The wall between processes is the file system: the sockets are 0600, so only the vault's user
  can open either of them. A gateway under another user needs the gateway socket's owner/group
  arranged at deploy time (not done here), and until the vault runs under its own user this is a
  convention, not a wall. There is no peer-credential check (needs `unsafe`/libc).
- Between bind and chmod the socket follows the umask: keep its parent directory 0700.
- No limit on wrong `unlock` attempts (Argon2 is the only brake). The terminal prompt echoes.

Spec: [`docs/specs/clio-vault-spec-v0.1.md`](../../docs/specs/clio-vault-spec-v0.1.md).
