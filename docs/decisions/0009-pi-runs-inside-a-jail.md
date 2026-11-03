# 0009: Pi runs inside a `bwrap` jail, so it cannot see the vault or the person's files

Status: accepted (2026-11-02). Phases 3a and 3b implemented (`crates/clio-brain/src/pi/jail.rs` and `net.rs`). The person decided that if the jail cannot be built or fails its self-test, Pi does not start: without it nothing can work as designed.

## Context

Pi and its roughly 120 npm dependencies run as the person, in the person's session (ADR 0007). Only the tools are confined: `bash` runs in the `bwrap` jail, files go through the toolhost. The threat model says it plainly: a flaw in Pi or in one of its dependencies "is stopped by none of this". The audit of 2026-11-02 (finding 5) made it concrete: the vault tells its gateway socket from its person socket only by which one a request came in on, and both are 0600 sockets of the same user. Anything running inside Pi can connect to both, list the secrets and ask for one with a `tool` and a `domain` it picks itself. The wall is not missing in the vault: it is that Pi can see the sockets at all (S-22, S-03, S-11).

The same holds for the person's files (`~/.ssh`, browser profile, other projects) and for the working copy itself: a compromised Pi could write `.git/hooks` or `skills/` directly and skip the toolhost's rules (S-16).

## Decision

Start Pi inside `bwrap`, with only what it needs visible. Two phases, because the second one is a different problem.

**3a, now.** The network is shared; the filesystem is not. Everything is mounted at the same absolute path it has outside, so no argument or environment variable changes.

| Visible inside | How |
|---|---|
| `/usr`, `/bin`, `/lib`, `/lib64`, the loader cache | read-only, as the tool jail |
| `/etc/resolv.conf`, `hosts`, `nsswitch.conf`, `ssl`, `ca-certificates` | read-only (DNS and TLS to the provider) |
| Node's folder and Pi's install folder (which holds the guard extension) | read-only |
| Pi's `agent/`, `sessions/` and `home/` | read-write (Clio lists the sessions) |
| the folder of the toolhost socket | the only way out to an effect |
| the working copy | **read-only**: every change goes through the toolhost and its rules |
| `/proc`, `/dev`, a private `/tmp` | as the tool jail |

Not visible: the vault folder and sockets, `XDG_RUNTIME_DIR`, the person's home, the data folder outside Pi's own. The environment is the one `hardened_env` already builds (the provider key stays in it: Pi needs it).

**Fail closed.** On Linux, Pi starts only inside the jail, and only if a self-test from inside passes (the vault's and the data folder's decoys are not visible, the working copy is read-only). If `bwrap` is missing, too old, cannot start here, or the self-test fails, Pi does not start and says why. The only way around is `CLIO_PI_JAIL=off`, which `doctor` shows as a FAIL. Where a jail cannot exist at all (macOS), Pi starts without it and `doctor` says, as a WARN, that Pi has the person's permissions.

**3b, after.** Close the network: own network namespace, and the provider as the only destination. See "Open".

## What the experiment showed (2026-11-02, bwrap 0.13.1, Pi 1.4.0, Node 22.23.2)

With the table above and `--share-net`, using the real Pi and the hardened flags of `launch.rs`:

- Pi starts and answers `get_state` in 0.3 s, with the configured model.
- A socket in the toolhost folder can be connected to; one in a folder that is not mounted fails with `ENOENT`.
- Inside, `/home/imontes` holds only the path to Node and to Pi; `~/.ssh` does not exist and there is no `/run/user`.
- DNS and TLS to the provider work (`fetch` to `https://opencode.ai/` returns 200).

And one limit it showed: **with the network shared, an abstract unix socket of the host is reachable** (a socket made on the host with a `\0` name connected from inside). So is any TCP service on the host's loopback. 3a closes the vault and the filesystem; it does not close those two. That is why 3b exists.

## What 3a left open, and what remains after 3b

3a left the open network and the host's abstract sockets and loopback services. 3b closes both. What remains, by design:

