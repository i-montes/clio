# The installer (design, 2026-11-19)

Status: asked for by the owner on 2026-11-19. Nothing here is built.

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
curl -fsSL https://raw.githubusercontent.com/i-montes/clio/main/install.sh | sh
```

## Two pieces

1. **`install.sh`** — small, POSIX `sh`, no dependencies beyond `curl` (or `wget`), `uname`,
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
- **Remove everything:** all of it, after one explicit confirmation that names what is lost.

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
the owner's review: `clio` and `clio-vault` for Linux x86_64 and aarch64 and macOS arm64 and
x86_64, `SHA256SUMS`, and `install.sh`, attached to the GitHub release. On macOS there is no jail
yet, so the installer says what works there and what does not.

## Not included

- Windows.
- Remote (OAuth) MCP servers: the catalog lists npm servers only.
- Unattended fleet installs: there is a `--yes` path for tests, nothing more.
