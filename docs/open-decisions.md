# Decisions pending for the person

Written during phase 1, with the person away, and kept up to date. Here is what was decided by default (and can be changed) and what only the person can decide, with what happens if nothing is touched.

## To decide (they block work)

### 1. Encrypted backups and Tailnet Lock on the host · tasks 093, 094, 095
The host is one machine (ADR 0006): the person's PC at first, the owner's VPS now. `infra/local/check-host.sh` checks the essentials: disk encryption, firewall, SSH off, nothing of Clio outside loopback, a Tailscale key with expiry.
- **Tool: restic (decided on 2026-11-02),** installed, with scripts in `infra/local/` (`docs/respaldos.md`). A default place per system (Linux and macOS, outside the data folder) and only the last 2 copies (decided on 2026-11-02). That place is on the same disk: **a destination off the host is still missing** (an external disk or encrypted cloud storage), as are creating the repository and testing a restore (093, 094). Until then, the memory and the vault live only on that disk.
- **Disk encryption on a VPS.** ADR 0011 (memory not encrypted by the engine) holds only where the host's disk is encrypted; on a host without it, that decision is reopened.
- **Tailnet Lock** is not on; it is optional and yours to decide (095).

### 2. Pi as the harness (decided by you on 2026-10-31, ADR 0007)
Pi 1.4.0 (published 2026-10-21, active) is the harness: loop, providers, sessions and compaction; Clio is the service, the guard and the interface around it. Smaller things were taken by default: the daily model (today `beam-free`, free; your Aria uses `mimo-v2.6-flash`, which does use your Go plan's quota). Pi already runs inside `bwrap`, with its own network and the provider as its only destination (ADR 0009, done on 2026-11-03).

### 3. Spending caps in each provider's dashboard · task 281 (S-45)
Set in the provider's web dashboard, not in code. Only the free model is used today, so the risk is nil; do it before using a paid model.

### 4. Rotating the OpenCode key
It was pasted in the chat of a phase 1 session. **Decided on 2026-11-02: not rotated for now.** If it is rotated later: change the key in the vault with `clio connect opencode --new-key`, then `gh secret set OPENCODE_API_KEY` (paste the new key when asked).

## Decided by default (change them if they do not suit you)

| Task | Decision | Alternative |
|---|---|---|
| 136 | Contract format: JSON | markdown with frontmatter |
| 137 | Where the contract record lives: the file it is given (today in the working workspace) | episodic memory (phase 2), or both |
| 161 | The anti-loop counts only repetitions | also count steps "without progress" |
| 221 | Gateway rules format: Clio's own JSON | unify with the policies (phase 5) |
| 270 | Vault: own implementation in Rust (XChaCha20-Poly1305 and Argon2id) | OpenBao or Infisical |
| 303 | Memory not encrypted by the engine: the host's encrypted disk and restic for the copies protect it ([ADR 0011](decisions/0011-memory-encryption-at-rest.md)); valid only where the disk is encrypted | SQLCipher, with the key in the vault |
| 304, 310, 314–316 | The memory daemon (and its remote API) moves to phase 4: Clio uses the library and the other agents MCP over stdio | do it in phase 2 |
| 271 | Unlocking the vault: a passphrase at the terminal | passkey (phase 4), or a key on another device |
| 205 | Approvals at the terminal (and in the chat) | passkey with the PWA (phase 4) |
| — | `/rápido` forces level 1 and wins over "possibly irreversible" | that the irreversible always goes up to 2 |
| — | Triage "short question" = 12 words or fewer; short verb lists, not tuned with real cases | tune with the bench |
| — | The gateway never allows the irreversible by policy: it always asks for approval | allow it with a policy that covers everything (phase 5) |
| — | Protected paths for the agent in copy mode (not writable): `extensions`, `skills`, `.pi`, `.claude`, `.git`, `.agents`, `.vscode`, `.husky`, `.githooks`, `.cargo`, `.devcontainer` (a task that writes in `.git` is refused) | widen it, or drop `.git` |
| — | real-02: the README now says "round half up" (ambiguous before; the agent failed 6 of 6 because of it) | leave it ambiguous as a specification test |
| — | Free model for tests: `opencode-go/beam-free` (not `iceman-contributor-free`, which lets them train on your prompts) | a paid model |

## Things left to do that are not decisions
See the master list (`docs/lista-maestra.md`): phase 1 tasks still open, above all measurements with the whole agent, the separate process and the vault socket (233, 234), and pre-production isolation (055).

- **The vault's Argon2id (finding 11 of the audit):** done. The cost is kept in the vault's file and new vaults use 64 MiB and 3 passes. An older vault keeps the old cost until you run `clio vault upgrade-kdf` (with the vault running; it asks for your passphrase and leaves a `vault.pre-argon2` copy you can delete once you check it opens). `clio doctor` warns while it keeps the old cost.
- **The GitHub token's `workflow` scope:** added to update branches with changes in `.github/workflows`. If you no longer need it: `gh auth refresh -h github.com -r workflow`.

## To test on a Mac
The TUI's interactive flow (starting a job and cancelling it from the screen, the `pty_*` tests) hangs in the macOS CI and could not be debugged without a Mac, so those two tests run only on Linux. On macOS, opening, quitting, restoring after a panic and help through a pipe do pass. The key-sequence test (`Ctrl-D`, `c`, `U`, `?`, `Ctrl-L`, `q`) does not run on macOS either: there `q` does not quit in time, and someone with a Mac has to see whether it is a real TUI bug or the test driver's. Your host is Linux, so it does not affect you; it only matters if you ever use Clio on a Mac.

- **CI with the real Pi:** the tests with Pi (`pi_real`, attacks 38–49) are skipped in CI because the runner does not bring Node ≥ 22.19. Requiring them needs `actions/setup-node` pinned by SHA (check the SHA before pinning it) and `CLIO_REQUIRE_PI=1`. Locally they all pass.
- **Budget on the Pi path (S-47):** each prompt is a task with the caps in `config.json` (`limits`: steps = tool calls, seconds, cost) and the builtin loop's anti-loop (the same call more than 4 times); the cut aborts Pi and, if it does not stop within 5 s, kills it. Two gaps are left: (1) the abort is not instant, so a call Pi had already started still runs (a counter inside the tool host would close it); (2) in copy mode the per-day cap exists but has no value in the configuration and is not kept between runs, as in `builtin`. Host mode has its own day budget across sessions (`limits.day_cost_micro_usd`, `limits.day_steps`; ADR 0012). End-to-end test with a real Pi and model: pending.
- **The guard's circuit breaker:** it opens after 10 denials in a row and does not close by itself; today the only remedy is to start another session. Do you want a dialog to resume it?
