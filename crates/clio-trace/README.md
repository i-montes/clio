# clio-trace

Spans and computed cost for all of Clio, in their own SQLite file (`clio-trace.db`, mode 0600).

- `Trace::open(path, clock)`: creates or opens the file; refuses one that group or others can read.
- `start_span` / `child` / `end_span`: a span carries ids, times and decisions. There is no column for content (S-46); names and attribute values are short and checked for secrets.
- `set_price`, `end_span` with `Usage`: cost in whole micro-USD, from the price in force when the span started. A model with no price is an error, never a zero.
- `Span::traceparent` / `Traceparent::parse`: W3C trace context, to hang remote spans from the original request.
- `summary(trace_id)`: spans, errors, open spans, tokens, cost and duration. This is what the bench stores.
- `names`: the only place that knows the OpenTelemetry `gen_ai.*` names.

Not yet: OpenTelemetry export, retention and rollups, the viewer.

Spec: [`docs/specs/`](../../docs/specs/).
