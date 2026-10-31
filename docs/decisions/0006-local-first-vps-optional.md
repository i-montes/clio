# 0006: The user's own machine is the base; a VPS is optional

## Context

The design (artifact and specs) assumed the brain runs on a VPS that never turns off, with the user's devices as extra nodes. The person decided on 2026-10-31 that for now there is no server: everything runs on their own PC, which already has Tailscale, and a VPS must stay an option, not the foundation.

## Decision

- **The base is one machine: the user's own.** The brain, gateway, vault, memory and trace run there, as the user's processes. The VPS is one more possible node (or a second host for the brain) added later through `clio-network`; nothing in the base may need it.
- **Layout on the host:** configuration in `~/.config/clio/` and data in `~/.local/share/clio/` (both mode 0700), runtime sockets in `$XDG_RUNTIME_DIR/clio/` (0700). The vault and the gateway stay separate processes talking over Unix sockets with their own permissions.
- **Network:** nothing listens on a public interface. Components talk over Unix sockets or loopback. Tailscale is for reaching the brain from the person's other devices (the PWA, phase 4), and the PWA will bind to the `tailscale0` address only, never to `0.0.0.0`. `infra/local/check-host.sh` checks this.
- **The security rules do not weaken, they move.** S-38 (no public ports), S-39 (separation) and S-40 (encrypted disk and backups) apply to the host, whichever it is: this PC (LUKS disk, firewall on, SSH off, Tailscale key with expiry, all verified on 2026-10-31) or a VPS. The VPS draft in `infra/vps/` stays as the optional path.
- **What gets harder locally:** the agent runs as the person's own user, so their files (`~/.ssh`, browser profile, other projects) are within reach of anything the agent can execute. The defense is the sandbox and the gateway, and for running commands an OS-level jail (`bwrap` is installed) is a follow-up, not yet done. Being in the `docker` group is root-equivalent: keep Docker out of the agent's reach.

## Consequences

The specs and the artifact still say VPS in several places; where they do, read «the host (this PC by default)». The repository is the authority, as with the conflict state in task 039. The tasks for a VPS (085 to 096) are rewritten for the host; the VPS-only parts stay in `infra/vps/` as optional.
