#!/usr/bin/env python3
"""The model adapter of the Anthropic API connector: reads a Request as JSON on stdin, asks the
Anthropic Messages API with an API key (billed per token), writes a Response as JSON on stdout.
Same contract as openai_compat.py.

The key comes in an envelope on stdin, {"key": ..., "request": <Request>}, which is how Clio calls
it; given a bare Request it comes from the environment (ANTHROPIC_API_KEY). It is never read from a
file and never printed. LLM_BASE_URL changes the API, but only to that server or to this machine.
Standard library only.
"""
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

BASE = os.environ.get("LLM_BASE_URL", "https://api.anthropic.com").rstrip("/")
TIMEOUT = int(os.environ.get("LLM_TIMEOUT_SECONDS", "90"))


def fail(message: str) -> None:
    print(f"clio-model-adapter: {message}", file=sys.stderr)
    sys.exit(1)


def check_base() -> None:
    """The key travels in a header: to Anthropic over TLS, or to this machine. Any other host (an
    `http://` one, or any other scheme, `file://` included) would get it readable, or be somewhere
    unintended."""
    u = urllib.parse.urlsplit(BASE)
    local = u.scheme == "http" and u.hostname in ("127.0.0.1", "localhost", "::1")
    own = u.scheme == "https" and u.hostname == "api.anthropic.com"
    if not own and not local:
        fail("LLM_BASE_URL debe ser https://api.anthropic.com (o http://127.0.0.1 o localhost)")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """`urllib` follows a 302 and keeps `x-api-key`, which would hand the key to whatever
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
    key, request = os.environ.get("ANTHROPIC_API_KEY"), sent
key = key.strip() if isinstance(key, str) else ""
if not key:
    fail("falta la llave: Clio la manda en el sobre, o define ANTHROPIC_API_KEY")
# A key with a line break or a non-ASCII character makes `http.client` raise an error that quotes
# the header, key included: refuse it here, without printing it.
if not all(0x21 <= ord(ch) <= 0x7E for ch in key):
    fail("la llave tiene un carácter que no puede ir en una cabecera HTTP (un salto de línea, un espacio, algo no ASCII)")

# Clio's `system` messages go in the API's `system`; the API takes only `role` and plain-text
# `content` in user/assistant messages and rejects empty ones, so that is all that is sent and
# anything else (other roles, images, extra fields) is left out.
messages, extra = [], []
for m in request["messages"]:
    text = text_of(m.get("content"))
    if not text:
        continue
    if m.get("role") == "system":
        extra.append(text)
    elif m.get("role") in ("user", "assistant"):
        messages.append({"role": m["role"], "content": text})
if not messages:
    fail("la petición no tiene ningún mensaje de usuario o asistente con texto")
body = {
    "model": request["model"],
    "max_tokens": request.get("max_tokens") or 4096,
    "messages": messages,
}
if extra:
    body["system"] = [{"type": "text", "text": "\n\n".join(extra)}]

http = urllib.request.Request(
    f"{BASE}/v1/messages",
    data=json.dumps(body).encode(),
    headers={
        "x-api-key": key,
        "Content-Type": "application/json",
        "Accept": "application/json",
        "anthropic-version": "2023-06-01",
        "User-Agent": "clio/0.1",
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
