#!/usr/bin/env python3
"""The model adapter of the Claude connector: reads a Request as JSON on stdin, asks the Anthropic
Messages API with a Claude subscription token, writes a Response as JSON on stdout. Same contract
as openai_compat.py.

The token comes in an envelope on stdin, {"key": ..., "request": <Request>}, which is how Clio
calls it; given a bare Request it comes from the environment (CLAUDE_CODE_OAUTH_TOKEN). It is never
read from a file and never printed. LLM_BASE_URL changes the API, but only to that server or to
this machine. Standard library only.
"""
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

BASE = os.environ.get("LLM_BASE_URL", "https://api.anthropic.com").rstrip("/")
TIMEOUT = int(os.environ.get("LLM_TIMEOUT_SECONDS", "90"))
# A subscription token is only accepted from Claude Code, so the call says it is Claude Code. The
# values are the ones Pi sends for such a token (pi-ai anthropic-messages.js, `claudeCodeVersion`,
# `getBetaFeatures`, the OAuth branch of `createClient` and `buildParams`); the API version is the
# Anthropic SDK's.
CLAUDE_CODE_VERSION = "2.1.280"
IDENTITY = "You are Claude Code, Anthropic's official CLI for Claude."


def fail(message: str) -> None:
    print(f"clio-model-adapter: {message}", file=sys.stderr)
    sys.exit(1)


def check_base() -> None:
    """The token travels in a header: to Anthropic over TLS, or to this machine. Any other host (an
    `http://` one, or any other scheme, `file://` included) would get it readable, or be somewhere
    unintended."""
    u = urllib.parse.urlsplit(BASE)
    local = u.scheme == "http" and u.hostname in ("127.0.0.1", "localhost", "::1")
    own = u.scheme == "https" and u.hostname == "api.anthropic.com"
    if not own and not local:
        fail("LLM_BASE_URL must be https://api.anthropic.com (or http://127.0.0.1 or localhost)")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """`urllib` follows a 302 and keeps `Authorization`, which would hand the token to whatever
    host the answer names. A redirect from the provider is an error here, not something to follow."""

    def redirect_request(self, *args, **kwargs):
        return None


def text_of(content) -> str:
    if isinstance(content, list):
        return "".join(p.get("text") or "" for p in content if isinstance(p, dict))
    return content if isinstance(content, str) else ""


check_base()
sent = json.load(sys.stdin)
if "key" in sent and "request" in sent:
    key, request = sent["key"], sent["request"]
else:
    key, request = os.environ.get("CLAUDE_CODE_OAUTH_TOKEN"), sent
key = key.strip() if isinstance(key, str) else ""
if not key:
    fail("the token is missing: Clio sends it in the envelope, or set CLAUDE_CODE_OAUTH_TOKEN")
# A key with a line break or a non-ASCII character makes `http.client` raise an error that quotes
# the header, token included: refuse it here, without printing it.
if not all(0x21 <= ord(ch) <= 0x7E for ch in key):
    fail("the token has a character that cannot go in an HTTP header (a line break, a space, non-ASCII)")

# Clio's `system` messages go in the API's `system` (after the identity block); the API takes only
# `role` and plain-text `content` in user/assistant messages and rejects empty ones, so that is all
# that is sent and anything else (other roles, images, extra fields) is left out.
system = [{"type": "text", "text": IDENTITY}]
messages, extra = [], []
for m in request["messages"]:
    text = text_of(m.get("content"))
    if not text:
        continue
    if m.get("role") == "system":
        extra.append(text)
    elif m.get("role") in ("user", "assistant"):
        messages.append({"role": m["role"], "content": text})
if extra:
    system.append({"type": "text", "text": "\n\n".join(extra)})
if not messages:
    fail("la petición no tiene ningún mensaje de usuario o asistente con texto")
body = {
    "model": request["model"],
    "max_tokens": request.get("max_tokens") or 4096,
    "system": system,
    "messages": messages,
}

http = urllib.request.Request(
    f"{BASE}/v1/messages",
    data=json.dumps(body).encode(),
    headers={
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
        "Accept": "application/json",
        "anthropic-version": "2023-06-01",
        "anthropic-beta": "claude-code-20250219,oauth-2025-04-20",
        "User-Agent": f"claude-cli/{CLAUDE_CODE_VERSION}",
        "x-app": "cli",
    },
)
try:
    with urllib.request.build_opener(NoRedirect).open(http, timeout=TIMEOUT) as response:
        answer = json.load(response)
except urllib.error.HTTPError as e:
    fail(f"HTTP {e.code} {e.reason}")  # never the body: it can echo the token
except urllib.error.URLError as e:  # network down
    fail(f"URLError: {e.reason}")
except Exception as e:  # timeout, bad JSON: only the kind, a message could quote the request
    fail(type(e).__name__)

if not isinstance(answer, dict):
    fail("la respuesta no es JSON válido")
usage = answer.get("usage")
usage = usage if isinstance(usage, dict) else {}
blocks = answer.get("content")
blocks = blocks if isinstance(blocks, list) else []
json.dump(
    {
        "content": "".join(b.get("text") or "" for b in blocks if isinstance(b, dict) and b.get("type") == "text"),
        "input_tokens": int(usage.get("input_tokens") or 0),
        "output_tokens": int(usage.get("output_tokens") or 0),
    },
    sys.stdout,
)
