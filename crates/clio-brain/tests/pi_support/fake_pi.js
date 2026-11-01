// A tiny, deterministic stand-in for `pi --mode rpc` (no network, no real Pi). Used by
// tests/pi_rpc.rs. The prompt text picks the behavior:
//   "ok"      message and tool events, a U+2028 inside text, an unknown event, an extension dialog
//   "slow"    answers the prompt, then stays silent for 5 s
//   "hang"    never settles by itself; `abort` settles it with aborted:true
//   "fail"    the prompt response is success:false
//   "crash"   exits with code 3 after writing escape sequences to stderr
//   "flood"   writes 200 KB to stderr, then settles
//   "evil"    starts a tool that is not allowed
// Env FAKE_PI_PIDFILE: writes its own pid and the pid of a `sleep` child (same group).
const { spawn } = require("node:child_process");
const fs = require("node:fs");

if (process.env.FAKE_PI_PIDFILE) {
  const kid = spawn("sleep", ["300"], { stdio: "ignore" });
  fs.writeFileSync(process.env.FAKE_PI_PIDFILE, `${process.pid} ${kid.pid}\n`);
}

const out = (o) => process.stdout.write(JSON.stringify(o) + "\n");
const resp = (id, command, data, success = true, error) =>
  out({ id, type: "response", command, success, ...(success ? { data } : { error }) });
let hanging = false;
const usage = (i, o) => ({ input: i, output: o, cacheRead: 0, cacheWrite: 0, totalTokens: i + o, cost: { total: 0.01 } });
const settle = (aborted) => { out({ type: "agent_end", messages: [], willRetry: false }); out({ type: "agent_settled", aborted }); };

function prompt(id, msg) {
  if (msg === "fail") return resp(id, "prompt", null, false, "\u001b[31mrejected\u001b[0m");
  if (msg === "crash") {
    process.stderr.write("boom \u001b[31mred\u001b[0m \u001b]0;title\u0007 end\n");
    return process.exit(3);
  }
  resp(id, "prompt", { disposition: "started" });
  out({ type: "agent_start" });
  out({ type: "turn_start" });
  if (msg === "hang") { hanging = true; return; }
  if (msg === "slow") { setTimeout(() => settle(false), 5000); return; }
  if (msg === "flood") process.stderr.write("x".repeat(200000));
  if (msg === "evil") out({ type: "tool_execution_start", toolCallId: "c9", toolName: "bash", args: { command: "id" } });
  out({ type: "message_start", message: { role: "assistant", content: [] } });
  out({ type: "message_update", usage: usage(100, 1), assistantMessageEvent: { type: "text_delta", delta: "hel lo " } });
  out({ type: "future_event_v9", payload: { a: 1 } });
  out({ type: "tool_execution_start", toolCallId: "c1", toolName: "read", args: { path: "a.txt" } });
  out({ type: "tool_execution_end", toolCallId: "c1", toolName: "read", result: { content: [] }, isError: false, durationMs: 4 });
  out({ type: "extension_ui_request", id: "ui-1", method: "confirm", title: "sure?" });
  out({ type: "message_update", usage: usage(100, 5), assistantMessageEvent: { type: "text_delta", delta: "world" } });
  out({ type: "message_end", message: { role: "assistant", content: [{ type: "text", text: "hel lo world" }], usage: usage(100, 5), stopReason: "stop" } });
  out({ type: "turn_end" });
  settle(false);
}

function handle(line) {
  let c;
  try { c = JSON.parse(line); } catch { return out({ type: "response", command: "parse", success: false, error: "bad json" }); }
  const { id, type } = c;
  switch (type) {
    case "prompt": return prompt(id, c.message);
    case "abort":
      resp(id, "abort", undefined);
      if (hanging) { hanging = false; settle(true); }
      return;
    case "get_state": {
      // The response goes out in small pieces on purpose.
      const s = JSON.stringify({ id, type: "response", command: "get_state", success: true, data: { model: { provider: "p", id: "m" }, sessionId: "s1", sessionFile: "/x/s1.jsonl", isStreaming: false, messageCount: 0 } }) + "\n";
      let i = 0;
      const t = setInterval(() => { process.stdout.write(s.slice(i, i + 7)); i += 7; if (i >= s.length) clearInterval(t); }, 5);
      return;
    }
    case "get_session_stats": return resp(id, type, { tokens: { input: 100, output: 5, total: 105 }, cost: 0.01, totalMessages: 3, toolCalls: 1, contextUsage: { tokens: 105, contextWindow: 1000, percent: 10.5 } });
    case "get_messages": return resp(id, type, { messages: [{ role: "user", content: "hi" }] });
    case "get_commands": return resp(id, type, { commands: [] });
    case "set_session_name": case "steer": case "follow_up": return resp(id, type, {});
    case "compact": return resp(id, type, { summary: "s", tokensBefore: 10 });
    case "new_session": case "switch_session": return resp(id, type, { cancelled: false });
    case "fork": return resp(id, type, { text: "orig", cancelled: false });
    case "never_answered": return;
    case "extension_ui_response": process.stderr.write(`ui-cancelled:${c.id}:${c.cancelled}\n`); return;
    default: return resp(id, type, undefined, false, "unknown command " + type);
  }
}

let buf = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (d) => {
  buf += d;
  let i;
  while ((i = buf.indexOf("\n")) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); if (l.trim()) handle(l); }
});
process.stdin.on("end", () => process.exit(0));
