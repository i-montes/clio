# fuzz

Fuzzing of the parsers (S-48) with `cargo-fuzz`. Needs nightly: `cargo +nightly fuzz run <target>` from the repository root.

Targets: `traceparent` (the header a node sends) and `bench_task` (a task file). The parsers of the WebSocket messages, the tokenizer and the importers get their target when their crate has them. It runs every night (`.github/workflows/nightly.yml`); a crash blocks.

This crate is outside the root workspace on purpose: it needs nightly and its own `Cargo.lock`.