- A compromised Pi still has the provider's key (Pi needs it) and can send what it read to the provider's address, the one destination the jail allows; the provider already receives the model's whole context.
- It still has the toolhost's socket, behind the same gateway rules as the model.
- The proxy resolves the provider's name on the host: a poisoned DNS answer would send Pi to another address under the same name. The allowlist is by name, not by address.

## 3b: what the experiment showed (2026-11-02, same versions)

Pi in its own network namespace (`--unshare-net`: only a loopback with nothing listening), a proxy on the host that listens on a unix socket and allows `CONNECT` to one `host:443`, and a forwarder inside the jail from `127.0.0.1:18080` to that socket. Pi runs with `HTTPS_PROXY` pointing at the forwarder. The key was a decoy.

- **Pi honours the proxy.** Its startup installs undici's `EnvHttpProxyAgent` as the global dispatcher (`core/http-dispatcher.js`). With a prompt, the proxy saw `CONNECT opencode.ai:443`, allowed. No code of Pi changes.
- **There is no other way out.** Inside the jail a direct TCP connection fails (`ENETUNREACH`), so does the host's loopback service (`ECONNREFUSED`) and the host's abstract socket (`ECONNREFUSED`): the two holes 3a leaves are closed.
- **The allowlist works.** The same proxy answers 200 for the provider and 403 for `example.com`.
- **Pi does try other hosts when it is not offline.** Without `--offline` it asked the proxy for `pi.dev:443` six times in a few seconds. The launcher always passes `--offline` and `PI_OFFLINE=1`, and with them the proxy saw only the provider. If a version of Pi ever stops honouring it, the allowlist still holds, and the denied attempt is worth showing to the person.
- **DNS moves to the host.** The jail needs no `resolv.conf`, `hosts` or `nsswitch.conf`: the proxy resolves the name.
- **TLS is end to end.** The proxy only sees `CONNECT host:port`; it cannot read or change what Pi sends.

**How it is implemented** (`net.rs`; the experiment used Python for both pieces, the product does not rely on `python3`):

- The forwarder is a subcommand of `clio` that binds `127.0.0.1:<port>` inside the jail, starts Pi as its child and ends with it. The `clio` binary is mounted read-only.
- The proxy runs on the host, one per session, with the session's lifetime like the toolhost. It listens on a unix socket in its own 0700 folder (mounted into the jail, like the toolhost's), allows one `host:port` taken from the provider's `base_url` (exact match, no wildcards), refuses anything that is not a well-formed `CONNECT`, limits request size, connections and idle time, and records every refusal.
- The self-test grows: from inside, a direct TCP connection, the loopback of the host, an abstract socket and a `CONNECT` to a host that is not the provider's must all fail, and the provider's must pass.
- A refused connection is shown in the chat ("Pi intentó conectar a X: negado"), not only logged: the person supervises there.
- Fail closed as in 3a: if the proxy cannot start, Pi does not start.

**Still open.** Whether a provider ever needs more than one host (a CDN for a model list, a separate auth host): for now one `base_url`, and a second host is a configuration decision, not a wildcard.

## What the implementation taught (3a)

- **The key goes in `bwrap`'s environment, never in its arguments.** The arguments of a process are readable by every user of the machine (`ps`); the environment is not.
- **No `--die-with-parent`.** It is tied to the thread that started `bwrap`, and a chat session is opened from a worker thread that ends right after: Pi died after opening. Pi (and so the jail) ends by itself when its input closes, also if `clio` is killed; a test pins that.
- **The probe cannot use `: > file` to test a read-only mount.** A failed redirection on a special builtin ends a POSIX shell, so the probe exited 1 exactly when the copy was correctly read-only. It runs the write in a subshell.
- **Mount the least that works.** The guard's own folder only when it is an extension (`index.ts` and its siblings); a loose file alone; the prompt as one file. The folder around a file holds other things (a test caught the whole test rig being exposed through the guard's parent).
- **A test that skips can hide a broken jail.** The first version of the tests "passed" because the jail failed to prepare and the helper printed SKIPPED, which cargo hides. The helper now skips only where `bwrap` cannot start at all; where it can, a jail that does not prepare fails the test.
- **Not covered here:** `clio-toolhost/tests/pi_stack.rs` (attacks 38 to 49) starts Pi through its own harness, not through `start`, so it runs Pi without the jail. The jail is tested by `clio-brain/tests/pi_jail.rs`, `pi_real.rs` and `pi_chat.rs`.

