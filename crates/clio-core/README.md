# clio-core

Shared pieces. Today: `Clock` with `SystemClock` and `FixedClock`, so everything that depends on time can be replayed. Later: ULID ids, level, contract, events, errors. clio-memory and clio-trace keep their own clocks (no dependency between crates).

Spec: [`docs/specs/`](../../docs/specs/). 
