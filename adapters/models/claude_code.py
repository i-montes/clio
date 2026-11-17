#!/usr/bin/env python3
"""The model adapter of the Claude connector: reads a Request as JSON on stdin, asks the official
Claude Code (`claude -p`, the binary the Agent SDK brings inside Clio's Pi) with a Claude
subscription token, writes a Response as JSON on stdout. Same contract as openai_compat.py. It
never calls the API itself.

The token comes in an envelope on stdin, {"key": ..., "request": <Request>}, which is how Clio
calls it; given a bare Request it comes from the environment (CLAUDE_CODE_OAUTH_TOKEN). It reaches
Claude Code only in its environment, never in argv, and is never printed. Clio passes the binary as
`--claude <path>`. Standard library only.
"""
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile

TIMEOUT = 170
MISSING = "falta Claude Code (pi-claude-agent-sdk): clio pi install"
# Claude Code talks to nothing but the model (the same variables Clio gives it inside the jail).
QUIET = {
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
    "DISABLE_AUTOUPDATER": "1",
    "DISABLE_TELEMETRY": "1",
}
# A person behind a proxy still reaches Anthropic through it.
PROXY = ("HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY", "https_proxy", "http_proxy", "no_proxy")


def stop(signum, frame):
    """Clio sends SIGTERM when a call runs past its deadline: leave through `finally`, which stops
    Claude Code and removes its HOME (the default action would skip both)."""
    raise SystemExit(1)


signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)


def fail(message: str) -> None:
    print(f"clio-model-adapter: {message}", file=sys.stderr)
    sys.exit(1)


def text_of(content) -> str:
    if isinstance(content, list):
        return "".join(p.get("text") or "" for p in content if isinstance(p, dict))
    return content if isinstance(content, str) else ""


args = sys.argv[1:]
claude = args[1] if len(args) == 2 and args[0] == "--claude" else ""
if not (claude and os.path.isfile(claude) and os.access(claude, os.X_OK)):
    fail(MISSING)

sent = json.load(sys.stdin)
if "key" in sent and "request" in sent:
    key, request = sent["key"], sent["request"]
else:
    key, request = os.environ.get("CLAUDE_CODE_OAUTH_TOKEN"), sent
key = key.strip() if isinstance(key, str) else ""
if not key:
    fail("falta el token: Clio lo manda en el sobre, o define CLAUDE_CODE_OAUTH_TOKEN")
if not all(0x21 <= ord(ch) <= 0x7E for ch in key):
    fail("el token tiene un carácter que no puede tener (un salto de línea, un espacio, algo no ASCII)")

# Clio's `system` messages become the system prompt (in place of Claude Code's own); the rest of
# the conversation goes on stdin as text, one block per turn when there is more than one.
system, turns = [], []
for m in request["messages"]:
    text = text_of(m.get("content"))
    if not text:
        continue
    if m.get("role") == "system":
        system.append(text)
    elif m.get("role") in ("user", "assistant"):
        turns.append((m["role"], text))
if not turns:
    fail("la petición no tiene ningún mensaje de usuario o asistente con texto")
prompt = turns[0][1] if len(turns) == 1 else "\n\n".join(f"[{r}]\n{t}" for r, t in turns)

argv = [
    claude, "-p",
    "--output-format", "json",
    "--model", str(request["model"]),
    "--tools", "",  # no tools: one answer, nothing run
    "--no-session-persistence",
    "--setting-sources", "",  # none of the person's settings, hooks or CLAUDE.md
    "--strict-mcp-config",
]
# A HOME of its own, so nothing of the person's (~/.claude, their login) is read or written.
home = tempfile.mkdtemp(prefix="clio-claude-")
env = {"PATH": os.environ.get("PATH", os.defpath), "HOME": home, "CLAUDE_CODE_OAUTH_TOKEN": key, **QUIET}
env.update({k: os.environ[k] for k in PROXY if os.environ.get(k)})
proc = None
try:
    # In a file: one argument may not pass 128 KiB (E2BIG), a system prompt can.
    system_file = os.path.join(home, "system-prompt.txt")
    with open(system_file, "w", encoding="utf-8") as f:
        f.write("\n\n".join(system))
    argv += ["--system-prompt-file", system_file]
    # Its own process group, so everything it starts is stopped with it. Never its stderr: what it
    # says there is not ours to show and could quote the request.
    proc = subprocess.Popen(
        argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        text=True, errors="replace", env=env, cwd=home, start_new_session=True,
    )
    stdout, _ = proc.communicate(prompt, timeout=TIMEOUT)
except subprocess.TimeoutExpired:
    fail(f"Claude Code no respondió en {TIMEOUT} s")
except OSError as e:
    fail(f"no pude ejecutar Claude Code: {type(e).__name__}")
finally:
    if proc is not None:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except OSError:
            pass  # the group is already gone
        proc.wait()
    shutil.rmtree(home, ignore_errors=True)

try:
    answer = json.loads(stdout)
except ValueError:
    answer = None
if not isinstance(answer, dict):
    fail(f"Claude Code no devolvió JSON válido (código {proc.returncode})")
if answer.get("is_error") or proc.returncode != 0:
    why = answer.get("result")
    if not isinstance(why, str):
        errors = answer.get("errors")
        why = "; ".join(e for e in errors if isinstance(e, str)) if isinstance(errors, list) else ""
    fail(f"Claude Code respondió con error: {why.replace(key, '***')[:300]}")
usage = answer.get("usage")
usage = usage if isinstance(usage, dict) else {}
result = answer.get("result")
json.dump(
    {
        "content": result if isinstance(result, str) else "",
        "input_tokens": int(usage.get("input_tokens") or 0),
        "output_tokens": int(usage.get("output_tokens") or 0),
    },
    sys.stdout,
)
