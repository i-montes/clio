# VPS (draft, not tested)

Nothing in this folder has run on a real server. It is a provider-neutral start so that, once a provider and a Tailscale account are chosen, the machine is rebuilt from files and not by hand (task 085).

| File | What it is | Tasks |
|---|---|---|
| `cloud-init.yaml` | First boot: an `admin` user with an SSH key, one system user per component, SSH hardened, firewall that denies everything except the tailnet, Tailscale from its signed repository, automatic security updates | 085–087, 089, 091 |
| `check-open-ports.sh` | From outside the tailnet, tries the public address; nothing may answer | 088 |

## What is missing and needs the person

- A provider, a region and a size; and an account (see `docs/decisiones-pendientes.md`).
- A Tailscale account: a one-use auth key for the first boot, key expiry and Tailnet Lock (095).
- **Disk encryption (092)** cannot be done from cloud-init: it depends on the provider (encrypted volumes) or on installing with LUKS.
- **Off-site encrypted backups (093) and the first restore on a clean machine (094):** need storage somewhere else and a key kept apart.
- **Containers without root (090):** decided when the first container exists.
- **Review per component (096):** a document the person reads; it needs the components running.

## Before using it

Replace `__ADMIN_SSH_PUBLIC_KEY__` and `__TAILSCALE_AUTHKEY__` in a copy kept out of the repository, boot a throwaway machine, check that `ssh` works only through Tailscale, run `check-open-ports.sh` from outside, and only then trust it.
