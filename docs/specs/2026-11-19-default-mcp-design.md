# Default MCP servers (design, 2026-11-19)

Status: asked by the owner ("propose default MCP servers like Playwright, Notion"). Second stage of
the installer (`2026-11-19-installer-design.md`). It builds on stage 4 of
`2026-11-18-clio-creates-design.md` and changes one of its rules (the network of a server).

## What was checked on the owner's server (2026-11-19)

- **Notion** runs: the owner had Clio install `@notionhq/notion-mcp-server@2.6.1` (host
  `api.notion.com`, secret as `$NOTION_TOKEN`, 1536 MB: its search died at 512 MB, 120 s a call).
- **Playwright** runs in the jail: `@playwright/mcp@0.0.86` with `--headless --isolated --browser
  chromium --executable-path <chromium> --no-sandbox`, `reads` the Chromium folder, 1536 MB, 120 s.
  It opened `https://example.com` and read the title. Three limits showed:
  1. A server reaches only the hosts it declares. A browser needs any public site.
  2. Chromium was already on that machine. A new machine has none, and no system libraries for it.
  3. `0.0.86` depends on alpha builds of Playwright, and asks for a newer Chromium than the one used.
- The reference servers (`@modelcontextprotocol/server-*`) now ask Node 24 or later. Clio's Node is
  22: they stay out until Clio's Node moves, which is its own change.

## The catalog

`mcp/catalog.json`, shipped in the binary as the skills are. One entry per server:

- `name`, a title and one line of what it is for, in English and Spanish;
- `package`, exact `version` and `integrity` (the install refuses anything else, as today);
- `args`, `env`, `network`, `reads`, `writes`, `limits`: the manifest's own fields, the values that
  were verified;
- `secrets`: name, variable, host, and one line (en/es) of where the person gets it;
- `modes`: the tools that only read, which start as `free` (shown in the box); every other tool
  starts as today (`free` if the server says read-only, else `ask`);
- `needs`: `browser` or nothing.

A server enters the catalog only after it was installed and called for real in Clio's jail at that
exact version. A check in CI holds the shape (names, exact versions, integrity, the manifest's
rules), not the network.

First entries: `playwright` and `notion`. Nothing else until verified the same way.

## `clio mcp add <name>` and the wizard

- `clio mcp add <name>` (the person's command; `control` for the model) composes the same manifest
  `mcp_propose` would, from the catalog, and runs the same checks, the same box and the same
  install and probe. At a terminal the person answers the box there. A secret is asked with no
  echo and goes to the vault bound to `mcp:<host>`; "later" installs the server off.
- `clio mcp add` alone lists the catalog and what is installed.
- The wizard gets one screen among its questions (all questions come first): the catalog as a
  checklist the person fills: nothing is ticked, each server is theirs to take or leave, and
  Enter with none ticked installs none (owner, 2026-11-19). Notion asks its token on that screen,
  with no echo, or "later". The installs run with the rest, after the last answer. A
  server that fails is a warning ("finishes later: `clio mcp add <name>`"), never a failed install.
- The model: `mcp-installer` says the catalog exists and that a catalog server is proposed with the
  catalog's values (`mcp_propose` accepts `catalog: <name>` and fills the rest itself). The person's
  yes is the same.
- `clio update` says when the catalog holds a newer version of an installed server. It does not
  install it: a new version is a new yes.

## A server that reaches any public site

`network: ["*"]` in a manifest means any public host on ports 443 and 80. Conditions, held by the
tool host:

- refused when the manifest has `secrets`, and when it `reads` or `writes` anything under the home
  but Clio's own browser folder: a server that can go anywhere holds nothing to take there;
- the proxy still refuses addresses that are not public (the machine, its networks, metadata);
- the box says "It can reach any public site" where the hosts were, and the manifest's digest
  covers it;
- the model may propose it too: the box and the person's yes are the gate, as for every server.

Why it is acceptable: the agent's own commands already reach any site as class `send`. A browser
call is the same act, and it waits for the person or their window the same way (`browser_navigate`
and every tool that acts stay `ask`). What a page returns is outside text, judged like every MCP
result.

## The browser

- Clio keeps its own Chromium in `<data>/browsers/chromium-<revision>/`, downloaded from
  Playwright's CDN and pinned by SHA-256 for each platform, as Node is. `clio mcp add playwright`
  and the wizard fetch it; `clio update` keeps it at the catalog's revision.
- The catalog pins a `@playwright/mcp` version that depends on a released Playwright, and the
  Chromium revision that version asks for.
- System libraries: checked with `ldd` on the binary. The missing ones go into the wizard's one
  sudo moment (Debian and Ubuntu package names, fixed). Where they cannot be installed, Playwright
  is left out and one line says why.
- `--no-sandbox` stays: Chromium's own sandbox cannot start inside the jail, and the jail is the
  sandbox (no home, no secrets, its own data folder).

## Not included

- Remote and OAuth servers (Notion's hosted one, Google's): no design yet.
- Servers that need Node 24.
- Screenshots as images for the model: results stay text (48 KiB).

## Small fixes that ride with it

- `clio mcp show` prints each tool's whole schema: show the argument names, and the schema only
  with `--schema`.
- An approval given outside the chat (`clio approve`) is not told to the model: the next message
  carries one line saying it ran.

## Decisions as built (2026-11-19, `feat/mcp-catalog`)

- **The catalog** is `mcp/catalog.json`, read by `clio_toolhost::catalog`. `{browser}` in an
  entry's `args` and `reads` stands for Clio's Chromium folder; the manifest the box shows has the
  real path. Its modes free a tool only when the installed manifest is exactly the catalog's (a
  changed argument makes it an ordinary server).
