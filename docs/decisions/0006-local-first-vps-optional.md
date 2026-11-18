# 0006: One machine is the base, and the guard is at the door

Status: accepted 2026-10-31. Its security part (the agent walled into a copy) was replaced on 2026-11-18 by "the guard is at the door", below. The layout and network parts still hold.

## Context (2026-10-31)

The design (artifact and specs) assumed the brain runs on a VPS that never turns off, with the person's devices as extra nodes. The person decided on 2026-10-31 that for now there is no server: everything runs on their own PC, which already has Tailscale, and a VPS must stay an option, not the foundation.

## Decision (2026-10-31)

- **The base is one machine, the host.** The brain, gateway, vault, memory and trace run there, as the person's processes. Other devices are nodes added later through `clio-network`; nothing in the base may need them. The host was the person's PC at first; Clio now lives on the owner's VPS. Nothing in the layout below depends on which one it is.
- **Layout on the host:** configuration in `~/.config/clio/` and data in `~/.local/share/clio/` (both mode 0700), runtime sockets in `$XDG_RUNTIME_DIR/clio/` (0700). The vault and the gateway stay separate processes talking over Unix sockets with their own permissions.
- **Network:** nothing listens on a public interface. Components talk over Unix sockets or loopback. Tailscale is for reaching the brain from the person's other devices (the PWA, phase 4), and the PWA will bind to the `tailscale0` address only, never to `0.0.0.0`. `infra/local/check-host.sh` checks this.
- **The security rules do not weaken, they move.** S-38 (no public ports), S-39 (separation) and S-40 (encrypted disk and backups) apply to the host, whichever it is. On the PC this was verified on 2026-10-31: LUKS disk, firewall on, OpenSSH off with only Tailscale SSH reachable through the tailnet, Tailscale key with expiry. The VPS draft in `infra/vps/` stays as a reference.
- **What got harder locally (replaced on 2026-11-18).** The agent runs as the person's own user, so their files (`~/.ssh`, browser profile, other projects) are within reach of anything the agent can execute. The 2026-10-31 answer was containment: the agent worked on a copy of one project, commands ran in an OS-level jail (`bwrap`), and the person brought changes back with `diff` and `apply`. Being in the `docker` group is root-equivalent, so Docker was to stay out of the agent's reach.

## 2026-11-18: the guard is at the door

### Why the old answer was replaced

The owner decided that Clio has full use of the terminal of the machine it lives on, and that the security layer must fit that. Working on an isolated copy inside a jail was, in the owner's words, "a straitjacket that does not fit the autonomy I wanted": an agent that only sees a copy of one project cannot manage the server it lives on, which is what Clio is for. Host mode became the factory mode the same day ([ADR 0012](0012-host-mode.md)). Copy mode still exists (`--copy`, `"mode": "copy"`, `clio run --workspace DIR`).

### Decision

Instead of walling the agent in, Clio guards the door every effect must go through:

1. **The harness and the model stay jailed, with no credential but the model key.** Pi runs in its `bwrap` jail with its own network namespace ([ADR 0009](0009-pi-runs-inside-a-jail.md)): it does not see the vault, the person's files or Clio's data. The only credential inside is the model key, and the jail's proxy lets it reach only the model provider.
2. **Every effect goes through the tool host.** The guard extension turns each tool into a request to `clio-toolhost`: reading, writing, running a command, the memory. The gateway decides each one in code, deny by default.
3. **A judge reads outside text before the model does.** Temis, the owner's own service, reached through the Jev adapter (TypeSafe's Jev is the adapter's other destination and the default when Jev is on), answers one closed question (`injection`) about each tool result that came from outside. A clear injection is withheld from the model, kept for the person, and the session is marked `suspect`; a suspect session's networked commands wait for the person. No answer, or no judge: as before.
4. **Four classes of action are set aside for the owner:** delete (`borrar`), spend (`gastar`), send (`enviar`) and control (`control`). They do not run until the person approves; they run once, exactly as saved.
5. **Secrets live in the vault, and the model never sees a value.** When a task needs one, the model asks with `request_secret` and the person types the value into Clio, which puts it in the vault. No tool of the model sends a stored secret anywhere yet.
6. **Everything is logged, and file changes can be undone.** The action log has a line before and after every command and write; the file tools copy what they replace, and `clio undo` walks it back.

The details, the classes, the launchers and the known limits are in [ADR 0012](0012-host-mode.md) and [`docs/threat-model.md`](../threat-model.md).

### What it costs

- The person's files, services and network are within the agent's reach in host mode. What stands between a fooled model and harm is the door, not a wall: the gateway, the judge, the four classes, the log.
- The list of what is set aside is a list, not a wall: a script, a variable or an alias gets around it. Against that there is the action log and the prompt's rule.
- With passwordless `sudo`, a command can become root, and then Clio's masked folders are not out of reach.
- The judge is optional and probabilistic. Without it, outside text still reaches the model wrapped as data and marks the session, as before.

## Consequences

- The specs and the artifact still say VPS, and some describe a containment model; where they disagree with the code or with this record, the code and this record win.
- The tasks for a VPS (085 to 096) were rewritten for the host; the VPS-only parts stay in `infra/vps/`.
- The README diagram (`docs/clio-architecture.svg`) shows the door, not the old gateway in front of a sandbox.
