# Web search through Tavily (design, 2026-11-22)

Status: asked by the owner on 2026-11-22: web search through **Tavily**; when Tavily's token is
not in the vault, Clio asks for it in the chat and in the setup, either place being enough. The
standing criterion: Clio works on its own and is not interrupted; the person is asked once, at the
moment it is needed, in one box when possible; a question they declined is not repeated on every
chat open; nobody has to leave the chat for a terminal command. It builds on
`2026-11-19-default-mcp-design.md` (Clio's MCP catalog) and S-70.

## What it is

Web search is one more server of Clio's catalog, `tavily`: Tavily's own MCP server, installed and
run exactly as the other catalog servers (exact version, pinned integrity, shipped lockfile, `npm
ci --ignore-scripts`, a probe with no network and no secrets, the service jail, the allow-list
proxy, its secret on standard input). There is no native Rust search tool, no second provider, no
cache and no summarising model: the model calls `tavily_search` and `tavily_extract` with
`mcp_call`, and what comes back is outside text like any MCP result.

## The catalog entry

Verified on 2026-11-22:

- **Package**: `tavily-mcp` **0.2.22** (npm `latest`; GitHub's `package.json` is at 0.2.23, not
  published). Repository `github.com/tavily-ai/tavily-mcp` (organisation "Tavily AI", linked
  from tavily.com's docs); published by `yashbtavily`, maintainers `dustinpulver`,
  `guyhartstein`, `yashbtavily`, `parakhjaggi`. MIT. Integrity
  `sha512-w2pmmmn/JKcNavylDoiNtNPflWt6SK2G1SVUvg0VvdvFJQUaOzEdanAm1VUWBOhUCqmVoBYVPstGcOoWvh8RQQ==`.
- **Tree**: 126 packages (`mcp/locks/tavily.json`, made with Clio's own npm 10.9.9 by `npm install
  --ignore-scripts`, the top-level `name` removed as in the other locks). `npm audit signatures`:
  126 registry signatures verified, 18 attestations. `npm audit` names three advisories, all in
  parts the server does not use over stdio (the MCP SDK's HTTP transports, OAuth client and bearer
  auth; `@hono/node-server`'s `serveStatic`).
- **No install scripts needed**; the server's code calls only `https://api.tavily.com/{search,
  extract,crawl,map,research}` (axios, which honours the proxy). `network: ["api.tavily.com"]`.
- **One secret**: `tavily-api-key` in `TAVILY_API_KEY`, bound to `api.tavily.com`, with where to
  get it in English and Spanish (app.tavily.com, keys start with `tvly-`, the free plan's size and
  a search's cost).
- **Limits**: 50 % CPU, 1536 MB, 120 s a call (search and extract of several pages can take
  tens of seconds).
- **Modes**: only `tavily_search` and `tavily_extract` run without asking (cheap reads);
  `tavily_crawl`, `tavily_map` and `tavily_research` ask each time.
- The server also has a keyless mode (search and extract without a key, rate-limited by Tavily).
  The owner chose the token: the entry always gives one.

## What the person sees with the token missing

- **In the setup** (`clio setup`, a first install or a re-run): the MCP checklist comes with "Web
  search (Tavily)" ticked, the others not, and a line saying that what is ticked is what Clio
  suggests. Then each ticked server's token is asked, masked; Enter leaves it for later (installed
  off, waiting for it). A re-run of `clio setup` offers it ticked again while it is not installed.
  `clio update` asks nothing and installs nothing new; `--yes` asks nothing.
- **In the chat, when search is first needed**: the model calls `tavily_search`; the tool host sees
  that `tavily` is the catalog's, installed exactly as the catalog has it, off waiting for its
  token, and leaves one secret request for the chat. The model reads: "«tavily» is off until the
  person gives its token: Clio asks them for it in the chat when this turn ends. Do not ask them
  yourself; go on with the rest without it. Once they type it the server turns on, and calling it
  again works." When the turn ends the chat's masked box asks: the secret's name, where it goes
  (`mcp:api.tavily.com`) and the catalog's words (Tavily's site, the free plan, a search's cost).
  Typed: stored in the vault and the server on, said in the chat ("is on now: call it
  again"). Esc: it stays off.
- **In the chat, approving it**: when the person says yes to a catalog server (any of them) whose
  token the vault lacks, it is installed, left off waiting for it, and the same box comes right
  after that yes.
- **Declined**: the server's record says it was declined; no call and no chat asks again, and a
  call tells the model not to ask. `clio mcp add tavily` and `clio setup` ask again.
- **With the token present**: nothing is asked anywhere: the setup skips the question, and the
  server is on.

## How it works

- **The request is the tool host's.** `mcp_call` on a server that is off: when it is a catalog
  server whose installed manifest is exactly the catalog's and whose record says it waits for its
  token, the tool host asks the vault (person socket, `List`) which names it has, and for each
  catalog secret missing it writes a class `secret` item (`tool: "mcp_token"`) with the catalog's
  name, `mcp:<host>`, why (the catalog's title and `where`) and the server's name. Nothing from the
  call's arguments or the model goes in. One waiting request per secret: a newer session's takes
  the place of another session's. A `suspect` session, a tool host without the vault, or a vault
  that cannot be listed asks nothing; the model gets a fixed line saying so.
- **The box is the existing one** (`request_secret`'s): masked, the value going straight from the
  box to `vault::store_requested` on the person's socket, never replacing a secret of that name.
- **Turning on.** After the vault took the value, the person's `clio` turns the server on when each
  of its secrets may reach its host (`vault::allows`), only for a catalog server installed as the
  catalog has it and still waiting for its token (never one the person turned off). The tool host
  reads the server's record on every call, and MCP tools reach the model through the fixed
  `mcp_tools`/`mcp_call`: the next call works in the same chat, with no restart of Pi.
- **A yes in the chat**: `approve` installs the server; when it is the catalog's and its token is
  missing, the same request is left for the session that proposed it, and the chat offers its box
  right after the yes. Approvals of the person's own `clio mcp add` and setup (run `person`) leave
  none: they ask themselves.
- **The no is kept** in the server's record (`off: you declined its token …`), as the shared-skills
  box keeps its no: a call of a declined server asks nothing.
- **The host prompt** (`clio-host@16`) adds one line: web search and page reading are the tools of
  `tavily`, called with `mcp_call`; when it is not installed and the task needs the web, propose it
  with `mcp_propose` and `catalog: "tavily"`; when a call says it waits for its token, Clio asks
  the person: never ask for it yourself.

## Decisions taken (the owner was not asked)

- **No new rule**: everything fits S-70 (a catalog server, its secret bound to `mcp:<host>`, the
  existing secret box). No new rule number, no new attack case.
- **Tavily goes last** in the catalog; the tick is a setup constant (`TICKED`), not a catalog field,
  so the catalog's format does not change.
- **Ticked only when not installed at all**: a server installed some other way is not offered
  ticked. The token question is skipped when the vault already lets that name reach the host.
- **The person's re-run of `clio setup` offers the catalog** (an update the person started); `clio
  update` never asks, and `--yes` never asks.
- **The no lives in the server's record**, not in a new file: it is where `clio mcp` already says
  why a server is off, and `clio mcp add`/`clio setup` overwrite it.
- **One request per secret across sessions**: the newest session's wins, so two chats never ask
  twice.
- **A request that is ignored (not declined) expires in 24 h** like any secret request; the next
  call after that asks again. Only Esc is a kept no.
- **A token the vault has by name** but that cannot reach the host (taken by another destination)
  is not asked again: the model is told the person turns the server on with `/mcp on tavily`, and
  `clio mcp` says for what the name is taken.
- **`clio approve` at a terminal** installs the server and leaves the request for the next chat of
  that session; the terminal does not ask for the token.

## Invariants and their tests

- The token never passes through the model, Pi, the guard, a log, the pending file or the action
  log, is bound only to `mcp:api.tavily.com`, and is never shown:
  `a_search_without_its_token_asks_for_it_in_the_chats_box_and_turns_it_on` (real Pi, pty;
  `assert_nowhere`, gateway resolve for `mcp` and refusal for `http_fetch`),
  `a_catalog_token_typed_in_the_chat_turns_its_server_on_and_a_no_is_kept`.
- The request carries only catalog data, one per server, none from a suspect session, none for a
  server that is not exactly the catalog's, none after a no:
  `a_catalog_server_waiting_for_its_token_asks_the_person_once_with_the_catalogs_words`,
  `a_token_the_vault_has_is_not_asked_again`,
  `a_token_answer_touches_only_the_catalogs_own_server_waiting_for_it`.
- A yes in the chat asks right then; the person's own installs do not:
  `a_yes_to_a_catalog_server_without_its_token_asks_for_it_in_that_chat`.
- The setup ticks web search, asks the token only when missing, and a re-run offers it:
  `web_search_comes_ticked_and_its_token_is_asked_only_when_the_vault_lacks_it`.
- The prompt line: `the_host_prompt_only_adds_the_web_search_line`.

Each was checked by breaking the code it covers and seeing it fail.

## Not included

- A real search: there was no Tavily key on the machine where this was built. A real install in a
  temporary home (`clio mcp add tavily --yes`: `npm ci`, 126 packages, the probe listing five
  tools) and a call with a made-up key that reached `api.tavily.com` through the jail and the
  proxy (Tavily answered "Unauthorized: missing or invalid API key") were run.
- Keyless mode, other providers, a cache, a native search tool.
- The immediate box after a yes in the chat has no test of its own: without it the same box still
  comes when the turn that yes starts ends.

## Version

Not bumped: the owner bumps the version.
