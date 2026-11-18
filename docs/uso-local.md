# Using Clio on your machine

Clio runs on its own host (ADR 0006): your PC or your VPS, no public ports. This guide explains how to use it today with the `clio` command.

The screen drawings below show the interface as the code draws it today, still in Spanish in places; the text around them is the reference.

## Two modes

**Host mode, the factory default (since 2026-11-18).** Clio works on your real machine. What deletes, spends, sends or touches Clio's own setup is set aside for your approval, and everything is logged. See "Host mode: Clio on your real machine" below.

**Copy mode** (`"mode": "copy"` in `config.json`, `--copy`, or `clio run --workspace DIR`). Here the rule is:

**The agent never touches your project.** `clio run` makes a COPY of the project and the agent works only on that copy. When it ends, you see the diff and decide with `clio apply` what goes back to the original. If you do not like it, nothing happens: the original stays as it was. If you apply and regret it, `clio undo` puts it back.

## The interface: type `clio`

In a terminal, `clio` with no arguments opens a full-screen interface (TUI). With Pi installed it is the chat (see "Pi, the agent harness"); with the builtin harness (`clio --harness builtin`) it is the list of runs and the screens below. Everything the commands below do can be done from it, with the same protections (`apply` and `undo` are the same code). Outside a terminal (a pipe, a script, CI), `clio` prints the help and exits with 0; `clio --help` too; an unknown command exits with 2. `clio --debug` (or `CLIO_DEBUG=1`) opens it with debug mode on (below).

**How it is meant to look.** Quiet: no boxes (only thin lines and white space), a one-line header, `clio · folder · model` (on the right, the screen and, when on, `debug`), the main text in your terminal's color and the secondary text dimmed. Cyan is only for selection and focus; green, yellow and red only for "good", "warning", "failure" and the diff. The only boxes are questions that wait for your answer (confirming an `apply`, a warning), and in the chat the box you type in and the sessions panel. Everything works with the keyboard (key table below; `?` shows the keys of the screen you are on); in the chat, also with the mouse.

**Always in view: usage and context.** The screen's last line carries a few keys on the left and, on the right, the usage, dimmed, on every screen where there is a run (on the home screen, the latest one). With room, the usage shows in full:

```
↑12,4k ↓3,1k tok · $0 · 8/40 pasos · 21 s/10 min   contexto ▕███░░░░░░░▏ 31 %   externo
```

- `↑` input tokens and `↓` output tokens, as the model reports them (if the adapter does not report them, 0).
- The cost in dollars with just enough precision: `$0` if free, `$0,0015` for a fraction of a cent, `$0,12` from one cent up. Inside, everything is whole micro-USD, no floating point.
- Steps and time against their limit (`8/40`, `21 s/10 min`; the limits are those of `~/.config/clio/config.json`).
- Context: how much of the model's window its next call will take, **approximate** (four characters per token). The window comes from the model catalog (`~/.config/clio/catalog.json` or the file in `CLIO_CATALOG`; with neither, the one that ships with Clio, which knows the default model); if the model is not there, 128k is used and the context view says so. The bar turns yellow from 70 % and red from 90 %.
- The outside marker appears only if the session read outside content (files, test output): it is what used to be the long "tainted" warning. The gateway is stricter then.

Since it shares the line with the keys, it is almost always abbreviated (words drop, the bar goes) but none of the four figures is lost; at 60 columns it is still whole. In the chat the line is shorter: `↑input ↓output · context N %`. The minimum size is 60x16; below that the screen says so instead of drawing badly.

The drawings show what each screen holds. Today the header also has a thin line under it, and the keys and the usage share the last line (keys on the left, abbreviated usage on the right).

**Home**: a status line (key, vault, doctor; only a problem gets color) and the recent runs. `/` filters the list by id, status, instruction or folder as you type (`Enter` keeps the filter and goes back to the keys, `Esc` clears it).

```
clio · proyecto · beam-free                                              inicio

 llave presente · vault sin crear (opcional)
 doctor: 12 ok · 3 avisos · 0 fallos

 ─ recientes ──────────────────────────────────────────────────────────────────
 ▸ 1031-1816-5a67 pendiente de revisar   1 arch arregla parse_date en dates.py
   1030-0900-0b1c aplicado               2 arch agrega un README
   1029-1200-ffff fallido                0 arch refactoriza todo
 Enter abrir   n nuevo   / filtrar   q salir   c contexto   U uso   ? ayuda
 ↑12,4k ↓3,1k tok · $0 · 7/40 pasos · 12 s/10 min   contexto ▕███░░░░░░░▏ 31 %
```

**New job** (`n`): the folder (the current one by default) and the instruction. The folder is checked as you type with the rules of `clio run` (it refuses `/`, your home folder and folders that are too big), and the screen tells you what will NOT be copied (secrets, `.git`, `node_modules`, symbolic links...) before it starts. In text fields letters are text, even where they are keys on other screens; `Alt-Enter` or `Ctrl-J` add a line to the instruction. What you paste goes through the cleaner: it cannot bring escape sequences or press `Enter`.

```
clio · proyecto · beam-free                                       nuevo trabajo

 Carpeta       /home/ivan/proyecto

 Instrucción   arregla parse_date en dates.py▏

 ✓ se copiarán 12 archivos (3,0 MB)
 ─ NO se copiarán a la copia de trabajo ───────────────────────────────────────
 parecen secretos: .env
 carpetas pesadas o internas: .git, node_modules
 siempre: .git, node_modules, target, .venv, __pycache__, .env*, *.pem, *.key,
 id_rsa*, id_ed25519*, .ssh, .aws, credentials*

 Tab cambiar campo   Enter lanzar   Alt-Enter línea nueva
```

**Live**: the agent runs in its own thread; you see each action the model asks for and the gateway's decision (allowed, asked or denied, with the rule and the size of the result). `Ctrl-C` (or `Esc`) cancels and ends after the current step; a second press quits at once (the run is left incomplete, the original was never touched). `j`/`k` walk the timeline.

```
clio · proyecto · beam-free                                             en vivo

 ⠋ trabajando en la copia…   1031-1816-5a67   nivel Simple
 arregla parse_date

 ─ línea de tiempo ────────────────────────────────────────────────────────────
   1  read_file  dates.py  ✓ permitido (step-8-reversib…)  1234 car.
   8  write_file .git/hooks/pre-commit  ✗ negado (S-16)
 Ctrl-C cancelar   Tab tiempo / debug   c contexto   U uso   ? ayuda
 ↑12,4k ↓3,1k · $0 · 8/40 pasos · 21 s/10 min   ctx ▕█░░░░░▏ 31 %  externo
```

With 100 columns or more the timeline is on the left, with the context (by layers) and the usage against its limits on the right; with fewer, one column, and `c` and `U` open the full views:

```
clio · proyecto · beam-free                                                                                     en vivo

 ⠋ trabajando en la copia…   1031-1816-5a67   nivel Simple
 arregla parse_date

 ─ línea de tiempo ───────────────────────────────────────────────────────── │ ─ contexto ─────────────────────────────
   1  read_file  dates.py  ✓ permitido (step-8-reversib…)  1234 car.         │ contexto ▕███░░░░░░░▏ 31 %
   8  write_file .git/hooks/pre-commit  ✗ negado (S-16)                      │ 39,7k de 128k tokens (aprox.)
                                                                             │
                                                                             │ sistema         1,2k    0,9 %
                                                                             │ objetivo         120    0,0 %
                                                                             │ herramientas     410    0,3 %
                                                                             │ conversación    3,1k    2,4 %
                                                                             │ externo        34,9k   27,2 %
                                                                             │     dates.py          2k externo
                                                                             │     big.json       32,9k externo
                                                                             │ salidas            0    0,0 %
                                                                             │
                                                                             │ ─ uso ──────────────────────────────────
                                                                             │ ↑12,4k ↓3,1k tokens · $0
                                                                             │ pasos   8 de 40  ▕██░░░░░░░░▏ 20 %
                                                                             │ tiempo  21 s de 10 min  ▕░░░░░░░░░░▏ 3 %
                                                                             │ costo   $0 de $0,10  ▕░░░░░░░░░░▏ 0 %
                                                                             │
                                                                             │
                                                                             │
                                                                             │
 Ctrl-C cancelar   Tab tiempo / debug   c contexto   U uso   ? ayuda
 ↑12,4k ↓3,1k tok · $0 · 8/40 pasos · 21 s/10 min   contexto ▕███░░░░░░░▏ 31 %   externo
```

