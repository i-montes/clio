# The installer (design, 2026-11-19)

Status: asked for by the owner on 2026-11-19. Built on the branch `feat/installer`, except "Default MCP servers" (the setup has an empty, marked step for it). The decisions taken while building it are at the end.

## What the owner asked for

One line copied from GitHub and pasted in a terminal. An attractive, guided installer that first
asks the language (Spanish or English), then installs everything Clio needs, so that the person
types no other command and can use Clio as soon as it ends. It detects an existing installation and
does what is best with it (update, repair, remove). A clean guide: it does not warn about
everything. It installs clio-memory, the vault and the rest by default, and proposes a default set
of MCP servers (Playwright, Notion and the like).

## Today

Installing means cloning the repository, a Rust toolchain, `just install`, `clio init`,
`clio vault start`, `clio connect …`, `clio pi install`, a system package (`bubblewrap`), Node 22,
clio-memory from its own repository, and `clio services enable`. There are no published binaries:
`release.yml` runs the gates and stops at "build the artifacts here".

## The line

```sh
curl -fsSL https://github.com/i-montes/clio/releases/latest/download/install.sh | sh
```

## Two pieces

1. **`install.sh`** — small, POSIX `sh`, no dependencies beyond `curl`, `uname`,
   `sha256sum`/`shasum`. It finds the system and processor, downloads the matching `clio` and
   `clio-vault` from the latest GitHub release with `SHA256SUMS`, checks them, puts them in
   `~/.local/bin`, and starts `clio setup` attached to the terminal (`/dev/tty`, since its own
   input is the pipe). It prints almost nothing itself. `CLIO_VERSION` picks a version.
2. **`clio setup`** — the guided installer, inside the binary, drawn with the same terminal
   library and palette as the chat. Everything the person sees and decides happens here.

## What `clio setup` does

Screens, in order; each is one decision or none:

1. **Language.** Español / English. Everything after it is in that language, and Clio's first
   conversation starts in it.
2. **What it found.** Nothing (a new install) or an existing one (version, where its data is,
   whether its vault and timer are there). For an existing one the default is **Update** (keeps
   everything); the other choices are Repair, Remove Clio but keep my data, Remove everything.
3. **What it will do.** A short list, one Enter.
4. **Doing it.** One line per step with its state (waiting, working with a progress bar for
   downloads, done, failed with one plain sentence). It stops only where a person is needed:
   - the system password, once, if a system package must be installed (`bubblewrap`) or lingering
     turned on;
   - the vault: a new passphrase (twice), or the existing one to open it;
   - the model: pick a connector (Claude subscription, Anthropic API, OpenCode) and paste its key or
     token, masked, straight into the vault;
   - the judge: skipped by default; a person with their own Temis gives its address and key;
   - the MCP servers: a checklist with a sensible default, and for each one that needs a token, a
     masked field.
5. **Done.** One line, and the chat opens.

Steps it runs: folders and config (`clio init`), `bubblewrap`, a private Node 22 (the official
tarball, checksum verified), Pi (`clio pi install`), clio-memory (its released binary, checksum
verified, and its embedding model), the vault (create, start, open), the model key, the services
timer (and lingering), the shipped skills, the chosen MCP servers, `clio doctor`. A step that is
already right is shown as done and skipped: running the installer again is always safe.

## Existing installations

- **Update:** replace the binaries, bring Pi, Node, clio-memory and the shipped skills up to date,
  keep config, vault, memory, conversations, skills, services and servers; restart the timer.
  Refused politely while a `clio` chat is open ("close it and press Enter").
- **Repair:** the same steps without changing versions.
- **Remove, keeping data:** binaries, Pi, Node and the timer go; config, vault, memory and
  conversations stay.
- **Remove everything:** all of it, clio-memory and its model included, after one explicit confirmation that names what is lost. Both removals take out the programs (clio, clio-vault, clio-memory, Node, Pi) and the PATH line install.sh added; Remove Clio but keep my data keeps the data, the configuration and the model (`~/.cache/clio-models`). A timer systemd cannot reach (no user session) is a warning: its unit files are removed anyway.

## `clio update`

There is no update command today. `clio update` brings everything that is Clio's up to date in one
step, with no questions: the `clio` and `clio-vault` binaries (latest release, checksum verified,
replaced atomically), Pi, the private Node, clio-memory and its model, the shipped skills, and the
MCP servers that came from the catalog when the catalog pins a newer version (those ask first, as
any server version does). It keeps config, vault, memory, conversations, skills, services and
servers, restarts the timer, runs the doctor and says in a few lines what changed. `clio update
--check` only says what is newer. It is the same code as the installer's Update, without the
screens. The round tells the person, once, when a newer Clio exists; it never updates by itself.

## Default MCP servers

A small catalog shipped with Clio (`mcp/catalog.json`): name, package, exact version, what it is
for, the hosts and token it needs, verified against the real registry and by a real start in the
jail before it is listed. Playwright and Notion are the first candidates; an entry that cannot run
inside Clio's jail is left out and said so. Choosing one in the installer is the person's approval
(a new person-side command, `clio mcp add <name>`); the model's own proposals keep asking.

## Releases

The installer needs published binaries. `release.yml` gains the build after its existing gates and
the owner's review: `clio` and `clio-vault` for Linux x86_64 and aarch64, `SHA256SUMS`, and
`install.sh`, attached to the GitHub release. macOS is left out until it has been run end to end
(CI still builds and tests it); `install.sh` says it is not supported yet.

## Not included

- Windows.
- Remote (OAuth) MCP servers: the catalog lists npm servers only.
- Unattended fleet installs: there is a `--yes` path for tests, nothing more.

