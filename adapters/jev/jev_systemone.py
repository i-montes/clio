#!/usr/bin/env python3
"""The Jev adapter of Clio: reads {"key": "...", "body": {...}} as JSON on stdin, asks TypeSafe's
System One API (`POST /v1/systemone`), writes the API's JSON answer on stdout. Standard library only.

The key comes from the caller (the vault, S-18): never from the environment or a file here, and it is
never printed. It goes in one header, to `api.typesafe.ai` over TLS and nowhere else (S-44):
`JEV_BASE_URL` may only point at this machine over plain http, for tests. A redirect is an error,
not something to follow: it would hand the key to whatever host the answer names.
"""
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

HOST = "api.typesafe.ai"
BASE = os.environ.get("JEV_BASE_URL", f"https://{HOST}").rstrip("/")
TIMEOUT = float(os.environ.get("JEV_TIMEOUT_SECONDS", "3"))
MAX_BYTES = 1 << 20


def fail(message: str) -> None:
    print(f"clio-jev-adapter: {message}", file=sys.stderr)
    sys.exit(1)


def check_base() -> None:
    u = urllib.parse.urlsplit(BASE)
    ours = u.scheme == "https" and u.hostname == HOST and u.port in (None, 443)
    local = u.scheme == "http" and u.hostname in ("127.0.0.1", "localhost", "::1")
    if not (ours or local) or u.username or u.password or u.path not in ("", "/"):
        fail(f"JEV_BASE_URL must be https://{HOST} (http only for 127.0.0.1 or localhost)")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


check_base()
raw = sys.stdin.buffer.read(MAX_BYTES + 1)
if len(raw) > MAX_BYTES:
    fail("the request is too large")
try:
    request = json.loads(raw)
    key, body = request["key"], request["body"]
    if not (isinstance(key, str) and key and isinstance(body, dict)):
        raise ValueError
except (ValueError, KeyError, TypeError):
    fail('stdin must be {"key": "...", "body": {...}}')

http = urllib.request.Request(
    f"{BASE}/v1/systemone",
    data=json.dumps(body).encode(),
    headers={
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
        # The API sits behind a proxy that may refuse Python's default User-Agent.
        "User-Agent": "clio/0.1",
    },
)
try:
    with urllib.request.build_opener(NoRedirect).open(http, timeout=TIMEOUT) as response:
        answer = response.read(MAX_BYTES + 1)
except urllib.error.HTTPError as e:
    # The status and its reason, nothing the server said: an answer may repeat the request's
    # header, and a part of the key is as good as the key (S-62).
    fail(f"HTTP {e.code} {e.reason}")
except Exception as e:  # network down, timeout
    fail(f"{type(e).__name__}")
if len(answer) > MAX_BYTES:
    fail("the answer is too large")
try:
    json.dump(json.loads(answer), sys.stdout)
except ValueError:
    fail("the answer is not JSON")
