"""Runs INSIDE the jail and tries forbidden things. One line per check:
RESULT <name> PASS|FAIL <detail>   (PASS = the forbidden action failed, as it must)
Everything it needs from the outside comes as one JSON argument."""
import errno
import json
import os
import signal
import socket
import sys

P = json.loads(sys.argv[1])


def result(name, ok, detail=""):
    print("RESULT %s %s %s" % (name, "PASS" if ok else "FAIL", detail.replace("\n", " ")[:200]), flush=True)


def can_open(path, mode):
    try:
        with open(path, mode) as f:
            if "r" in mode:
                f.read(1)
        return True
    except OSError:
        return False


# 0. positive control: the program runs, and sees /work and nothing else of the person's.
try:
    seen = set(os.listdir("/"))
    allowed = {"work", "tmp", "dev", "proc", "etc", "usr", "bin", "sbin", "lib", "lib32", "lib64"}
    ok = open("/work/marker").read() == "marker" and seen <= allowed
    result("sees_only_work", ok, "root has: " + ",".join(sorted(seen)))
except OSError as e:
    result("sees_only_work", False, str(e))

# 1. the person's files.
result("home_decoy_unreadable", not can_open(P["home_decoy"], "r"), P["home_decoy"])
result("home_folder_invisible", not os.path.exists(P["real_home"]) or P["real_home"] == "/", P["real_home"])
if P.get("runtime_decoy"):
    result("runtime_dir_invisible", not can_open(P["runtime_decoy"], "r"), P["runtime_decoy"])

# 2. the network: a TCP port the host opened, and an abstract unix socket of the host.
try:
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(3)
    s.connect(("127.0.0.1", P["port"]))
    result("network_tcp", False, "connected to the host's port")
except OSError as e:
    result("network_tcp", True, str(e))
try:
    u = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    u.connect("\0" + P["abstract"])
    result("network_abstract_socket", False, "connected to the host's abstract socket")
except OSError as e:
    result("network_abstract_socket", True, str(e))

# 3. writes: the system, the root, and the person's home.
result("write_usr", not can_open("/usr/clio-escape", "w"))
result("write_etc", not can_open("/etc/clio-escape", "w"))
result("write_root", not can_open("/clio-escape", "w"))
result("write_real_home", not can_open(P["real_home"] + "/clio-escape-" + P["nonce"], "w"))

# 4. other processes: not listed, not signalable.
pids = [int(x) for x in os.listdir("/proc") if x.isdigit()]
result("host_processes_hidden", P["host_pid"] not in pids and len(pids) < 10, "visible pids: %s" % pids)
try:
    os.kill(P["host_pid"], 0)
    gone = False
except ProcessLookupError:
    gone = True
except OSError:
    gone = True  # EPERM would also mean it cannot be signaled
if gone and P["host_pid"] not in pids:
    try:
        os.kill(P["host_pid"], signal.SIGKILL)
        gone = False
    except OSError:
        pass
result("kill_host_parent", gone, "pid %d" % P["host_pid"])

# 5. the environment: nothing inherited, here or in any process's /proc/<pid>/environ.
allowed_env = {"PATH", "HOME", "LANG", "TMPDIR", "PWD", "PYTHONDONTWRITEBYTECODE"}
extra = sorted(set(os.environ) - allowed_env)
leaked = []
for pid in pids:
    try:
        data = open("/proc/%d/environ" % pid, "rb").read()
    except OSError:
        continue
    entries = data.split(b"\0")
    for name in P["bait"]:
        if any(e.startswith(name.encode() + b"=") for e in entries):
            leaked.append((pid, name))
fixed = os.environ.get("HOME") == "/tmp/home" and os.environ.get("PATH") == "/usr/bin:/bin"
result("environment_clean", not extra and not leaked and fixed, "extra vars: %s leaked: %s fixed: %s" % (extra, leaked, fixed))

# 6. no new namespaces from inside (that is how a jail gets rearranged).
try:
    flag = getattr(os, "CLONE_NEWUSER", 0x10000000)
    if hasattr(os, "unshare"):
        os.unshare(flag)
    else:
        import ctypes
        if ctypes.CDLL(None, use_errno=True).unshare(flag) != 0:
            raise OSError(ctypes.get_errno(), "unshare")
    result("no_nested_user_namespace", False, "unshare(CLONE_NEWUSER) worked")
except OSError as e:
    result("no_nested_user_namespace", True, str(e))

print("DONE", flush=True)
