"""Drives `clio` (the Pi harness: session selector and chat) inside a real pseudo-terminal and
prints a JSON verdict. Stdlib only. Linux only (it reads /proc).

usage: python3 -I pty_chat_driver.py SCENARIO CONFIG_JSON
SCENARIO: new_chat | abort | steer | reopen | sigkill

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
ENTER, ESC, CTRL_D, CTRL_C = b"\r", b"\x1b", b"\x04", b"\x03"
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


def spawn():
    env = dict(cfg["env"], CLIO_TUI_SCREEN_DUMP=cfg["dump"])
    try:
        os.unlink(cfg["dump"])
    except OSError:
        pass
    pid, fd = pty.fork()
    if pid == 0:
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 110, 0, 0))
        os.chdir(cfg["cwd"])
        os.execve(cfg["clio"], ["clio"], env)
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
    """True when the screen shows `text` (and not `absent`) within the timeout."""
    end = time.time() + timeout
    while time.time() < end:
        s = screen()
        if text in s and (absent is None or absent not in s):
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
    """From the selector: the first row is 'nueva sesión'."""
    ok = wait_screen(fd, "nueva sesión")
    send(fd, ENTER)
    return ok and wait_screen(fd, "listo", 60)


def ask(fd, text, answer, timeout=60):
    type_text(fd, text)
    send(fd, ENTER)
    return wait_screen(fd, answer, timeout) and wait_screen(fd, "listo", 30)


out = {}
pid, fd = spawn()
if scenario == "new_chat":
    out["selector"] = wait_screen(fd, "sesiones de Pi")
    out["remote_row"] = wait_screen(fd, "remoto: pendiente")
    out["opened"] = open_new_chat(fd)
    out["procs_while_open"] = len(pi_procs())
    out["answered"] = ask(fd, "hola", "hello from the fake model")
    s = screen()
    out["user_line_shown"] = "hola" in s
    out["usage_bar"] = "↑" in s and "$" in s and "contexto" in s
    out["screen_after_answer"] = s
    send(fd, CTRL_D)
    out["debug_raw_events"] = wait_screen(fd, "eventos de Pi (crudos)", 10) and "MessageUpdate" in screen()
    send(fd, CTRL_D)
    send(fd, ESC)  # back to the selector
    out["back_in_selector"] = wait_screen(fd, "sesiones de Pi", 15, absent="eventos de Pi")
    out["listed_again"] = wait_screen(fd, "hola", 15) and wait_screen(fd, "msgs", 15)
    out["procs_after_leaving"] = gone(20) if True else None
    # The remote row cannot be opened: it says why instead of opening anything.
    send(fd, b"G")
    send(fd, ENTER)
    out["remote_notice"] = wait_screen(fd, "todavía no existe", 10)
    send(fd, b"x")  # any key closes the notice
    time.sleep(0.2)
    send(fd, b"q")
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = len(pi_procs())
elif scenario == "abort":
    out["opened"] = open_new_chat(fd)
    type_text(fd, "algo lento")
    send(fd, ENTER)
    out["working"] = wait_screen(fd, "trabajando", 30)
    time.sleep(1.0)
    send(fd, ESC)
    out["aborting_shown"] = wait_screen(fd, "abortando", 10) or "listo" in screen()
    out["idle_again"] = wait_screen(fd, "listo", 30, absent="trabajando")
    out["slow_text_absent"] = "never shown" not in screen()
    out["next_turn_answered"] = ask(fd, "otra vez", "answer after the abort")
    send(fd, ESC)
    time.sleep(0.5)
    send(fd, b"q")
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = len(pi_procs())
elif scenario == "steer":
    out["opened"] = open_new_chat(fd)
    type_text(fd, "escribe un archivo")
    send(fd, ENTER)
    out["working"] = wait_screen(fd, "trabajando", 30)
    type_text(fd, "mejor en minúsculas")
    send(fd, ENTER)
    out["steer_sent"] = wait_screen(fd, "mensaje enviado al agente", 15)
    out["tool_line"] = wait_screen(fd, "Escribir archivo", 30)
    out["final"] = wait_screen(fd, "done after the steer", 60) and wait_screen(fd, "listo", 30)
    out["tool_ok_with_duration"] = " s" in screen() or " ms" in screen()
    out["denied_line"] = wait_screen(fd, "negado", 10)
    out["taint_notice"] = wait_screen(fd, "leyó contenido externo", 10) and "externo" in screen().splitlines()[-1]
    send(fd, ESC)
    time.sleep(0.5)
    send(fd, b"q")
    out["code"] = wait_exit(fd, pid)
    out["procs_after_quit"] = len(pi_procs())
elif scenario == "reopen":
    out["opened"] = open_new_chat(fd)
    out["answered"] = ask(fd, "primera pregunta", "first answer")
    send(fd, CTRL_C)  # idle: Ctrl-C leaves cleanly
    out["code1"] = wait_exit(fd, pid)
    out["procs_after_first"] = len(pi_procs())
    pid, fd = spawn()
    out["selector"] = wait_screen(fd, "sesiones de Pi")
    out["row_listed"] = wait_screen(fd, "primera pregunta", 20)
    send(fd, b"j")  # first row is "nueva sesión"
    send(fd, ENTER)
    out["reopened"] = wait_screen(fd, "listo", 60)
    s = screen()
    out["history_user"] = "primera pregunta" in s
    out["history_answer"] = "first answer" in s
    out["answered_again"] = ask(fd, "segunda pregunta", "second answer")
    out["history_still_there"] = "first answer" in screen()
    send(fd, ESC)
    time.sleep(0.5)
    send(fd, b"q")
    out["code2"] = wait_exit(fd, pid)
    out["procs_after_quit"] = len(pi_procs())
elif scenario == "sigkill":
    out["opened"] = open_new_chat(fd)
    out["procs_while_open"] = len(pi_procs())
    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)
    out["orphans_gone"] = gone(30)
    out["orphans_left"] = pi_procs()
else:
    sys.exit("unknown scenario")
out["raw_has_key"] = cfg["key"].encode() in RAW
out["screen_has_key"] = cfg["key"] in screen()
print(json.dumps(out))
