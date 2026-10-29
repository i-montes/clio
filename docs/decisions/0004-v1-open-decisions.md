# 0004: Closing the open decisions of clio-memory v1

## Decision

- **Name:** `clio-memory` stays. Free on crates.io; the `clio` crate and a few similar GitHub repos exist, so searches can confuse them.
- **License:** `MIT OR Apache-2.0`, with `LICENSE-MIT` and `LICENSE-APACHE` at the root (copyright "I. Montes", the git author name; change it if a full name is wanted).
- **Default model:** potion-multilingual-128M stays; it is the only multilingual model in its family. Measured: 507 MB on disk, about 1.1 GB resident once loaded (1.6 GB peak while loading), 1.4 s to load, about 11 µs to encode a query.
  - CPU only. There is no GPU path: encoding is a table lookup and a mean, and `model2vec-rs` has none.
  - The embedder drops the model after 5 idle minutes (about 530 MB come back; about 550 MB stay held inside `model2vec-rs`, cause not found) and reloads it on the next use, checking the files did not change. `Model2VecEmbedder::from_dir_idle` sets the time or turns it off.
  - The CLI loads the model on every call; the v2 daemon removes that cost.
- **Acceptance 20:** the compact format must use at most half the tokens of the JSON one (measured: about a tenth).
- **Acceptance 25:** recall over 50 000 memories under 150 ms median (measured: 73 ms on a Ryzen 7 5700X). CI runs it on Linux; if the runner turns out too slow, raise the limit here and in the test together.

## Still open

Vector index, encryption at rest, remote API: they wait for evidence (see the spec).
