#!/usr/bin/env python3
"""The program behind `clio-bench real|record --cmd`: reads a Request as JSON on stdin, asks an
OpenAI-compatible chat completions API, writes a Response as JSON on stdout.

The key comes only from the environment (OPENCODE_API_KEY) and is never printed. Defaults point
at OpenCode Go; LLM_BASE_URL changes the API. Standard library only.
"""
import json
import os
import sys
import urllib.error
import urllib.request
import uuid

BASE = os.environ.get("LLM_BASE_URL", "https://opencode.ai/inference/go/openai/v1").rstrip("/")
TIMEOUT = int(os.environ.get("LLM_TIMEOUT_SECONDS", "90"))


def fail(message: str) -> None:
    print(f"llm_openai_compat: {message}", file=sys.stderr)
    sys.exit(1)


key = os.environ.get("OPENCODE_API_KEY")
if not key:
    fail("OPENCODE_API_KEY is not set")

request = json.load(sys.stdin)
body = {
    "model": request["model"],
    "messages": request["messages"],
    "temperature": 0,
}
if request.get("max_tokens"):
    body["max_tokens"] = request["max_tokens"]

http = urllib.request.Request(
    f"{BASE}/chat/completions",
    data=json.dumps(body).encode(),
    headers={
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
        # The API sits behind Cloudflare, which refuses Python's default User-Agent (error 1010).
        "User-Agent": "clio-bench/0.1",
        # OpenCode asks for a session id per conversation, to route and cache. Each call here is
        # its own conversation, so each gets its own id.
        "x-opencode-session": str(uuid.uuid4()),
    },
)
try:
    with urllib.request.urlopen(http, timeout=TIMEOUT) as response:
        answer = json.load(response)
except urllib.error.HTTPError as e:
    fail(f"HTTP {e.code}: {e.read().decode(errors='replace')[:300]}")
except Exception as e:  # network down, timeout, bad JSON
    fail(f"{type(e).__name__}: {e}")

message = (answer.get("choices") or [{}])[0].get("message", {})
usage = answer.get("usage", {})
json.dump(
    {
        "content": message.get("content") or "",
        "input_tokens": int(usage.get("prompt_tokens", 0)),
        "output_tokens": int(usage.get("completion_tokens", 0)),
    },
    sys.stdout,
)
