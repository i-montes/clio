#!/usr/bin/env python3
"""A fake OpenAI-compatible model server for tests (chat/completions only, no network).

It listens on 127.0.0.1 only. Each POST to .../chat/completions takes the next scripted reply
(the last one repeats). Usage:

    server.py --script script.json [--log requests.jsonl] [--port 0] [--expect-key KEY]

On start it prints one JSON line to stdout: {"port": N, "base_url": "http://127.0.0.1:N/v1"}.
The script is a JSON list; each item is one reply:

    {"text": "hello"}                                   plain answer
    {"text": "...", "usage": [100, 5]}                  with [prompt_tokens, completion_tokens]
    {"tool_calls": [{"name": "t", "arguments": {...}}]} the model asks for tools
    {"status": 500, "body": "boom"}                     an HTTP error
    {"delay_ms": 200, ...}                              wait before answering (any item)
    {"hang": true}                                      never answer (for timeout tests)

Both `stream: true` (SSE, what Pi uses) and plain JSON answers are supported. The log has one
JSON line per request: path, model, the roles of the messages, the names of the declared tools,
and the headers (the Authorization value is never written: only whether it was present and,
with --expect-key, whether it equals `Bearer KEY`).
"""
import argparse
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOCK = threading.Lock()
STATE = {"n": 0}


def reply_for(script):
    with LOCK:
        i = min(STATE["n"], len(script) - 1)
        STATE["n"] += 1
    return script[i]


def usage_of(item):
    p, c = item.get("usage", [10, 2])
    return {"prompt_tokens": p, "completion_tokens": c, "total_tokens": p + c}


def message_of(item):
    msg = {"role": "assistant", "content": item.get("text")}
    calls = item.get("tool_calls")
    if calls:
        msg["tool_calls"] = [
            {
                "id": "call_%d" % k,
                "type": "function",
                "function": {"name": c["name"], "arguments": json.dumps(c.get("arguments", {}))},
            }
            for k, c in enumerate(calls)
        ]
    return msg


def make_handler(script, log_path, expect_key):
    class H(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *a):
            pass

        def _send(self, code, body, ctype="application/json"):
            data = body.encode() if isinstance(body, str) else body
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path.rstrip("/").endswith("/models"):
                self._send(200, json.dumps({"object": "list", "data": [{"id": "fake", "object": "model"}]}))
            else:
                self._send(404, "{}")

        def do_POST(self):
            n = int(self.headers.get("Content-Length") or 0)
            req = json.loads(self.rfile.read(n) or b"{}")
            if log_path:
                headers = {k.lower(): v for k, v in self.headers.items() if k.lower() != "authorization"}
                headers["authorization_present"] = self.headers.get("Authorization") is not None
                headers["authorization_matches"] = (
                    expect_key is not None and self.headers.get("Authorization") == "Bearer " + expect_key
                )
                entry = {
                    "path": self.path,
                    "model": req.get("model"),
                    "stream": bool(req.get("stream")),
                    "roles": [m.get("role") for m in req.get("messages", [])],
                    "tools": [t.get("function", {}).get("name") for t in req.get("tools", [])],
                    "headers": headers,
                }
                with LOCK, open(log_path, "a") as f:
                    f.write(json.dumps(entry) + "\n")
            if not self.path.rstrip("/").endswith("/chat/completions"):
                return self._send(404, "{}")
            item = reply_for(script)
            if item.get("hang"):
                time.sleep(3600)
            time.sleep(item.get("delay_ms", 0) / 1000)
            if "status" in item:
                return self._send(item["status"], item.get("body", "error"), "text/plain")
            usage = usage_of(item)
            model = req.get("model", "fake")
            finish = "tool_calls" if item.get("tool_calls") else "stop"
            if not req.get("stream"):
                body = {
                    "id": "chatcmpl-fake",
                    "object": "chat.completion",
                    "created": 0,
                    "model": model,
                    "choices": [{"index": 0, "message": message_of(item), "finish_reason": finish}],
                    "usage": usage,
                }
                return self._send(200, json.dumps(body))
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.end_headers()

            def chunk(delta, finish_reason=None, usage_obj=None):
                c = {"id": "chatcmpl-fake", "object": "chat.completion.chunk", "created": 0, "model": model,
                     "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}]}
                if usage_obj is not None:
                    c["choices"] = []
                    c["usage"] = usage_obj
                self.wfile.write(b"data: " + json.dumps(c).encode() + b"\n\n")
                self.wfile.flush()

            chunk({"role": "assistant", "content": ""})
            text = item.get("text") or ""
            for k in range(0, len(text), 8):
                chunk({"content": text[k:k + 8]})
            for k, c in enumerate(item.get("tool_calls", [])):
                chunk({"tool_calls": [{"index": k, "id": "call_%d" % k, "type": "function",
                                       "function": {"name": c["name"],
                                                    "arguments": json.dumps(c.get("arguments", {}))}}]})
            chunk({}, finish)
            chunk(None, usage_obj=usage)
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
            self.close_connection = True

    return H


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--script", required=True)
    ap.add_argument("--log")
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--expect-key", help="log whether the request carried this (test) API key")
    a = ap.parse_args()
    with open(a.script) as f:
        script = json.load(f)
    srv = ThreadingHTTPServer(("127.0.0.1", a.port), make_handler(script, a.log, a.expect_key))
    srv.daemon_threads = True
    port = srv.server_address[1]
    print(json.dumps({"port": port, "base_url": "http://127.0.0.1:%d/v1" % port}), flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    sys.exit(main())
