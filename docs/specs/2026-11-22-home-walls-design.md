# The home's walls: credential stores and other agents' homes (design, 2026-11-22)

Status: asked by the owner on 2026-11-22 (S-75 and S-76), to be built unattended with every
decision recorded here and in the PR. It generalises S-74's read-only `~/.agents`
(`2026-11-20-shared-skills-design.md`) into one table.

## Why

A host-mode command runs as the person, in their home. A fooled model (a repository's README,
a log line, a web page) could have a script that the classifier never reads, such as
`node scripts/setup.js`:

- read `~/.ssh`, `~/.config/gh`, `~/.aws`, a browser profile or another agent's login, and send
  it out (or keep it for a later networked command);
- plant a session-start hook in `~/.claude/settings.json`, an MCP entry in `~/.cursor/mcp.json`,
  a credential helper in `~/.gitconfig` or a `ProxyCommand` in `~/.ssh/config`. Each of these
  runs code the next time the person, or another agent, starts.

The list of what is set aside is a word list. These have to be walls in the jail.

## One table

`crates/clio-toolhost/src/walls.rs` holds `WALLS`. Each row has a path, what the jail does with
it (`Hidden` or `ReadOnly`) and the clients that open it.

- **Credential stores, hidden:**
  - `~/.ssh` (ssh's clients and git); `~/.gnupg` (none); `~/.aws` (aws, q); `~/.azure` (az);
    `~/.config/gcloud` (gcloud); `~/.config/gh` (gh, git); `~/.kube` (kubectl);
    `~/.docker` (docker).
  - `~/.npmrc` (npm); `~/.netrc`, `~/.git-credentials` and `~/.config/git/credentials` (git);
    `~/.pypirc` (twine); `~/.cargo/credentials` and `credentials.toml` (none).
  - With no client: `~/.password-store`, `~/.local/share/keyrings`, and the browser profiles
    (`~/.mozilla`, `~/.config/google-chrome`, `~/.config/chromium`, `~/.config/BraveSoftware`).
  - The agents' logins, each opened by its own client: `~/.claude/.credentials.json`,
    `~/.codex/auth.json`, `~/.gemini/oauth_creds.json`, `~/.local/share/opencode/auth.json`,
    `~/.config/github-copilot`, `~/.local/share/amazon-q`.
- **Sockets, hidden:**
  - Docker's and Podman's (`/run/docker.sock`, `/run/user/<uid>/docker.sock`,
    `/run/podman/podman.sock`, `/run/user/<uid>/podman`), opened by docker.
  - The user bus and `/run/user/<uid>/systemd`, opened by the bus programs.
  - The keyring's and gpg-agent's (`/run/user/<uid>/keyring`, `/run/user/<uid>/gnupg`) and
    `$SSH_AUTH_SOCK`, which nothing opens.
- **Other agents' homes, read-only:**
  - `~/.agents` (S-74), opened by nothing.
  - `~/.claude` and `~/.claude.json` (claude), `~/.codex`, `~/.gemini`, `~/.cursor`,
    `~/.config/opencode`, `~/.continue`, `~/.cline`, `~/.kiro`, `~/.codeium`,
    `~/.aider.conf.yml`, `~/.copilot`, `~/.tabby`, `~/.tabby-client`, `~/.aws/amazonq` (q) and
    `~/.pi`, each opened by its own client.
- **git's config, read-only:** `~/.gitconfig` and `~/.config/git/config`, opened by `git config`.
  A planted helper, alias or hook path there would run with the stores open.

`walls::closed(home, opened, named)` turns the table into the jail's lists:

- Only paths that exist are listed, each at its real place (`canonicalize`), so a store that is
  itself a link is hidden where it leads.
- A read-only row inside a hidden one is dropped, so the hidden one wins.

Services, MCP servers, the round, reviews and subagents already mask the whole home and `/run`,
or run read-only. They are not changed.

## In the jail

The `HostJob` already had `masks` (hidden) and `shows` (read-only).

- **bwrap:**
  - A mask that is not a folder is bound as `/dev/null` read-only, so it reads as empty and
    connects to nothing.
  - A mask inside a shown folder is bound after the show, so the show does not cover it again.
    `~/.claude/.credentials.json` stays hidden inside a read-only `~/.claude`.
- **Systemd:**
  - A file or socket mask is `InaccessiblePaths=`.
  - systemd orders the mounts by path, so a mask inside a show comes after it.
- **The shell:** `HostJob.login` chooses `bash -lc` (the person's login files, their `PATH`) or
  `bash -c` with the system's `PATH`.
- **Fail closed:** the launcher's self-test proves, with decoys, that:
  - a masked file reads as empty;
  - a link the probe itself makes to that file reads as empty;
  - a mask inside a shown folder reads as empty;
  - a shown folder is not writable.

  Without that, there is no `exec`. These checks are exit codes 12 and 14 of the probe.

## When a store opens

`walls::line(command, home, at_run)` returns the clients a command line is made of, or `None`.
It is `None` unless:

- **There is no shell trick.** The line has no `$`, backtick, here-document, process
  substitution or line continuation.
- **Every simple command is a pure client** of a row, or a `cd`. A client is pure only when no
  option would run other code or print a token:
  - ssh: no `-F`, `-I`, `-S`, `-o ProxyCommand` or other such options, and the destination is
    not this machine;
  - git: only `clone`, `fetch`, `pull`, `push`, `ls-remote`, `submodule update` and
    `remote update`, and none of `--upload-pack`, `--receive-pack`, `--exec`, `--template`,
    `--config`, `ext::`, `fd::` or clone's `-c`;
  - gh: no `auth token`, and no `auth status` with `-t` or `--show-token`;
  - docker: no host bind mount, `--volumes-from`, `--privileged`, `--cap-add`, `--device`,
    `--security-opt`, `-H` or `--config`, and no host namespace;
  - kubectl: no `--kubeconfig`, `config --raw`, `config --flatten` or `plugin`;
  - aws: no `configure` (`configure list` excepted);
  - gcloud: no `auth print-*`, `config config-helper` or `--flags-file`;
  - az: no `account get-access-token`;
  - npm: a list of verbs, and `publish` only with `--ignore-scripts`;
  - twine: `upload` only;
  - an agent's client: none of the options that skip its permissions, no `mcp add`,
    `mcp remove` or `mcp import`;
  - the bus programs are always pure. `systemd-run`, `busctl`, `gdbus` and `dbus-send` stay
    `control` for the classifier, so they wait for the person anyway.
- **No path goes into an opened store.** No path argument is above, at or inside a store the
  line opens. The exceptions are an `-i` key for ssh, scp and sftp, and an `-f` for
  `ssh-keygen`.
- **Clio can follow every `cd`.** Each `cd` goes somewhere Clio can tell, and never into an
  opened store.
- **When it runs (`at_run`):**
  - links are resolved;
  - the folders an uploading client sends (`scp`, `rsync`, `gh`, `docker cp`, `aws s3`…) are
    walked, up to 10,000 entries, for a link into an opened store;
  - every credential client must be found in the system's folders, where neither it nor its
    folder can be written by this user.

Anything Clio cannot tell is only the client is closed. A store is never opened by a guess.

## What runs with a store open

- **Credential clients** (`Kind::Cred`):
  - The line runs with `bash -c` and the system's `PATH`, so no start-up file runs and no
    program planted in `~/bin` is found.
  - It runs after `walls::CRED_ENV`, which turns git's repository-controlled programs off
    through `GIT_CONFIG_COUNT`: `core.hooksPath=/dev/null`, `core.fsmonitor=false`,
    `core.sshCommand=ssh`, `protocol.ext.allow=never`, `core.pager=cat`, `core.editor=true`
    and `gpg.program=gpg`.
  - It also sets `GIT_ASKPASS=`, `GIT_TERMINAL_PROMPT=0` and `PYTHONNOUSERSITE=1`.
- **Agent clients and the bus programs** run with the person's login shell, as before, because
  they are often installed in the home (`~/.local/bin/claude`). Such a client gets its own home
  writable and its own login visible, and nothing else.
- **`git commit` and every other verb that is not a remote one** run as before, hooks included,
  with the stores closed.

## Who never opens one

- `ToolHost::job_walls` computes no line for a review, a round (`unattended`) or a subagent. They
  run with every store closed.
- A permission window or bypass never opens a named wall. Only `host/approved`, the person's own
  yes, does. `bypass/approved` and `grant/approved` do not.
- Background jobs go through the same `job_walls`, and never as approved.

## The classifier

S-74's `Agents` became `ask::Named`, one per row, built by `HostRules::with_control`. A command
names a row in every spelling S-74 knows:

- `~`, `~user`, `$HOME`, `${HOME…}`, the absolute path, `$PWD`, `$(pwd)` and `/proc/self/cwd`;
- after `..`, and by a glob.

The bare name (`.ssh` with the home as the folder) counts only for the hidden rows and
`~/.agents`. A project has its own `.claude` and `.codex`, so a bare agent home is not taken as
the person's.

`classify_command` then decides, for each named row whose clients the line does not open:

- a hidden row is `control`;
- a read-only row is `control` when the command writes there: a `>`, or any program but a
  reader or `cd`.

Reading an agent's settings stays free, and so does a client naming its own key. `control` here
is never the machine's alone (`machine_only` requires no named row), so bypass never answers it.

The file tools follow the same code path:

- `fs_write`, `fs_edit` and `fs_mkdir` in a row are `control`, through S-74's `classify_write`
  generalised to every row.
- `fs_read`, `fs_list` and `fs_glob` inside a hidden row are `control` (`classify_read`).
- `fs_glob` over the home is given the hidden rows to skip.

The person's yes to such a command runs it with what it names open, through `named_walls`.

## The hint

A failed foreground command gets one fixed line (`walls::HINT`) when either:

- its line was not pure yet names a credential client, or
- its output reads like a closed store, such as `Permission denied (publickey)`,
  `could not read Username`, `Cannot connect to the Docker daemon`, `gh auth login`,
  `Unable to locate credentials` or `Failed to connect to bus`.

The line says to run that step as a command of its own. `clio-host@15` adds one line to "What
you can and cannot do" saying the same, following the prompts' versioning (a new file, the
manifest updated, and a test that `@15` is `@14` plus that line).

## Decisions taken

- **One table** for S-75 and S-76, with S-74's `~/.agents` as one of its rows. The classifier and
  the jail both read it.
- **A credential client runs from the system's folders, non-login, with git's repository
  programs off.** Otherwise a planted `~/bin/git` or a `.bashrc` would run with the stores open.
  The cost: a client installed only in the home keeps its stores closed.
- **Agent clients and the bus programs run with the login shell.** They are usually installed
  in the home, and their stores are their own.
- **`~/.gitconfig` and `~/.config/git/config` are read-only, opened by `git config` alone.** A
  credential helper planted there would otherwise run on the next push.
- **`pull` stays a client.** It can run a merge driver from `.git/config` with the stores open,
  but it is the commonest remote verb. See the open items.
- **No client for `gpg`, `pass`, the keyrings, the browsers or cargo.** Signing a commit, `pass`
  and `cargo publish` do not work from Clio. They are rarely what an agent should do unasked,
  and they each open a world of code (pinentry, the agent, build scripts).
- **Amazon Q's `q` opens `~/.aws`**, where its login lives.
- **The folders an uploading client sends are walked for links**, up to 10,000 entries. Past
  that, the line is closed.
- **ssh to this machine closes the line**, so `ssh localhost 'cat ~/.ssh/id_ed25519'` does not
  run outside the jail with the key.
- **docker:** host bind mounts and the options that reach the host close the line. Compose files
  are not read, because the daemon is root anyway (see the open items).
- **The hint is for the foreground only.** A background job's end is read through
  `job_output`, which has no command line at hand.
- **A bare agent home is not classified.** A project's `.claude` would ask for nothing that
  matters.
- **Services and MCP servers were not changed.** They already mask the whole home and `/run`.
- **No version bump.** A parallel PR would conflict. The owner bumps it when merging.

## What remains open

- **A repository's own `.git/config` with a store open:**
  - `credential.helper`, `include`, filters and merge drivers (on `pull`), `uploadpack`,
    `receivepack`, `url.<x>.insteadOf` and `core.gitProxy` can still run a program, or send
    a credential elsewhere, during `git fetch`, `pull` or `push` in a repository the model can
    write.
  - Closing them needs either reading the repository's config (racy) or running git with a
    clean config, which would break the person's own remotes.
  - A git older than 2.31 ignores `GIT_CONFIG_COUNT`.
- **What a client is asked to do:**
  - Using Docker is being root. `docker compose up` with a compose file that mounts the home,
    or an image's own instructions, reach past every wall.
  - `kubectl`, `aws`, `gcloud` and `az` act with the person's rights.
  - An agent's client may change its own settings.
- **The time between the check and the run.** A background program can swap a link, or the
  client, after the check.
- **Homes:**
  - A home that is itself a link is protected where it leads, but the link can be replaced.
  - A missing home can be created by a command. It is protected from the next command on.
- **The Systemd launcher's limits:** `/proc/<pid>/root` and `sudo` reach past every mask. This
  is known and not addressed here.
- **What does not work from Clio:**
  - ssh in the jail has no agent (the environment is cleared), so a key with a passphrase does
    not work.
  - Clients installed only in the home keep their stores closed.
  - Background jobs get no hint.
- **Coverage:**
  - The ssh-to-self check goes by name and address (`localhost`, `127.*`, `::1`, the host's
    name), not by resolving.
  - Editors' agents not in the table (Cody, VS Code's own folders) are not walled.