- **Playwright is pinned at `@playwright/mcp@0.0.30`**, not 0.0.86: every later version depends on
  alpha builds of Playwright; 0.0.30 depends on Playwright 1.54.1, which asks for Chromium
  revision 1181. Its tools all say `openWorldHint: true`, so none would start free by the server's
  word; the catalog frees the five that only read (`browser_snapshot`, `browser_console_messages`,
  `browser_network_requests`, `browser_tab_list`, `browser_wait_for`). `--caps tabs,history,wait`
  leaves out the install, file, PDF and testing tools.
- **An entry may pin packages deep in its tree** (`overrides`, npm's own, written into the install
  folder's `package.json`, exact versions only, from Clio's catalog only, never from a proposal).
  `@playwright/mcp@0.0.30` declares no `zod` and takes the MCP SDK's; today the SDK's range
  resolves to `zod` 4, and its `zod-to-json-schema` then gives every tool an empty schema (the
  model would see no arguments). The entry pins `zod` 3.25.76, the last 3.x; every package is
  still a registry tarball with its integrity in the lockfile.
- **Verified for real** (2026-11-19, aarch64, Ubuntu 24.04, a throwaway home under `/var/tmp`):
  `clio mcp add playwright --yes` downloaded Chromium 1181 (SHA-256 checked), installed 102
  packages, probed 21 tools (5 free by the catalog), and one `browser_navigate` to
  `https://example.com` through the jail and the proxy returned the page ("Example Domain");
  `https://127.0.0.1/` and `https://169.254.169.254/` failed to connect; `http://example.com/`
  failed (below). `clio mcp add notion --yes` installed 177 packages and probed 24 tools whose names
  match the catalog's 14 modes; with a made-up token in the vault (`mcp:api.notion.com`),
  `API-get-self` reached `api.notion.com` and got its `401 unauthorized`. Without a token the
  server is installed off, saying why. The x86_64 Chromium archive was checked by its SHA-256
  only, not run.
- **`"*"` is stricter than written**: besides secrets and writes, any read but Clio's own browser
  folder of the catalog's revision is refused, under the home or not (`/srv` too). It is checked at
  the proposal, at the install and before every start (`Runner::prepare`, which also refuses `"*"`
  for anything that is not an MCP server).
- **Plain `http://` pages do not load**: the proxy tunnels `CONNECT` only, and Chromium sends a
  plain `GET` through a proxy for `http://`. Port 80 is allowed in the rule but reached only by a
  client that tunnels. shortcut: https only; a `GET` path in the proxy is its own change.
- **The libraries Chromium lacks** are predicted before any download, from the sonames Chromium
  1181 needs (fixed in `fetch.rs`, the names from Playwright's own `nativeDeps`) and `ldconfig -p`;
  the `t64` package names on Ubuntu 24.04 and Debian 13 and later. After the download `ldd` checks
  again, and a server whose Chromium lacks a library is not started, saying which and that
  `clio setup` (Repair) installs them. `unzip` joins the sudo moment when it is missing.
- **The wizard**: the checklist comes after the judge's question and before the sudo moment; a tick
  is the person's yes (no second box). Notion's token is asked on its own screen, masked; Esc
  leaves it for later, and the server is installed off ("waiting for its token"). An update says
  when the catalog holds a newer version and keeps Chromium at the catalog's revision; older
  revisions are not removed.
- **`clio mcp add <name>`** shows the same box at a terminal (`--yes` answers it), asks a missing
  token without echo, and installs. The model's `mcp_propose {catalog, reason}` takes nothing else.
- **Prompts**: no system prompt changed.
