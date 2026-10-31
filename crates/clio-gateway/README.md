# clio-gateway

The one place every action goes through before it runs. It assumes the model was already fooled: the checks live in code, fail closed, and a deny always wins. No network inside; it decides, the node executes.

- `Gateway::decide(&ActionRequest) -> Decision` (allow / ask / deny, the id of the rule that decided, a reason, a decision id and the hash of the normalized action). `decide_or_deny` turns a panic into a deny.
- Evaluation order of the spec: declared tool, pinned description and schema (S-15) -> normalize paths (S-13), argv (S-12) and destination -> hard prohibitions (S-02, S-16, S-17, S-56) -> budget (S-47) -> secrets tied to tool and destination (S-19) -> tainted session sending out asks (S-06) -> plan -> reversible inside the sandbox -> policy hook (phase 5) -> approval over the action hash (S-14).
- Commands are an argv whose first element is in an allowed list; nothing is ever given to `sh -c`.
- `Session` / `wrap_untrusted` (S-05, S-08) and `ModelOutput` (S-10: no way to a command or to HTML without validating).
- `Approver` (terminal in this phase) and `execute`: runs only if the hash being run is the approved one; away mode sets aside what would ask.
- Rules are one JSON document (`Rules`); `Telemetry` counts decisions by kind and rule.
- Binary `clio-gateway <rules.json>`: one JSON request per line on stdin, one decision per line on stdout, as its own process (S-11). `GatewayClient` is the loop's side; if the process is gone it answers deny.

Spec: [`docs/specs/clio-gateway-spec-v0.1.md`](../../docs/specs/clio-gateway-spec-v0.1.md). Nothing is public yet.