## Tests

Every test was verified by mutation: the code it covers was broken, the test failed, and the code
was restored.

- **`clio-toolhost/tests/walls.rs`.** It uses a temporary home with decoys, never the real one,
  and every jailed assertion runs under the launcher the machine has.
  - `a_plain_program_sees_no_store_and_changes_no_agents_settings`: a bash script, a Python
    program and a copied `cat` through `xargs`, with and without network, plus a background job.
    None of them reads a store (by path, through a link, or in a store that is itself a link),
    plants anything in an agent's home or git's config, or connects to the user bus. The rest of
    the home still reads and writes.
  - `a_line_that_is_nothing_but_its_client_opens_its_own_stores_without_asking`:
    - `gh auth status` alone sees its login, and a planted `.profile` does not run;
    - a script, a pipe or `&&` keeps the store closed;
    - `git config --global` writes;
    - the fake `claude` sees its login and writes its settings, and its skip-permissions line is
      closed;
    - no pending item is left.
  - `a_failure_behind_a_closed_store_says_what_to_do`: the hint, once.
  - `naming_a_store_or_writing_an_agents_home_waits_for_the_person`:
    - the file tools and commands that name a store wait;
    - reading an agent's settings and a client naming its own key do not;
    - `fs_glob` over the home walks past the stores;
    - the person's yes returns the decoy.
  - `neither_bypass_nor_a_window_nor_a_child_opens_a_wall`: bypass (its yes too), a window, a
    review, a round and both subagents.
  - `git_commit_keeps_its_hooks_and_a_push_with_the_stores_open_runs_none`.
  - `the_walls_cases_are_active_and_refused`: attack cases 65 and 66.
- **`walls::tests` (unit):**
  - each client opens only its own rows;
  - a store opens only for its client alone, covering paths, `cd` and ssh to this machine;
  - a read-only row inside a hidden one stays hidden;
  - a planted client or a link into a store keeps it closed;
  - the hint's conditions.
- **`clio-node/tests/host.rs`:**
  - `a_masked_file_reads_empty_and_a_mask_inside_a_shown_folder_comes_after_it` checks the
    bwrap and Systemd arguments, the login flag, a real run, and a socket that connects to
    nothing;
  - the self-test's new decoys are covered by `bwrap_for_real_…`.
- **`clio-brain` `pi::session`:** `the_host_prompt_only_adds_the_walls_line`.

## Version

None: the version stays 0.5.0 in this change.
