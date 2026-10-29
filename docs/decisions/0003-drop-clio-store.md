# 0003: Drop the clio-store crate

## Context

The first layout planned `clio-store` (SQLite, job queue, retention and rollups) with `clio-memory` on top of it. The clio-memory spec (v0.2) puts all of that inside clio-memory: its own `Store` trait, the queue in the same file, and the `retention` module "in the same crate" for clio-trace to reuse. clio-memory v1 was built that way, and `clio-store` stayed an empty crate nothing depended on.

## Decision

Remove `crates/clio-store`. Storage, queue and (in v2) retention live in clio-memory, as the spec says. If clio-trace later needs retention without the rest of clio-memory, split it out then.
