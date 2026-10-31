"""Drives `clio` inside a real pseudo-terminal (stdlib only) and prints a JSON verdict.

usage: python3 -I pty_driver.py SCENARIO CONFIG_JSON
SCENARIO: quit | ctrl_c | panic | help_when_piped | live | cancel
The raw terminal stream is not parsed (a diff-based renderer repaints only changed cells), so the
scenarios check what is robust: the alternate-screen and cursor sequences, the terminal modes,
the exit status, and effects on the disk.
"""

import fcntl
import json
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time

scenario = sys.argv[1]
cfg = json.loads(sys.argv[2])
buf = b""
ALT_ON, ALT_OFF = b"\x1b[?1049h", b"\x1b[?1049l"
CHILDREN = []


def kill_children():
    for child in CHILDREN:
        try:
            os.kill(child, signal.SIGKILL)
        except OSError:
            pass


def on_alarm(*_):
    # Never hang a test run: whatever went wrong, kill the children and say so.
    kill_children()
    print(json.dumps({"timeout": True}))
    os._exit(3)


signal.signal(signal.SIGALRM, on_alarm)
signal.alarm(110)


def spawn(extra_env=None):
    env = dict(cfg["env"], **(extra_env or {}))
    pid, fd = pty.fork()
    if pid == 0:
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
        os.chdir(cfg["cwd"])
        os.execve(cfg["clio"], ["clio"], env)
    CHILDREN.append(pid)
    return pid, fd


def pump(fd, until, timeout=15.0):
    global buf
    end = time.time() + timeout
    while time.time() < end:
        if until():
            return True
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                d = os.read(fd, 65536)
            except OSError:
                return until()
            if not d:
                return until()
            buf += d
    return until()


def drain(fd):
    pump(fd, lambda: False, 0.5)


def modes(fd):
    try:
        lf = termios.tcgetattr(fd)[3]
    except termios.error:
        return None
    return {"canonical": bool(lf & termios.ICANON), "echo": bool(lf & termios.ECHO)}


def wait_for(path_fn, timeout=25.0):
    end = time.time() + timeout
    while time.time() < end:
        v = path_fn()
        if v:
            return v
        time.sleep(0.1)
    return None


def finish(pid, fd, timeout=20.0):
    drain(fd)
    end = time.time() + timeout
    status = None
    while time.time() < end:
        done, st = os.waitpid(pid, os.WNOHANG)
        if done:
            status = st
            break
        drain(fd)
        time.sleep(0.05)
    if status is None:
        # The program did not exit: kill it and report it, instead of waiting for ever.
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
        return "timeout"
    drain(fd)
    return os.waitstatus_to_exitcode(status)


def type_text(fd, s):
    for ch in s:
        os.write(fd, ch.encode())
        time.sleep(0.01)


def latest_run():
    runs = cfg["runs"]
    if not os.path.isdir(runs):
        return None
    names = sorted(os.listdir(runs))
    return os.path.join(runs, names[-1]) if names else None


def read(p):
    try:
        with open(p) as f:
            return f.read()
    except OSError:
        return None


out = {}
if scenario in ("quit", "ctrl_c"):
    pid, fd = spawn()
    out["alt_on"] = pump(fd, lambda: ALT_ON in buf)
    pump(fd, lambda: b"Inicio" in buf)
    out["title_drawn"] = b"Inicio" in buf
    out["during"] = modes(fd)
    os.write(fd, b"q" if scenario == "quit" else b"\x03")
    out["code"] = finish(pid, fd)
    out["alt_off_after_on"] = buf.rfind(ALT_OFF) > buf.find(ALT_ON) >= 0
    out["after"] = modes(fd)
elif scenario == "panic":
    pid, fd = spawn({"CLIO_TUI_PANIC_TEST": "1"})
    out["code"] = finish(pid, fd)
    out["alt_on"] = ALT_ON in buf
    out["alt_off_after_on"] = buf.rfind(ALT_OFF) > buf.find(ALT_ON) >= 0
    first_off = buf.find(ALT_OFF, max(buf.find(ALT_ON), 0))
    out["message_after_restore"] = first_off >= 0 and buf.find(b"deliberate panic") > first_off
    out["after"] = modes(fd)
elif scenario == "help_when_piped":
    # stdin is a terminal but stdout is a pipe: no TUI, the help, exit 0.
    master, slave = pty.openpty()
    p = subprocess.run(
        [cfg["clio"]], stdin=slave, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        env=cfg["env"], cwd=cfg["cwd"], timeout=20,
    )
    out["code"] = p.returncode
    out["stdout"] = p.stdout.decode()
    out["escapes"] = b"\x1b" in p.stdout
elif scenario in ("live", "cancel"):
    pid, fd = spawn()
    pump(fd, lambda: b"Inicio" in buf)
    time.sleep(0.3)
    os.write(fd, b"n")
    time.sleep(0.3)
    type_text(fd, "arregla total en app.py")
    time.sleep(0.8)  # the folder check
    os.write(fd, b"\r")
    if scenario == "live":
        out["summary"] = bool(wait_for(lambda: read(os.path.join(latest_run() or "/nonexistent", "summary.json"))))
        time.sleep(0.6)
        os.write(fd, b"\r")  # open the detail
        time.sleep(0.8)
        os.write(fd, b"a")  # asks
        time.sleep(0.6)
        out["asked_before_apply"] = read(cfg["app_py"]) == cfg["old"]
        os.write(fd, b"y")
        out["applied"] = bool(wait_for(lambda: read(cfg["app_py"]) == cfg["new"]))
        time.sleep(0.8)
        os.write(fd, b"u")
        time.sleep(0.4)
        out["asked_before_undo"] = read(cfg["app_py"]) == cfg["new"]
        os.write(fd, b"y")
        out["undone"] = bool(wait_for(lambda: read(cfg["app_py"]) == cfg["old"]))
        time.sleep(0.6)
        os.write(fd, b"\x1b")
        time.sleep(0.4)
        os.write(fd, b"q")
    else:
        # The adapter is slow: cancel while the first step is in progress.
        out["started"] = bool(wait_for(lambda: latest_run() and os.path.exists(os.path.join(latest_run(), "manifest.json"))))
        time.sleep(0.8)
        os.write(fd, b"\x1b")
        s = wait_for(lambda: read(os.path.join(latest_run() or "/nonexistent", "summary.json")), 30)
        out["ending"] = json.loads(s)["ending"] if s else None
        time.sleep(0.6)
        os.write(fd, b"\x1b")  # back to the home screen
        time.sleep(0.4)
        os.write(fd, b"q")
    out["code"] = finish(pid, fd)
    out["after"] = modes(fd)
else:
    sys.exit("unknown scenario")
print(json.dumps(out))
