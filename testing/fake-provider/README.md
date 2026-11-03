# testing/fake-provider

A fake OpenAI-compatible model server (`server.py`, Python 3 stdlib, listens on 127.0.0.1 only, no network). It answers `POST .../chat/completions` from a scripted list, as plain JSON or as a stream (what Pi uses), including tool calls, HTTP errors, delays and a hang.

```sh
python3 testing/fake-provider/server.py --script script.json [--log requests.jsonl] [--port 0] [--expect-key KEY]
# stdout, one line: {"port": 41234, "base_url": "http://127.0.0.1:41234/v1"}
```

The script is a JSON list; each request takes the next item (the last repeats): `{"text": "hi", "usage": [100, 5]}`, `{"tool_calls": [{"name": "bash", "arguments": {...}}]}`, `{"status": 500, "body": "boom"}`, `{"delay_ms": 200, ...}`, `{"hang": true}`. The log records, per request, the model, the message roles, the declared tool names and the headers; the Authorization value is never written (only whether it was present, and with `--expect-key` whether it matched). Point a provider's `baseUrl` at `base_url` (in Pi's `models.json`, for example). Used by `crates/clio-brain/tests/pi_real.rs` (helper `FakeProvider` in `tests/pi_support/`).
