# testing/chaos

Crate `clio-chaos`: the skeleton of the chaos harness. It breaks one thing at a time (network cut, VPS down, disk full, provider down, vault locked) and checks that the system fails closed (`FailedClosed`) or keeps only the safe part (`Degraded`) instead of corrupting data or leaking.

A fault with nothing built to test reports `Unknown`: listed as pending, never counted as safe. The gate blocks on `Corrupted` or `Leaked`.

To use it, implement `System::inject` for the real thing (it applies the fault, observes, and puts everything back). Runs every night (see `.github/workflows/nightly.yml`).
