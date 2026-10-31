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

bench:
    @echo "bench: not set up yet"

attacks:
    @echo "attacks: not set up yet (see testing/attacks)"

fuzz:
    @echo "fuzz: not set up yet (S-48)"
