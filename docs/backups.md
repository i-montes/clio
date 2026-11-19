# Backups with restic (S-40)

Clio's memory, the vault and the configuration live on the host's disk. These scripts make encrypted copies and check that they can be read back. They work the same on Linux and macOS (shared scripts are named `.unix.sh`; what belongs to one system, `.linux.` or `.macos.`). Decided on 2026-11-02: the tool is `restic`.

## Where the copies go

With no argument, `backup-init.unix.sh` creates the repository in a place that depends on the system and sits **outside Clio's data folder**, so deleting the data does not delete the copies:

| System | Default repository |
|---|---|
| Linux | `${XDG_DATA_HOME:-~/.local/share}/clio-backup/repo` |
| macOS | `~/Library/Application Support/clio-backup/repo` |

**That place is on the same disk as your data.** It protects against deleting or damaging files, not against losing the disk. To survive that, give a different destination when you create the repository: a folder on an external disk (`/run/media/$USER/DISK/clio-backup`, `/Volumes/DISK/clio-backup`), `sftp:user@host:/path`, `b2:bucket:path`. `check-host.sh` reminds you with a warning while the repository is on the same disk.

## How many copies

Only the **last 2** (`restic forget --keep-last 2 --prune` after each copy). Change it with `CLIO_BACKUP_KEEP=N` (an integer, 1 or more; it never deletes everything). With one copy a day that is two days of history: if a problem goes unnoticed longer, there is no earlier good copy. restic stores only what changed, so a higher number costs little space.

## What is backed up

- `~/.local/share/clio/memory.db` (a hot copy with `clio-memory backup` or `sqlite3 .backup`, never the file in use), if it exists.
- `~/.local/share/clio/vault.bin` (already encrypted by the vault; restic encrypts it again), if it exists.
- `~/.config/clio/config.json`.
- `~/.local/share/clio/services/` without its `locks/` (S-69): every approved version of each service (`service.json` and its files), the records (on, off, paused), the last 50 runs of each (their output already scrubbed), the notices, and each service's state folder (`state/<name>/`, what the service keeps between runs). A restored service runs only if its files still hash to its record; the timer's two unit files (`~/.config/systemd/user/clio-services.{service,timer}`) are not copied: `clio services enable` writes them again.
- `~/.local/share/clio/runs/` without the working copies (`sandbox/` and `scratch/`): the records, diffs, effect logs and, for host sessions, the tool host's files (file backups for `clio undo`, what the judge withheld, background job output) stay.

The model key lives in the vault (`vault.bin`, which is already in the copy, encrypted). Not backed up: the restic passphrase, what can be reinstalled (`pi/`), the trash of deleted conversations (`~/.local/share/clio/trash/sessions/`, kept 30 days, 0700: each entry holds the conversation and, for a copy-mode session, its working copy, which can weigh as much as the project; `clio sessions restore` brings one back whole), and the host-mode files outside `runs/` (`~/.local/share/clio/host/`: the action log, what is pending and the day's budget; `~/.config/clio/host.json`).

## Setting it up (once)

```sh
infra/local/backup-init.unix.sh [REPOSITORY]   # creates the repository and the passphrase (0600)
infra/local/backup.unix.sh                     # first copy
infra/local/backup-restore-test.unix.sh        # checks that it can be restored
infra/local/install-backup.linux.sh            # daily copy: a systemd timer (Linux)
infra/local/install-backup.macos.sh            # daily copy: launchd (macOS, not tested on a Mac)
```

`infra/local/backup-selftest.unix.sh` tests all of this against throwaway repositories in a fake HOME, without touching anything real. Run it on a Mac before you trust the macOS installer.

## The passphrase

`backup-init` creates a random passphrase in `~/.config/clio/restic-password`. **It is on the same disk as the data**, so if the disk is lost the copies cannot be read even if they are elsewhere. Keep a copy of that passphrase off this machine (a password manager) right after you create the repository. The script never prints it.

## Restoring

```sh
set -a; . ~/.config/clio/backup.env; set +a
restic snapshots                              # pick a snapshot
restic restore latest --target /empty/path    # files come out under their original path
```

Copy by hand only what you need from `/empty/path` to its place; nothing is overwritten by itself.

## How it is checked

- `infra/local/check-host.sh` (Linux) warns if no repository is configured, if the last snapshot is more than 8 days old, or if the repository is on the same disk.
- `backup-restore-test.unix.sh` restores the last snapshot into a throwaway folder and checks that the memory passes `integrity_check` and that the vault and the configuration are not empty. Run it every month (S-40: "a restore tested every month").
