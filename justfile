default:
    @just --list

launcher-dev *args:
    @pnpm --filter pomme-launcher tauri dev {{ args }}

launcher-build *args:
    @pnpm --filter pomme-launcher tauri build {{ args }}

launcher-pre-pr:
    @cargo fmt -p pomme-launcher -- --check
    @cargo clippy -p pomme-launcher --release --all-targets --all-features -- -D warnings
    @pnpm --filter pomme-launcher pre-pr

client-dev *args:
    @cargo run -p pomme-client {{ args }}

# Optimized release client for accurate benchmarking (supplies the launch token the guard needs).
client-release *args:
    #!/usr/bin/env bash
    cargo run --release -p pomme-client -- --launch-token "$(mktemp)" {{ args }}

client-flamegraph *args:
    #!/usr/bin/env bash
    cargo flamegraph -p pomme-client -- --launch-token "$(mktemp)" {{ args }}

client-build *args:
    @cargo build -p pomme-client {{ args }}

client-pre-pr:
    @cargo fmt -p pomme-client -- --check
    @cargo clippy -p pomme-client --release --all-targets --all-features -- -D warnings