## Decisions (as built)

- Release files: plain binaries `clio-<target>` and `clio-vault-<target>` (no archive, so `install.sh` needs no `tar`), plus `SHA256SUMS`, `install.sh` and `VERSION`; the latest release is the one `releases/latest/download/VERSION` names, and every file then comes from `releases/download/v<VERSION>/`, so two releases are never mixed.
- Linux portability: `cargo zigbuild` against glibc 2.28 (Ubuntu 22.04 has 2.35, Debian 12 2.36); the release checks the highest `GLIBC_` symbol with `objdump` and starts both binaries in `ubuntu:22.04` and `debian:12` containers. Not musl: clio-memory loads ONNX Runtime, a glibc library.
- `install.sh` uses `mkdir -p` (for `~/.local/bin`) and `rm -rf` (only on the temporary folder it made with `mktemp -d`) besides the tools listed: without them a failed download would leave files behind.
- `CLIO_RELEASE_BASE` is one release's folder (or `https://` URL, never `http://`) with the same file names; with it, `CLIO_VERSION` is not used, and `install.sh` names it in one line. `clio` reads it only in debug builds, so a variable left in a profile cannot redirect `clio update`.
- The one line is the latest release's own `install.sh` (`releases/latest/download/install.sh`), which went through every gate, not `main`'s.
- `install.sh` puts `~/.local/bin` on the `PATH` of the next terminal when it is not there: one marked line, once, in `~/.bash_profile` (or `~/.profile`) and `~/.zshrc` for zsh. It refuses root unless `CLIO_ALLOW_ROOT=1`, and a folder in the place of a binary.
- The private Node: 22.23.5, the official tarball with its SHA-256 pinned in the binary, unpacked in `~/.local/share/clio/node` (a `.clio-node` mark says which archive); everything that starts Node or npm for Clio (Pi, its install, the reviewer, MCP servers) puts its `bin` first in the `PATH`.
- clio-memory: the binary of release v0.14.0 (the one Clio's library is pinned to), checked against that release's `SHA256SUMS`, whose own SHA-256 is pinned; a newer one already installed is kept. Its model and ONNX Runtime come from the specs in `clio_memory::setup` (the same URLs and SHA-256 as its own installer), with the runtime's `.sha256` mark, so `clio-memory setup` finds them done.
- Ask first: every question (language, what to do with an installation, the vault's passphrase, the model, the judge, the sudo moment) comes before any download; after the last answer nothing stops until the last screen. An existing vault is opened while asking (a mistyped passphrase is asked again); a new one is created by its step. An update that will replace the binary asks nothing: the new binary asks after it took over.
- `sudo`: the package manager's install of `bubblewrap`, `loginctl enable-linger` and, only where `kernel.apparmor_restrict_unprivileged_userns` is 1, no profile names `/usr/bin/bwrap` and the jail still does not start, the fixed AppArmor profile `clio-bwrap`, run back to back in one moment with the exact commands shown first, so `sudo` asks once; Clio never reads the password. Then the jail is tried: when it cannot start, the last screen names what to ask an administrator for and the chat does not open. As root they run directly; without `sudo`, one line names what to ask an administrator for.
- A Claude subscription offers "sign in when the install finishes" (first) or "paste a token" (a masked field). Signing in runs Claude Code's own `setup-token` (the bridge's bundled copy) after the last step, outside the installer's screen; the token pasted back goes to the vault and the chat opens. If the sign-in fails or is cancelled, the one thing left is to run the installer again and choose the model. "Later" is always a choice, and the last screen then says the exact next step. The installer's screen never ends in a command to type for the subscription.
- The memory model: the runtime first, then the files gathered in a hidden `.<dir>.partial` folder that takes the model folder's place only when all are verified. A failure is a warning ("search by meaning: finishes later"), leaves no model folder (memory searches by text) and `clio update` tries again.
- The runtime folder for the sockets, the same for every command: `$XDG_RUNTIME_DIR/clio`; without it, `/run/user/<uid>/clio` when that folder is the person's; else a private `clio-<uid>` (0700, checked without following a symlink) in the temporary folder.
- No UTF-8 locale: the screens and lines go out in ASCII.
- Secrets go from the installer's process straight to the vault (`Vault::init`, then unlock and put over its socket): never an argument, an environment variable or a child's input.
- The language is recorded in `config.json` (`ui.language`, `es` or `en`); the first conversation's presentation request asks for that language instead of English while the memory has nothing in a language of its own. The system prompt does not change.
- Update replaces the binaries first; if they changed, the new `clio` is started in place of the old one (with a hidden argument, `setup --resume-update` or `update --reexec`, so nothing stays in the environment of the chat) to bring Node, Pi, clio-memory and the skills to what it pins. The wizard's Update also finishes a half-done install (the vault, the model); `clio update` never asks.
- Both removals keep clio-memory and its model (other agents may use them); Remove-keeping-data keeps Pi's conversations (`pi/sessions`, `pi/agent`, `pi/home`) and removes Pi's installed versions, the private Node, the binaries and the timer's units.
- The round learns of a newer release at most once a day (`~/.local/share/clio/update.json`) and adds one line to its data when there is one.
- Tests use debug-only seams: `CLIO_TEST_PINS` (local fakes in place of the pinned downloads), `CLIO_TEST_SYSTEM` (fake `sudo`, package manager, `systemctl`, `loginctl`, `bwrap`, `tee`, `apparmor_parser`, the sysctl and the AppArmor folder) and `CLIO_RELEASE_BASE`; release builds ignore them.
- macOS: not released yet (`install.sh` refuses it in one line).
