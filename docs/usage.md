# Using Clio on your machine

Clio runs on its own host (ADR 0006): your PC or your VPS, no public ports. This guide explains how to use it today with the `clio` command.

## Two modes

**Host mode, the factory default (since 2026-11-18).** Clio works on your real machine. What deletes, spends, sends or touches Clio's own setup needs your approval (one box at the end of the turn; a yes there lets Clio act without asking for the next 4 hours), and everything is logged. See "Host mode: Clio on your real machine" below.

**Copy mode** (`"mode": "copy"` in `config.json`, `--copy`, or `clio run --workspace DIR`). Here the rule is:

**The agent never touches your project.** `clio run` makes a COPY of the project and the agent works only on that copy. When it ends, you see the diff and decide with `clio apply` what goes back to the original. If you do not like it, nothing happens: the original stays as it was. If you apply and regret it, `clio undo` puts it back.

## The interface: type `clio`

In a terminal, `clio` with no arguments opens a full-screen interface (TUI). With Pi installed it is the chat, reopened where your last conversation ended (`clio --new` opens a new one; see "Pi, the agent harness"); with the builtin harness (`clio --harness builtin`) it is the list of runs and the screens below. Everything the commands below do can be done from it, with the same protections (`apply` and `undo` are the same code). Outside a terminal (a pipe, a script, CI), `clio` prints the help and exits with 0; `clio --help` too; an unknown command exits with 2. `clio --debug` (or `CLIO_DEBUG=1`) opens it with debug mode on (below).

**How it is meant to look.** Quiet: no boxes (only thin lines and white space), a one-line header, `clio · screen · folder` (in the chat, `clio · title`) and, on the right, what is wrong (in the warning color) and the model, the main text in your terminal's color and the secondary text dimmed. Cyan is only for selection and focus; green, yellow and red only for "good", "warning", "failure" and the diff. The only boxes are questions that wait for your answer (confirming an `apply`, a warning), and in the chat the box you type in and the sessions panel. Everything works with the keyboard (key table below; `?` shows the keys of the screen you are on). The mouse stays the terminal's, to select and copy text, unless you give it to the chat with `/mouse on`.

**Always in view: usage and context.** The screen's last line carries a few keys on the left and, on the right, the usage, dimmed, on every screen where there is a run (on the home screen, the latest one). With room, the usage shows in full:

```
↑12.4k ↓3.1k tok · $0 · 8/40 steps · 21 s/10 min   context ▕███░░░░░░░▏ 31 %   external
```

- `↑` input tokens and `↓` output tokens, as the model reports them (if the adapter does not report them, 0).
- The cost in dollars with just enough precision: `$0` if free, `$0.0015` for a fraction of a cent, `$0.12` from one cent up. Inside, everything is whole micro-USD, no floating point.
- Steps and time against their limit (`8/40`, `21 s/10 min`; the limits are those of `~/.config/clio/config.json`).
- Context: how much of the model's window its next call will take, **approximate** (four characters per token). The window comes from the model catalog (`~/.config/clio/catalog.json` or the file in `CLIO_CATALOG`; with neither, the one that ships with Clio, which knows the default model); if the model is not there, 128k is used and the context view says so. The bar turns yellow from 70 % and red from 90 %.
- The `external` marker appears only if the session read outside content (files, test output): it is what used to be the long "tainted" warning. The gateway is stricter then.

Since it shares the line with the keys, it is almost always abbreviated (words drop, the bar goes) but none of the four figures is lost; at 60 columns it is still whole. In the chat the line is shorter: `↑input ↓output · context N %`. The minimum size is 60x16; below that the screen says so instead of drawing badly.

The drawings show what each screen holds. Today the header also has a thin line under it, and the keys and the usage share the last line (keys on the left, abbreviated usage on the right).

**Home**: a status line (key, vault, doctor; only a problem gets color) and the recent runs. `/` filters the list by id, status, instruction or folder as you type (`Enter` keeps the filter and goes back to the keys, `Esc` clears it).

```
 clio · home · project                                                beam-free
────────────────────────────────────────────────────────────────────────────────

 key present · vault not created (optional)
 doctor: 12 ok · 3 warnings · 0 failures

 ─ recent ─────────────────────────────────────────────────────────────────────
 ▸ 1031-1816-5a67 pending review         1 file  fix parse_date in dates.py
   1030-0900-0b1c applied                2 files add a README
   1029-1200-ffff failed                 0 files refactor everything

 Enter open · n new · q quit · ? help    ↑12.4k ↓3.1k $0 7/40st 12s/10m ctx 31%
```

**New run** (`n`): the folder (the current one by default) and the instruction. The folder is checked as you type with the rules of `clio run` (it refuses `/`, your home folder and folders that are too big), and the screen tells you what will NOT be copied (secrets, `.git`, `node_modules`, symbolic links...) before it starts. In text fields letters are text, even where they are keys on other screens; `Alt-Enter` or `Ctrl-J` add a line to the instruction. What you paste goes through the cleaner: it cannot bring escape sequences or press `Enter`.

```
 clio · new run · project                                             beam-free
────────────────────────────────────────────────────────────────────────────────

 Folder        /home/ivan/project

 Task          fix parse_date in dates.py▏

 ✓ 12 files will be copied (3.0 MB)
 ─ NOT copied to the working copy ─────────────────────────────────────────────
 look like secrets: .env
 heavy or internal folders: .git, node_modules
 always: .git, node_modules, target, .venv, __pycache__, .env*, *.pem, *.key,
 id_rsa*, id_ed25519*, .ssh, .aws, credentials*

 Tab next field · Enter launch · Alt-Enter new line · Ctrl-U clear the field
```

**Live**: the agent runs in its own thread; you see each action the model asks for and the gateway's decision (allowed, asked or denied, with the rule and the size of the result). `Ctrl-C` (or `Esc`) cancels and ends after the current step; a second press quits at once (the run is left incomplete, the original was never touched). `j`/`k` walk the timeline.

```
 clio · live · project                                                beam-free
────────────────────────────────────────────────────────────────────────────────

 ⠋ working on the copy…   1031-1816-5a67   level Simple
 fix parse_date

 ─ timeline ───────────────────────────────────────────────────────────────────
   1  read_file  dates.py  ✓ allowed (step-8-reversib…)  1234 ch.
   8  write_file .git/hooks/pre-commit  ✗ denied (S-16)

 Ctrl-C cancel · ? help              ↑12.4k ↓3.1k $0 8/40st 21s/10m ctx 31% ext
```

With 100 columns or more the timeline is on the left, with the context (by layers) and the usage against its limits on the right; with fewer, one column, and `c` and `U` open the full views:

```
 clio · live · project                                                                                        beam-free
────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────

 ⠋ working on the copy…   1031-1816-5a67   level Simple
 fix parse_date

 ─ timeline ──────────────────────────────────────────────────────────────── │ ─ context ──────────────────────────────
   1  read_file  dates.py  ✓ allowed (step-8-reversib…)  1234 ch.            │ context ▕███░░░░░░░▏ 31 %
   8  write_file .git/hooks/pre-commit  ✗ denied (S-16)                      │ 39.7k of 128k tokens (approx.)
                                                                             │
                                                                             │ system          1.2k    0.9 %
                                                                             │ objective        120    0.0 %
                                                                             │ tools            410    0.3 %
                                                                             │ memory             0    0.0 %
                                                                             │ conversation    3.1k    2.4 %
                                                                             │ external       34.9k   27.2 %
                                                                             │     dates.py          2k external
                                                                             │     big.json       32.9k external
                                                                             │ outputs            0    0.0 %
                                                                             │
                                                                             │ ─ usage ────────────────────────────────
                                                                             │ ↑12.4k ↓3.1k tokens · $0
                                                                             │ steps   8 of 40  ▕██░░░░░░░░▏ 20 %
                                                                             │ time    21 s of 10 min  ▕░░░░░░░░░░▏ 3 %
                                                                             │ cost    $0 of $0.10  ▕░░░░░░░░░░▏ 0 %
                                                                             │
                                                                             │
                                                                             │

 Ctrl-C cancel · Tab timeline / debug · c context · U usage · ? help  ↑12.4k ↓3.1k · $0 · 8/40st · 21s/10m  ctx 31% ext
```

