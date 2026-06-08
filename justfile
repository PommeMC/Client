default:
    @just --list

launcher-dev:
    @pnpm --filter pomme-launcher tauri dev

launcher-build *args:
    @pnpm --filter pomme-launcher tauri build {{ args }}

launcher-pre-pr:
    @cargo fmt -p pomme-launcher -- --check
    @cargo clippy -p pomme-launcher --release --all-targets --all-features -- -D warnings
    @pnpm --filter pomme-launcher pre-pr

client-dev:
    @cargo run -p pomme-client

client-build *args:
    @cargo build -p pomme-client {{ args }}

client-pre-pr:
    @cargo fmt -p pomme-client -- --check
    @cargo clippy -p pomme-client --release --all-targets --all-features -- -D warnings