## What the implementation taught (3b)

- **A blocking `accept` woken through the socket's path hangs on drop.** The proxy's `Drop` connected to its own socket to wake the accept loop; if the folder was already gone (a test's temporary folder, a runtime folder cleaned at logout), the connect failed and `join` waited forever. The accept loop now polls a stop flag and does not depend on the file system.
- **The self-test's own refusals must not reach the chat.** It asks the proxy for a destination it knows will be refused; the list is cleared once the self-test passes, so what the chat shows is what Pi asked for.
- **A shared network made the mutation test loud.** With `--share-net` put back, the self-test reports the internet, the host's loopback port and the host's abstract socket as reachable and Pi does not start; two jails at once also fought over the forwarder's port, which a private network makes impossible.
- **A fourth place built a `Launch`, outside `crates/`** (the bench). The compiler found it: `Launch.helper` (the `clio` binary) is required, not optional, so no launch path can forget the forwarder.
- **`clio` runs inside the jail** because every library it links is under `/usr`; a build that links something else would fail the self-test, not run without a network.
- **Pi's environment gains six variables** (`HTTPS_PROXY`, `HTTP_PROXY`, their lowercase forms, and an empty `NO_PROXY` and `no_proxy`); no destination is exempt from the proxy. `pi_real.rs` pins them.
- **The self-test decides whether Pi starts, so a flaky self-test is a flaky product.** The proxy wrote its answer with `write!`, which sends each piece of the format on its own, and the probe read it with one `read`: now and then it saw only `HTTP/1.1 `, concluded the proxy let a foreign destination through, and Pi refused to start ("Pi no arranca" in the chat, about one run in three of the chat suite under load). The proxy now answers in one write, the probe reads the status line to its end, and `pi_jail.rs` prepares the jail 60 times, four at a time.
- **A refused connection is a quiet note in the chat** ("la jaula negó una conexión de Pi a …"), once, with the destination as Pi asked for it.

## Tests

- `crates/clio-brain/tests/pi_jail.rs` (attack-54): a process the jail starts cannot reach the vault's sockets, reaches the toolhost's, finds the copy read-only; and Pi does not start if the jail cannot be built. (It replaces the ignored test of `clio-vault`, which failed on purpose until now.)
- The 15 tests of `clio-toolhost/tests/pi_stack.rs` (attacks 38 to 49) and `pi_chat.rs` must pass with the jail on: they launch the real Pi through `start`, so the jail goes inside `start`.
- 3b: `crates/clio-brain/tests/egress_proxy.rs` (the proxy alone: what it tunnels, what it refuses, tricks that read like the allowed address, a head without end) and a second test in `pi_jail.rs` (attack-55). From inside Pi's jail, a host abstract socket, a loopback TCP port and a host that is not the provider's are not reachable, and the provider is (through the proxy).

## Consequences

- S-22 stops depending on "review of the deployment": there is a test that fails.
- The threat model's table row for the extension changes from "corre como la persona" to the jail's view, and "trabajo futuro" becomes 3b.
- One more moving part at start (the self-test costs a `bwrap` start). The tool jail already pays it.
- `clio doctor` gets a line for Pi's jail next to the tool jail's.

## Seeing what the host struck out

The chat also says when the host struck values out of what Pi read ("Clio tachó 2 valores con forma de llave en lo que leyó Pi: el modelo no los ve"). The host counts the marks it added and puts the number in the `data` of its answer; the guard records it in the session as a `clio-guard.masked` entry (the same channel as the taint); the chat reads the entries after each turn and says what is new. `pi_chat.rs` checks the whole chain with the real chat, Pi and host.