**Context** (`c`, live, in the detail and on the home screen with the last run): a view in LAYERS of what is in the model's context, each layer with its approximate size in tokens and its share of the window: the system prompt (with its version, for example `clio-agent@3`), the goal (your request, which is the contract's goal), the tool descriptions, the conversation (the model's answers and protocol notices, in turns), the OUTSIDE content (each file read, with its sanitized name, its tokens and the outside mark) and the output of the other tools. The layers add up to what the model gets in its next call. It warns quietly past 70 % and 90 %. On a finished run it shows the context of its last model call (without its last answer).

```
 clio · context                                                       beam-free
────────────────────────────────────────────────────────────────────────────────

 context ▕███████░░░░░░░░░░░░░░░░░▏ 31 %  39.7k of 128k tokens (approx.)

 system prompt            clio-agent@3      1.2k    0.9 %  ▕░░░░░░░░░░░░▏
 objective / contract     your request       120    0.0 %  ▕░░░░░░░░░░░░▏
 tools                    descriptions       410    0.3 %  ▕░░░░░░░░░░░░▏
 memory                   0 memories           0    0.0 %  ▕░░░░░░░░░░░░▏
 conversation             7 turns           3.1k    2.4 %  ▕░░░░░░░░░░░░▏
 external                 2 reads          34.9k   27.2 %  ▕███░░░░░░░░░▏
     dates.py                                     2k external
     big.json                                  32.9k external
 tool outputs                                  0    0.0 %  ▕░░░░░░░░░░░░▏

 Approx.: four characters per token. The layers add up to what the model sees.

 U see the usage · ? help            ↑12.4k ↓3.1k $0 8/40st 21s/10m ctx 31% ext
```

**Usage** (`U`): a compact table per step (model call): input and output tokens, cost, model time and the running totals; below, the total against the run's step, time and cost limits.

```
 clio · usage                                                         beam-free
────────────────────────────────────────────────────────────────────────────────

 usage ↑12.4k ↓3.1k tokens · $0

 step    input   output       cost     model   cum.tok    cum.cost
    1      10k       2k         $0     1.2 s       12k          $0
    2     2.4k     1.1k         $0     2.1 s     15.5k          $0

 ─ against the run limit ──────────────────────────────────────────────────────
 steps   8 of 40  ▕██░░░░░░░░▏ 20 %
 time    21 s of 10 min  ▕░░░░░░░░░░▏ 3 %
 cost    $0 of $0.10  ▕░░░░░░░░░░▏ 0 %

 Tokens and cost are reported by the model; the context is approximate.

 c see the context · ? help          ↑12.4k ↓3.1k $0 8/40st 21s/10m ctx 31% ext
```

**Run detail**: summary, the agent's final message (wrapped), changed files (`M` modified, `A` new, `D` deleted) and, with `Enter` or `d`, the diff of the chosen file: `+` in green, `-` in red, `@@` dimmed. `a` applies and `u` undoes. Before applying, the confirmation box lists the files that will be written (`M` changed, `+` new, `-` deleted) and warns if the agent read outside content or if the run kept no record of that; the full diff is on this screen (`Enter` or `d`). In a chat session the warning about outside content comes from the tool service (which is what saw it: Pi could say it read nothing) and stays in the run folder (`toolhost/tainted.json`, only if it happened); if the service ran and left nothing, the box does not warn, and if there is no record at all it says so. Only a lone `y` or `s` confirms (Ctrl-S or Alt-Y do not count), and if the plan takes a while and you already changed screens, the box does not appear: you are told, and you ask again. It is the same code as `clio apply` and `clio undo`.

```
 clio · detail · project                                              beam-free
────────────────────────────────────────────────────────────────────────────────

 pending review · 20261031-181646-5a67
 folder  /home/ivan/project
 request fix parse_date
 level Simple · 7 steps · 12.3 s · finished · 0 denied

 ─ agent message ──────────────────────────────────────────────────────────────
 Fixed parse_date.
 Could not run the tests.

 ─ changed files (2) ──────────────────────────────────────────────────────────
 ▸ M dates.py
   A docs/new.md

 Enter d see the diff · ? help           ↑12.4k ↓3.1k $0 7/40st 12s/10m ctx 31%
```

```
 clio · detail · project                                              beam-free
────────────────────────────────────────────────────────────────────────────────

 M dates.py  · 1/2 · line 1/7
 --- a/dates.py
 +++ b/dates.py
 @@ -1,3 +1,3 @@
  def parse(s):
 -    return int(s)
 +    return None
  x = 1

 ] next file · a apply · ? help          ↑12.4k ↓3.1k $0 7/40st 12s/10m ctx 31%
```

**Doctor** (`D`): each check with its mark (`✓` good, `!` warning, `✗` failure) and, under each warning or failure, a line with what to do (`r` repeats). Each check shows once: the permission and port checks `clio` makes itself are not repeated by `infra/local/check-host.sh`.

```
 clio · doctor                                                        beam-free
────────────────────────────────────────────────────────────────────────────────

 doctor: 1 ok · 1 warning · 1 failure

 ✓  /home/ivan/.config/clio is private (700)
 !  connector opencode: the key is missing from the vault
    → With the vault running: clio connect opencode
 ✗  /x/y tiene modo 755, debe ser 700 (S-23)
    → chmod 700 /x/y

 r repeat · Esc back · ? help
```

**Help** (`?`): the keys of the screen you are on, in one short view.

```
 clio · help                                                          beam-free
────────────────────────────────────────────────────────────────────────────────

 keys · Clio works on a copy; you decide what goes back to the original

 j       down              /       filter
 k       up                q       quit
 g       first             c       context
 G       last              U       usage
 Enter   open              m       memory
 n       new               r       refresh
 d       diff              D       doctor
 a       apply             Esc     clear the filter
 u       undo              ?       help

 Ctrl-D  debug mode        Ctrl-L  redraw

 Esc ? close the help             ↑12.4k ↓3.1k · $0 · 7/40st · 12s/10m  ctx 31%
```

### Keys

The same on every screen: a key means the same thing wherever it exists (`c` context, `U` usage, `?` help, `r` refresh, `d` diff, `a` apply, `u` undo, `g`/`G` start and end, `j`/`k` move, `m` memory, `p` approve a memory; the only exception is the Memory screen, where `a` archives, because there are no runs to apply). A test walks each screen's table and fails if a key is repeated or means different things. `Esc` or `q` go back (on the home screen `q` quits; `Esc` only clears the filter); `Ctrl-C` cancels the live run (the second time it quits) and on the other screens does the same as `Esc`. Pages with `Ctrl-F`/`Ctrl-B` (or `PgDn`/`PgUp`): `Ctrl-D` cannot be "half a page" because it is debug mode. To avoid clashes: `U` (capital) opens usage and `u` undoes.

- **Home**: `j` `↓` down · `k` `↑` up · `g` `Home` first · `G` `End` last · `Enter` open · `n` new · `d` diff · `a` apply · `u` undo · `/` filter · `q` `Ctrl-C` quit · `c` context · `U` usage · `m` memory · `r` refresh · `D` doctor · `Esc` clear the filter · `?` help.
- **Home, typing the filter**: `Enter` keep the filter · `Esc` clear the filter · `↓` down · `↑` up · `Ctrl-C` clear the filter.
- **New run**: `Tab` `Shift-Tab` next field · `Enter` launch · `Alt-Enter` `Ctrl-J` new line · `Ctrl-U` clear the field · `F1` help · `Esc` `Ctrl-C` back.
- **Live, running**: `Esc` `Ctrl-C` cancel · `Tab` timeline / debug · `c` context · `U` usage · `j` `↓` newer · `k` `↑` older · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `?` help.
- **Live, finished**: `Enter` see the run · `Esc` `q` `Ctrl-C` home · `Tab` timeline / debug · `c` context · `U` usage · `j` `↓` newer · `k` `↑` older · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `?` help.
- **Detail, list of files**: `j` `↓` down · `k` `↑` up · `g` `Home` first · `G` `End` last · `Enter` `l` `→` `d` see the diff · `a` apply · `u` undo · `c` context · `U` usage · `Tab` pick the memory used · `+` the picked memory was useful · `-` the picked memory was wrong · `?` help · `Esc` `q` `Ctrl-C` back.
- **Detail, diff**: `j` `↓` down · `k` `↑` up · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `]` next file · `[` previous file · `a` apply · `u` undo · `c` context · `U` usage · `?` help · `Esc` `q` `h` `←` `Ctrl-C` to the list.
- **Doctor**: `j` `↓` down · `k` `↑` up · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `r` repeat · `?` help · `Esc` `q` `Ctrl-C` back.
- **Context**: `j` `↓` down · `k` `↑` up · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `U` see the usage · `?` help · `Esc` `q` `Ctrl-C` `c` back.
- **Usage**: `j` `↓` down · `k` `↑` up · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `c` see the context · `?` help · `Esc` `q` `Ctrl-C` `U` back.
- **Help**: `j` `↓` down · `k` `↑` up · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `Esc` `q` `?` `Enter` `Ctrl-C` close the help.
- **Memory**: `j` `↓` down · `k` `↑` up · `g` `Home` first · `G` `End` last · `Enter` see it all · `p` approve · `a` archive · `y` copy · `/` search · `r` refresh · `?` help · `Esc` `q` `Ctrl-C` back.
- **Memory, typing the search**: `Enter` keep the search · `Esc` clear the search · `↓` down · `↑` up · `Ctrl-C` clear the search.
- **Chat, with the sessions open (Ctrl-S)**: `j` `↓` down · `k` `↑` up · `g` `Home` first · `G` `End` last · `Enter` open · `n` new · `x` delete · `Esc` `Ctrl-S` `F2` close · `r` refresh · `D` doctor · `m` memory · `q` `Ctrl-C` quit · `?` help.
- **Chat, idle**: `Enter` send · `Ctrl-S` `F2` sessions · `Alt-Enter` `Ctrl-J` new line · `←` previous character · `→` next character · `Home` `Ctrl-A` start of the line · `End` `Ctrl-E` end of the line · `Ctrl-←` `Alt-←` previous word · `Ctrl-→` `Alt-→` next word · `Backspace` delete backwards · `Del` delete forwards · `Ctrl-W` `Alt-Backspace` delete the word · `Ctrl-U` delete to the start of the line · `↑` line above, or scroll the conversation up · `↓` line below, or scroll the conversation down · `Ctrl-B` `PgUp` page up · `Ctrl-F` `PgDn` page down · `Ctrl-Home` the start of the conversation · `Ctrl-End` the latest · `Ctrl-C` quit · `F1` help.
- **Chat, the agent works**: `Esc` `Ctrl-C` stops (`Ctrl-C` again quits) · `Enter` steers the agent · `Ctrl-S` `F2` sessions · `Alt-Enter` `Ctrl-J` new line · `←` previous character · `→` next character · `Home` `Ctrl-A` start of the line · `End` `Ctrl-E` end of the line · `Ctrl-←` `Alt-←` previous word · `Ctrl-→` `Alt-→` next word · `Backspace` delete backwards · `Del` delete forwards · `Ctrl-W` `Alt-Backspace` delete the word · `Ctrl-U` delete to the start of the line · `↑` line above, or scroll the conversation up · `↓` line below, or scroll the conversation down · `Ctrl-B` `PgUp` page up · `Ctrl-F` `PgDn` page down · `Ctrl-Home` the start of the conversation · `Ctrl-End` the latest · `F1` help.
- **Chat, always**: `Ctrl-C` twice quits, whatever runs or is open (a turn, a compaction, what you approved, a review, a box, a question, a list), stopping it as closing does; `/quit` too. A key the terminal sends for `Shift`, `Ctrl` or `Alt` alone, or for a key let go, is no key: it answers, disarms and closes nothing, and does not count against a question's quiet second.
- **On every screen**: `Ctrl-D` debug mode · `Ctrl-L` redraw.

While it runs, Clio asks the terminal for its plain key codes (kitty's keyboard protocol off, on the screen it was on and on its own) and gives them back when it ends, also after a crash or a signal; it also turns xterm's "modifyOtherKeys" off (another program may have left it on, and then `Ctrl-C`, capitals and `_` would not arrive), and leaves it off at exit, which is the terminal's default. Where a terminal still reports `Shift` apart, `Shift` with a letter types its capital; in the secret field, the "Other…" field and an answer's digit, `Shift` with anything else is refused with a note (the character it meant depends on the keyboard): paste the value instead.

### Debug mode

It turns on with `--debug` (in `clio` and in `clio run`), with `CLIO_DEBUG=1` (any value but empty, `0`, `false` or `no`) or with `Ctrl-D` inside the interface, which toggles it at any time, also with a run going (what happened before you turned it on has no detail). With debug on, `Tab` on the live screen switches between the timeline and the debug view, which shows for each model call:

- how the context was built (the layers and each outside file), turn by turn;
- the request (only the messages new in that call) and the answer, cut with a note of how much is missing; long lines continue on the next line instead of being cut;
- what the memory recalled at the start (id, text and vector scores, or that it abstained) and what it proposed to save at the close (one line per memory, with its state), as a memory block;
- each gateway decision with its tool, arguments, rule and full reason, and how long it took;
- the model time, tokens, cost and retries (when the adapter failed and Clio called again), and protocol errors.

```
 clio · live · project                                        debug   beam-free
────────────────────────────────────────────────────────────────────────────────

 ⠋ working on the copy…   1031-1816-5a67   level Simple
 fix parse_date

 ─ debug: what the model saw and said (sanitised) ─────────────────────────────
 call 2
   context 39.7k tok approx.: system 1.2k (clio-agent@3) · objective 120 · tool
       s 410 · conversation 3.1k (7 turns) · external 34.9k (2) · outputs 0 · m
       emory 0 (0)
     external: dates.py 2k
     external: big.json 32.9k
   model 2.1 s · input 2.4k · output 1.1k tok · $0
   request: 3 messages in all (4200 ch.), new in this call: 1
     [user] Result of read_file:
       hola
   reply: 21 ch.
     {"tool":"write_file"}
   gateway #1 write_file {"path":".git/hooks/x"}
     denied · rule S-16 · 3 ms
     reason: the path is protected (S-16)

 Ctrl-C cancel · ? help              ↑12.4k ↓3.1k $0 8/40st 21s/10m ctx 31% ext
```

With debug on, a `debug.log` is also written in the run folder (`~/.local/share/clio/runs/<id>/debug.log`): JSON per line (`started`, `context`, `memory`, `usage`, `exchange`, `step`, `decision`, `tool`, `protocol_error`, `final`), mode 0600 from birth, with every long text cut at 8000 characters and a note of how much is missing. In `clio run --debug` the same lines (sanitized) go to standard error with the prefix `[debug]`, and the log's path is printed at the start. **Without debug none of this is written.**

**What it guarantees.** Everything in the view, in `debug.log` and on standard error goes first through the secret scrubber (`Scrubber`), which knows the model key (the connector's variable or the vault's) and recognizes a known secret in clear, in base64 and URL-encoded; then through terminal sanitizing (escape sequences, `\r`, bidirectional characters). The API key never shows. The tests check it with decoys (`clio-canaries`) in the model's answer, in a file read, in a file name, in the gateway's arguments and reason and in a protocol error: neither the screen (at four sizes), nor `debug.log`, nor standard error, nor `summary.json`, `transcript.json` or `usage.json` contain them. Limit: the scrubber only knows the secrets it was taught (the model key and what the vault hands it) and the providers' key patterns; a secret Clio does not know and that has no key shape cannot be hidden.

### What each run keeps

Besides the usual, each run keeps `usage.json` (0600): the limits it ran with, the model's window, the usage of each call (tokens, cost in micro-USD, time, retries) and the context's layers before the last call, with the names of the files read. It feeds the bar and the `c` and `U` views of a finished run; a run older than this feature does not have it and the view says so (the totals come from its `summary.json`). A run's final message, report and transcript also go through the secret scrubber before they are saved.

**Colors and symbols**: by default, the 8 basic ANSI colors plus bold and dim, so your terminal's palette picks the shades (it works on dark and light themes and without 24 bits). If the terminal declares 24-bit color (`COLORTERM=truecolor` or `24bit`), it uses a palette of its own (violet accent, soft green, amber and red, grey for what is dimmed) only for text: it never changes the background or the normal text color. With `NO_COLOR=1` or `TERM=dumb` there is no color; if the terminal does not declare UTF-8, or it is the Linux console, the symbols (`·` `▸` `✓` `✗` `↑` `↓` `─` and the bars; in the chat also `▌` `•` `›` `●` and the rounded borders) become ASCII (`-` `>` `+` `x` `^` `v` `-` `[###...]`; `|` `*` `>` `*` and `+-|`).

**Screen safety.** What comes from files, the model, file names, diffs, the instruction, the answers and reasons of debug mode or the filter can carry escape sequences (change the window title, clear the screen, fake another screen), a `\r` that overwrites a line, or bidirectional characters (like U+202E) that reverse the text. Before drawing or printing any outside text, Clio removes whole escape sequences (CSI, OSC, DCS, APC...) and replaces the other control characters, line separators and invisible or bidirectional characters with `�`; a tab becomes four spaces. This holds on every screen (home, new run, live, context, usage, debug, detail, doctor, help) and for what `run`, `runs`, `show`, `diff`, `apply` and `doctor` print in a terminal. The run's files keep the text as it came (minus secrets). Limit: zero-width joiners (ZWJ) are replaced too, so a composed emoji can look like several. If Clio falls over with an error, the terminal is restored before the message shows (it leaves the alternate screen and raw mode); it cannot do so if it is killed with `kill -9` or a signal; then type `reset`. `Ctrl-L` redraws the whole screen (clears the terminal and repaints every cell).

## Memory: what Clio remembers

Clio remembers across runs, **all on your machine**: one file, `~/.local/share/clio/memory.db` (0600, created by `clio init` or the first run, and nowhere else). No network. The embedding model is optional and is the one clio-memory picks: **Granite Embedding 311M** (`clio-memory setup model`, which also fetches what it needs to run, on the CPU), otherwise the static `potion-multilingual-128M` (`clio-memory setup model potion` or `just setup-model-linux`). With either, search also uses meaning; if Granite does not load, it uses potion; with neither, it searches by text only and the interface says so in a quiet line. It never fails because of that. (Loading the model takes seconds and memory. If the `clio-memory` binary is on your `PATH`, Clio does not load it: it asks the clio-memory daemon for vectors, the process that holds the only copy of the model for all your agents, and starts it the first time it needs a vector. Without the binary, with `CLIO_MEMORY_NO_DAEMON=1`, or if the daemon does not have the same model, Clio loads it, once per run and only if there is already something to recall; Pi's tool service and the sleep always load it themselves. `clio` starts with ONNX Runtime's telemetry off: if `ORT_DISABLE_TELEMETRY=1` is not set, it runs itself again with it.)

**What it remembers.**

- **What you tell it with `clio memory remember "text"`**: a fact of yours (origin `persona`), active at once. With `--block identidad|reglas|preferencias|vigilancia` it also goes into one of the primordial memories (below).
- **One line per run** (episodic): what you asked, which files it changed, how it ended and an excerpt of what it said. It stays **active**, with origin `agente` if the run never read outside content (no files, no test output) or `externo` if it did.
- **What the model asks to save** in its final message with a line that starts with `RECORDAR:` (a fact) or `PROCEDIMIENTO:` (a "how it is done"). Up to 5 per run; the rest is dropped. Active, always with origin `externo`: the loop of `clio run` does not show them (the Pi chat does, as data).

**The primordial memories: who Clio is and who you are.** They are four blocks of the memory that only you set: `identidad` (who Clio is and how she speaks), `reglas` (how she works and her limits), `preferencias` (who you are) and `vigilancia` (what she watches, one thing per entry). Clio ships with two entries of `identidad` and two of `reglas`, written the first time you open a Pi session with a memory that has neither. The first three go whole to the model in the first message of every session (also when resuming it and after a compaction), under "who you are and how you work (set by the person; follow it)" and "who the person is (as they said)": not as loose data, but as what the model follows; `vigilancia` is not sent every time, Clio reads it when she needs it. The first time you talk to Clio and the memory does not know who you are, your first message carries the request to introduce herself: in her answer she introduces herself and asks you four questions (your name and how you want to be addressed, what you do and what is on this machine, what you want her to watch, what counts as important enough to tell you); if you close without answering she asks again in the next session, and `clio run` never does it. **You change them by talking to Clio** ("from now on address me formally", "stop watching the disk"): she uses `memory_block_edit`, and saves it as said by you. In a **clean** session it is written at once; if the session already read outside content (a file, command output, a memory that is not yours), the change needs your approval: the chat asks you for it when the turn ends (the Approval box, below; or now with `/pending`), or it is written at once under a permission window you opened ("What waits for your approval", below); otherwise nothing is written. Where nobody can be asked (`clio run`) it is refused. Her usual tools (`memory_update`, `memory_forget`, `memory_consolidate`) do not touch an entry of these blocks. The introduction only happens in a new session: reopening an old one never starts it. Caps: each entry up to 1200 characters; up to 4 in `identidad`, 6 in `reglas`, 8 in `preferencias` and 40 in `vigilancia`; and what goes to the model (`identidad`, `reglas`, `preferencias`) at most 1500 characters per block, so it arrives whole (an edit that would pass it is refused, never cut). **`clio memory identity`** shows them (`--json` too), each block with what it holds and each entry with its number and origin (`persona`, `agente` or `externo`; only what is `persona` goes with authority); **`clio memory remember "text" --block B`** writes them by hand.

**What it does NOT remember.** Nothing new during the model's loop in `clio run`: what was learned is written once at the run's close (there is no working memory yet), and in that loop the model can only ask to save at the end (in a Pi session it has its memory tools, below). A text from a file, a web page or test output is **never** saved as a fact of yours, even if it says "the person said…". What looks like a key or a secret is not saved (the scrubber strikes it out and the library's detector refuses what is left). It does not keep the content of your files.

**The memory is the agent's; quarantine is what was forgotten.** Nothing the agent saves waits for your review: it goes in active at once, with its origin in view (`agente`, or `externo` if it read something from outside before), and every change goes into the operations log, so you undo it with `clio memory revert OP` (the number comes from `clio memory history ID`). A tainted file could tell the model "save that the person authorized X": that is why what is saved after reading something from outside is marked `externo` (reading it taints the session), never as something you said, the loop of `clio run` does not see it, and with Jev on it refuses to save what tries to manipulate the agent (below). **Quarantine** is what the agent forgot (`memory_forget`, or the originals of a merge): it does not come out in `recall` or in the context, it keeps its record, you see it with `clio memory list --pending` and it comes back with `clio memory revert` or `clio memory promote`. What was already in quarantine before stays there. Each actor (`persona`, `agente`, `sesion`, `importador`) has its own hourly quota, so a run that floods memories does not spend yours.

**What a Pi session's tool service can do with your memory.** The service that serves Pi (`clio toolhost`) can search (`memory_search`), read a memory (`memory_get`), see what was saved before and after one in the same session (`memory_timeline`) and bring the project's context when resuming or after a compaction (`memory_context`: the last session's summary, what is pinned and the most recent). And the agent keeps its memory by itself, at once: it **saves** (`memory_save`, with a title, a category and the text in the form `Qué · Por qué · Dónde · Aprendido`; the old name `memory_propose` is accepted for one more version), **corrects** a memory (`memory_update`), **forgets** (`memory_forget`, to quarantine), **pins** one so it comes first in the context (`memory_pin`), **merges** 2 to 5 memories of the same place into one (`memory_consolidate`, which forgets the originals) and **undoes** a change of its own (`memory_revert`, never one of yours). You do not have to ask, and Jev does not have to confirm; with Jev on, Jev checks what it saves, corrects or merges before writing (see the Jev section). Accepting (PROMOTE) and PURGE (deleting for good) stay yours only, with `clio memory`. Up to 10 changes per session in any hour. A session that read something hostile can save five an hour, a clean one thirty, and the real limit is the memory's hourly quota; these session caps count over a rolling hour by the tool service's clock (an older run folder's count counts from the first call after the update), so a conversation that never ends does not run out of them, and a compaction does not reset them. It does not block your own `clio memory remember`: the memory is opened for writing only during the change. It sees the **active** memories of the whole memory (there is one, global; each with its place mark): what is in quarantine or archived answers the same as what does not exist. Each session has a hard cap of 20 calls and 24 KiB of returned text in a rolling hour; the cap survives a restart of the service. What comes back arrives wrapped as data, and if it is not something you said (origin `agente` or `externo`) the session is marked as tainted. The `clio-guard` extension declares them to Pi (`memory_search`, `memory_get`, `memory_timeline`, `memory_context`, `memory_save`, `memory_session_summary`, `memory_update`, `memory_consolidate`, `memory_forget`, `memory_revert`, `memory_pin`, `memory_block` and `memory_block_edit`, next to the six file tools and `ask_person`, `request_secret` and `todo_write`: twenty-two in all, twenty-four in host mode with `job_output` and `job_stop`; the prompts are `clio-agent@22` in copy mode and `clio-host@6` in host mode); the service offers the memory tools only if it starts with `--memory-data`.

**Project, personal and lessons.** Your memory is **one and global**: any session, in any folder, reads everything active. What changes is each memory's **mark**, which says which place it belongs to: the **personal** part (who you are, your preferences), the **project** (decisions, architecture, configuration; in that project's session it comes first and unnamed, from another it shows as "project X") and the lessons. What is new is saved in the session's project; Clio recognizes it from the session's folder: the `CLIO_PROJECT` variable if you set it; otherwise a `.clio-project` file in the folder or higher up inside the repository (`clio memory project set NAME` creates it; `clio memory project` says which it is and where it comes from); otherwise the name of the repository of git's `origin` remote, the repository's root folder or the folder itself. The model cannot choose it, and `clio apply` never applies a `.clio-project` a session wrote. A detected project is remembered per repository (in `projects.json`, next to the memory): if one day another is detected (you changed the remote, or cloned another repository at the same path), Clio uses the current one, so as not to mix two repositories in one memory, and tells you (`clio memory project`, and the MCP client's log) with the command to join them if they are the same (`clio memory project set PREVIOUS-NAME`). **Lessons** (mistakes and their cause, patterns, traps) are marked as a lesson of their project, to learn from what happened elsewhere. Where a new memory goes is decided by Jev from its category and its text; without Jev, the rule: `profile` (who you are: name, where you live, what you do) and `preference` (how you want answers, writing and work) go to the personal part, `bugfix`, `pattern` and `learning` to the lessons, the rest to the project. What you already had stays in the personal part.

**A summary of each session.** Like Engram, Clio keeps a summary of what was done in a Pi session (goal, what was done and checked, what was found, next steps, files) to resume it later: it is the project's episode, titled with the session's date, and it is the first thing `memory_context` brings. The model writes it with `memory_session_summary` and it is saved at once. Clio asks for it at two moments: when Pi compacts the conversation (in the next message) and when you close a session in which you wrote at least four messages (a last short turn, up to four steps and 30 seconds, with no words of yours; in that turn the service only lets it save the summary: no files, no commands, no memory changes with what you said before; if you leave the TUI it tells you it is closing the session, and a session that is closing is not reopened until it ends). Without a project it goes to the personal part. If the model is in the middle of a turn when you leave, there is no summary. Clio's requests are turned off with `"memory": {"session_summary": false}` in `~/.config/clio/config.json`; the model can still save a summary by itself after a long piece of work. Your messages are not kept apart: Pi already has them. You see everything: `clio memory` and the TUI's memory screen list, search, archive, revert and purge any project's (marked "project X"), as the model of any session also sees them.

The point-by-point comparison with Engram is in `docs/specs/2026-11-04-engram-comparacion.md`.

**Your memory in other agents (MCP).** The recommended way is [clio-memory](https://github.com/i-montes/clio-memory), Clio's memory engine in its own repository: `clio-memory setup claude-code` makes it Claude Code's main memory (an MCP server with its 12 tools, including correct, forget, pin, merge and undo, and a skill with its safety measures), over the same file Clio uses. Clio also brings its own, narrower server: `clio memory mcp` is an MCP server (JSON-RPC over standard input and output) for another agent to read and save in your memory through Clio's tool service (for example, `claude mcp add clio-memory -- clio memory mcp`; do not register it under the same name if you already use `clio-memory setup`). It offers `memory_search`, `memory_get`, `memory_timeline`, `memory_context`, `memory_save` (it also accepts `memory_propose`), `memory_session_summary` and `memory_current_project`; correct, forget, pin, merge and undo are in clio-memory's server. Accepting, archiving and purging are yours, with `clio memory`. It is the same service Pi uses, with the same controls: what it returns is marked as data, each process has the cap of 20 reads and 24 KiB in a rolling hour (it starts again when the process restarts), what it saves is the agent's and stays active marked `externo` (nobody knows what that agent read before): five an hour per process; the session is named after the first client that introduces itself (`mcp:claude-code`). It leaves no folders between runs: it uses a temporary one it deletes on exit. The project is the one of the folder where the client starts it. It uses the vault's filter if the vault is running.

**Bringing a memory up to date.** When a memory is out of date or wrong, the model uses `memory_update` with its number and the new text, instead of saving another one on the same thing: if you tell it "my name is not Iván Ruiz but Iván Montes", it corrects that memory. It replaces it at once, whoever's it is (also one of yours or a procedure), and the old one stays in the log: `clio memory revert` brings it back. It counts in the same cap as saving and passes the same controls. If the model gives no title or category, the old ones stay.

**Copying a memory.** On the memory screen, `y` copies the chosen memory to the clipboard (through the terminal's OSC 52 sequence, already cleaned; most terminals accept it).

**Nearby memories.** Each memory the model opens with `memory_get` ends with a line of up to 5 nearby memories (number and beginning, marked if they are `externo`). Each search result ends with 3 numbers. The model opens the ones that help and goes on until it has what it needs.
- They are worked out when read (by meaning, or by text without a vector model) and not stored, so no memory is left loose.
- If a nearby one is not yours, reading it taints the session like any memory.
- When saving something almost the same as an existing memory, the answer says so with its number, so the agent corrects instead of duplicating.
- When correcting or merging memories, their relations move to the new memory (except "contradicts").

**Relations between memories.** When the agent saves something new, Jev compares it with its 3 nearest memories and notes only the contradictions it is sure of (at least 0.9 confidence); it is a note, it changes neither. It was measured this way: Jev found the test's 3 contradictions at 0.95 or more and its only false one at 0.77, and got the other kinds ("compatible", "concrete case", "related") right 10 % of the time or less (`bench/relations/results.md` in clio-memory). For the rest, the model already sees each memory's nearby ones. The model's search shows it on each memory's line ("contradicts m12") only if the other memory is also readable and still active.

**Updating clio-memory.** `clio memory update` (the same as `clio-memory update`) installs the latest version of the `clio-memory` binary, the one the other agents use, with the installer's same checks, and updates its skill without disconnecting anything. Clio's memory is compiled into Clio: it moves up when the commit pinned in `crates/clio-brain/Cargo.toml` moves and Clio is rebuilt (`just install`).

**The memory's sleep.** At night, if nobody uses it, the memory reviews itself (clio-memory's same sleep phase; S-66). `clio memory setup sleep` installs a user timer that runs `clio memory sleep` every half hour between 23:00 and 05:30 (`--remove` removes it); it runs at most once a night, only if the memory has been still for 30 minutes, and stops if you use it. Each night it checks the file and copies it (in `~/.local/share/clio/sleep/`, the last 3), sends to quarantine what has a secret or looks like an attack, keeps one of two almost identical memories, **merges near-duplicates with the model you configured** (`model` in `~/.config/clio/config.json`, whichever connector; with a cap of 60,000 tokens a night and, if Jev is on, its yes for each original), notes contradictions "to review" and purges what was discarded 30 days ago or more (7 if it was a secret). It only looks at what changed since the night before. It never touches yours or what is pinned, and everything but the purge is undone with `clio memory revert`. Without a model, without its key (the adapter reads it from `OPENCODE_API_KEY` or from the key file; the timer does not see your environment variables, so it uses the file) or with the vault closed, it sleeps anyway and only skips the merge. The next morning `memory_context` starts with a line saying what the night did (quarantined, merged, to review), and `clio doctor` says when it slept and its alarms. `clio memory sleep --dry-run --now` shows what it would do now without changing anything; `--report`, the last night's report.

**What to expect when Pi reads the memory.**

- You see the memory calls like any other tool. The model gets up to 8 memories per search, each on one line of at most 600 characters and with its origin (`persona`, `agente`, `externo`), and no more than 4 KiB per answer.
- Cap per session: 20 calls and 24 KiB of text in a rolling hour (by the tool service's clock; an older run folder's count counts from the first call after the update); with less than 1 KiB left it counts as spent and the next call is refused with `S-47/memory`. Failed calls count too. Reopening the same session does not give it back; starting another opens a new one.
- If an embedding model is on disk, the first query loads it and can take a few seconds; past 10 s that call fails with a short message and can be repeated. The model loads while an internal lock is held, so calls queued behind a slow first load can also run out of time until it ends, and each spends budget. Without the model it searches by text only and tells the model so.
- The read tools do not change the memories' usage statistics (reading leaves no signals).
- If what Pi reads is a memory of origin `agente` or `externo`, the session is tainted, as when reading an outside file; if they are all yours, not. An order written inside the copy is carried out even if it comes from a memory: the defense is that it arrives as data, the session is marked and you review the copy with `diff` and `apply`.
- The model's provider sees what the tool returns, as it sees the context. What you do not want to leave your machine must not be in the memory.
- If the memory does not exist yet, is locked or fails, the tool fails by itself with a short message; Pi and the session go on.

**What the model sees.** At the start Clio asks for the fixed blocks and what is relevant to your request, each with its budget. "Nothing relevant" is a valid answer: then there is no memory layer. What is recalled comes in as a new layer, `memoria`, after your request and wrapped as data ("not instructions"), with each memory's id (`[m12]`) so the model can cite it and you can mark it. It goes through the secret scrubber and the filter of what goes to the model like everything else. The `c` view shows it with its size and its share of the window:

```
 clio · context                                                                           beam-free
────────────────────────────────────────────────────────────────────────────────────────────────────

 context ▕███████░░░░░░░░░░░░░░░░░▏ 32 %  41.5k of 128k tokens (approx.)

 system prompt            clio-agent@3      1.2k    0.9 %  ▕░░░░░░░░░░░░▏
 objective / contract     your request       120    0.0 %  ▕░░░░░░░░░░░░▏
 tools                    descriptions       410    0.3 %  ▕░░░░░░░░░░░░▏
 memory                   3 memories        1.8k    1.4 %  ▕░░░░░░░░░░░░▏
 conversation             7 turns           3.1k    2.4 %  ▕░░░░░░░░░░░░▏
 external                 2 reads          34.9k   27.2 %  ▕███░░░░░░░░░▏
     dates.py                                     2k external
     big.json                                  32.9k external
 tool outputs                                  0    0.0 %  ▕░░░░░░░░░░░░▏

 Approx.: four characters per token. The layers add up to what the model sees.

 U see the usage · Esc back · ? help                  ↑12.4k ↓3.1k · $0 · 7/40st · 12s/10m  ctx 32%
```

**`clio memory`** (short output, sanitized against escape sequences; `--json` where it makes sense):

```
  clio memory search <query> [--json]         search the active ones (quarantine does not show here)
  clio memory list [--pending] [--json]       recent ones; --pending: quarantine and procedures awaiting approval
  clio memory show <id> [--json]              one whole memory, with where it came from
  clio memory remember "text" [--block identidad|reglas|preferencias|vigilancia]
                                              a fact of yours (origin: persona), active at once
  clio memory identity [--json]               the primordial memories: who Clio is, how it works,
                                              who you are and what it watches (changed by talking to it)
  clio memory promote <id> [--yes]            approve a pending memory (only you; it asks first)
  clio memory archive <id> [--yes]            archive it: it is no longer remembered (it asks first)
  clio memory purge <id> [--reason "reason"] [--yes]
                                              deletes the content FOR GOOD (text, vector, history);
                                              leaves a tombstone with no content. It cannot be undone
  clio memory review [--json]                 quarantine, what was forgotten (same as list --pending)
  clio memory history <id> [--json]           every change that touched a memory (its versions)
  clio memory revert <change> [--yes]         undoes a change (its number comes from history)
  clio memory context [--json]                what a session that starts in this folder receives
  clio memory conflicts [--json]              memories Jev judged to contradict each other
  clio memory import <file>                   brings an export into an empty memory
  clio memory project [set <name>]            this folder's project and where it comes from; set fixes it (.clio-project)
  clio memory setup <agent>                   how to connect claude-code, cursor, codex, gemini or opencode over MCP
  clio memory setup sleep [--remove]          installs (or removes) the night sleep: a timer that runs
                                              `clio memory sleep` every half hour from 23:00 to 05:30
  clio memory update                          installs the latest clio-memory (the binary
                                              the other agents use) and updates its skill; Clio's own
                                              memory goes up when Clio is rebuilt
  clio memory sleep [--now] [--dry-run] [--report] [--json]
                                              one night of sleep: checks and copies, quarantines
                                              secrets and attacks, merges repeated and near-equal ones (with your
                                              model), purges what was discarded long ago. Only if nobody
                                              used the memory in 30 min and it did not sleep in 20 h (--now skips
                                              both waits; --dry-run only looks)
  clio memory projects [--json]               every place (personal, lessons, each project) and what it holds
  clio memory projects merge <from> <into>    moves a whole project to another (merge or rename; undoable)
  clio memory projects archive <project>      archives a whole project (does not delete it)
  clio memory review-due [--json]             what is past its review date (decisions, configuration...)
  clio memory reviewed <id>                   you looked at it and it is still true: the date starts again
  clio memory timeline <id> [--json]          what was saved before and after, in the same session
  clio memory pin|unpin <id>                  pins a memory (it comes first in the context) or lets it go
  clio memory feedback <id> useful|wrong      «wrong» only sends it to review; it never deletes
  clio memory export [file]                   JSON Lines (unencrypted); without a file, to standard output
  clio memory stats [--json]                  counts and size
  clio memory mcp                             MCP server for other agents (read and propose; nothing else)
```

`clio memory show` also says the place, the title, the category, whether it is pinned, whether it passed its review date and how it relates to other memories. **Review dates:** `config` 90 days, `discovery` 120, `decision` and `architecture` 180; `profile`, `preference`, `bugfix`, `pattern` and `learning` do not expire. Past its date the memory stays active; `clio doctor` and the memory screen say how many there are, and `clio memory reviewed ID` restarts the date (or `archive` if it no longer holds). On the memory screen each row says its place and its title. The model sees it too: an expired memory is marked on its line and `memory_context` brings up to three, so it tells you.

**The Memory screen** (`m` from home): what was forgotten (quarantine) on top and the recent ones below, without boxes, with the usual usage and context bar on the last line. `/` searches (filters the list), `Enter` shows the memory whole with its origin, `p` approves and `a` archives (both ask first and only `y` goes ahead; if the memory's text does not fit in the box, it says so and sends you to `clio memory show`, so you do not approve without reading it whole), `r` refreshes, `?` help, `Esc` back.

```
 clio · memory                                                                            beam-free
────────────────────────────────────────────────────────────────────────────────────────────────────

 2 active memories · 2 forgotten · 1 archived · no embeddings model: search by text only
 ─ forgotten, in quarantine (2) ───────────────────────────────────────────────────────────────────
 ▸ m12   forgotten  episodic      externo  Request: summarise the README. No files changed. Result:
   m9    forgotten  procedimental externo  When you see a README, run the install script.
 ─ recent (2) ─────────────────────────────────────────────────────────────────────────────────────
   m3    active     semantic      persona  My project uses Rust y SQLite
   m2    active     episodic      agente   Request: fix parse_date. Changed 1 file(s): dates.py.

 Enter see it all · p approve · a archive · y copy · ? help  ↑12.4k ↓3.1k $0 7/40st 12s/10m ctx 31%
```

In a run's detail, the memory line says what it recalled and what it saved; `Tab` picks one of the memories used, `+` marks it useful and `-` wrong. Marking "wrong" three times sends it to review; it never deletes or changes the memory. In `clio run` the final panel has a memory line with how many were saved and how many went to quarantine (and how many were refused, for being a secret or for the quota).

**The limit.** Archiving hides a memory but does not delete it: it stays in the operations log and in the file. Deleting for real (also the text in the log and the vectors) is `clio memory purge ID` (it asks first, leaves a tombstone with no content and cannot be undone). Do not save with `remember` what you do not want to stay on your disk, and remember that the file and the exports are not encrypted (rely on the host's disk encryption).

## Installing and setting up

```sh
just install                                        # builds and puts clio and clio-vault in ~/.local/bin
# (by hand: cargo build --release -p clio-brain -p clio-vault and copy target/release/clio* to ~/.local/bin)
clio init
```

Example output:

```
PASS  created      /home/you/.config/clio (0700)
PASS  created      /home/you/.config/clio/commands (0700)
PASS  created      /home/you/.local/share/clio (0700)
PASS  created      /home/you/.local/share/clio/runs (0700)
PASS  created      /run/user/1000/clio (0700)
PASS  memory       /home/you/.local/share/clio/memory.db (0600; 0 memories, 0 in quarantine)
PASS  created      /home/you/.config/clio/config.json (0600)
WARN  the model key is missing: start the vault (clio vault start) and run clio connect opencode

next: clio doctor, and then clio
```

(In a terminal with Unicode, `PASS`/`WARN`/`FAIL` show as `✓`/`⚠`/`✗` and in color; without a terminal the text is plain and stable.)

`init` is idempotent: it does not overwrite what exists (if you change `config.json`, it stays as you left it). It leaves the folders at 0700, never prints the key and warns if the active connector's key is missing.

**The model key and the connectors.** Clio talks to one model at a time, through one of three connectors: `opencode` (OpenCode Go), `claude` (your Claude subscription, through Claude Code) or `anthropic-api` (an Anthropic API key, billed per token). The key lives in the vault, which has to be open: `clio vault start` once per boot (on a server, `loginctl enable-linger $USER` so the vault's runtime folder is not deleted when you log out). Then, once per connector:

```sh
clio vault start
clio connect opencode        # asks for the key with no echo, stores it in the vault only for opencode.ai and tests the model
clio connect claude          # paste the token `claude setup-token` gives; it will only go to api.anthropic.com, through Claude Code
clio connect anthropic-api   # paste an Anthropic API key (sk-ant-api…); billed per token
clio connect claude --model claude-sonnet-5-5   # another model; --new-key replaces the stored key
```

`clio connect` writes `model.provider` and `model.model` in `config.json` (the active connector) and never the key. If there was a `~/.config/clio/opencode-api-key`, `clio connect opencode` offers to move it into the vault and delete it: Clio no longer reads that file. **Claude through Claude Code.** The `claude` connector does not talk to the API: each turn is done by the official Claude Code, which `clio pi install` brings with Pi (the `pi-claude-agent-sdk` package, pinned at 0.9.2; about 400 MB, once), and the one-off calls (the sleep, `clio run` with the `builtin` harness, the test in `clio connect`) use that same binary with `claude -p`. It uses your plan's quota, like Claude Code; the package warns that Anthropic could change how it bills the Agent SDK. The token (the one from `claude setup-token`) stays in the vault and reaches Claude Code only through the environment. Without the package installed, `clio connect claude` and the chat tell you to run `clio pi install`. `clio connect anthropic-api` is the other way: an API key (`sk-ant-api…`) that goes straight to `api.anthropic.com`, billed per token. The variables `OPENCODE_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN` and `ANTHROPIC_API_KEY` (the active connector's), when set, win over the vault (tests, bench, CI). The key is read by the model adapter (`clio run`) or by Clio, which hands it to Pi inside its jail (chat); Clio does not print it or keep it in any run.

**Configuration** (`~/.config/clio/config.json`): provider `opencode-go`, model `beam-free` (free), the adapter's command (`model.command`), the prompt of the builtin loop `clio-agent@2` (with the jail tested, `clio-agent@3`, which explains `run_tests`), and the limits: 40 steps, 600 seconds, 0.10 USD per task, and for the copy 5000 files and 200 MB. `init` leaves `model.command` as `[]`: the adapter is the one inside the binary, written to `~/.local/share/clio/adapters/openai_compat.py` (0600) and written again if the file differs, so the config keeps no path and keeps working when you update Clio, move the repository or install it on another machine. A `model.command` that points at an `adapters/…/openai_compat.py` (what earlier versions of `init` wrote) is also read as the bundled one: that file is not run. Another command is your own adapter and runs as it is; `CLIO_ADAPTER` swaps the bundled one for another script (to work on the adapter).

## A job from start to end (copy mode)

```sh
clio run --workspace ~/projects/my-app "fix parse_date in dates.py so it returns None on invalid input"
```

```
run 20261118-214857-c801: 2 files copied from /home/you/projects/my-app
  WARN NOT copied, it looks like a secret: .env
  - not copied (heavy or internal folders): .git
  - jail active (bwrap 0.9.0, 14 escape tests stopped): the agent can run tests
working on the copy (it can take a few minutes; up to 40 steps or 600 s)...
    0s  [0/40] task level: Simple
    0s  [1/40] read_file app.py  allowed (step-8-reversible-in-sandbox)  193 chars
    0s  [2/40] write_file app.py  allowed (step-8-reversible-in-sandbox)  179 chars
    0s  [3/40] write_file .git/x  denied (S-16)
    0s  [4/40] the model says it is done

run:      20261118-214857-c801
level:    Simple   steps: 4   time: 0.1s   end: finished
changes:  1 modified, 0 created, 0 deleted
  M app.py
note:     external: it read files from the project; check the diff carefully
agent:    Fixed total() in app.py: it no longer adds 1 too many. I could not run
          the tests; run them before applying.
memory:   memory: 1 saved, 0 in quarantine (no embedding model: memory searches by text only)

The original was NOT touched. Check:  clio diff 20261118-214857-c801   and then  clio apply 20261118-214857-c801
```

(This is the output of `clio run` without a terminal, with a fake model that follows a script; the jail line shows only where the jail is available; with a real model the steps and the text change. The "not copied" lines show only when there is something to leave out. There is one line per step instead of minutes of silence; in a terminal with color the decision shows in green, yellow or red. `clio run` exits with 0 if the agent finished and 1 if not.)

Options of `run`: `--workspace DIR` (a copy of DIR; without it, `clio run` follows the configuration's mode, and in copy mode works on the current folder), `--max-steps N`, `--allow-broad` (removes the size caps; it does not remove the refusal of `/` or of your home folder), `--host` or `--copy` (win over the configuration; `--host` with `--workspace` is an error).

```sh
clio diff 20261031-1816          # a unique prefix of the id is enough
clio apply 20261031-1816         # shows the diff and asks [y/N]
clio undo 20261031-1816          # restores from the backup
clio runs                        # the 20 most recent (--all for all)
clio show 20261031-1816          # summary, the agent's message and the next step
```

`diff` shows a unified text diff (for binaries, only that the file changed), in color in a terminal. `runs` is an aligned table:

```
RUN                   STATUS                FILES  INSTRUCTION
20261118-214857-c801  pending review               1  fix total in app.py
```

The states are: pending review (it ended and there are changes), no changes, failed (cut by a limit, the model failed or it stopped), cancelled, applied, undone and incomplete (the process died before closing). The stable words are the JSON ones below.

## In scripts: `--json`, `NO_COLOR`

`clio runs`, `clio show <id>` and `clio doctor` accept `--json` and write JSON on standard output. A run's `status` field is a stable word: `pending_review`, `no_changes`, `failed`, `cancelled`, `applied`, `undone` or `incomplete`; `ending` is how the agent ended (`Final`, `Stopped(Steps)`, `Cancelled`, ...).

```sh
clio runs --json | jq -r '.[] | select(.status=="pending_review") | .id'
clio doctor --json | jq '{ok, warn, fail}'
```

Color and symbols only show when the output is a terminal. `NO_COLOR=1` or `TERM=dumb` remove them; if the terminal does not declare UTF-8 (`LANG`/`LC_ALL`) or it is the Linux console, ASCII words are used (`PASS`, `WARN`, `FAIL`) instead of `✓ ⚠ ✗`. Without a terminal (a pipe, a file) the text is plain and stable. Errors go to standard error, short and with what can be done, and the exit code is 1 (2 for an unknown command).

## The jail and `run_tests` (copy mode, builtin loop)

With the builtin harness the agent cannot run anything by itself. The only thing it can run is `run_tests`, and only when the **jail** is available and tested. It is the tool that lets it check its work: without it, nothing checked what it wrote and the contract never closed as done.

**What the jail is.** A program running as your user could read `~/.ssh` and the browser profile and reach the network. The jail uses `bwrap` (bubblewrap) to run ONE command, with explicit arguments (never a shell), inside new namespaces and without privileges. The same jail runs Pi's `bash` in copy mode.

**What code inside can do:**

- Read and write the project's copy, which it sees as `/work` (the run's working folder and nothing else).
- Read `/usr` and what it needs to start (`/bin`, `/lib`, `/lib64`, the loader cache), all read-only.
- Write in its own `/tmp` (a 256 MB tmpfs that goes away when it ends) and in a fake HOME inside it.
- Use up to 2 GiB of virtual memory, 128 processes or threads, 120 s of CPU and 120 s of wall clock, files up to 64 MB and 512 open descriptors; it keeps the last 64 KB of each output.

**What it cannot do (and `clio doctor` checks it every time):**

- See your home folder, `~/.ssh`, `~/.config`, the browser profile or Clio's sockets: they do not exist inside. A symbolic link in the copy that points outside points at nothing.
- Open connections: there are no network interfaces (only its own empty `lo`), no DNS, no abstract sockets of the host.
- Write in `/usr`, `/etc`, the root or your real HOME. Nor in `.git`, `.claude`, `.pi`, `.agents`, `.vscode`, `.husky`, `.githooks`, `.cargo`, `.devcontainer`, `extensions` and `skills` inside the copy (they are mounted read-only, whether they exist or not: they are folders a tool loads and runs by itself).
- See or kill other processes (it has its own process namespace), or create other namespaces.
- Inherit your environment: no `OPENCODE_API_KEY`, no `SSH_AUTH_SOCK`, nothing. Its environment is `PATH=/usr/bin:/bin`, `HOME=/tmp/home`, `LANG` and `TMPDIR`.
- Outlive the time limit: when it runs out the whole process namespace is killed, daemons it left included.

**How it is offered.** Only on Linux, with `bwrap` 0.8 or newer and `prlimit` (util-linux) in `/usr/bin`, and `python3`. When a `clio run` starts, Clio starts the jail and runs a self-test (below); if anything fails, the agent does not get `run_tests` and uses the prompt `clio-agent@2` (without the tool). If it is tested, it uses `clio-agent@3`. `clio run` says which in one line. `CLIO_JAIL=off` turns it off. On macOS and other systems the tool does not exist: it says the jail is Linux-only and everything else works the same.

**The tool.** `{"runner": "unittest" | "pytest", "target": "<optional>"}`. It is not a shell: Clio builds the command. `unittest` runs `python3 -m unittest -q [target]`; `pytest` runs `python3 -m pytest -q [target]` if pytest is installed (if not, a clear error). `target` is a file or folder of the copy, only with letters, digits, `_`, `.`, `/` and `-`, not starting with `-`, without `..`, and it must exist (links that leave the copy are refused). There is no `cargo_test`: the `cargo` in `/usr/bin` is usually a rustup launcher whose tools live in your HOME, and making it work would mean mounting part of your HOME inside the jail.

**What Clio does with what the tests print.** It is outside content, like a file: it goes through the secret scrubber, reaches the model wrapped as data (`UNTRUSTED-DATA`) and marks the session as tainted. The exit code (0 = passed; "no test ran" counts as not passing) is a signal from outside the model: if the last `run_tests` came AFTER the last write and passed, the contract (level Full or above) closes as `ClosedOk`; if no tests ran, they failed or there were writes afterwards, it stays `Blocked` as before. Passing the existing tests does not prove the task is complete: review the diff.

**Snapshots.** With the jail on, the sandbox's snapshot copies the files instead of using hard links: a program that edits a file in place cannot corrupt what a cancellation restores.

**`clio doctor` and the self-test.** Inside the jail a program tries what is forbidden and checks it fails: read a decoy Clio creates in your HOME (and deletes), open a socket to a local port and an abstract socket Clio opens, write in `/usr`, `/etc`, the root and your HOME, see and kill a foreign process (Clio starts a `sleep` for that), inherit variables from your environment and create other namespaces. It shows `PASS` or `FAIL` for each (`clio doctor --json` has `jail: {available, checks}`). A `FAIL` closes the tool and makes `doctor` exit with 1. If the kernel does not allow unprivileged user namespaces (or AppArmor blocks `bwrap`), `doctor` says so with a `WARN` and the tool is off; then, on a recent Ubuntu, look at `kernel.apparmor_restrict_unprivileged_userns`.

**What the jail does not cover:** see the honest list in `docs/threat-model.md` (kernel attacks, side channels, CPU time, disk). If `clio` is killed with `kill -9` during `doctor`, a `.clio-jail-decoy-*` decoy can be left in your HOME: delete it, it holds nothing.

## What each safety step does (copy mode)

| Step | What it does | Why |
|---|---|---|
| Check the workspace | Refuses `/`, your whole `$HOME`, folders that contain Clio's, and more than 5000 files or 200 MB (unless `--allow-broad`) | So a slip does not copy half the disk |
| Copy | Leaves out `.git`, `node_modules`, `target`, `.venv`, `__pycache__`, symbolic links and files made to hold credentials (`.env*`, `*.pem`, `*.key`, `*.p12`, `*.pfx`, `*.kdbx`, `*.keystore`, `*.tfstate`, `*.tfstate.backup`, `id_rsa*`, `id_ed25519*`, `id_ecdsa*`, `id_dsa*`, `credentials*`, `.ssh`, `.aws`, `.gnupg`, `.kube`, `.docker`, `.netrc`, `.pgpass`, `.npmrc`, `.git-credentials`) and tells you. A token inside another file is not left out: it is struck out when the agent reads it (see "Striking out") | What the agent reads travels to the model's provider: it must not read secrets or git history (S-57) |
| Run folder | `~/.local/share/clio/runs/<id>/`, all 0700/0600 | Only your user reads it (S-23) |
| The agent | Triage, contract (from level 2), gateway, executor and step/time/cost limits; it sees only the copy | Rules S-05, S-13, S-16, S-47 |
| The tests (`run_tests`) | Inside the `bwrap` jail: no network, no HOME, no environment, with limits | A command without a jail would have your whole user (ADR 0006) |
| Pi (the chat) | Inside a `bwrap` jail: it does not see the vault or your files, the copy is read-only to it and its network reaches only the model's provider; if the jail does not start or fails its self-test, Pi does not start (see "Pi's jail") | Pi and its npm dependencies run as your user (ADR 0009, S-59) |
| `apply` | Checks each path, asks for confirmation, backs up what it overwrites and notes each file in a journal (`applied.json`) **before** touching it: if the process dies halfway (the terminal closes, a `kill`, a power cut), `clio undo` brings back the originals, also the half-written one | S-14: what leaves the sandbox is approved by a person |

What `apply` checks before writing anything (all or nothing: if one path fails, no file is applied):

- The path must be relative and simple (no `..`, no absolute path).
- It never writes in `.git`, `.claude`, `.pi`, `.agents`, `.vscode`, `.husky`, `.githooks`, `.cargo`, `.devcontainer` (at any depth, case-insensitive; they are folders a tool loads and runs by itself when it opens, builds or commits a project), nor in `extensions` and `skills` at the root (protected paths), nor in files with a secret's name, nor in `node_modules`, `target`, `.venv` or `__pycache__` (the copy leaves them out: what the agent makes there does not go back to the original and does not show as a change). `--force` does NOT lift this.
- The diff also shows a change only of line endings (`␍` marks a carriage return: a `#!/bin/sh` with CRLF does not run) or of the final newline (`\ No newline at end of file`).
- If a change cannot be shown whole (a binary file, one over 8 MiB, or a difference too big for the line diff), it refuses: open it by hand in `runs/<id>/sandbox/` and, if it is fine, repeat with `--force`.
- What is written is exactly what was reviewed: if the copy changed between the review and the write, it refuses and touches nothing.
- If there is a symbolic link on the way (for example, `docs` turned into a link to another folder), it refuses. `--force` does NOT lift this.
- If a file of the original changed since the copy (its hash no longer matches), it refuses to overwrite it and points it out. With `--force` it overwrites, and the backup keeps YOUR version so you can undo.
- A new file that already exists in the original also counts as a conflict.
- The backup of each file overwritten or deleted stays in `runs/<id>/backup/`, and what was applied in `runs/<id>/applied.json`. `undo` restores only if the original is still as `apply` left it (or with `--force`).

**Striking out: a token inside a file.** The copy leaves out the files that exist to hold credentials, but a token can also be inside an ordinary file (a `deploy.sh`, a `settings.json`). That file is copied, and what the agent reads goes through the scrubber: the key shapes it knows (18 prefixes: GitHub, AWS, Google, Slack, Notion, npm, GitLab, Stripe, Hugging Face, `sk-`…, and private key blocks) and the vault's values are replaced by a mark that says what happened and what to do, in the value's place (one form for a key shape that is not in the vault, another that names the vault's secret). The agent cannot write that text back: `write` with a mark is refused (to change the file, `edit` reads and writes the real bytes) and `clio apply` refuses to apply a file that carries one, because it would overwrite the real value with the mark. The chat tells you how many values were struck out. Limits: only the key shapes it recognizes are struck out (a loose password like `password = …` is not); that is why `.npmrc`, whose old tokens have no recognizable shape, is left out by name instead of struck out.

What a run folder holds: `sandbox/` (the copy, with the agent's changes), `manifest.json` (the hash of each copied file), `meta.json`, `contract.json` (from level 2), `effects.jsonl`, `transcript.json` (the whole conversation), `report.json` (done, pending, next step), `summary.json`. A chat session's run also holds `toolhost/` (the taint, the memory budget, and in host mode the file backups for undo, what the judge withheld and the background jobs' output).

## The vault

The vault keeps the model key (`clio connect`, above) and, if you want, other keys. The model sees only `{{secret:NAME}}` marks, and a value struck out of a text reads `[secret:NAME redacted]`; the old spellings (`{{secreto:NAME}}`, `[secreto:… tachado]`), kept by earlier sessions, files and memories, are still read:

```sh
clio vault start            # asks for the passphrase; creates the vault the first time, starts it and unlocks it
clio vault put github --dest http_fetch:api.github.com   # the value is read with no echo, never printed
clio vault list
clio vault revoke github
clio vault status
clio vault upgrade-kdf      # raises the cost of your passphrase (see below); with the vault running
clio vault stop
```

**The passphrase's cost.** The vault stretches your passphrase with Argon2id and keeps that cost in its file. New vaults use 64 MiB and 3 passes; one you made earlier uses 19 MiB and 2 and still opens, but `clio doctor` warns that its cost is old (for whoever gets the file, guessing the passphrase is cheaper). `clio vault upgrade-kdf` migrates it without changing your passphrase or your secrets: it asks for it, seals everything again with a new salt and key, and leaves a copy of the old file as `vault.pre-argon2` (mode 0600, and it holds all your secrets at the old cost: delete it once you check that `clio vault start` opens fine). If something cannot be opened, it writes nothing.

It is a wrapper around the `clio-vault` binary (it looks for it next to `clio`, or at the absolute path in `CLIO_VAULT_BIN`; never on the `PATH`, because it gets your master passphrase). The two sockets are in `$XDG_RUNTIME_DIR/clio/` with mode 0600; the vault's file is `~/.local/share/clio/vault.bin`.

**Secrets the agent asks for.** In the chat the model can ask you to store a secret a task needs (`request_secret`: a name, the one place it may be sent as `TOOL:DOMAIN`, and why). See "Host mode" below for how you answer it. You type the value into Clio; the model never sees it, and today no tool of the model sends a stored secret anywhere.

## Pi, the agent harness

Pi (decision 0007) runs the agent loop, the providers, the sessions and compaction. With Pi installed, `clio` opens **one screen: the chat over Pi, with the sessions in a panel** (`clio --harness builtin` or `CLIO_HARNESS=builtin` go back to the builtin harness's list of runs; `clio --harness pi` requires Pi and says what is missing). `clio run` also uses Pi when it is installed (`--harness builtin` turns it off). If Pi is not available and you did not ask for a harness, `clio` falls back to the builtin loop and says so in one line on stderr (that loop does not run in Pi's jail and has no host mode); with `--harness pi` the error says what is missing. A model message that arrives without usage figures counts 0.01 USD against the cost cap, so a provider that hides them does not void the cap.

When it opens, `clio` is the chat alone, no lists beside it, **reopened where your last conversation ended**: the newest session of Clio's that the drawer lists for this mode (in copy mode, for this project folder), its history on screen at once while Pi starts behind it. You can type right away; a message you send before Pi is up goes out when it is. It keeps its marks, its run folder and its undo history, as reopening it from the drawer does (it is the same path). Pi can write its session files, so Clio seals each one (its size and SHA-256, in the run folder) when the session opens and again when it closes, after Pi has stopped: if the file is not as Clio left it when it is reopened (or, in a run from before the seals, it changed after Clio started sealing), the session is marked as having read outside content and a quiet line says so. A conversation is open in one `clio` at a time (a lock in its run folder): if your last one is open in another terminal, `clio` opens a new one and says so, and the drawer refuses to open it. With none, `clio` is a new session ready to type into; if the last one cannot be reopened, a new session too, with a quiet line that says why. `clio --new` opens a new session directly, and `"session": {"continuous": false}` in `~/.config/clio/config.json` makes that the default (it is `true`). `/new`, `n` in the drawer and the drawer itself work as before. A new session (its run and Pi) is created with its first message, so a new session that you leave empty leaves nothing on disk; the first time, Clio introduces herself in that first answer, in the language of your memories and blocks (in English when there are none).

**The screen.** At the top, one line: `clio`, the session's title (or a new-session label) and, on the right, the model; if something is wrong (the configuration does not load, the model key is missing, the vault is stopped, the doctor found failures, or debug mode) it says so there, in yellow, before the model. Below, a thin line. The conversation uses the terminal's whole width (with a one-column margin on each side): your message in bold with a colored bar on the left; the agent's answer with its formatting (paragraphs, bold and italics, inline code in another color, headings, lists with `•`, quotes, code blocks with their language and a thin bar, links underlined with the address dimmed beside them; never as a terminal hyperlink); tables with aligned columns, the header in bold over a thin line and, if they do not fit, the widest columns narrow and their text wraps inside the cell (never cut); one line per tool (✓ green, ✗ red, ◇ yellow if it was set aside waiting for your approval, or the spinner while it runs; what it does, with a group's titles at one width up to 30 columns so the dimmed, summarized commands start in the same column; on the right, aligned, failed or denied in red, set aside in yellow, and the duration; a tool still running when its turn settles or is stopped is drawn as stopped, in the warning color); quiet one-line notes and errors in red. With the draft empty, in the middle: a greeting and where the agent works (on this server in host mode, on a copy of a folder in copy mode). If you are not at the bottom, a scroll bar on the right and a "more below" mark. Above the box you type in, a line says what is happening (working, with the time and that Esc stops; starting; aborting; or the last notice), and in host mode how many background jobs run (`1 job`, `2 jobs`). Above that line, when the model keeps a checklist (`todo_write`), the list: at most six rows, then `+N more`, done items dimmed with a check, the one in progress in the accent color. The box you type in (`›`) grows from one to six lines and then scrolls; the cursor is the terminal's and moves and edits with the usual keys (table above). At the very bottom, one line: a few keys on the left and `↑input ↓output · context N %` on the right (yellow from 70 %, red from 90 %; the cost only if it is not zero).

**The sessions** are in a panel that opens with `Ctrl-S` (or `F2`) and closes with `Ctrl-S`, `F2` or `Esc`: about 38 columns on the left, or the whole width if the terminal has fewer than 80. On top, a new-session row; then each session of Clio's Pi folder on two lines (its title and, dimmed, its age and message count), the open one marked with `●` and the chosen one highlighted; while the panel is open the ages are redrawn at least every 30 s. `↑` `↓` (or `j` `k`) choose, `Enter` opens the chosen one, closes the panel and leaves the keys in the chat, `n` starts a new one, `x` deletes (not the open one; it asks first: the box shows the conversation's title in full, its age and its message count, says that all of it goes to the trash for 30 days and can be brought back whole, and only `y` confirms; any other key cancels; afterwards one quiet line in the chat says where it went and how to bring it back). Pi sessions that are not Clio's are marked as such and do not open (remote sessions, through `ssh <host> clio serve-rpc`, do not exist yet and are not listed). A reopened session shows its conversation as Pi's session file recorded it, every message, also what a compaction took out of the model's context: the last 200 turns at once, before Pi is up, and the 200 before them when you scroll to the very top, with the view where it was. A quiet line on top, `earlier messages · scroll up`, says there are more, and `earlier messages were summarised here` marks where Pi compacted. Clio only reads that file, never writes it; if it cannot read it, it shows what Pi keeps (after a compaction, the summary and the last part) and says why. A file over 128 MiB is not read (the same fallback), a line over 512 KiB is skipped, and a page holds at most 5,000 entries however few of your messages it has. The session stays open while you look at the panel and closes (saving its summary) when you open another or quit. While the agent works, while a session opens or while what you approved runs, you cannot switch sessions, start a new one or delete one: `Esc` in the chat stops the turn first; the rest waits, and the panel or the status line says so. Quitting never waits: `Ctrl-C` twice, or `/quit`, stops what runs as closing does. **The mouse** is the terminal's by default: it selects and copies text as anywhere else, and everything is done with keys. `/mouse on` gives it to the chat (saved as `"ui": {"mouse": true}` in `~/.config/clio/config.json`, the way `/effort` saves the effort, and applied at once; `/mouse off` gives it back): then a click on a panel row opens it, a click outside the panel closes it and the wheel scrolls what is under the pointer; to select text, hold `Shift` while dragging. A click or the wheel never answers the question about an action set aside: only keys do.

**The questions Clio asks you** come in a centered box, one at a time; one that arrives while another is open shows when that one closes.

- **What was set aside** (host mode): ONE box for everything this conversation has waiting (`Approval`, `Clio set aside 3 items.`). Its first row says what a yes does: *Yes approves all of this and lets Clio act without asking for the next 4 hours; you get a summary when they end.* Then each item whole, as its own box would show it: `1 of 3 · <id> · «class»` in yellow on its own line, then the command, or the whole block change, skill, service, MCP server or MCP call. It scrolls with the arrows (`PgUp`/`PgDn` too) and is never cut. The question hears its first key only after it has been on screen for a second and a second passed with no key (meanwhile the box says to wait a second without typing), so neither the tail of what you were typing nor a `y⏎` typed in a pause answers it before you see it; `y` (or `s`) arms it only once its last row was on screen, and `Enter` right after says yes to all of it; `n` or `Esc` say no to all of it. Any other key after the `y` disarms it and the wait starts again. If the session became `suspect`, the box shows the reason and its yes approves only what it lists: no window opens.
- **A change to your own blocks** (a session that read outside content; host mode or a chat): in the same box, with the same keys and the same quiet second. Its rows say it is written to your own block as said by you, then the block, the action, the exact text whole and, for a replace or a remove, the entry as it reads now, and a warning line if the session is `suspect`. A text longer than the box scrolls with the arrows (`PgUp`/`PgDn` too) and is never cut; `y` arms only once its last row was on screen. Approved, it is written exactly as shown, once, as said by you; if that entry changed meanwhile, nothing is written and the chat says why. The model learns what came of it in the turn after your answers ("What waits for your approval", below).
- **`ask_person`**: a box with an accent border (not the warning one), the question and 2 to 4 answers, the recommended one first, and "Other…" last, which opens a one-line field for your own words. Arrows and `Enter`, or the answer's digit; `Esc` skips the question without stopping the turn. Keys count only after the same quiet second; a click only selects, and only the row drawn there. A long description wraps to at most 3 rows under its answer, and the "Other…" field warns never to type a secret there (Clio asks for secrets in their own box). If nobody answers in ten minutes, or the turn settles, the box goes and the model decides by itself and says what it assumed. The model's checklist (`todo_write`) shows under the conversation while it works; when every item is done it goes as the turn ends.
- **`request_secret`**: when the turn ends ("What waits for your approval", below) or from `/pending`, in its own box (a permission window never answers a secret), a box with the secret's name, the one place it can be sent (in the warning color), the model's reason, a line when the session is suspect, and a masked field (one dot per character; pasting is accepted only here; the same quiet second first). `Enter` stores the value in the vault, through the vault's person socket, from Clio's own process: it never replaces a name that already exists and never passes through Pi, the guard, the tool host or the model. `Esc` declines. If the vault refuses the store (it is locked, or the name was taken meanwhile), the box comes back instead of losing the request. The model is told that the person is asked in the chat when the turn ends; it learns that the secret was saved or declined in a later turn (the one a saved secret starts by itself, or with your next message).

**The chat** shows the agent's text as it arrives, quiet lines for compaction and retries, notes when Pi's jail refuses a connection or the filter strikes values out of what Pi read, a note when the judge withheld outside text from the model (how many bytes, from which tool and source, and the file to read it with `less`), a note once per session when the judge is on but did not answer, a note when a turn ends with background commands still running, and a marker when the session read outside content (in the help, `F1`, which also sums up the session: tokens, cost, steps, time and context). `Enter` sends; with the agent working, `Enter` steers it and `Esc` stops the turn. `Ctrl-D` shows Pi's events below as they arrive. To reach the other screens: `Ctrl-S` opens the sessions and there `m` goes to the memory and `D` to the doctor.

**What starts a session** (all in one place, `src/cli/chat/local.rs`): a run folder (`~/.local/share/clio/runs/<id>/`); in copy mode with a copy of the project in it (`sandbox`, so `clio diff|apply|undo <id>` work for what Pi changed), in host mode without a copy; `clio toolhost` over it (the only one that touches files and runs commands); and Pi with the `clio-guard` guard. Everything stops when you leave the session, when you quit `clio`, on a panic and also if `clio` is killed (Pi and the tool host end by themselves when their input closes; the tool host stops what it runs before it exits). A session that is not a Clio copy (another tool made it) is listed but not opened here. The key comes only from the connector's variable or from the vault, never from a file.

**Pi's jail (ADR 0009).** Pi and its ~120 npm dependencies run as your user; so, on Linux, Pi starts only inside a `bwrap` jail:

- **What it sees.** The system read-only, Node and its own install, its `agent/`, `sessions/` and `home/` folders, the prompt file, the tool host's socket folder and the working folder: in copy mode the copy, **read-only** (every change goes through the tool host and its rules); in host mode an empty folder. **It does not see** the vault or its sockets, your home folder, `~/.ssh` or Clio's data.
- **Its own network.** Inside there is no internet, no host loopback and no abstract sockets. The only way out is the model provider's address (the host and port of its URL), through a proxy of `clio` that runs outside the jail and refuses any other destination; TLS is end to end.
- **Fail closed.** When a session opens, Clio tests the jail from inside: the copy cannot be written, the decoys are not visible, direct internet, a loopback port and an abstract socket cannot be reached, and the proxy refuses a foreign destination. If `bwrap` is missing or does not start, or a test fails, **Pi does not start** and the message says why. `CLIO_PI_JAIL=off` skips it on your own responsibility and `clio doctor` marks it as a failure. On macOS there is no jail: Pi starts without it and `doctor` warns. **With the claude connector there is no way to skip it**: without the jail (off with `CLIO_PI_JAIL=off` or impossible, as on macOS) Pi does not start, and `doctor` marks it as a failure; use another connector, for example `clio connect opencode`.
- **What is left.** Pi keeps the provider's key (it needs it) and can send what it read to that one address, which already gets the model's whole context; it also talks to the tool host, behind the same rules as the model. Its environment carries six proxy variables (`HTTPS_PROXY` and the like, no exceptions). It needs `bwrap` 0.8 or newer and `prlimit` (util-linux).

Pi's own commands:

```sh
clio pi install            # installs the pinned Pi (1.4.0) and the Claude bridge (pi-claude-agent-sdk, ~400 MB) with `npm ci --ignore-scripts`; `clio init` and `just install` do too
clio pi doctor [--json]    # Node 22.19+, Pi installed and at the pinned version, its folder not ~/.pi
clio pi sessions [--json]  # the sessions Pi kept (title, messages, folder, last activity)
clio pi sessions rm <id>…  # deletes sessions (or --all); with their working copy if they are Clio's
clio sessions restore [id] # the trash of deleted conversations: with no id it lists it; with an id (or its start) it brings one back whole
```

- It needs **Node 22.19 or newer** on the `PATH` (mise, nvm or your system's manager) and the network the first time. If something is missing, the message says what to do. `clio init` never fails because of Pi: it warns and you can repeat with `clio pi install`.
- Pi lives only in Clio's folders, all 0700: `~/.local/share/clio/pi/1.4.0/` (the install, from the `package-lock.json` embedded in the binary, without install scripts), `pi/agent/` (`models.json`, without the key), `pi/sessions/` (the sessions, 0600) and `pi/home/`. Your `~/.pi` (your own Pi's) is neither touched nor read, and `clio doctor` checks it.
- The model key is written to no file and no command line: it goes only in the environment of Pi's process, which starts with an empty environment (no `SSH_AUTH_SOCK`, no `HOME` of yours, no other variables of yours). With the jail, that environment is `bwrap`'s and not its arguments: a process's arguments can be read by any user of the machine with `ps`.
- **Deleting sessions:** `clio pi sessions rm ID…` (the id or the file name; `--all` for all) or the `x` key in the TUI's session list. It deletes the conversation's file and, if the session is Clio's, its working copy (`runs/<id>`): both go to the trash (see below), none is removed for good; first it says what goes and, if the copy has changes you did not apply to your folder, it warns and asks you to confirm. Your memory is not touched. A session that is closing (saving its summary) is deleted when it ends.
- **The trash and `clio sessions restore`:** nothing a delete takes is removed for good. Everything it takes moves, by rename, into one folder, `~/.local/share/clio/trash/sessions/<unix-ms>-<session id>/` (folders 0700, files 0600): the conversation's file with its own name, and, if the session is Clio's copy-mode one, its working copy (`runs/<id>`, with the sandbox, the seal, the lock and the marks) as `run/` and the copies of the conversation that Claude Code keeps for it (the claude connector's, in `~/.local/share/clio/pi/home/.claude/projects/<folder>`) as `claude/<folder>/`. A host-mode session has no working copy, so only its conversation moves (its run folder and marks stay, as they always did). It is all or nothing: if any piece cannot move, the others go back and you see the error. A conversation that is open in another `clio` is not deleted (it says so). When `clio` opens, in the background, what has been in the trash for more than 30 days is removed, whole (only folders named `<digits>-<session id>`; anything else there is left alone). `clio sessions restore` lists the trash (date deleted, in your local time, messages, title and entry name); `clio sessions restore <id>` (the entry name, the id, or the unique start of the id; an exact entry name or id wins over a start) puts every piece back where it was, so the conversation comes back whole and opens again from the drawer with its marks and its undo history. It refuses, and restores nothing, if any of those places is taken. The trash is under Clio's data folder, so `clio doctor` checks its modes like the rest (S-23).
- `clio pi sessions` reads only the header and the minimum of each file (it opens instantly with hundreds); titles and names are cleaned of escape sequences; a broken file is ignored with a warning. The message count carries `~` when it is an estimate (big files).

### Chat commands

A `/` as the first letter of an empty message opens, over the box you type in, the list of commands: each with what it does, filtered by its name, its aliases or the description as you type. `↑` `↓` move, `Tab` completes the name, `Enter` (or a click) runs it and `Esc` closes the list and keeps the text. A command that does not exist is not sent: the status line says so and the text stays. With arguments only the whole name or its alias counts (`/m sonnet` is not `/model sonnet`); without them, `Enter` runs the highlighted one and `Tab` completes the start of a name; if you really want to send something that starts with `/`, start the message with a space. Only what you type is a command: nothing the model or a tool says goes through there. The ones that change the session wait for the agent to finish (the status line says so); the ones that only look work always. None answers or hides an open question.

| Command | Also | What it does |
|---|---|---|
| `/model [name]` |  | switch the connector's model (waits for the agent) |
| `/connector [name]` |  | switch the connector (from the next session) (waits for the agent) |
| `/effort [level]` |  | how hard the model thinks (waits for the agent) |
| `/new` | `/clear` | start a new session (waits for the agent) |
| `/sessions` | `/resume` | open the sessions drawer |
| `/name <title>` |  | title the session (waits for the agent) |
| `/compact [instructions \| N]` | `/compress` | summarise the conversation now; N: compact past N thousand tokens (waits for the agent, except N) |
| `/context` |  | how much context the session holds |
| `/usage` | `/cost` | tokens, cost, steps and time of the session |
| `/pending` |  | what waits for your approval |
| `/grant [off]` |  | the permission window: how long is left, or close it |
| `/actions` |  | what this session did, action by action |
| `/undo` |  | undo this session's file changes (waits for the agent) |
| `/review [what \| stop]` |  | have a second agent check the last change (or what you say) |
| `/skills [off \| on \| undo \| remove <name>]` |  | Clio's skills: list them, turn one off or on, go back a version |
| `/services [show \| on \| off \| undo \| remove \| run \| logs <name> \| enable \| disable]` |  | Clio's services, run on a schedule: list them, look at one, turn one off, run one now |
| `/mcp [show \| on \| off \| confirm \| remove \| logs <name> \| allow \| ask <name> <tool>]` |  | MCP servers you approved: list them, look at their tools (confirm is optional), turn one on or off, choose which tools ask |
| `/copy` |  | copy the last answer (OSC 52) |
| `/mouse [on \| off]` |  | let the chat take the mouse, or leave it to the terminal to select text |
| `/memory` |  | open the memory |
| `/doctor` |  | check the installation |
| `/help` |  | keys and commands |
| `/quit` | `/exit` | close Clio (always: what runs stops, as closing does) |

- **`/model`, `/connector` and `/effort`** without a name open their options in the same list, with the current one marked; with a name they apply it. A model's name has to be in its list: another is refused, saying which there are, and nothing is saved. The model and the effort change in the open session (Pi changes them at once) and stay as the usual ones in `config.json` (`model.model`, `model.thinking`). **The model list is Pi's own**: in a live session, the models Pi says it has for the session's connector (the one Pi says it runs). Pi's answer is kept per connector in `~/.local/share/clio/pi-models.json` (0600) and refreshed whenever a session starts; before the first message, the list kept from the last session shows. Only when there is no kept list yet does the catalog stand in, and the picker's footer says to open a session once to see every model. The models of the other connectors whose key is in the vault are listed under their name (`anthropic-api · claude-opus-5-5`); choosing one is `/connector` plus that model, for the next session. The effort takes Pi's levels (`off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`) and shows only the ones the model has; the factory one is `low`, because with Pi's (`medium`) a reasoning model thinks thousands of tokens on a simple step. The header shows it: `claude-sonnet-5-5 · low`. The connector offers only the ones with a key (the others are dimmed, with `clio connect <name>` to add one); it is saved with the last model you used on it, or its factory one, and **applies to the next session**: the open one keeps its own (its approval questions and its copy are its own). `/new` starts it.
- **`/compact`** (or `/compress`) summarises the conversation now, with what you write after it as instructions for the summary. **Clio also compacts by herself**: when a turn ends and nothing waits (no question or approval open, nothing you approved still running, no message held), if the context Pi reports is over the cap, she sends the same compaction, with a quiet line that says it was automatic and at what size; never in the middle of a turn, at most once per turn, and if the context is still over the cap after it she says so once (and that `/compact N` raises the cap; not again until the context is back under the cap) and does not try again until another turn ends. `Esc` stops an automatic compaction (Pi's `abort`; a quiet line says so), and quitting does not wait for one: closing stops Pi as always, and the compaction in flight is simply not recorded. A `/compact` you asked for is waited for. What you type meanwhile stays in the composer, and a message you send while a compaction runs goes out when it ends. After any compaction, yours or hers, the next message says who Clio is again and asks for the session's summary. The cap is `"session": {"context_cap_tokens": 200000}` in `~/.config/clio/config.json` (200 000 by default); **`/compact N`**, when all you write after it is a whole number, sets it to N thousand tokens (50 to 1000; outside that, a note with the range and nothing changes; a value outside it written by hand in `config.json` is held to the nearest end), saves it there as `/effort` saves the effort, and applies at once, even while the agent works. Anything else after `/compact` is instructions.
- **`/context` and `/usage`** open a box with the session's numbers as Pi gives them; `/context` also shows the cap. `/usage` also gives the cache hit rate: the tokens read from the provider's prompt cache over every prompt token (input, cache read and cache write), with the total read from cache; when the provider reports no cache use at all, it says so instead of 0 %. **`/pending`** lists what this session set aside and still waits, of every class; `Enter` on one opens its own box, the one Clio opens by herself, with the same rules: an action's question, a change to your blocks, a skill or a service shown whole, a secret's masked field. Answering there does not reset the hours between Clio's questions (below). **`/actions`** shows the log of what this session ran on the machine: the local time (`HH:MM:SS`), the tool and the result in columns, and the command in the code color (`clio actions` gives it whole, with the UTC time). **`/undo`** asks first and then reverts what the session changed (in host mode, with its own journal; in a copy, what was applied), like `clio undo`.
- **`/skills`** lists Clio's skills: each with its version, whether it is on, what it is for, and the failures reported against it. `/skills off <name>` and `/skills on <name>` turn one off or on (a new version does not turn on one you turned off), `/skills undo <name>` goes back one version exactly as it was, and `/skills remove <name>` asks first and moves it aside (to `skills/removed/`). The same from a terminal: `clio skills [off|on|undo|remove <name>]` (`remove` asks; `--yes` skips the question). **What a skill is**: a folder with a `SKILL.md` (a name, a description that says when to use it, the steps, and a `## How to verify` section with the check) and, if it needs them, scripts. Pi lists every enabled skill's name and description in its prompt and the model reads the rest when a task fits. **Where they come from**: only Clio writes them, with `skill_write`, when you ask for one or when the same kind of request keeps coming back; she extends one that already covers it instead of writing a near-duplicate. The draft is checked (name, sizes, paths, no hidden characters, nothing secret-shaped, a check to run that is more than `true`, `exit 0` or an `echo`), the judge reads it (with Jev on: **skills need the judge**; with `jev.enabled` off no draft is accepted, Clio is told to say so, and `/skills` and `clio skills` print one line about it), and a second agent reviews it in the background (`/review`'s reviewer, which gets every file whole and runs the check on a copy kept in Clio's data folder, which in host mode only its commands see, read-only). At most six drafts an hour per session. The verdict decides: a review that `passes` registers it at once in a clean session, with one line in the chat; in a session that read outside content, when the review could not run the check (it needs the network, say), for a change to `skill-creator` itself, or for a name you removed, it waits for your approval: the box shows every file whole (`SKILL.md` and its scripts, each under `--- <path> ---`), and `y` works once you scrolled to its end (or `clio pending` and `clio approve <id>` at a terminal, which prints it all and asks before registering). A review that `passes` may still leave notes (problems outside what the skill claims): the line says how many and where the full text is, and they go to Clio once, with the request to write a new version only if a note is worth it; that turn is not reviewed, and the notes on the version it writes are shown to you only (a skill's notes reach Clio once an hour). A review that `fails` (the check fails, the skill does not do what it says, or it is unsafe to run unattended) goes back to Clio to fix; after three failures in a row the draft is dropped, you are told, and Clio cannot write that name again for an hour. A draft whose review was cut short (you quit or opened another session) is reviewed when you open its session again. **When it is used**: from the next session (or when you reopen one); a running Pi does not reload in the middle of a conversation. When Clio follows a skill she reports how it went; three failures in a row turn it off and you are told. The first skill, `skill-creator` (how to write one for Clio), is installed by `clio init` or the first chat when missing (not after you removed it: `clio skills on skill-creator` brings it back), and updated there to the text of a new Clio while what is installed is the shipped text unchanged. Skills are kept in `~/.local/share/clio/skills/` with every version; nothing from the jail, a file tool or a command can write there. A skill's scripts run in host mode only, read-only, with the same approvals for their command line, judge and log; what a script does inside is checked by the review (and by you, when it waited), not by the approval classes.
- **`/services`** lists Clio's services (S-69): programs that run by themselves on a schedule, with no model, so work that repeats costs no tokens and is done the same way every time. Each line says whether it is on (or off and why: you, or paused by Clio), its schedule, when it runs next and its last run; then the last notices, and a line if the timer is not set up. `/services show <name>` shows its status, its last runs and every file of the approved version whole; `/services logs <name>` its last 50 runs with what each printed (scrubbed); `/services run <name>` runs it now; `on`, `off`, `undo` (back one version) and `remove` (asks first; kept in `services/removed/`) as for skills; `enable` and `disable` the timer. The same from a terminal: `clio services [list|show|on|off|undo|remove|run|logs <name>|enable|disable]`. **Where they come from**: only Clio writes them, with `service_write`, in a host-mode chat, when you want something done again and again (the shipped `service-creator` skill says how); she approves, registers or changes one only by writing a new version, never touches the timer, and what else she may do with one is below ("What Clio may do with what she created"). The draft is checked (a schedule of at least 5 minutes, `every: "15m"`, `"6h"` or `"1d"`, with `at: "HH:MM"` for a daily one, no cron; the hosts it may reach; the secrets it needs, each for one of those hosts; the paths it may read, the only part of your home it sees; the paths it may write besides its own state folder, existing, under your home and never Clio's folders, `~/.ssh`, `~/.gnupg`, your shell's start files, a name that starts with a dot, a repository or a folder that starts programs; its limits, by default 50 % CPU, 512 MB and 300 s, at most 100 %, 2048 MB and an hour), the judge reads it, the reviewer runs its check, and then **it needs your approval** (your yes, or a permission window you opened), a new service and every new version: the box shows what it is for, the schedule, the command, the hosts, the secrets (each with every host it can reach), what it may read ("nothing in your home" when nothing), the write paths, the limits and how they are held, the review's verdict and notes, and every file whole (`y` only once you scrolled to the end; `clio approve <id>` at a terminal prints it all and asks). A review that fails goes back to Clio and never reaches you. **How it runs**: `clio services enable` writes two files, `~/.config/systemd/user/clio-services.service` and `clio-services.timer` (it prints them), and starts the timer with your own `systemctl --user`, no `sudo`: every minute it runs `clio services run-due`, which runs what is due and exits (a machine that was off runs each service once when it comes back, not once per missed slot). If lingering is off for your user the timer stops when you log out: Clio says so and prints the command (`loginctl enable-linger <you>`); it does not run it. `clio services disable` removes exactly those two files. `clio doctor` warns when a service is on and the timer is not. Each run goes through bwrap (`/usr/bin/bwrap` must be installed; the systemd launcher is not used for services, because on systemd before 257 it cannot hide your other processes, and through them Clio's data, from the run), as you and never as root, with no way to gain privileges, the machine read-only except its state folder and its declared paths, **nothing of your home visible** but what it declared to read or write (not your files, sockets or keys), nor of `/var`, `/srv`, `/opt`, `/mnt`, `/media`, `/root` or other homes (a declared read may name a path under those), its own `/tmp` (counted in its memory) and its own processes only, Clio's folders and `/run` (your D-Bus, Docker's socket) hidden, files it writes private (`umask 077`), and the network only to its declared hosts through a proxy (`HTTPS_PROXY`, `https://` only) that never connects to this machine or a private address. Its limits hold for the whole run when your user's systemd can make a scope for it (`systemd-run --user --scope`, no `sudo`), else for each of its processes; `/services logs` says which. Its files must still be exactly what you approved, or it is paused and you are told. **Secrets**: a service gets one only when the vault lets it go to every host the service declared (it could send it to any of them): bind it to `service:<host>` for each (`request_secret` asks you for it, or `clio vault put NAME --dest service:<host>`); it gets it only as its variable; while the vault is locked or the secret is missing the run waits (you get one note, and it is tried again every 15 minutes) and that is not a failure. Everything a run prints is scrubbed before it is kept. **Results**: exit 0 is ok; exit 10 means "look at this" and you get a note with its last lines when a chat opens; anything else is a failure, and three in a row pause it with a note. A run still going when the next is due makes it skip that one; one slow service does not hold up the others when scopes work. `remove` takes its state and run history along into `removed/`: a new service of the same name starts empty.
- **`/mcp`** lists the MCP servers you approved (S-70): someone else's program from npm that gives Clio more tools (a clock, a search, an API). Each line says whether it is on (or off and why: off by you, or turned off because its files changed), its package and version, its tools and their modes, and its last error. `/mcp show <name>` shows what you approved (hosts, secrets, paths, limits) and every tool as the server describes it; `/mcp confirm <name>` (optional) shows every tool whole in a box (its description and, in plain words, whether it runs without asking or asks you each time; `y` only after a quiet second and once you scrolled to the end, then Enter) and turns it on, refused if the list changed since it was shown; `on` and `off`; `allow <name> <tool>` lets a tool run when Clio calls it, `ask <name> <tool>` makes each call wait for you; `logs <name>` its last starts and calls (never their arguments or results) and why an install failed; `remove` (asks first; kept in `mcp/removed/`). The same from a terminal: `clio mcp [list|show|on|off|confirm|remove|logs <name>|allow|ask <name> <tool>]`. **Only npm servers that run on this machine**, at an exact version and started with Node: no remote (HTTP) servers, no OAuth, no `uvx`/Python and no Docker ones; Clio says so when asked for one. **Where they come from**: Clio proposes one with `mcp_propose`, in a host-mode chat (the shipped `mcp-installer` skill says how to choose and check one): the package, the exact version, the arguments and settings it starts with (`args`, at most 20; `env`, at most 20, never `NODE_*` or a secret's variable), the hosts it may reach, the secrets it needs (each for one of those hosts), the paths it may read (under your home or under `/var`, `/srv`, `/opt`, `/mnt`, `/media`, `/root` or `/home`, never one of those itself) and write (under your home), checked as a service's, its limits (by default 50 % CPU, 1024 MB and 60 s a call; at most 100 %, 2048 MB and 300 s; at least 512 MB, Node needs it) and why. At most six proposals an hour in a session, none in a session the judge marked `suspect`; a name belongs to one package (another package under it is refused), and a proposal identical to what is installed is refused. Clio's own npm (one at a time), in a jail that reaches only `registry.npmjs.org`, asks the registry about that package and version (only its name and version leave the machine; nothing is downloaded), and the proposal **needs your approval** as class `mcp` (like every waiting item: in the one box at the end of the turn, from `/pending`, or under a permission window you opened): the box shows what it is for, npm's facts (who published it and its maintainers, by name; whether the registry marks it deprecated; how many direct dependencies; the license and repository the package claims; the integrity hash), its arguments and settings, the hosts, the secrets (each with every host it can reach), what it may read and write, its limits, and a plain sentence: this is someone else's code, and it will hold those credentials and can send them to those hosts (`y` only once you scrolled to the end; `clio approve <id>` at a terminal prints it all and asks whether to download it). **On approval** Clio downloads exactly that version with `npm install --ignore-scripts` (no install scripts run), in the same jail, into `~/.local/share/clio/mcp/servers/<name>/<version>/`, checks the lockfile (that version, the integrity npm showed you, every package a registry tarball with an integrity, no links or git or file sources), and starts it once **with no network, no secrets and an empty home** to list its tools. If a step fails Clio is told only which step (for the lookup: the registry has no such version, it could not be reached, or Clio's own npm failed); `/mcp logs <name>` says why. npm, the probe and the server run at normal priority (a service runs at the lowest): you are waiting for them. The confirm box counts every package installed. Approving it installs it **and turns it on**: no second step (an older server still waiting for `/mcp confirm` is turned on by `/mcp on`). Tools the server marks as read-only (and not destructive or open-world) start as `free` ("runs without asking"); the rest, and a tool with no hint, as `ask`. **How it runs**: started when Clio first calls it in a session (one process per server per session, stopped after 10 minutes unused, when the session ends, and before the next call once you turn it off, remove it or update it; a slow server holds up no other), in a service's jail (as you, the machine read-only, nothing of your home, `/var`, `/srv`, `/opt`, `/mnt`, `/media`, `/root` or other homes visible but what it declared, of Node only its binary and npm's folder, its own `HOME` in `mcp/data/<name>/`, Clio's folders and `/run` hidden, the network only to its declared hosts through the allow-list proxy, its limits as a service's), with only its own secrets (`mcp:<host>`, given on its standard input, never in a file or argv). Before each start its installed files must hash to what was installed, or it is turned off and you are told. **What Clio sees**: two fixed tools, `mcp_tools` (the servers that are on and their tools) and `mcp_call` (one tool, with arguments that must fit the tool's schema and not look like a secret). A `free` tool runs; an `ask` tool, any tool whose description or arguments changed since it was installed or confirmed (flagged until you confirm again), and every call in a session the judge marked `suspect` need your approval as class `send` (under a permission window the first two run at once, recorded; a `suspect` session's always wait), showing the server, the tool and the whole arguments (scroll to the end before `y`), and run exactly that once approved, in a fresh process of the server. A host chat starts even when its MCP side cannot; the two tools then say why. Everything a server returns is outside text: scrubbed, judged and marking the session. A server cannot ask Clio for anything (sampling, roots, elicitation are refused).
- **What Clio may do with what she created** (`service_manage`, `skill_manage`, `mcp_manage`, in a host-mode chat; S-69). **With no approval**: see one's `status` (no name: the list) and a service's or server's `logs` (outside text, judged, it marks the session); turn one `off` (something already off stays off for the reason it had: yours, or a pause); `run` a service that is on and reaches no host and holds no secret (the same jail, digest check and limits as `/services run`, so a service whose files changed is paused instead, and its run history says Clio started it). `off` and `run` happen when her turn ends, done by your own `clio`, not by the jail (a run on a thread of its own: the chat never waits for it; one run per service and 20 requests at most each time; a request more than an hour old is not done): the chat says `Clio did this by herself: turn the service «disk-check» off (Its disk is gone). «disk-check» is off`; a service or server she turned off is also a notice the next chat shows (`turned off by Clio: <her reason>`), and `/services`, `/skills` and `/mcp` say `turned off by Clio`; under an open permission window it is in the window's record and summary. Once the chat is idle and nothing is typed, Clio starts one turn by herself, `turn started by what Clio did: she goes on with your request`, that gives her what came of it so she goes on with the rest of what you asked (a message you send first carries it instead); at most 3 such turns in a row without a message from you, then she stops and says so. **With your approval** (the one box, `/pending` or `clio approve`, or under a permission window you opened, done when her turn ends, then the same turn by herself, and in the window's summary): turn one back `on`, go back a version (`undo`), `remove` one (to `removed/`, never one that is running) and `run` a service that reaches hosts or holds secrets (class `send`: the box says which hosts and secrets). The box says it in plain words with her reason: the version `undo` goes back to, and what goes to `removed/`. **Always asks you, window or not**: turning on what **you** turned off, taking a **service** back a version (the box shows what that version may do: schedule, command, hosts, secrets, reads, writes, limits), removing or taking back a skill Clio ships, and every action but `status` in a session the judge marked `suspect`. An approved `on` of something turned off again since you were asked does nothing. She cannot register or update one, set an MCP tool to `allow` or `ask`, confirm a server's tools or touch the services timer: those stay yours. At most 20 such calls an hour in a session. Your own commands are unchanged.
- **`/review`** has a second agent (the reviewer) check a piece of work and try to break it. With nothing after it, it reviews the last turn that changed something (a file written or a command run, as the action log records it): your request, the commands and files, and the model's last message. With text, the text says what to review. It is refused with a note while a turn runs, and it runs one at a time. **Clio also asks for one by herself**: when a turn that changed something ends and nothing waits (the automatic compaction's rule), she asks the judge one closed question, whether that work needs an independent review, with your request and the list of what was done; on yes (`review.threshold`, 0.8) a quiet line says the judge asked for a review and it starts. Never after a turn that only read, never when the judge is off (`jev.enabled`, off by default) or does not answer, never twice for one turn, never for the turn that answers a review's findings, never while a question is open (once it is answered and nothing waits, the judge is asked), and at most `review.max_per_hour` (3) an hour (yours do not count). The review runs in the background: the status line says `reviewing…` with the seconds, you go on typing and working, `Esc` does not stop it and **`/review stop`** does (so does leaving the session). When it ends, one quiet line, `review: passes · opus · 48 s · ran 6 commands` (the verdict is `passes`, `fails` or `could not check`); if it has findings, a block under it with each one and its evidence (cleaned, wrapped, cut) and the path of the reviewer's whole answer; a `fails` with no findings shows what was run and goes nowhere. Findings, with any verdict, go to the model when the chat is next idle and you are not typing, as a turn the chat shows as started by the review: the message carries Clio's request ("an independent review of your last work found the following; fix what is right, and say which findings you do not accept and why") and the findings as data, never as your words. A message of yours waiting goes first. The judge reads the findings first, like any outside text: if it flags them the model gets a notice instead and the session is marked. If the reviewer read something from outside (a file, a command's output) it ends marked, and its findings mark this session the same way; a clean one marks nothing; if the mark cannot be made, the findings are not given to Clio (a note says so; you have them on screen). What the reviewer answers that is not its fixed block is `could not check`, and its text is shown as data and goes nowhere. `/usage` shows what the reviews spent on their own line. **What the reviewer is**: another Pi, started like the session's (the same jail, connector and launcher rules), with its own session, run folder (`runs/<id>/reviews/<n>/`: its `review.json`, `answer.txt` and Pi files) and tool host; its prompt `clio-reviewer@3` (`fails` only when the given check fails, the thing does not do what it says or it is unsafe to run unattended; `passes` when the check passes and it does what it says, with what lies outside its claims as findings; `could_not_check` as before); only the review request (never the conversation, your blocks or the memory); and four tools, `read`, `bash`, `find` and `ls`, under the session's mode rules. It cannot ask you anything: what would wait for your approval is refused at once and nothing is left pending. **It has no network and no `sudo`, and it does not write your files**: in a copy the copy is read-only to it, in host mode the machine is read-only but `/tmp`; so a review cannot check what needs the network, `sudo` or writing next to the code, and says `could not check`. It starts with the session's marks, runs on the session's connector (the model it has now, after `/model`, when the connector does not offer the reviewer's), spends from the same day's budget, and what it runs is in the action log under its own run (`<id>.review-<n>`): `/actions` and `clio actions --run <id>` show those rows with the session's, each marked `review N ·`. Each time the judge is asked, its decision (a review, the score under the threshold, the judge off or silent, no vault) is one line in `runs/<id>/reviews/judged.jsonl`; with `Ctrl-D` (debug) it is also a quiet line in the chat. Its settings, in `"review"` of `config.json`: `model` (by default `claude-opus-5-5` with the `claude` connector and the session's model with the others; if the connector does not offer it, the review runs on the session's model and says so), `thinking` (`medium`; a level Pi does not know is `medium`), `max_seconds` (600, held to 30–3600), `max_steps` (40, 1–200) and `max_cost_micro_usd` (1,000,000, one dollar: the review's own cap, not `limits.max_cost_micro_usd`) (a review past any of them is `could not check` with what it had, and says which limit), `auto` (true), `threshold` (0.8, 0–1) and `max_per_hour` (3, 0–60). A value out of its range is brought into it when read.
- **`/copy`** puts the last answer on the clipboard with the terminal's OSC 52 sequence (in base64, up to 64 kB): it only works in terminals that allow it. It copies what you see: escape sequences do not pass, and control, invisible or direction characters come out as `�`, as on screen.
- **Your own commands.** Each file `~/.config/clio/commands/<name>.md` is a command `/<name>`: the name is lowercase letters, digits and `-` (others are ignored, with a note), the first line, if it starts with `# `, is its description, and what you type after the name replaces `$ARGUMENTS` (if the text does not have it, it goes on its own line at the end). It is sent as your message, as it is, without being read again as a command. They are read each time the list opens; one of Clio's wins over a file of the same name or alias, and a file over 32 kB is ignored, as are a link, a FIFO or a device: only regular files are read. That folder is Clio's: the agent can neither read nor write it.

## Host mode: Clio on your real machine

It is the factory mode since 2026-11-18 (before, the copy was, as above). In host mode (ADR 0012) there is no copy: the tools act on your machine, in your home folder, with the network when a command asks for it, and with `sudo` where you have passwordless `sudo`. Pi stays in its jail; what acts is the tool service. Only with Pi: the builtin harness has no host mode, so without Pi `clio` asks for `--copy` (or `clio pi install`).

- **Turning it on and off.** It is on by default: a `config.json` without `"mode"` (and the one `clio init` writes) is `"host"`. `"mode": "copy"` turns it off; for one time, `clio --copy` / `clio run --copy "…"` and `clio --host` / `clio run --host "…"` win over the configuration. `clio run --workspace DIR` names a folder, so it works on a copy of it (with `--host` it is an error). A host session reopens as host.
- **The console.** With passwordless `sudo` and `systemd-run`, each command runs in a transient system unit, as your user and with `sudo` available; otherwise in `bwrap --bind / /`, without `sudo`. In both cases Clio's folders look empty and the command has no network unless it asks for `network: true` (and then it marks the session). `clio doctor` says which one there is and whether it passes its self-test; without one that passes, there is no console.
- **Time limits.** The model gives a command a `timeout` (in seconds, up to 3600) when it expects it to end by then; without one the command has no time limit and runs until it ends or is stopped. `Esc` in the chat, an abort, quitting or the end of the session stops it, under both consoles, and the action log says `stopped`.
- **Background commands.** For a server, a long build or a watcher, the model starts `bash` with `background: true` and gets a job id at once. It reads what the job printed with `job_output` and stops it with `job_stop`. At most 4 run at once in a session; each job's output is kept in the run folder (0600), capped at 4 MiB (the oldest half is dropped) and handed over at most 64 KiB at a time; every job stops when the session ends, and then its output folder (`runs/<id>/toolhost/jobs/`) is removed. `job_output` hands over whole lines only while more may come, since a value cut in two would get past the scrubber. The chat shows how many run, and the action log gets a start line and then done or stopped. While a command runs, the tool host still answers `filter`, `judge`, `jobs`, `job_output` and `job_stop`, so the guard's filter and a stop are never stuck behind it; a request whose client left while it waited is refused and runs nothing.
- **Leftovers of a crash.** A new host-mode session stops the systemd units that a `clio` which died left running (only those whose `clio` is dead), so a crashed session never leaves a command running with no time limit.
- **Clio's things are yours.** `~/.config/clio`, `~/.local/share/clio`, the runtime folder and clio-memory's: the tools refuse them.
- **What waits for your approval.** Deleting what cannot be recovered (`delete`), spending money (`spend`), sending something in your name (`send`), and touching Clio's setup, the service manager, D-Bus, `sudoers`, keys, who may use the machine, the schedulers (`crontab`, `at`, `batch`), or turning it off (`control`) need your approval, as do a block change in a session that read outside content (`memory`), a skill or a service draft its review did not fail but could not register (`skill`, `service`), an MCP server (`mcp`) and an `ask` MCP call (`send`). **A yes is worth four hours.** When no permission window is open, such an item is set aside: it does not run, the model is told so and the task goes on. **When the turn ends**, Clio asks you in ONE box that lists every item this conversation has waiting ("The questions Clio asks you", above): `y` then `Enter` says yes to all of it **and opens a permission window**; `n` or `Esc` says no to all of it. She also looks when a chat opens and every 10 minutes while it stays idle (so nothing expires unasked), never in the middle of a turn; an item already on screen or in line is never in a second box. The status line counts what waits (`2 waiting for approval`). **The permission window** lasts `approvals.grant_hours` (`"approvals": {"grant_hours": 4}` in `~/.config/clio/config.json`; 4 by default; **0 means no window**: every item is asked for). While it is open, **only what ends with the act** and was set aside after it opened is approved by Clio and done at once, in any conversation: a command or write of class `delete`, `spend` or `send` and an `ask` MCP call (the tool host runs them inline, with the command's own time limit and in the background when it asked for that), a skill that would wait (the chat registers it), and turning what Clio created back on, back a version or away, or running a service that reaches out (`*_manage`, done by the chat when her turn ends). **What always asks you, window or not**: `control` (from there a fooled model could rewrite the window or schedule a yes), a secret, a change to your blocks (they become your own words and Clio's rules), a new service or MCP server, or a new version of one (code that outlives the window), turning on what you turned off, taking a service back a version, removing or taking back a skill Clio ships, an MCP tool that changed since you confirmed it, and anything that was already waiting when the window opened; each such row in the box says `this one always asks you`, and your yes to it opens a window like any other yes. It goes through the same approval path as your yes (the same checks, hash and log lines, rule `grant/approved`), with one extra field on the item saying the window approved it, and it is **recorded in the window's record before it runs** (`~/.local/share/clio/host/grants/<id>.jsonl`), then its outcome. The chat says `approved under your permission (3 h 12 min left): …`, and the status line shows `permission: 3 h 12 min left`. A window opens only from your yes, in a box or with `clio approve` **at a terminal** (a `clio approve` whose standard input is not a terminal, `--yes` piped or run by a scheduler, approves only its item), and only once the first approved item ran; a yes to a secret alone opens none (it is never answered by a window). **With the judge off** (`jev.enabled` false, the default) no window opens: the box and `clio approve` say `the judge is off: this approves only what is listed`, and a window found open with the judge off closes. A yes while one is open does not extend it. It ends at its time even if `clio` was closed; it never outlives the boot it opened in, and a clock set back does not extend it; a missing or damaged `~/.local/share/clio/host/grant.json` (0600, written only by your `clio`) means no window. An act that starts just before the end either runs whole and recorded, or is set aside. **What a window never does**: it never answers a secret (only you type one); it never acts for a session the judge marked `suspect`, and the judge's first flag of an injection, in any conversation, **closes the window for every conversation** (a yes in a suspect session's box approves only what it lists and opens no window); it never registers a skill or service whose review failed, and never skips the review, the judge or a digest check. `/grant` shows the window and `/grant off` closes it now; `clio grant` prints the last windows and what each approved, and `clio grant off` closes it. **When a window ends**, Clio sums up what she approved under it: one turn by herself, `turn started by the end of your permission window: Clio sums up what she did without asking`, if a chat is open and idle, or when the next chat opens; never in a turn that carries your message (it waits for the next idle moment). A conversation that took part tells it (another only when none that did is open). Once per window (within a week), claimed when it is sent and owed again if the send fails; a window that approved nothing ends silently; the record gives each act's outcome in fixed words (`done (exit 0)`, `failed (exit 1)`), never what it printed; the summary turn cannot open or extend a window, and starts nothing new. **At most 20 items of a session wait at once**: past that, what the model asks is refused with `too many actions wait for the person: wait for their answers`, and nothing more is set aside until you answer. **After you approve**, once nothing else waits and the chat is idle, Clio starts one turn by herself, `turn started by the approvals: Clio says what came of them`, only if at least one answer did something (it ran, was written, was registered, or a secret was stored): the model gets what came of each answer (an approved command's output judged as always; an approval that could not be done, said as such) and her request to say briefly what happened with each, go on with what was left pending and not try again, in any way, what you rejected. Rejections and failures alone start nothing: they go with your next message, as do the outcomes of what the window approved in the chat. A message of yours that goes out first carries them instead; reopened later, that message shows what you typed, and what came of the approvals as the same quiet lines it had live. **An answer cut short** (Clio quit or was killed while an approved command ran) leaves its item *interrupted: it may have run in part*: `/pending` and `clio pending` list it as such, and it is never offered again. Quitting waits (10 s at most) for answers that only write (a block, a skill, a service, a secret) and stops only a running command. What is answered or expired is removed from `~/.local/share/clio/host/pending/` a week after its time. One from a session that is no longer open stays in `clio pending` (`clio run` ends by saying what it set aside), and what came of an answer waits for its session. A set-aside action expires after 24 hours; what it printed, once approved, is judged before the model sees it (with Jev on), and an approved command that asked for no `timeout` runs with a limit of 3600 s (the question says so). It is a list, not a wall: a script gets around it; that is why everything is logged.
- **The judge at the door.** With Jev on (below), what a tool brings in from outside (a foreign folder, a command with the network, a program that reaches out, logs) is judged before the model reads it. If the judge says it tries to give an AI orders, the model gets only a notice with its size and source, the text is kept for you (`~/.local/share/clio/runs/<id>/toolhost/withheld/`, 0600; the chat says which file), and the session becomes `suspect`: its background jobs with the network are stopped, and from then on every command that asks for the network, reaches out by itself (such as `ssh`, `docker` or `nsenter`; a scheduler such as `crontab` is `control`, always) or names `sudo` waits for you as send, with the reason shown. The judge reads all of a text, in windows of 24 KiB that overlap by 2 KiB, all asked at once; beyond 16 windows the text counts as unjudged. If the judge does not answer within 5 s, the text goes on as before and the chat says so once.
- **Skills.** A skill draft that cannot be registered by itself waits as class `skill`: `clio pending` lists it, `clio approve <id>` prints every file of it whole, asks, and registers exactly that (`--yes` skips the question), and `clio reject <id>` drops it. See `/skills` above.
- **Services.** Every service draft that its review did not fail waits as class `service`, always: `clio pending` lists it, `clio approve <id>` prints all of it (what it may reach, write and use, its limits, the review and every file) and asks before registering it (`--yes` skips the question), and `clio reject <id>` drops it. See `/services` above.
- **MCP servers.** Every `mcp_propose` waits as class `mcp`, always: `clio pending` lists it, `clio approve <id>` prints what the box shows and asks before downloading anything (`--yes` skips the question), then installs, checks and probes it and turns it on, and `clio reject <id>` drops it. A call to an `ask` tool waits as `send` and `clio approve <id>` runs that exact call once. See `/mcp` above.
- **Changes to your blocks.** In a session that read outside content, `memory_block_edit` waits as class `memory` (in a chat in copy mode too). `clio pending` lists it with its whole text (and, for a replace or a remove, the entry as it reads now), `clio approve <id>` prints what the box shows and writes it, and `clio reject <id>` drops it.
- **Secrets the agent asks for.** A `request_secret` waits like an action set aside, as class `secret`: the chat asks for the value when Clio asks, or from `/pending` (above). At a terminal, `clio pending` lists it, `clio approve <id>` asks for the value with no echo and stores it in the vault, and `clio reject <id>` declines it. A name that already exists, or a stopped or locked vault, is refused when the model asks. The destination is `TOOL:HOST`: the tool is lowercase letters and `_`, and the host has at least two labels (`api.linear.app`, never `com`), since the vault binds a value to the host and every name under it.

```sh
clio pending [--json]          # what waits for your approval, and the secrets asked for
clio approve <id>              # runs it once, exactly as it was set aside (a secret request: asks for the value; a block change: writes it); at a terminal, with the judge on, it then opens a permission window
clio reject <id>               # drops it
clio grant [off]               # the permission window and the last ones, with what each approved; off closes it
clio actions [--run <id>] [--json]   # the log: every command and write, before and after
clio undo <id>                 # in a host session: undoes the files it changed
```

- **Undo** works for what Clio changed with `write`/`edit` (it keeps a copy first): it restores content and permissions, removes what it created and leaves alone a file that changed afterwards. What a console command changed has no copy.
- **A budget per day**, not per task: `limits.day_cost_micro_usd` (3,000,000 by default) and `limits.day_steps` (1000) in `config.json`, counted across sessions in `~/.local/share/clio/host/day.json`. When it is spent, Clio says so and goes on the next day.
- **`~/.config/clio/host.json`** (optional; it only adds, it removes nothing of the factory list): `{"foreign": ["/srv/mailbox", "attachments"], "ask": [{"class": "send", "pattern": "^rsync .*prod:"}]}`. A `questions_per_day` from before is read and ignored, as is an `approvals.every_hours`: `approvals.grant_hours` in `config.json` took their place. `foreign` are foreign folders (reading them marks the session; by default `/var/log`, `/var/mail`, `/var/spool`, `uploads`, `Downloads`, `Descargas`); `ask` adds patterns to one of the four classes (`delete`, `spend`, `send`, `control`). Files written with the old Spanish names (`borrar`, `gastar`, `enviar`) — this one and the pending items already saved — are still read.
- **What it does not cover:** with `sudo`, a command can do anything, also read Clio's things; with the `systemd` console, even without `sudo`, a command can reach Clio's files through `/proc/<pid>/root`. Details in `docs/threat-model.md`.

## Checking the machine

```sh
clio doctor
```

It checks: Node and Pi (see "Pi, the agent harness"), the jail (self-test, one line per check, see above), Pi's jail (whether `bwrap` starts; off with `CLIO_PI_JAIL=off` is a failure; see "Pi's jail"), in host mode the console and its self-test, the permissions of Clio's folders (0700) and files (0600), the active connector's key (in the vault or in its variable) and whether an `opencode-api-key` file is left that was not moved into the vault, 0600 sockets, that no `clio*` process listens outside loopback, that what Clio wrote (its runs, transcripts, Pi sessions and memory; not the copies of your projects) holds nothing key-shaped nor, if the vault is running and unlocked, any vault value (it tells you the file and the secret's name, never the value; with the vault closed or stopped it warns that it could not check), and it runs `infra/local/check-host.sh` (it looks for it in `CLIO_HOST_CHECK` or above the `clio` executable, never in the folder you are in: a foreign repository could bring one) and shows its output (the script's `PASS`/`WARN`/`FAIL` lines count as more checks). Without the script and without `CLIO_HOST_CHECK` (for example, `clio` installed outside the repository) it says nothing about it. Clio runs it with `CLIO_DOCTOR_EMBEDDED=1`, which makes it skip the two checks `clio` already made itself (permissions of Clio's folders and files, S-23, and Clio processes that listen outside loopback, S-38): each shows once. If you run the script alone, it does them all and ends with its own summary line, with the same exit code as always. Under each warning or failure it prints a `->` line with what to do, and at the end a summary. It exits with 1 if there is any FAIL; WARNs are your call. With `--json` it writes `{ok, pass, warn, fail, host_script, jail, checks: [{level, message, hint}]}`. The TUI shows the same with `D`.

## What Clio does not do yet

- **In copy mode it runs no free commands.** With Pi, `bash` exists but always runs inside the jail (no network, no HOME, protected folders read-only). With the builtin harness, only `run_tests` (unittest or pytest), and only with the jail tested. Without a jail (another system, no `bwrap`, a kernel without namespaces, a failed self-test) the agent only reads, lists, writes and makes folders, and says it could not run anything: run the tests yourself before `apply`. It does not run `cargo test` either (see the jail). Host mode is different: see above.
- **In copy mode there is no network for the agent** beyond the model. With Pi, only the provider's address, through a proxy that refuses the rest; with the builtin harness, only the model adapter talks to the internet (with your key): it requires `https` (or `http` to `localhost`) and does not follow redirects.
- **No long tasks with interactive approval of the contract**: since you wrote the instruction at the terminal, it counts as approval.
- **In copy mode the agent deletes no files** (it has no tool for it); `apply` would know how to apply a deletion if one showed up in the copy.
- No tool of the model sends a stored secret anywhere; a registered service can use one bound to `service:<host>` (see `/services`), and an MCP server you approved one bound to `mcp:<host>` (see `/mcp`). The vault is not connected to the agent in `run`; the secrets the copy leaves out simply do not exist for it.
- There is no PWA. Background commands exist only in host mode (above). Memory across runs exists, but it is short and careful (see "Memory"); `clio memory purge` (deleting for real, the vectors too) will come separately, with PURGE. In copy mode the agent does not see changes you make to the original after the copy (that is why `apply` detects them by hash).

## Risks worth keeping in mind

- The adapter runs as your user and sends the model the text of the copied files. What you do not want to leave your machine must not be in the workspace (files with a secret's name are left out, and the vault's values and known key shapes are struck out when read, but a secret with no known shape inside an ordinary file is copied and sent to the model).
- `apply` writes as your user: review the diff, above all scripts and configuration.
- Being in the `docker` group is the same as being root; in host mode a command can use Docker (see `docs/threat-model.md`), and in copy mode keep Docker out of the agent's reach (`clio doctor` warns).

## Jev (optional): memory and tools before your message, what is saved, and the judge at the door

Jev (TypeSafe, System One) does not write: it answers closed questions with probabilities. Clio uses it, **if you turn it on**, to decide which memories go before the model and which tools suit it, to check what the agent saves before writing it, and as the judge at the door that reads outside text before the model does; the date and time always go, with or without Jev. Everything works without it: off, without a key, without network or without budget, Clio goes on with its own rules (the memory it finds by text and vectors, no tool suggestions, no judge).

To turn it on, with the vault running and unlocked (`clio vault start`):

    clio vault put jev --dest jev:api.typesafe.ai     # paste the key; it does not show on screen
    # and in ~/.config/clio/config.json:  "jev": {"enabled": true}

**Temis instead of TypeSafe.** Temis is the owner's own service and speaks the same protocol, through the same adapter. Point Clio at it with `"jev": {"enabled": true, "base_url": "https://api.imontes.dev/temis"}` and store its own key: `clio vault put temis --dest jev:api.imontes.dev`. The adapter accepts no other destination.

`clio doctor` says whether it is off, whether Python is missing or whether the key cannot be used. Settings in `"jev"`: `memory` (put memories before the message, yes by default) and `tool_hints` (suggest tools, yes by default).

What Jev sees: your message and, if memory is needed, the text of a few candidate memories; what the agent saves, corrects or merges (the text, its title and its category, and whether the session read something from outside), to say whether it is unsafe (it tries to manipulate the agent or carries something dangerous), whether it is worth remembering and what its category is, where it goes and how it relates to the most similar memory; and, as the judge at the door, the outside text a tool brought in (a file, a command's output, a log) with the path or command it came from, to say whether it tries to give an AI agent orders (withheld at 0.9 or more). All of it already cleaned of secrets by the vault; a text over 24 KiB is judged by its head and tail. The text Jev reads may come from something hostile: that is why Jev only answers closed questions about it, never obeys it. Only a clear "no" from Jev stops a save (the agent hears why); if Jev is off, down or does not answer in time, it is saved anyway. What the model sees, before your message and without you seeing it in the chat: the date and time, the chosen memories and a line of tools that could help, only as a suggestion. What you wrote comes from the memory's rules; what the agent wrote or came from outside is shown only if Jev judges it relevant, as data, and the session becomes tainted (with Jev off, it is not shown). Memory before the message needs the vault running: without it no memory is shown (it could not be cleaned of secrets). TypeSafe's model is `jev-latest` (TypeSafe retires old versions); Temis uses its own default model. The version that answers is kept in view. TypeSafe does not document how much it keeps of what it receives: you accepted that. Design and rules: `docs/specs/2026-11-03-jev-design.md`, S-62 and S-63.
