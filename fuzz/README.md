# fuzz

Fuzzing of the parsers (S-48) with `cargo-fuzz`. Needs nightly: `cargo +nightly fuzz run <target>` from the repository root.

Targets: `traceparent` (the header a node sends), `bench_task` (a task file), `pi_text` (the cleaner of text from Pi: no control or hiding character survives), `rpc_event` (every JSON line Pi writes) and `scrubber` (the vault's output filter: a known value never survives). The parsers of the WebSocket messages, the tokenizer and the importers get their target when their crate has them. It runs every night (`.github/workflows/nightly.yml`); a crash blocks.

This crate is outside the root workspace on purpose: it needs nightly and its own `Cargo.lock`.
