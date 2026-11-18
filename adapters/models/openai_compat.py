#!/usr/bin/env python3
"""The model adapter of Clio (also the program behind `clio-bench real|record --cmd`): reads a Request as JSON on stdin, asks an
OpenAI-compatible chat completions API, writes a Response as JSON on stdout.

The key comes in an envelope on stdin, {"key": ..., "request": <Request>}, which is how Clio calls
it; given a bare Request it comes from the environment (OPENCODE_API_KEY), which is how the bench
calls it. It is never read from a file and never printed. Defaults point at OpenCode Go;
LLM_BASE_URL changes the API, but only to that server or to this machine. Standard library only.
"""
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid

BASE = os.environ.get("LLM_BASE_URL", "https://opencode.ai/inference/go/openai/v1").rstrip("/")
TIMEOUT = int(os.environ.get("LLM_TIMEOUT_SECONDS", "90"))
# A reasoning model spends its tokens thinking before it answers. Measured with opencode-go/beam-free
# on the same request: default 46 s and 4,773 reasoning tokens, "low" 7 s and 799, same answer; with
# a max_tokens cap the thinking can eat the whole cap and the content comes back empty. "off" omits it.
EFFORT = os.environ.get("LLM_REASONING_EFFORT", "low")


def fail(message: str) -> None:
    print(f"clio-model-adapter: {message}", file=sys.stderr)
    sys.exit(1)


def check_base() -> None:
    """The key travels in a header: to OpenCode over TLS, or to this machine. Any other host (an
    `http://` one, or any other scheme, `file://` included) would get it readable, or be somewhere
    unintended."""
    u = urllib.parse.urlsplit(BASE)
    local = u.scheme == "http" and u.hostname in ("127.0.0.1", "localhost", "::1")
    own = u.scheme == "https" and u.hostname == "opencode.ai"
    if not own and not local:
        fail("LLM_BASE_URL must be https://opencode.ai/... (or http://127.0.0.1 or localhost)")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """`urllib` follows a 302 and keeps `Authorization`, which would hand the key to whatever host
    the answer names. A redirect from the provider is an error here, not something to follow."""

    def redirect_request(self, *args, **kwargs):
        return None


check_base()
sent = json.load(sys.stdin)
if "key" in sent and "request" in sent:
    key, request = sent["key"], sent["request"]
else:
    key, request = os.environ.get("OPENCODE_API_KEY"), sent
key = key.strip() if isinstance(key, str) else ""
if not key:
    fail("missing key: Clio sends it in the envelope, or set OPENCODE_API_KEY")
# A key with a line break or a non-ASCII character makes `http.client` raise an error that quotes
# the header, token included: refuse it here, without printing it.
if not all(0x21 <= ord(ch) <= 0x7E for ch in key):
    fail("the key has a character that cannot go in an HTTP header (a line break, a space, something not ASCII)")

body = {
    "model": request["model"],
    "messages": request["messages"],
    "temperature": 0,
}
if request.get("max_tokens"):
    body["max_tokens"] = request["max_tokens"]
if EFFORT != "off":
    body["reasoning_effort"] = EFFORT

http = urllib.request.Request(
    f"{BASE}/chat/completions",
    data=json.dumps(body).encode(),
    headers={
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
        # The API sits behind Cloudflare, which refuses Python's default User-Agent (error 1010).
        "User-Agent": "clio/0.1",
        # OpenCode asks for a session id per conversation, to route and cache. Each call here is
        # its own conversation, so each gets its own id.
        "x-opencode-session": str(uuid.uuid4()),
    },
)
try:
    with urllib.request.build_opener(NoRedirect).open(http, timeout=TIMEOUT) as response:
        answer = json.load(response)
except urllib.error.HTTPError as e:
    fail(f"HTTP {e.code} {e.reason}")  # never the body: it can echo the key
except urllib.error.URLError as e:  # network down
    fail(f"URLError: {e.reason}")
except Exception as e:  # timeout, bad JSON: only the kind, a message could quote the request
    fail(type(e).__name__)

choice = (answer.get("choices") or [{}])[0]
message = choice.get("message", {})
if not message.get("content") and choice.get("finish_reason") == "length":
    print("clio-model-adapter: empty answer: max_tokens ran out while the model was reasoning", file=sys.stderr)
usage = answer.get("usage", {})
json.dump(
    {
        "content": message.get("content") or "",
        "input_tokens": int(usage.get("prompt_tokens", 0)),
        "output_tokens": int(usage.get("completion_tokens", 0)),
    },
    sys.stdout,
)