**Context** (`c`, live, in the detail and on the home screen with the last run): a view in LAYERS of what is in the model's context, each layer with its approximate size in tokens and its share of the window: the system prompt (with its version, for example `clio-agent@3`), the goal (your request, which is the contract's goal), the tool descriptions, the conversation (the model's answers and protocol notices, in turns), the OUTSIDE content (each file read, with its sanitized name, its tokens and the outside mark) and the output of the other tools. The layers add up to what the model gets in its next call. It warns quietly past 70 % and 90 %. On a finished run it shows the context of its last model call (without its last answer).

```
clio · proyecto · beam-free                                            contexto

 contexto ▕███████░░░░░░░░░░░░░░░░░▏ 31 %  39,7k de 128k tokens (aprox.)

 prompt de sistema        clio-agent@3      1,2k    0,9 %  ▕░░░░░░░░░░░░▏
 objetivo / contrato      tu pedido          120    0,0 %  ▕░░░░░░░░░░░░▏
 herramientas             descripciones      410    0,3 %  ▕░░░░░░░░░░░░▏
 conversación             7 turnos          3,1k    2,4 %  ▕░░░░░░░░░░░░▏
 externo                  2 lecturas       34,9k   27,2 %  ▕███░░░░░░░░░▏
     dates.py                                     2k externo
     big.json                                  32,9k externo
 salidas de herramientas                       0    0,0 %  ▕░░░░░░░░░░░░▏

 Aprox.: cuatro caracteres por token. Las capas suman lo que ve el modelo.
 U ver el uso   Esc volver   ? ayuda
 ↑12,4k ↓3,1k · $0 · 8/40 pasos · 21 s/10 min   ctx ▕█░░░░░▏ 31 %  externo
```

**Usage** (`U`): a compact table per step (model call): input and output tokens, cost, model time and the running totals; below, the total against the run's step, time and cost limits.

```
clio · proyecto · beam-free                                                 uso

 uso ↑12,4k ↓3,1k tokens · $0

 paso  entrada   salida      costo    modelo  acum.tok  acum.costo
    1      10k       2k         $0     1,2 s       12k          $0
    2     2,4k     1,1k         $0     2,1 s     15,5k          $0

 ─ frente al límite del run ───────────────────────────────────────────────────
 pasos   8 de 40  ▕██░░░░░░░░▏ 20 %
 tiempo  21 s de 10 min  ▕░░░░░░░░░░▏ 3 %
 costo   $0 de $0,10  ▕░░░░░░░░░░▏ 0 %

 Los tokens y el costo los informa el modelo; el contexto es aproximado.
 c ver el contexto   Esc volver   ? ayuda
 ↑12,4k ↓3,1k · $0 · 8/40 pasos · 21 s/10 min   ctx ▕█░░░░░▏ 31 %  externo
```

**Run detail**: summary, the agent's final message (wrapped), changed files (`M` modified, `A` new, `D` deleted) and, with `Enter` or `d`, the diff of the chosen file: `+` in green, `-` in red, `@@` dimmed. `a` applies and `u` undoes. Before applying, the confirmation box lists the files that will be written (`M` changed, `+` new, `-` deleted) and warns if the agent read outside content or if the run kept no record of that; the full diff is on this screen (`Enter` or `d`). In a chat session the warning about outside content comes from the tool service (which is what saw it: Pi could say it read nothing) and stays in the run folder (`toolhost/tainted.json`, only if it happened); if the service ran and left nothing, the box does not warn, and if there is no record at all it says so. Only a lone `y` or `s` confirms (Ctrl-S or Alt-Y do not count), and if the plan takes a while and you already changed screens, the box does not appear: you are told, and you ask again. It is the same code as `clio apply` and `clio undo`.

```
clio · proyecto · beam-free                                             detalle

 pendiente de revisar · 20261031-181646-5a67
 carpeta /home/ivan/proyecto
 pedido  arregla parse_date
 nivel Simple · 7 pasos · 12,3 s · terminó · 0 denegadas

 ─ mensaje del agente ─────────────────────────────────────────────────────────
 Arreglé parse_date.
 No pude correr las pruebas.

 ─ archivos cambiados (2) ─────────────────────────────────────────────────────
 ▸ M dates.py
   A docs/new.md
 Enter d ver el diff   a aplicar   u deshacer   c contexto   U uso   ? ayuda
 ↑12,4k ↓3,1k tok · $0 · 7/40 pasos · 12 s/10 min   contexto ▕███░░░░░░░▏ 31 %
```

```
clio · proyecto · beam-free                                             detalle

 M dates.py  · 1/2 · línea 1/7
 --- a/dates.py
 +++ b/dates.py
 @@ -1,3 +1,3 @@
  def parse(s):
 -    return int(s)
 +    return None
  x = 1
 ] otro archivo   a aplicar   u deshacer   Esc a la lista   ? ayuda
 ↑12,4k ↓3,1k tok · $0 · 7/40 pasos · 12 s/10 min   contexto ▕███░░░░░░░▏ 31 %
```

**Doctor** (`D`): each check with its mark (`✓` good, `!` warning, `✗` failure) and, under each warning or failure, a line with what to do (`r` repeats). Each check shows once: the permission and port checks `clio` makes itself are not repeated by `infra/local/check-host.sh`.

```
clio · proyecto · beam-free                                              doctor

 doctor: 1 ok · 1 aviso · 1 fallo

 ✓  /home/ivan/.config/clio es privada (700)
 !  conector opencode: la llave no se puede usar (no está en el baúl): clio connect opencode
    → Abre el baúl (`clio vault start`) y ejecuta `clio connect opencode`.
 ✗  /x/y tiene modo 755, debe ser 700 (S-23)
    → chmod 700 /x/y

 r repetir   Esc volver   ? ayuda
```

**Help** (`?`): the keys of the screen you are on, in one short view.

```
clio · proyecto · beam-free                                               ayuda

 atajos · Clio trabaja en una copia; tú decides qué vuelve al original

 j         bajar             q         salir
 k         subir             c         contexto
 g         primero           U         uso
 G         último            r         refrescar
 Enter     abrir             D         doctor
 n         nuevo             Esc       quitar el filtro
 d         diff              ?         ayuda
 a         aplicar           Ctrl-D    modo debug
 u         deshacer          Ctrl-L    redibujar
 /         filtrar
 Esc ? cerrar la ayuda
 ↑12,4k ↓3,1k tok · $0 · 7/40 pasos · 12 s/10 min   contexto ▕███░░░░░░░▏ 31 %
```

### Keys

The same on every screen: a key means the same thing wherever it exists (`c` context, `U` usage, `?` help, `r` refresh, `d` diff, `a` apply, `u` undo, `g`/`G` start and end, `j`/`k` move, `m` memory, `p` approve a memory; the only exception is the Memory screen, where `a` archives, because there are no runs to apply). A test walks each screen's table and fails if a key is repeated or means different things. `Esc` or `q` go back (on the home screen `q` quits; `Esc` only clears the filter); `Ctrl-C` cancels the live run (the second time it quits) and on the other screens does the same as `Esc`. Pages with `Ctrl-F`/`Ctrl-B` (or `PgDn`/`PgUp`): `Ctrl-D` cannot be "half a page" because it is debug mode. To avoid clashes: `U` (capital) opens usage and `u` undoes.

- **Home**: `j` `↓` down · `k` `↑` up · `g` `Home` first · `G` `End` last · `Enter` open · `n` new · `d` diff · `a` apply · `u` undo · `/` filter · `q` `Ctrl-C` quit · `c` context · `U` usage · `m` memory · `r` refresh · `D` doctor · `Esc` clear the filter · `?` help.
- **Home, typing the filter**: `Enter` keep the filter · `Esc` clear the filter · `↓` down · `↑` up · `Ctrl-C` clear the filter.
- **New job**: `Tab` `Shift-Tab` switch field · `Enter` launch · `Alt-Enter` `Ctrl-J` new line · `Ctrl-U` clear the field · `F1` help · `Esc` `Ctrl-C` back.
- **Live, running**: `Esc` `Ctrl-C` cancel · `Tab` timeline / debug · `c` context · `U` usage · `j` `↓` newer · `k` `↑` older · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `?` help.
- **Live, finished**: `Enter` see the run · `Esc` `q` `Ctrl-C` home · `Tab` timeline / debug · `c` context · `U` usage · `j` `↓` newer · `k` `↑` older · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `?` help.
- **Detail, file list**: `j` `↓` down · `k` `↑` up · `g` `Home` first · `G` `End` last · `Enter` `l` `→` `d` see the diff · `a` apply · `u` undo · `c` context · `U` usage · `Tab` pick a memory used · `+` the chosen memory was useful · `-` the chosen memory was wrong · `?` help · `Esc` `q` `Ctrl-C` back.
- **Detail, diff**: `j` `↓` down · `k` `↑` up · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `]` next file · `[` previous file · `a` apply · `u` undo · `c` context · `U` usage · `?` help · `Esc` `q` `h` `←` `Ctrl-C` to the list.
- **Doctor**: `j` `↓` down · `k` `↑` up · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `r` repeat · `?` help · `Esc` `q` `Ctrl-C` back.
- **Context**: `j` `↓` down · `k` `↑` up · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `U` see usage · `?` help · `Esc` `q` `Ctrl-C` `c` back.
- **Usage**: `j` `↓` down · `k` `↑` up · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `c` see context · `?` help · `Esc` `q` `Ctrl-C` `U` back.
- **Help**: `j` `↓` down · `k` `↑` up · `g` `Home` the start · `G` `End` the end · `Ctrl-F` `PgDn` page down · `Ctrl-B` `PgUp` page up · `Esc` `q` `?` `Enter` `Ctrl-C` close help.
- **Memory**: `j` `↓` down · `k` `↑` up · `g` `Home` first · `G` `End` last · `Enter` see it whole · `p` approve · `a` archive · `y` copy · `/` search · `r` refresh · `?` help · `Esc` `q` `Ctrl-C` back.
- **Memory, typing the search**: `Enter` keep the search · `Esc` clear the search · `↓` down · `↑` up · `Ctrl-C` clear the search.
- **Chat, with the sessions open (Ctrl-S)**: `j` `↓` down · `k` `↑` up · `g` `Home` first · `G` `End` last · `Enter` open · `n` new · `x` delete · `Esc` `Ctrl-S` `F2` close · `r` refresh · `D` doctor · `m` memory · `q` `Ctrl-C` quit · `?` help.
- **Chat, waiting**: `Enter` send · `Ctrl-S` `F2` sessions · `Alt-Enter` `Ctrl-J` new line · `←` previous character · `→` next character · `Home` `Ctrl-A` start of the line · `End` `Ctrl-E` end of the line · `Ctrl-←` `Alt-←` previous word · `Ctrl-→` `Alt-→` next word · `Backspace` delete back · `Del` delete forward · `Ctrl-W` `Alt-Backspace` delete the word · `Ctrl-U` delete to the start of the line · `↑` line above, or scroll the conversation up · `↓` line below, or scroll the conversation down · `Ctrl-B` `PgUp` page up · `Ctrl-F` `PgDn` page down · `Ctrl-Home` the start of the conversation · `Ctrl-End` the latest · `Ctrl-C` quit · `F1` help.
- **Chat, the agent working**: `Esc` `Ctrl-C` stop · `Enter` steer the agent · `Ctrl-S` `F2` sessions · `Alt-Enter` `Ctrl-J` new line · `←` previous character · `→` next character · `Home` `Ctrl-A` start of the line · `End` `Ctrl-E` end of the line · `Ctrl-←` `Alt-←` previous word · `Ctrl-→` `Alt-→` next word · `Backspace` delete back · `Del` delete forward · `Ctrl-W` `Alt-Backspace` delete the word · `Ctrl-U` delete to the start of the line · `↑` line above, or scroll the conversation up · `↓` line below, or scroll the conversation down · `Ctrl-B` `PgUp` page up · `Ctrl-F` `PgDn` page down · `Ctrl-Home` the start of the conversation · `Ctrl-End` the latest · `F1` help.
- **Everywhere**: `Ctrl-D` debug mode · `Ctrl-L` redraw.

### Debug mode

It turns on with `--debug` (in `clio` and in `clio run`), with `CLIO_DEBUG=1` (any value but empty, `0`, `false` or `no`) or with `Ctrl-D` inside the interface, which toggles it at any time, also with a run going (what happened before you turned it on has no detail). With debug on, `Tab` on the live screen switches between the timeline and the debug view, which shows for each model call:

- how the context was built (the layers and each outside file), turn by turn;
- the request (only the messages new in that call) and the answer, cut with a note of how much is missing; long lines continue on the next line instead of being cut;
- what the memory recalled at the start (id, text and vector scores, or that it abstained) and what it proposed to save at the close (one line per memory, with its state), as a memory block;
- each gateway decision with its tool, arguments, rule and full reason, and how long it took;
- the model time, tokens, cost and retries (when the adapter failed and Clio called again), and protocol errors.

```
clio · proyecto · beam-free                                       debug en vivo

 ⠋ trabajando en la copia…   1031-1816-5a67   nivel Simple
 arregla parse_date

 ─ debug: lo que vio y dijo el modelo (saneado) ───────────────────────────────
 llamada 2
   contexto 39,7k tok aprox.: sistema 1,2k (clio-agent@3) · objetivo 120 · herr
       amientas 410 · conversación 3,1k (7 turnos) · externo 34,9k (2) · salida
       s 0
     externo: dates.py 2k
     externo: big.json 32,9k
   modelo 2,1 s · entrada 2,4k · salida 1,1k tok · $0
   petición: 3 mensajes en total (4200 car.), nuevos en esta llamada: 1
     [user] Result of read_file:
       hola
   respuesta: 21 car.
     {"tool":"write_file"}
   pasarela #1 write_file {"path":".git/hooks/x"}
     negado · regla S-16 · 3 ms
     razón: the path is protected (S-16)
 Ctrl-C cancelar   Tab tiempo / debug   c contexto   U uso   ? ayuda
 ↑12,4k ↓3,1k · $0 · 8/40 pasos · 21 s/10 min   ctx ▕█░░░░░▏ 31 %  externo
```

With debug on, a `debug.log` is also written in the run folder (`~/.local/share/clio/runs/<id>/debug.log`): JSON per line (`started`, `context`, `memory`, `usage`, `exchange`, `step`, `decision`, `tool`, `protocol_error`, `final`), mode 0600 from birth, with every long text cut at 8000 characters and a note of how much is missing. In `clio run --debug` the same lines (sanitized) go to standard error with the prefix `[debug]`, and the log's path is printed at the start. **Without debug none of this is written.**

**What it guarantees.** Everything in the view, in `debug.log` and on standard error goes first through the secret scrubber (`Scrubber`), which knows the model key (the connector's variable or the vault's) and recognizes a known secret in clear, in base64 and URL-encoded; then through terminal sanitizing (escape sequences, `\r`, bidirectional characters). The API key never shows. The tests check it with decoys (`clio-canaries`) in the model's answer, in a file read, in a file name, in the gateway's arguments and reason and in a protocol error: neither the screen (at four sizes), nor `debug.log`, nor standard error, nor `summary.json`, `transcript.json` or `usage.json` contain them. Limit: the scrubber only knows the secrets it was taught (the model key and what the vault hands it) and the providers' key patterns; a secret Clio does not know and that has no key shape cannot be hidden.

### What each run keeps

Besides the usual, each run keeps `usage.json` (0600): the limits it ran with, the model's window, the usage of each call (tokens, cost in micro-USD, time, retries) and the context's layers before the last call, with the names of the files read. It feeds the bar and the `c` and `U` views of a finished run; a run older than this feature does not have it and the view says so (the totals come from its `summary.json`). A run's final message, report and transcript also go through the secret scrubber before they are saved.

**Colors and symbols**: by default, the 8 basic ANSI colors plus bold and dim, so your terminal's palette picks the shades (it works on dark and light themes and without 24 bits). If the terminal declares 24-bit color (`COLORTERM=truecolor` or `24bit`), it uses a palette of its own (violet accent, soft green, amber and red, grey for what is dimmed) only for text: it never changes the background or the normal text color. With `NO_COLOR=1` or `TERM=dumb` there is no color; if the terminal does not declare UTF-8, or it is the Linux console, the symbols (`·` `▸` `✓` `✗` `↑` `↓` `─` and the bars; in the chat also `▌` `•` `›` `●` and the rounded borders) become ASCII (`-` `>` `+` `x` `^` `v` `-` `[###...]`; `|` `*` `>` `*` and `+-|`).

**Screen safety.** What comes from files, the model, file names, diffs, the instruction, the answers and reasons of debug mode or the filter can carry escape sequences (change the window title, clear the screen, fake another screen), a `\r` that overwrites a line, or bidirectional characters (like U+202E) that reverse the text. Before drawing or printing any outside text, Clio removes whole escape sequences (CSI, OSC, DCS, APC...) and replaces the other control characters, line separators and invisible or bidirectional characters with `�`; a tab becomes four spaces. This holds on every screen (home, new job, live, context, usage, debug, detail, doctor, help) and for what `run`, `runs`, `show`, `diff`, `apply` and `doctor` print in a terminal. The run's files keep the text as it came (minus secrets). Limit: zero-width joiners (ZWJ) are replaced too, so a composed emoji can look like several. If Clio falls over with an error, the terminal is restored before the message shows (it leaves the alternate screen and raw mode); it cannot do so if it is killed with `kill -9` or a signal; then type `reset`. `Ctrl-L` redraws the whole screen (clears the terminal and repaints every cell).

## Memory: what Clio remembers

Clio remembers across runs, **all on your machine**: one file, `~/.local/share/clio/memory.db` (0600, created by `clio init` or the first run, and nowhere else). No network. The embedding model is optional and is the one clio-memory picks: **Granite Embedding 311M** (`clio-memory setup model`, which also fetches what it needs to run, on the CPU), otherwise the static `potion-multilingual-128M` (`clio-memory setup model potion` or `just setup-model-linux`). With either, search also uses meaning; if Granite does not load, it uses potion; with neither, it searches by text only and the interface says so in a quiet line. It never fails because of that. (Loading the model takes seconds and memory. If the `clio-memory` binary is on your `PATH`, Clio does not load it: it asks the clio-memory daemon for vectors, the process that holds the only copy of the model for all your agents, and starts it the first time it needs a vector. Without the binary, with `CLIO_MEMORY_NO_DAEMON=1`, or if the daemon does not have the same model, Clio loads it, once per run and only if there is already something to recall; Pi's tool service and the sleep always load it themselves. `clio` starts with ONNX Runtime's telemetry off: if `ORT_DISABLE_TELEMETRY=1` is not set, it runs itself again with it.)

**What it remembers.**

- **What you tell it with `clio memory remember "text"`**: a fact of yours (origin `persona`), active at once. With `--block identidad|reglas|preferencias|vigilancia` it also goes into one of the primordial memories (below).
- **One line per run** (episodic): what you asked, which files it changed, how it ended and an excerpt of what it said. It stays **active**, with origin `agente` if the run never read outside content (no files, no test output) or `externo` if it did.
- **What the model asks to save** in its final message with a line that starts with `RECORDAR:` (a fact) or `PROCEDIMIENTO:` (a "how it is done"). Up to 5 per run; the rest is dropped. Active, always with origin `externo`: the loop of `clio run` does not show them (the Pi chat does, as data).

**The primordial memories: who Clio is and who you are.** They are four blocks of the memory that only you set: `identidad` (who Clio is and how she speaks), `reglas` (how she works and her limits), `preferencias` (who you are) and `vigilancia` (what she watches, one thing per entry). Clio ships with two entries of `identidad` and two of `reglas`, written the first time you open a Pi session with a memory that has neither. The first three go whole to the model in the first message of every session (also when resuming it and after a compaction), under "who you are and how you work (set by the person; follow it)" and "who the person is (as they said)": not as loose data, but as what the model follows; `vigilancia` is not sent every time, Clio reads it when she needs it. The first time you talk to Clio and the memory does not know who you are, your first message carries the request to introduce herself: in her answer she introduces herself and asks you four questions (your name and how you want to be addressed, what you do and what is on this machine, what you want her to watch, what counts as important enough to tell you); if you close without answering she asks again in the next session, and `clio run` never does it. **You change them by talking to Clio** ("from now on address me formally", "stop watching the disk"): she uses `memory_block_edit`, and saves it as said by you, only in a **clean** session; if the session already read outside content (a file, command output, a memory that is not yours), she tells you to open a new session and ask there. Her usual tools (`memory_update`, `memory_forget`, `memory_consolidate`) do not touch an entry of these blocks. The introduction only happens in a new session: reopening an old one never starts it. Caps: each entry up to 1200 characters; up to 4 in `identidad`, 6 in `reglas`, 8 in `preferencias` and 40 in `vigilancia`; and what goes to the model (`identidad`, `reglas`, `preferencias`) at most 1500 characters per block, so it arrives whole (an edit that would pass it is refused, never cut). **`clio memory identity`** shows them (`--json` too), each block with what it holds and each entry with its number and origin (`persona`, `agente` or `externo`; only what is `persona` goes with authority); **`clio memory remember "text" --block B`** writes them by hand.

**What it does NOT remember.** Nothing new during the model's loop in `clio run`: what was learned is written once at the run's close (there is no working memory yet), and in that loop the model can only ask to save at the end (in a Pi session it has its memory tools, below). A text from a file, a web page or test output is **never** saved as a fact of yours, even if it says "the person said…". What looks like a key or a secret is not saved (the scrubber strikes it out and the library's detector refuses what is left). It does not keep the content of your files.

**The memory is the agent's; quarantine is what was forgotten.** Nothing the agent saves waits for your review: it goes in active at once, with its origin in view (`agente`, or `externo` if it read something from outside before), and every change goes into the operations log, so you undo it with `clio memory revert OP` (the number comes from `clio memory history ID`). A tainted file could tell the model "save that the person authorized X": that is why what is saved after reading something from outside is marked `externo` (reading it taints the session), never as something you said, the loop of `clio run` does not see it, and with Jev on it refuses to save what tries to manipulate the agent (below). **Quarantine** is what the agent forgot (`memory_forget`, or the originals of a merge): it does not come out in `recall` or in the context, it keeps its record, you see it with `clio memory list --pending` and it comes back with `clio memory revert` or `clio memory promote`. What was already in quarantine before stays there. Each actor (`persona`, `agente`, `sesion`, `importador`) has its own hourly quota, so a run that floods memories does not spend yours.

**What a Pi session's tool service can do with your memory.** The service that serves Pi (`clio toolhost`) can search (`memory_search`), read a memory (`memory_get`), see what was saved before and after one in the same session (`memory_timeline`) and bring the project's context when resuming or after a compaction (`memory_context`: the last session's summary, what is pinned and the most recent). And the agent keeps its memory by itself, at once: it **saves** (`memory_save`, with a title, a category and the text in the form `Qué · Por qué · Dónde · Aprendido`; the old name `memory_propose` is accepted for one more version), **corrects** a memory (`memory_update`), **forgets** (`memory_forget`, to quarantine), **pins** one so it comes first in the context (`memory_pin`), **merges** 2 to 5 memories of the same place into one (`memory_consolidate`, which forgets the originals) and **undoes** a change of its own (`memory_revert`, never one of yours). You do not have to ask, and Jev does not have to confirm; with Jev on, Jev checks what it saves, corrects or merges before writing (see the Jev section). Accepting (PROMOTE) and PURGE (deleting for good) stay yours only, with `clio memory`. Up to 10 changes per session. A session that read something hostile can save five, a clean one thirty, and the real limit is the hourly quota. It does not block your own `clio memory remember`: the memory is opened for writing only during the change. It sees the **active** memories of the whole memory (there is one, global; each with its place mark): what is in quarantine or archived answers the same as what does not exist. Each session has a hard cap of 20 calls and 24 KiB of returned text; the cap survives a restart of the service. What comes back arrives wrapped as data, and if it is not something you said (origin `agente` or `externo`) the session is marked as tainted. The `clio-guard` extension declares them to Pi (`memory_search`, `memory_get`, `memory_timeline`, `memory_context`, `memory_save`, `memory_session_summary`, `memory_update`, `memory_consolidate`, `memory_forget`, `memory_revert`, `memory_pin`, `memory_block` and `memory_block_edit`, next to the six file tools and `ask_person`, `request_secret` and `todo_write`: twenty-two in all, twenty-four in host mode with `job_output` and `job_stop`; the prompts are `clio-agent@20` in copy mode and `clio-host@4` in host mode); the service offers the memory tools only if it starts with `--memory-data`.

**Project, personal and lessons.** Your memory is **one and global**: any session, in any folder, reads everything active. What changes is each memory's **mark**, which says which place it belongs to: the **personal** part (who you are, your preferences), the **project** (decisions, architecture, configuration; in that project's session it comes first and unnamed, from another it shows as "project X") and the lessons. What is new is saved in the session's project; Clio recognizes it from the session's folder: the `CLIO_PROJECT` variable if you set it; otherwise a `.clio-project` file in the folder or higher up inside the repository (`clio memory project set NAME` creates it; `clio memory project` says which it is and where it comes from); otherwise the name of the repository of git's `origin` remote, the repository's root folder or the folder itself. The model cannot choose it, and `clio apply` never applies a `.clio-project` a session wrote. A detected project is remembered per repository (in `projects.json`, next to the memory): if one day another is detected (you changed the remote, or cloned another repository at the same path), Clio uses the current one, so as not to mix two repositories in one memory, and tells you (`clio memory project`, and the MCP client's log) with the command to join them if they are the same (`clio memory project set PREVIOUS-NAME`). **Lessons** (mistakes and their cause, patterns, traps) are marked as a lesson of their project, to learn from what happened elsewhere. Where a new memory goes is decided by Jev from its category and its text; without Jev, the rule: `profile` (who you are: name, where you live, what you do) and `preference` (how you want answers, writing and work) go to the personal part, `bugfix`, `pattern` and `learning` to the lessons, the rest to the project. What you already had stays in the personal part.

**A summary of each session.** Like Engram, Clio keeps a summary of what was done in a Pi session (goal, what was done and checked, what was found, next steps, files) to resume it later: it is the project's episode, titled with the session's date, and it is the first thing `memory_context` brings. The model writes it with `memory_session_summary` and it is saved at once. Clio asks for it at two moments: when Pi compacts the conversation (in the next message) and when you close a session in which you wrote at least four messages (a last short turn, up to four steps and 30 seconds, with no words of yours; in that turn the service only lets it save the summary: no files, no commands, no memory changes with what you said before; if you leave the TUI it tells you it is closing the session, and a session that is closing is not reopened until it ends). Without a project it goes to the personal part. If the model is in the middle of a turn when you leave, there is no summary. Clio's requests are turned off with `"memory": {"session_summary": false}` in `~/.config/clio/config.json`; the model can still save a summary by itself after a long piece of work. Your messages are not kept apart: Pi already has them. You see everything: `clio memory` and the TUI's memory screen list, search, archive, revert and purge any project's (marked "project X"), as the model of any session also sees them.

The point-by-point comparison with Engram is in `docs/specs/2026-11-04-engram-comparacion.md`.

**Your memory in other agents (MCP).** The recommended way is [clio-memory](https://github.com/i-montes/clio-memory), Clio's memory engine in its own repository: `clio-memory setup claude-code` makes it Claude Code's main memory (an MCP server with its 12 tools, including correct, forget, pin, merge and undo, and a skill with its safety measures), over the same file Clio uses. Clio also brings its own, narrower server: `clio memory mcp` is an MCP server (JSON-RPC over standard input and output) for another agent to read and save in your memory through Clio's tool service (for example, `claude mcp add clio-memory -- clio memory mcp`; do not register it under the same name if you already use `clio-memory setup`). It offers `memory_search`, `memory_get`, `memory_timeline`, `memory_context`, `memory_save` (it also accepts `memory_propose`), `memory_session_summary` and `memory_current_project`; correct, forget, pin, merge and undo are in clio-memory's server. Accepting, archiving and purging are yours, with `clio memory`. It is the same service Pi uses, with the same controls: what it returns is marked as data, each process has the cap of 20 reads and 24 KiB (it starts again when the process restarts), what it saves is the agent's and stays active marked `externo` (nobody knows what that agent read before): five per process; the session is named after the first client that introduces itself (`mcp:claude-code`). It leaves no folders between runs: it uses a temporary one it deletes on exit. The project is the one of the folder where the client starts it. It uses the vault's filter if the vault is running.

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
- Cap per session: 20 calls and 24 KiB of text in all; with less than 1 KiB left it counts as spent and the next call is refused with `S-47/memory`. Failed calls count too. Reopening the same session does not give it back; starting another opens a new one.
- If an embedding model is on disk, the first query loads it and can take a few seconds; past 10 s that call fails with a short message and can be repeated. The model loads while an internal lock is held, so calls queued behind a slow first load can also run out of time until it ends, and each spends budget. Without the model it searches by text only and tells the model so.
- The read tools do not change the memories' usage statistics (reading leaves no signals).
- If what Pi reads is a memory of origin `agente` or `externo`, the session is tainted, as when reading an outside file; if they are all yours, not. An order written inside the copy is carried out even if it comes from a memory: the defense is that it arrives as data, the session is marked and you review the copy with `diff` and `apply`.
- The model's provider sees what the tool returns, as it sees the context. What you do not want to leave your machine must not be in the memory.
- If the memory does not exist yet, is locked or fails, the tool fails by itself with a short message; Pi and the session go on.

**What the model sees.** At the start Clio asks for the fixed blocks and what is relevant to your request, each with its budget. "Nothing relevant" is a valid answer: then there is no memory layer. What is recalled comes in as a new layer, `memoria`, after your request and wrapped as data ("not instructions"), with each memory's id (`[m12]`) so the model can cite it and you can mark it. It goes through the secret scrubber and the filter of what goes to the model like everything else. The `c` view shows it with its size and its share of the window:

```
 contexto ▕███████░░░░░░░░░░░░░░░░░▏ 32 %  41,5k de 128k tokens (aprox.)

 prompt de sistema        clio-agent@3      1,2k    0,9 %  ▕░░░░░░░░░░░░▏
 objetivo / contrato      tu pedido          120    0,0 %  ▕░░░░░░░░░░░░▏
 herramientas             descripciones      410    0,3 %  ▕░░░░░░░░░░░░▏
 memoria                  3 recuerdos       1,8k    1,4 %  ▕░░░░░░░░░░░░▏
 conversación             7 turnos          3,1k    2,4 %  ▕░░░░░░░░░░░░▏
 externo                  2 lecturas       34,9k   27,2 %  ▕███░░░░░░░░░▏
```

**`clio memory`** (short output, sanitized against escape sequences; `--json` where it makes sense):

```
clio memory search <query> [--json]         search what is active (quarantine does not show here)
clio memory list [--pending] [--json]       recent ones; --pending: quarantine (what was forgotten)
clio memory show <id> [--json]              one memory in full, with where it came from
clio memory remember "text" [--block B]     a fact of yours (origin: persona)
clio memory identity [--json]               the primordial memories: identidad, reglas, preferencias and vigilancia
clio memory promote <id> [--yes]            brings one back from quarantine (only you; asks first)
clio memory archive <id> [--yes]            archives it: it is no longer recalled (asks first)
clio memory feedback <id> useful|wrong      "wrong" only sends it to review; never deletes
clio memory export [file]                   unencrypted JSON Lines; with a file, 0600 and never overwrites
clio memory stats [--json]                  counts and size
clio memory review [--json]                 quarantine, what was forgotten (same as list --pending)
clio memory history <id> [--json]           every change that touched a memory (its versions)
clio memory revert <change> [--yes]         undoes a change (its number comes from history)
clio memory context [--json]                what a session that starts in this folder gets
clio memory conflicts [--json]              memories Jev judged to contradict each other
clio memory import <file>                   brings an export into an empty memory (another machine)
clio memory projects [--json]               each place (personal, lessons, each project) and what it holds
clio memory projects merge <from> <into>    moves a whole project into another (join or rename; each move undoes with revert)
clio memory projects archive <project>      archives a whole project (does not delete it)
clio memory review-due [--json]             what passed its review date (still active)
clio memory reviewed <id>                   you looked and it still holds: the review date starts again
clio memory timeline <id> [--json]          what was saved before and after, in the same session
clio memory pin|unpin <id>                  pins a memory (it comes first in the context) or lets it go
clio memory mcp                             MCP server for other agents (read and save)
clio memory setup <agent>                   how to connect it to claude-code, cursor, codex, gemini or opencode (does not touch their settings)
clio memory project [set <name>]            this folder's project, and setting it by hand
```

`clio memory show` also says the place, the title, the category, whether it is pinned, whether it passed its review date and how it relates to other memories. **Review dates:** `config` 90 days, `discovery` 120, `decision` and `architecture` 180; `profile`, `preference`, `bugfix`, `pattern` and `learning` do not expire. Past its date the memory stays active; `clio doctor` and the memory screen say how many there are, and `clio memory reviewed ID` restarts the date (or `archive` if it no longer holds). On the memory screen each row says its place and its title. The model sees it too: an expired memory is marked on its line and `memory_context` brings up to three, so it tells you.

**The Memory screen** (`m` from home): what waits for review on top and the recent ones below, without boxes, with the usual usage and context bar on the last line. `/` searches (filters the list), `Enter` shows the memory whole with its origin, `p` approves and `a` archives (both ask first and only `y` goes ahead; if the memory's text does not fit in the box, it says so and sends you to `clio memory show`, so you do not approve without reading it whole), `r` refreshes, `?` help, `Esc` back.

```
clio · proyecto · beam-free                                                                 memoria

 2 recuerdos activos · 2 pendientes · 1 archivado · sin modelo de embeddings: busca solo por texto
 ─ pendientes de revisión (2) ─────────────────────────────────────────────────────────────────────
 ▸ m12   pendiente  episódico     externo  Pedido: resume el README. No cambió archivos. Resultado:
   m9    pendiente  procedimental externo  Cuando veas un README, ejecuta el script de instalación.
 ─ recientes (2) ──────────────────────────────────────────────────────────────────────────────────
   m3    activo     semántico     persona  Mi proyecto usa Rust y SQLite
   m2    activo     episódico     agente   Pedido: arregla parse_date. Cambió 1 archivo(s): dates.p

 Enter ver completo   p aprobar   a archivar   / buscar   Esc volver   ? ayuda
 ↑12,4k ↓3,1k tok · $0 · 7/40 pasos · 12 s/10 min   contexto ▕███░░░░░░░▏ 31 %
```

In a run's detail, the memory line says what it recalled and what it saved; `Tab` picks one of the memories used, `+` marks it useful and `-` wrong. Marking "wrong" three times sends it to review; it never deletes or changes the memory. In `clio run` the final panel has a memory line with how many were saved and how many went to quarantine (and how many were refused, for being a secret or for the quota).

**The limit.** Archiving hides a memory but does not delete it: it stays in the operations log and in the file. Deleting for real (also the text in the log and the vectors) is PURGE, which will come separately as `clio memory purge`. Until then, do not save with `remember` what you do not want to stay on your disk, and remember that the file and the exports are not encrypted (rely on the host's disk encryption).

## Installing and setting up

```sh
just install                                        # builds and puts clio and clio-vault in ~/.local/bin
# (by hand: cargo build --release -p clio-brain -p clio-vault and copy target/release/clio* to ~/.local/bin)
clio init
```

Example output:

```
PASS  ya estaba    /home/tu/.config/clio (0700)
PASS  creada       /home/tu/.local/share/clio (0700)
PASS  creada       /home/tu/.local/share/clio/runs (0700)
PASS  creada       /run/user/1000/clio (0700)
PASS  creada       /home/tu/.config/clio/config.json (0600)
WARN  falta la llave del modelo: abre el baúl (clio vault start) y ejecuta clio connect opencode

siguiente: clio doctor, y luego clio
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
run 20261031-191737-ccb5: 2 archivos copiados de /home/tu/proyectos/mi-app
  WARN NO se copió, parece un secreto: .env
  - no se copió (carpetas pesadas o internas): .git
trabajando en la copia (puede tardar unos minutos; hasta 40 pasos o 600 s)...
    0s  [0/40] nivel de la tarea: Simple
    0s  [1/40] read_file app.py  permitido (step-8-reversible-in-sandbox)  193 car.
    0s  [2/40] write_file app.py  permitido (step-8-reversible-in-sandbox)  179 car.
    0s  [3/40] write_file .git/x  negado (S-16)
    0s  [4/40] el modelo dice que terminó

run:      20261031-191737-ccb5
nivel:    Simple   pasos: 4   tiempo: 0.1s   fin: terminó
cambios:  1 modificados, 0 creados, 0 borrados
  M app.py
nota:     externo: leyó archivos del proyecto; revisa el diff con cuidado
agente:   Corregí total() en app.py: ya no suma 1 de más. No pude correr las
          pruebas; ejecútalas antes de aplicar.

El original NO se tocó. Revisa:  clio diff 20261031-191737-ccb5   y luego  clio apply 20261031-191737-ccb5
```

(This is the output of `clio run` without a terminal, with a fake model that follows a script; with a real model the steps and the text change. The "not copied" lines show only when there is something to leave out. There is one line per step instead of minutes of silence; in a terminal with color the decision shows in green, yellow or red. `clio run` exits with 0 if the agent finished and 1 if not.)

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
RUN                   ESTADO                ARCHIVOS  INSTRUCCIÓN
20261031-191737-ccb5  pendiente de revisar         1  arregla total en app.py
```

The states are: pending review (it ended and there are changes), no changes, failed (cut by a limit, the model failed or it stopped), cancelled, applied, undone and incomplete (the process died before closing). The words on screen are still in Spanish; the stable words are the JSON ones below.

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

The vault keeps the model key (`clio connect`, above) and, if you want, other keys:

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

When it opens, `clio` is the chat alone, no lists beside it: a new session ready to type into. You type and press `Enter`. The session (its run and Pi) is created with that first message, so opening Clio leaves nothing on disk; the first time, Clio introduces herself in that first answer.

**The screen.** At the top, one line: `clio`, the session's title (or a new-session label) and, on the right, the model; if something is wrong (the configuration does not load, the model key is missing, the vault is stopped, the doctor found failures, or debug mode) it says so there, in yellow, before the model. Below, a thin line. The conversation uses the terminal's whole width (with a one-column margin on each side): your message in bold with a colored bar on the left; the agent's answer with its formatting (paragraphs, bold and italics, inline code in another color, headings, lists with `•`, quotes, code blocks with their language and a thin bar, links underlined with the address dimmed beside them; never as a terminal hyperlink); tables with aligned columns, the header in bold over a thin line and, if they do not fit, the widest columns narrow and their text wraps inside the cell (never cut); one line per tool (✓ green, ✗ red, ◇ yellow if it was set aside waiting for your approval, or the spinner while it runs; what it does, with a group's titles at one width up to 30 columns so the dimmed, summarized commands start in the same column; on the right, aligned, failed or denied in red, set aside in yellow, and the duration; a tool still running when its turn settles or is stopped is drawn as stopped, in the warning color); quiet one-line notes and errors in red. With the draft empty, in the middle: a greeting and where the agent works (on this server in host mode, on a copy of a folder in copy mode). If you are not at the bottom, a scroll bar on the right and a "more below" mark. Above the box you type in, a line says what is happening (working, with the time and that Esc stops; starting; aborting; or the last notice), and in host mode how many background jobs run (`1 job`, `2 jobs`). Above that line, when the model keeps a checklist (`todo_write`), the list: at most six rows, then `+N more`, done items dimmed with a check, the one in progress in the accent color. The box you type in (`›`) grows from one to six lines and then scrolls; the cursor is the terminal's and moves and edits with the usual keys (table above). At the very bottom, one line: a few keys on the left and `↑input ↓output · context N %` on the right (yellow from 70 %, red from 90 %; the cost only if it is not zero).

**The sessions** are in a panel that opens with `Ctrl-S` (or `F2`) and closes with `Ctrl-S`, `F2` or `Esc`: about 38 columns on the left, or the whole width if the terminal has fewer than 80. On top, a new-session row; then each session of Clio's Pi folder on two lines (its title and, dimmed, its age and message count), the open one marked with `●` and the chosen one highlighted; while the panel is open the ages are redrawn at least every 30 s. `↑` `↓` (or `j` `k`) choose, `Enter` opens the chosen one, closes the panel and leaves the keys in the chat, `n` starts a new one, `x` deletes (not the open one; it asks first). Pi sessions that are not Clio's are marked as such and do not open (remote sessions, through `ssh <host> clio serve-rpc`, do not exist yet and are not listed). The session stays open while you look at the panel and closes (saving its summary) when you open another or quit. While the agent works, while a session opens or while what you approved runs, you cannot switch sessions, start a new one, delete one or quit: `Esc` in the chat stops the turn first; the rest waits, and the panel or the status line says so. **The mouse** is on: a click on a panel row opens it, a click outside the panel closes it and the wheel scrolls what is under the pointer; to select text, hold `Shift` while dragging. A click or the wheel never answers the question about an action set aside: only keys do.

**The questions Clio asks you** come in a centered box, one at a time; one that arrives while another is open shows when that one closes.

- **An action set aside** (host mode): the action's class in yellow on its own line and the command under it. The question hears its first key only after it has been on screen for a second and a second passed with no key (meanwhile the box says to wait a second without typing), so neither the tail of what you were typing nor a `y⏎` typed in a pause answers it before you see it: then `y` (or `s`) arms it and `Enter` right after runs it; `n` or `Esc` reject it. Any other key after the `y` disarms it and the wait starts again. If the session became `suspect`, the box shows the reason.
- **`ask_person`**: a box with an accent border (not the warning one), the question and 2 to 4 answers, the recommended one first, and "Other…" last, which opens a one-line field for your own words. Arrows and `Enter`, or the answer's digit; `Esc` skips the question without stopping the turn. Keys count only after the same quiet second; a click only selects, and only the row drawn there. If nobody answers in ten minutes, or the turn settles, the box goes and the model decides by itself and says what it assumed.
- **`request_secret`**: at the end of the turn, a box with the secret's name, the one place it can be sent (in the warning color), the model's reason, a line when the session is suspect, and a masked field (one dot per character; pasting is accepted only here; the same quiet second first). `Enter` stores the value in the vault, through the vault's person socket, from Clio's own process: it never replaces a name that already exists and never passes through Pi, the guard, the tool host or the model. `Esc` declines. The model learns with your next message that the secret was saved or declined.

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
```

- It needs **Node 22.19 or newer** on the `PATH` (mise, nvm or your system's manager) and the network the first time. If something is missing, the message says what to do. `clio init` never fails because of Pi: it warns and you can repeat with `clio pi install`.
- Pi lives only in Clio's folders, all 0700: `~/.local/share/clio/pi/1.4.0/` (the install, from the `package-lock.json` embedded in the binary, without install scripts), `pi/agent/` (`models.json`, without the key), `pi/sessions/` (the sessions, 0600) and `pi/home/`. Your `~/.pi` (your own Pi's) is neither touched nor read, and `clio doctor` checks it.
- The model key is written to no file and no command line: it goes only in the environment of Pi's process, which starts with an empty environment (no `SSH_AUTH_SOCK`, no `HOME` of yours, no other variables of yours). With the jail, that environment is `bwrap`'s and not its arguments: a process's arguments can be read by any user of the machine with `ps`.
- **Deleting sessions:** `clio pi sessions rm ID…` (the id or the file name; `--all` for all) or the `x` key in the TUI's session list. It deletes the conversation's file and, if the session is Clio's, its working copy (`runs/<id>`); first it says what goes and, if the copy has changes you did not apply to your folder, it warns and asks you to confirm. Your memory is not touched. A session that is closing (saving its summary) is deleted when it ends.
- `clio pi sessions` reads only the header and the minimum of each file (it opens instantly with hundreds); titles and names are cleaned of escape sequences; a broken file is ignored with a warning. The message count carries `~` when it is an estimate (big files).

### Chat commands

A `/` as the first letter of an empty message opens, over the box you type in, the list of commands: each with what it does, filtered by its name, its aliases or the description as you type. `↑` `↓` move, `Tab` completes the name, `Enter` (or a click) runs it and `Esc` closes the list and keeps the text. A command that does not exist is not sent: the status line says so and the text stays. With arguments only the whole name or its alias counts (`/m sonnet` is not `/model sonnet`); without them, `Enter` runs the highlighted one and `Tab` completes the start of a name; if you really want to send something that starts with `/`, start the message with a space. Only what you type is a command: nothing the model or a tool says goes through there. The ones that change the session wait for the agent to finish (the status line says so); the ones that only look work always. None answers or hides an open question.

| Command | Also | What it does |
|---|---|---|
| `/model [name]` |  | changes the connector's model (waits for the agent) |
| `/connector [name]` |  | changes the connector (for the next session) (waits for the agent) |
| `/effort [level]` |  | how much the model thinks (waits for the agent) |
| `/new` | `/clear` | starts a new session (waits for the agent) |
| `/sessions` | `/resume` | opens the sessions panel |
| `/name <title>` |  | gives the session a title (waits for the agent) |
| `/compact [instructions]` |  | summarizes the conversation to free context (waits for the agent) |
| `/context` |  | how much context the session holds |
| `/usage` | `/cost` | the session's tokens, cost, steps and time |
| `/pending` |  | what waits for your approval |
| `/actions` |  | what this session did, action by action |
| `/undo` |  | undoes this session's file changes (waits for the agent) |
| `/copy` |  | copies the last answer (OSC 52) |
| `/memory` |  | opens the memory |
| `/doctor` |  | checks the installation |
| `/help` |  | keys and commands |
| `/quit` | `/exit` | closes Clio (waits for the agent) |

- **`/model`, `/connector` and `/effort`** without a name open their options in the same list, with the current one marked; with a name they apply it. A model's name has to be in its list: another is refused, saying which there are, and nothing is saved. The model and the effort change in the open session (Pi changes them at once) and stay as the usual ones in `config.json` (`model.model`, `model.thinking`). **The model list is Pi's own**: in a live session, the models Pi says it has for the session's connector (the one Pi says it runs). Pi's answer is kept per connector in `~/.local/share/clio/pi-models.json` (0600) and refreshed whenever a session starts; before the first message, the list kept from the last session shows. Only when there is no kept list yet does the catalog stand in, and the picker's footer says to open a session once to see every model. The models of the other connectors whose key is in the vault are listed under their name (`anthropic-api · claude-opus-5-5`); choosing one is `/connector` plus that model, for the next session. The effort takes Pi's levels (`off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`) and shows only the ones the model has; the factory one is `low`, because with Pi's (`medium`) a reasoning model thinks thousands of tokens on a simple step. The header shows it: `claude-sonnet-5-5 · low`. The connector offers only the ones with a key (the others are dimmed, with `clio connect <name>` to add one); it is saved with the last model you used on it, or its factory one, and **applies to the next session**: the open one keeps its own (its approval questions and its copy are its own). `/new` starts it.
- **`/context` and `/usage`** open a box with the session's numbers as Pi gives them. **`/pending`** lists what this session set aside and still waits; `Enter` on one asks the same approval question as at the end of a turn, with the same rules. A secret request cannot be approved there: answer it in its own box at the end of the turn, or with `clio approve <id>` in a terminal. **`/actions`** shows the log of what this session ran on the machine: the local time (`HH:MM:SS`), the tool and the result in columns, and the command in the code color (`clio actions` gives it whole, with the UTC time). **`/undo`** asks first and then reverts what the session changed (in host mode, with its own journal; in a copy, what was applied), like `clio undo`.
- **`/copy`** puts the last answer on the clipboard with the terminal's OSC 52 sequence (in base64, up to 64 kB): it only works in terminals that allow it. It copies what you see: escape sequences do not pass, and control, invisible or direction characters come out as `�`, as on screen.
- **Your own commands.** Each file `~/.config/clio/commands/<name>.md` is a command `/<name>`: the name is lowercase letters, digits and `-` (others are ignored, with a note), the first line, if it starts with `# `, is its description, and what you type after the name replaces `$ARGUMENTS` (if the text does not have it, it goes on its own line at the end). It is sent as your message, as it is, without being read again as a command. They are read each time the list opens; one of Clio's wins over a file of the same name or alias, and a file over 32 kB is ignored, as are a link, a FIFO or a device: only regular files are read. That folder is Clio's: the agent can neither read nor write it.

## Host mode: Clio on your real machine

It is the factory mode since 2026-11-18 (before, the copy was, as above). In host mode (ADR 0012) there is no copy: the tools act on your machine, in your home folder, with the network when a command asks for it, and with `sudo` where you have passwordless `sudo`. Pi stays in its jail; what acts is the tool service. Only with Pi: the builtin harness has no host mode, so without Pi `clio` asks for `--copy` (or `clio pi install`).

- **Turning it on and off.** It is on by default: a `config.json` without `"mode"` (and the one `clio init` writes) is `"host"`. `"mode": "copy"` turns it off; for one time, `clio --copy` / `clio run --copy "…"` and `clio --host` / `clio run --host "…"` win over the configuration. `clio run --workspace DIR` names a folder, so it works on a copy of it (with `--host` it is an error). A host session reopens as host.
- **The console.** With passwordless `sudo` and `systemd-run`, each command runs in a transient system unit, as your user and with `sudo` available; otherwise in `bwrap --bind / /`, without `sudo`. In both cases Clio's folders look empty and the command has no network unless it asks for `network: true` (and then it marks the session). `clio doctor` says which one there is and whether it passes its self-test; without one that passes, there is no console.
- **Time limits.** The model gives a command a `timeout` (in seconds, up to 3600) when it expects it to end by then; without one the command has no time limit and runs until it ends or is stopped. `Esc` in the chat, an abort, quitting or the end of the session stops it, under both consoles, and the action log says `stopped`.
- **Background commands.** For a server, a long build or a watcher, the model starts `bash` with `background: true` and gets a job id at once. It reads what the job printed with `job_output` and stops it with `job_stop`. At most 4 run at once in a session; each job's output is kept in the run folder (0600), capped at 4 MiB (the oldest half is dropped) and handed over at most 64 KiB at a time; every job stops when the session ends. The chat shows how many run, and the action log gets a start line and then done or stopped.
- **Clio's things are yours.** `~/.config/clio`, `~/.local/share/clio`, the runtime folder and clio-memory's: the tools refuse them.
- **What waits for your approval.** Deleting what cannot be recovered (`borrar`), spending money (`gastar`), sending something in your name (`enviar`), and touching Clio's setup, `sudoers`, who may use the machine, or turning it off (`control`) are set aside: they do not run, the task goes on, and in the chat at the end of the turn Clio asks you (see "The questions Clio asks you" above). If the question comes while another is open, it shows when that one closes; one from a session that is no longer open stays in `clio pending` (`clio run` ends by saying what it set aside). At most 3 questions a day; the rest go to the day's summary. A set-aside action expires after 24 hours. It is a list, not a wall: a script gets around it; that is why everything is logged.
- **The judge at the door.** With Jev on (below), what a tool brings in from outside (a foreign folder, a command with the network, a program that reaches out, logs) is judged before the model reads it. If the judge says it tries to give an AI orders, the model gets only a notice with its size and source, the text is kept for you (`~/.local/share/clio/runs/<id>/toolhost/withheld/`, 0600; the chat says which file), and the session becomes `suspect`: its background jobs with the network are stopped, and from then on every command that asks for the network, reaches out by itself (such as `ssh`, `docker`, `crontab` or `nsenter`) or names `sudo` waits for you as send, with the reason shown. If the judge does not answer within 5 s, the text goes on as before and the chat says so once.
- **Secrets the agent asks for.** A `request_secret` waits like an action set aside, as class `secret`: the chat asks for the value at the end of the turn (above). At a terminal, `clio pending` lists it, `clio approve <id>` asks for the value with no echo and stores it in the vault, and `clio reject <id>` declines it. A name that already exists, or a stopped or locked vault, is refused when the model asks.

```sh
clio pending [--json]          # what waits for your approval, and the secrets asked for
clio approve <id>              # runs it once, exactly as it was set aside (a secret request: asks for the value)
clio reject <id>               # drops it
clio actions [--run <id>] [--json]   # the log: every command and write, before and after
clio undo <id>                 # in a host session: undoes the files it changed
```

- **Undo** works for what Clio changed with `write`/`edit` (it keeps a copy first): it restores content and permissions, removes what it created and leaves alone a file that changed afterwards. What a console command changed has no copy.
- **A budget per day**, not per task: `limits.day_cost_micro_usd` (3,000,000 by default) and `limits.day_steps` (1000) in `config.json`, counted across sessions in `~/.local/share/clio/host/day.json`. When it is spent, Clio says so and goes on the next day.
- **`~/.config/clio/host.json`** (optional; it only adds, it removes nothing of the factory list): `{"questions_per_day": 3, "foreign": ["/srv/mailbox", "attachments"], "ask": [{"class": "enviar", "pattern": "^rsync .*prod:"}]}`. `foreign` are foreign folders (reading them marks the session; by default `/var/log`, `/var/mail`, `/var/spool`, `uploads`, `Downloads`, `Descargas`); `ask` adds patterns to one of the four classes (`borrar`, `gastar`, `enviar`, `control`).
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
- No tool of the model sends a stored secret anywhere yet; a secret it asks for is kept for later work. The vault is not connected to the agent in `run`; the secrets the copy leaves out simply do not exist for it.
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
