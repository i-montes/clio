"""Drives `clio` (the Pi harness: the chat, with the sessions a key away) inside a real pseudo-terminal and
prints a JSON verdict. Stdlib only. Linux only (it reads /proc).

usage: python3 -I pty_chat_driver.py SCENARIO CONFIG_JSON
SCENARIO: new_chat | abort | steer | reopen | sigkill | hangup | sigterm | one_turn | reopen_turn |
    review | approve | …

The terminal stream is a diff and is not parsed. In a debug build `clio` writes the text of every
frame to the file named by CLIO_TUI_SCREEN_DUMP: that is what the waits read. Every wait is
bounded, and a global alarm kills everything so a test run can never hang.
"""

import fcntl
import json
import os
import pty
import select
import signal
import struct
import sys
import termios
import time

scenario = sys.argv[1]
cfg = json.loads(sys.argv[2])
ENTER, ESC, CTRL_D, CTRL_C, CTRL_S = b"\r", b"\x1b", b"\x04", b"\x03", b"\x13"
# What the screen shows when the chat waits for a message, and while a turn runs.
IDLE, BUSY = "Enter send", ["working", "starting"]
# The sessions drawer is open.
DRAWER = "n new"
CHILDREN = []
RAW = b""


def on_alarm(*_):
    for pid in CHILDREN:
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass
    print(json.dumps({"timeout": True, "screen": screen()}), flush=True)
    os._exit(3)


signal.signal(signal.SIGALRM, on_alarm)
signal.alarm(170)


def screen():
    try:
        with open(cfg["dump"]) as f:
            return f.read()
    except OSError:
        return ""


def spawn(args=("--new",)):
    # `--new`: most scenarios start on a new session; `clio` alone reopens the last one.
    env = dict(cfg["env"], CLIO_TUI_SCREEN_DUMP=cfg["dump"])
    try:
        os.unlink(cfg["dump"])
    except OSError:
        pass
    pid, fd = pty.fork()
    if pid == 0:
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 110, 0, 0))
        os.chdir(cfg["cwd"])
        os.execve(cfg["clio"], ["clio", *args], env)
    CHILDREN.append(pid)
    return pid, fd


def pump(fd, secs=0.1):
    global RAW
    r, _, _ = select.select([fd], [], [], secs)
    if r:
        try:
            d = os.read(fd, 65536)
        except OSError:
            return
        RAW += d


def wait_screen(fd, text, timeout=45.0, absent=None):
    """True when the screen shows `text` (and none of `absent`) within the timeout."""
    end = time.time() + timeout
    while time.time() < end:
        s = screen()
        gone_ = [absent] if isinstance(absent, str) else (absent or [])
        if text in s and not any(a in s for a in gone_):
            return True
        pump(fd)
    return False


def send(fd, data):
    os.write(fd, data)
    time.sleep(0.15)


def type_text(fd, s):
    for ch in s:
        os.write(fd, ch.encode())
        time.sleep(0.01)


def alive(pid):
    try:
        done, _ = os.waitpid(pid, os.WNOHANG)
        return done == 0
    except ChildProcessError:
        return False


def wait_exit(fd, pid, timeout=30.0):
    end = time.time() + timeout
    while time.time() < end:
        try:
            done, st = os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            return None
        if done:
            return os.waitstatus_to_exitcode(st)
        pump(fd)
    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)
    return "timeout"


def pi_procs():
    """Pi and toolhost processes that belong to this test's temporary folder."""
    out = []
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        try:
            cmd = open(f"/proc/{d}/cmdline", "rb").read().replace(b"\0", b" ").decode("utf8", "replace")
            env = open(f"/proc/{d}/environ", "rb").read().decode("utf8", "replace")
        except OSError:
            continue
        # Pi renames its own process, so it is known by the folder it was given.
        mine = cfg["root"] in env
        if mine and ("toolhost" in cmd or "PI_CODING_AGENT_DIR=" in env):
            out.append((int(d), cmd[:90]))
    return out


def gone(timeout=20.0):
    end = time.time() + timeout
    while time.time() < end:
        if not pi_procs():
            return True
        time.sleep(0.2)
    return False


def open_new_chat(fd):
    """`clio` opens on a new session with the keys in the chat: nothing to press. The session (a
    run, Pi) starts with its first message."""
    return wait_screen(fd, "new session") and wait_screen(fd, "How can I help?", 30)


def idle(fd, timeout=30):
    return wait_screen(fd, IDLE, timeout, absent=BUSY)


