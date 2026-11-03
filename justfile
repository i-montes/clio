# One-time setup after cloning: enable the secret-scanning pre-commit hook (S-43).
setup:
    git config core.hooksPath .githooks

# macOS only: download and verify the default embedding model (decision 0004).
setup-model-macos:
    infra/models/install-potion.macos.sh

# Linux only: download and verify the default embedding model (decision 0004).
setup-model-linux:
    infra/models/install-potion.linux.sh

check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    scripts/rule-coverage.sh

test:
    cargo test --workspace --all-features

# Supply-chain gates (S-41); also run in CI.
audit:
    cargo deny check
    cargo audit

# The bench in replay mode (no network); see testing/bench/README.md for the real and agent modes.
bench:
    cargo run -q --locked -p clio-bench -- replay --tasks testing/bench/tasks --tape testing/bench/tapes/beam-free.jsonl --prices testing/bench/prices.json --provider opencode-go --model beam-free --repeats 3 --baseline testing/bench/baseline.json --thresholds testing/bench/thresholds.json
    python3 testing/bench/verify_real_tasks.py

# The attack battery, alone and against the whole agent.
attacks:
    cargo test --locked -p clio-attacks -p clio-gateway --test attacks -p clio-brain --test attacks

# Needs nightly and cargo-fuzz (the nightly CI installs them); one minute per target.
fuzz:
    cargo +nightly fuzz run traceparent -- -max_total_time=60
    cargo +nightly fuzz run bench_task -- -max_total_time=60

# Build the release binaries and put `clio` and `clio-vault` in ~/.local/bin (it must be in your PATH).
# Run it again after any change to the code: the installed copies do not update by themselves.
install:
    cargo build --release --locked -p clio-brain -p clio-vault
    install -d -m 0755 "$HOME/.local/bin"
    install -m 0755 target/release/clio target/release/clio-vault "$HOME/.local/bin/"
    @echo "installed clio and clio-vault in $HOME/.local/bin"
    # Pi, the agent harness, pinned by version and lockfile (npm ci --ignore-scripts; needs Node 22.19+ and the network once).
    -"$HOME/.local/bin/clio" pi install
