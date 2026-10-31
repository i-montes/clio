# Host: this PC (the base)

Clio runs on the user's own machine (ADR 0006); a VPS is optional (`infra/vps/`).

`./check-host.sh` is read-only, needs no sudo and takes a second. Its messages are in Spanish (the person reads them in `clio doctor`); `PASS`/`WARN`/`FAIL`, the `S-xx` keys and the exit code are the stable part. `clio doctor` runs it with `CLIO_DOCTOR_EMBEDDED=1`, which skips the two checks `clio` already does itself (S-38 for Clio's own listeners and S-23 for the permissions of Clio's folders), so each shows once. It checks, for the machine that runs Clio:

| Check | Rule | FAIL or WARN |
|---|---|---|
| No Clio process listens outside loopback and `tailscale0` | S-38 | FAIL |
| `~/.config/clio`, `~/.local/share/clio` and `$XDG_RUNTIME_DIR/clio` are 700, their files 600 | S-23 | FAIL |
| The disk holding `$HOME` is encrypted | S-40 | WARN |
| A firewall is active; OpenSSH is off; whether **Tailscale SSH** (its own server, no `sshd`, invisible to `ss`) is on | S-38 | WARN |
| Tailscale runs and this node's key expires; Tailnet Lock | S-31 | WARN |
| `bwrap` exists; the user is in the `docker` group; a backup tool exists | S-03, S-39, S-40 | WARN |

A WARN is the person's call; a FAIL means something of Clio is exposed. Run it after installing or changing anything, and before leaving the PC unattended with the agent running.

Layout on the host: configuration in `~/.config/clio/`, data in `~/.local/share/clio/`, runtime sockets in `$XDG_RUNTIME_DIR/clio/` (all 0700).