def open_drawer(fd):
    # Not before the first frame: until `clio` puts the terminal in raw mode, Ctrl-S is XOFF.
    wait_screen(fd, "clio", 30)
    send(fd, CTRL_S)
    return wait_screen(fd, DRAWER, 15)


def ask(fd, text, answer, timeout=60):
    # Each key once the screen shows the ones before it (the TUI took them), and Enter only once
    # it shows them all: a key that is lost fails here, not later as a request without it.
    for i, ch in enumerate(text):
        os.write(fd, ch.encode())
        if not wait_screen(fd, text[: i + 1], 10):
            print(f"typed {text[: i + 1]!r} but the screen never showed it", file=sys.stderr)
            return False
    send(fd, ENTER)
    return wait_screen(fd, answer, timeout) and idle(fd)


out = {}
pid, fd = spawn()
if scenario == "new_chat":
    out["opened"] = open_new_chat(fd)
    # The sessions are a key away, and the same key puts them away again.
    out["selector"] = open_drawer(fd)
    # Nothing that cannot be opened is listed (the remote source is not built yet).
    out["no_remote_row"] = "remoto" not in screen()
    send(fd, CTRL_S)
    out["opened"] = out["opened"] and wait_screen(fd, "Type a message", 15, absent=DRAWER)
    out["nothing_before_the_first_message"] = len(pi_procs()) == 0
    out["answered"] = ask(fd, "hola", "hello from the fake model")
    out["procs_while_open"] = len(pi_procs())
    s = screen()
    out["user_line_shown"] = "hola" in s
    out["usage_bar"] = "↑" in s and "context" in s
    out["screen_after_answer"] = s
    send(fd, CTRL_D)
    out["debug_raw_events"] = wait_screen(fd, "Pi events (raw)", 10) and "MessageUpdate" in screen()
    send(fd, CTRL_D)
    send(fd, CTRL_S)  # the keys go to the list; the session stays open
    out["back_in_selector"] = wait_screen(fd, DRAWER, 15, absent="Pi events")
    out["listed_again"] = wait_screen(fd, "hola", 15) and wait_screen(fd, "now", 15)
    send(fd, b"g")  # "new session": the session before it closes
    send(fd, ENTER)
    out["procs_after_leaving"] = gone(20)
    open_drawer(fd)
    send(fd, b"q")
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = len(pi_procs())
elif scenario == "abort":
    out["opened"] = open_new_chat(fd)
    type_text(fd, "algo lento")
    send(fd, ENTER)
    out["working"] = wait_screen(fd, "working", 30)
    time.sleep(1.0)
    send(fd, ESC)
    out["aborting_shown"] = wait_screen(fd, "aborting", 10) or IDLE in screen()
    out["idle_again"] = idle(fd)
    out["slow_text_absent"] = "never shown" not in screen()
    out["next_turn_answered"] = ask(fd, "otra vez", "answer after the abort")
    time.sleep(0.5)
    send(fd, CTRL_C)  # idle: Ctrl-C leaves
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = len(pi_procs())
elif scenario == "steer":
    out["opened"] = open_new_chat(fd)
    type_text(fd, "escribe un archivo")
    send(fd, ENTER)
    out["working"] = wait_screen(fd, "working", 30)
    type_text(fd, "mejor en minúsculas")
    send(fd, ENTER)
    out["steer_sent"] = wait_screen(fd, "message sent to the agent", 15)
    out["tool_line"] = wait_screen(fd, "probar", 30)
    out["final"] = wait_screen(fd, "done after the steer", 60) and idle(fd)
    out["tool_ok_with_duration"] = " s" in screen() or " ms" in screen()
    out["denied_line"] = wait_screen(fd, "negado", 10)
    out["taint_hidden"] = "externo" not in screen()
    time.sleep(0.5)
    send(fd, CTRL_C)  # idle: Ctrl-C leaves
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = len(pi_procs())
elif scenario == "masked":
    # The model reads a file that holds two keys: the chat says they were struck out.
    out["opened"] = open_new_chat(fd)
    type_text(fd, "lee las llaves")
    send(fd, ENTER)
    out["final"] = wait_screen(fd, "sin ver los valores", 60) and idle(fd)
    out["note"] = wait_screen(fd, "redacted 2 key-shaped values", 15)
    out["keys_not_shown"] = "ghp_" not in screen() and "AKIA" not in screen()
    time.sleep(0.5)
    send(fd, CTRL_C)  # idle: Ctrl-C leaves
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = len(pi_procs())
elif scenario == "reopen":
    out["opened"] = open_new_chat(fd)
    out["answered"] = ask(fd, "primera pregunta", "first answer")
    send(fd, CTRL_C)  # idle: Ctrl-C leaves cleanly
    out["code1"] = wait_exit(fd, pid)
    out["procs_after_first"] = len(pi_procs())
    # `clio` alone reopens the last conversation: its history, with no key pressed.
    pid, fd = spawn(())
    out["row_listed"] = wait_screen(fd, "primera pregunta", 30)
    out["reopened"] = wait_screen(fd, "first answer", 60) and idle(fd, 60)
    s = screen()
    out["history_user"] = "primera pregunta" in s
    out["history_answer"] = "first answer" in s
    out["answered_again"] = ask(fd, "segunda pregunta", "second answer")
    out["history_still_there"] = "first answer" in screen()
    time.sleep(0.5)
    send(fd, CTRL_C)  # idle: Ctrl-C leaves
    out["code2"] = wait_exit(fd, pid)
    out["procs_after_quit"] = len(pi_procs())
