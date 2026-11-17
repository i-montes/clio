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
        return "".join(p.get("text", "") for p in content if isinstance(p, dict))
    return content or ""


check_base()
sent = json.load(sys.stdin)
if "key" in sent and "request" in sent:
    key, request = sent["key"], sent["request"]
else:
    key, request = os.environ.get("CLAUDE_CODE_OAUTH_TOKEN"), sent
if not key:
    fail("the token is missing: Clio sends it in the envelope, or set CLAUDE_CODE_OAUTH_TOKEN")

# Clio's `system` messages go in the API's `system` (after the identity block), the rest as they are.
system = [{"type": "text", "text": IDENTITY}]
extra = "\n\n".join(t for t in (text_of(m.get("content")) for m in request["messages"] if m.get("role") == "system") if t)
if extra:
    system.append({"type": "text", "text": extra})
body = {
    "model": request["model"],
    "max_tokens": request.get("max_tokens") or 4096,
    "system": system,
    "messages": [m for m in request["messages"] if m.get("role") in ("user", "assistant")],
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
except Exception as e:  # network down, timeout, bad JSON
    fail(f"{type(e).__name__}: {e}")

usage = answer.get("usage", {})
json.dump(
    {
        "content": "".join(b.get("text", "") for b in answer.get("content", []) if b.get("type") == "text"),
        "input_tokens": int(usage.get("input_tokens", 0)),
        "output_tokens": int(usage.get("output_tokens", 0)),
    },
    sys.stdout,
)
