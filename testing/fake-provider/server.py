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
    {"tool_calls": [{"id": "x", "name": "t", ...}]}     with that call id (default call_N / toolu_fake_N)
    {"thinking": "...", ...}                            streamed reasoning before the answer
    {"status": 500, "body": "boom"}                     an HTTP error
    {"delay_ms": 200, ...}                              wait before answering (any item)
    {"hang": true}                                      never answer (for timeout tests)

POST .../v1/messages answers in Anthropic's format (the same script items: "text", "usage" as
[input_tokens, output_tokens], "status", "tool_calls" as `tool_use` blocks, "thinking" as a
`thinking` block signed with "signature", default "sig-fake"), plain or with `stream: true`.
In the OpenAI format "thinking" streams as `reasoning_content`. With --log-body each log
line also has the request's JSON body ("body"; the Authorization header is still never written).

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


def call_id(c, k, prefix="call_"):
    return c.get("id", "%s%d" % (prefix, k))


def anthropic_blocks(item):
    """The content blocks of an Anthropic reply: thinking, text, then tool_use."""
    blocks = []
    if "thinking" in item:
        blocks.append({"type": "thinking", "thinking": item["thinking"],
                       "signature": item.get("signature", "sig-fake")})
    calls = item.get("tool_calls", [])
    if item.get("text") or not (blocks or calls):
        blocks.append({"type": "text", "text": item.get("text") or ""})
    for k, c in enumerate(calls):
        blocks.append({"type": "tool_use", "id": call_id(c, k, "toolu_fake_"), "name": c["name"],
                       "input": c.get("arguments", {})})
    return blocks


def message_of(item):
    msg = {"role": "assistant", "content": item.get("text")}
    calls = item.get("tool_calls")
    if calls:
        msg["tool_calls"] = [
            {
                "id": call_id(c, k),
                "type": "function",
                "function": {"name": c["name"], "arguments": json.dumps(c.get("arguments", {}))},
            }
            for k, c in enumerate(calls)
        ]
    return msg


def last_user(messages):
    """The text of the last `user` message (what the model was just asked), for tests."""
    for m in reversed(messages):
        if m.get("role") == "user":
            c = m.get("content")
            if isinstance(c, list):
                c = "".join(p.get("text", "") for p in c if isinstance(p, dict))
            return c if isinstance(c, str) else None
    return None


def make_handler(script, log_path, expect_key, log_body=False):
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

        def _anthropic(self, req, item):
            p, c = item.get("usage", [10, 2])
            model, blocks = req.get("model", "fake"), anthropic_blocks(item)
            stop = "tool_use" if item.get("tool_calls") else "end_turn"
            if not req.get("stream"):
                return self._send(200, json.dumps({
                    "id": "msg_fake", "type": "message", "role": "assistant", "model": model,
                    "content": blocks, "stop_reason": stop,
                    "usage": {"input_tokens": p, "output_tokens": c}}))
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.end_headers()

            def event(name, data):
                data["type"] = name
                self.wfile.write(b"event: " + name.encode() + b"\ndata: " + json.dumps(data).encode() + b"\n\n")
                self.wfile.flush()

            event("message_start", {"message": {"id": "msg_fake", "type": "message", "role": "assistant",
                                                "model": model, "content": [],
                                                "usage": {"input_tokens": p, "output_tokens": 1}}})
            for i, b in enumerate(blocks):
                if b["type"] == "thinking":
                    start = {"type": "thinking", "thinking": "", "signature": ""}
                    deltas = [{"type": "thinking_delta", "thinking": b["thinking"]},
                              {"type": "signature_delta", "signature": b["signature"]}]
                elif b["type"] == "tool_use":
                    start = dict(b, input={})
                    deltas = [{"type": "input_json_delta", "partial_json": json.dumps(b["input"])}]
                else:
                    start = {"type": "text", "text": ""}
                    deltas = [{"type": "text_delta", "text": b["text"]}]
                event("content_block_start", {"index": i, "content_block": start})
                for d in deltas:
                    event("content_block_delta", {"index": i, "delta": d})
                event("content_block_stop", {"index": i})
            event("message_delta", {"delta": {"stop_reason": stop}, "usage": {"output_tokens": c}})
            event("message_stop", {})
            self.close_connection = True

        def do_POST(self):
            n = int(self.headers.get("Content-Length") or 0)
            req = json.loads(self.rfile.read(n) or b"{}")
            if log_path:
                # Neither carrier of a key is ever written (Anthropic API keys travel in x-api-key).
                headers = {k.lower(): v for k, v in self.headers.items()
                           if k.lower() not in ("authorization", "x-api-key")}
                headers["authorization_present"] = self.headers.get("Authorization") is not None
                headers["authorization_matches"] = (
                    expect_key is not None and self.headers.get("Authorization") == "Bearer " + expect_key
                )
                entry = {
                    "path": self.path,
                    "model": req.get("model"),
                    "stream": bool(req.get("stream")),
                    "roles": [m.get("role") for m in req.get("messages", [])],
                    "last_user": last_user(req.get("messages", [])),
                    "tools": [t.get("function", {}).get("name") for t in req.get("tools", [])],
                    "headers": headers,
                }
                if log_body:
                    entry["body"] = req
                with LOCK, open(log_path, "a") as f:
                    f.write(json.dumps(entry) + "\n")
            # The Anthropic SDK may add a query (`?beta=true` for OAuth).
            route = self.path.split("?")[0].rstrip("/")
            anthropic = route.endswith("/v1/messages")
            if not anthropic and not route.endswith("/chat/completions"):
                return self._send(404, "{}")
            item = reply_for(script)
            if item.get("hang"):
                time.sleep(3600)
            time.sleep(item.get("delay_ms", 0) / 1000)
            if "status" in item:
                return self._send(item["status"], item.get("body", "error"), "text/plain")
            if anthropic:
                return self._anthropic(req, item)
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
            if item.get("thinking"):
                chunk({"reasoning_content": item["thinking"]})
            text = item.get("text") or ""
            for k in range(0, len(text), 8):
                chunk({"content": text[k:k + 8]})
            for k, c in enumerate(item.get("tool_calls", [])):
                chunk({"tool_calls": [{"index": k, "id": call_id(c, k), "type": "function",
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
    ap.add_argument("--log-body", action="store_true", help="also log each request's JSON body")
    ap.add_argument("--expect-key", help="log whether the request carried this (test) API key")
    a = ap.parse_args()
    with open(a.script) as f:
        script = json.load(f)
    srv = ThreadingHTTPServer(("127.0.0.1", a.port), make_handler(script, a.log, a.expect_key, a.log_body))
    srv.daemon_threads = True
    port = srv.server_address[1]
    print(json.dumps({"port": port, "base_url": "http://127.0.0.1:%d/v1" % port}), flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    sys.exit(main())