elif scenario == "drawer_reopen":
    # The same session, opened from the drawer: `clio --new`, Ctrl-S, its row, Enter.
    out["opened"] = open_new_chat(fd)
    out["answered"] = ask(fd, "primera pregunta", "first answer")
    send(fd, CTRL_C)  # idle: Ctrl-C leaves cleanly
    out["code1"] = wait_exit(fd, pid)
    out["procs_after_first"] = len(pi_procs())
    pid, fd = spawn()
    out["selector"] = open_drawer(fd)
    out["row_listed"] = wait_screen(fd, "primera pregunta", 20)
    send(fd, b"j")  # first row is "new session"
    send(fd, ENTER)
    out["reopened"] = wait_screen(fd, "first answer", 60) and idle(fd, 60)
    s = screen()
    out["history_user"] = "primera pregunta" in s
    out["history_answer"] = "first answer" in s
    out["answered_again"] = ask(fd, "segunda pregunta", "second answer")
    out["history_still_there"] = "first answer" in screen()
    time.sleep(0.5)
    send(fd, CTRL_C)  # idle: Ctrl-C leaves
    out["code2"] = wait_exit(fd, pid)
    out["procs_after_quit"] = len(pi_procs())
elif scenario == "one_turn":
    # A new session, one turn (cfg "ask" / "answer"), and leave.
    out["opened"] = open_new_chat(fd)
    out["answered"] = ask(fd, cfg["ask"], cfg["answer"])
    if not out["answered"]:
        out["screen"] = screen()
    send(fd, CTRL_C)
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = 0 if gone(15) else len(pi_procs())
elif scenario == "reopen_turn":
    # Reopen the session listed as cfg "row" (the first after "new session") for one more turn.
    out["selector"] = open_drawer(fd)
    out["row_listed"] = wait_screen(fd, cfg["row"], 20)
    send(fd, b"j")
    send(fd, ENTER)
    out["reopened"] = wait_screen(fd, IDLE, 60, absent=DRAWER) and idle(fd, 60)
    out["answered"] = ask(fd, cfg["ask"], cfg["answer"])
    if not out["answered"]:
        out["screen"] = screen()
    time.sleep(0.5)
    send(fd, CTRL_C)  # idle: Ctrl-C leaves
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = 0 if gone(15) else len(pi_procs())
elif scenario == "review":
    # A turn that writes a file, `/review`, its verdict on screen, and the turn the review starts
    # with its findings (cfg "fixed": what the model answers to them); then leave.
    out["opened"] = open_new_chat(fd)
    out["answered"] = ask(fd, "escribe out.txt", "wrote it")
    type_text(fd, "/review")
    send(fd, ENTER)
    out["reviewing"] = wait_screen(fd, "reviewing", 30)
    out["verdict"] = wait_screen(fd, "review: fails", 90)
    out["evidence"] = wait_screen(fd, "evidence: out.txt", 10)
    out["review_turn"] = wait_screen(fd, "turn started by the review", 30)
    out["fixed"] = wait_screen(fd, cfg["fixed"], 60) and idle(fd)
    if not out["fixed"]:
        out["screen"] = screen()
    time.sleep(0.5)
    send(fd, CTRL_C)  # idle: Ctrl-C leaves
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = 0 if gone(15) else len(pi_procs())
elif scenario == "approve":
    # Host mode: a turn whose write is set aside (cfg "ask" / "answer"), the box Clio opens once
    # the chat is idle, approved (`y`, then Enter), and the turn that approval starts (cfg
    # "after": what the model answers in it); then leave.
    out["opened"] = open_new_chat(fd)
    out["answered"] = ask(fd, cfg["ask"], cfg["answer"])
    out["box"] = wait_screen(fd, "y then Enter", 30)
    time.sleep(1.5)  # the box hears its first key after a quiet second
    send(fd, b"y")
    out["armed"] = wait_screen(fd, "other key", 10)
    send(fd, ENTER)
    out["approvals_turn"] = wait_screen(fd, "turn started by the approvals", 30)
    out["after"] = wait_screen(fd, cfg["after"], 60) and idle(fd)
    if not out["after"]:
        out["screen"] = screen()
    time.sleep(0.5)
    send(fd, CTRL_C)  # idle: Ctrl-C leaves
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = 0 if gone(15) else len(pi_procs())
elif scenario in ("review_quit", "review_new"):
    # A turn that writes a file, `/review`, and while the reviewer runs (its model has it run a
    # long command) the person leaves, or starts a new session: no reviewer Pi or tool host stays.
    out["opened"] = open_new_chat(fd)
    out["answered"] = ask(fd, "escribe out.txt", "wrote it")
    type_text(fd, "/review")
    send(fd, ENTER)
    out["reviewing"] = wait_screen(fd, "reviewing", 30)
    # The conversation's Pi and tool host, and the reviewer's.
    end = time.time() + 30
    while time.time() < end and len(pi_procs()) < 4:
        pump(fd)
    out["reviewer_up"] = len(pi_procs()) >= 4
    if scenario == "review_new":
        type_text(fd, "/new")
        send(fd, ENTER)
        out["new"] = wait_screen(fd, "How can I help?", 30)
        # A new session starts its Pi with its first message: nothing of the old one is left.
        out["gone_after_new"] = gone(20)
        if not out["gone_after_new"]:
            out["left"] = pi_procs()
    send(fd, CTRL_C)
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = 0 if gone(15) else len(pi_procs())
elif scenario == "summary":
    # Four messages, the keys to the list, and leave at once: leaving asks for the summary of the
    # open session and waits for it.
    out["opened"] = open_new_chat(fd)
    out["answered"] = all(ask(fd, f"paso {i}", f"respuesta {i}") for i in range(1, 5))
    out["back_in_selector"] = open_drawer(fd)
    send(fd, b"q")
    out["code"] = wait_exit(fd, pid, 60)
    out["procs_at_exit"] = [c for _, c in pi_procs()]
    out["procs_after_quit"] = 0 if gone(15) else len(pi_procs())
elif scenario == "present":
    # A new session: its first message (cfg "ask") carries the request to introduce herself, and
    # she does (cfg "answer"); then leave.
    out["opened"] = open_new_chat(fd)
    type_text(fd, cfg.get("ask", "hola"))
    send(fd, ENTER)
    out["spoke_first"] = wait_screen(fd, cfg["answer"], 60) and idle(fd)
    if not out["spoke_first"]:
        out["screen"] = screen()
    out["screen_after"] = screen()
    send(fd, CTRL_C)
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = 0 if gone(15) else len(pi_procs())
elif scenario == "sigkill":
    out["opened"] = open_new_chat(fd) and ask(fd, "hola", "hello from the fake model")
    out["procs_while_open"] = len(pi_procs())
    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)
    out["orphans_gone"] = gone(30)
    out["orphans_left"] = pi_procs()
elif scenario in ("hangup", "sigterm"):
    # The terminal goes away (its window closes: the master side is closed), or a plain `kill`:
    # `clio` leaves through its normal way out within seconds, and takes Pi and its toolhost.
    out["opened"] = open_new_chat(fd) and ask(fd, "hola", "hello from the fake model")
    out["procs_while_open"] = len(pi_procs())
    if scenario == "hangup":
        os.close(fd)
    else:
        os.kill(pid, signal.SIGTERM)
    end, code = time.time() + 10, "timeout"
    while time.time() < end:
        done, st = os.waitpid(pid, os.WNOHANG)
        if done:
            code = os.waitstatus_to_exitcode(st)
            break
        if scenario == "sigterm":
            pump(fd)
        else:
            time.sleep(0.05)
    out["secs"] = round(10 - (end - time.time()), 1)
    if code == "timeout":
        out["state"] = open(f"/proc/{pid}/stat").read().split()[2]
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    out["code"] = code
    out["orphans_gone"] = gone(10)
    out["orphans_left"] = pi_procs()
else:
    sys.exit("unknown scenario")
out["raw_has_key"] = cfg["key"].encode() in RAW
out["screen_has_key"] = cfg["key"] in screen()
print(json.dumps(out))
