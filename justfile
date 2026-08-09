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
    @cargo fmt -p pomme-protocol -- --check
    @cargo clippy -p pomme-client --release --all-targets --all-features -- -D warnings
    @cargo clippy -p pomme-protocol --release --all-targets --all-features -- -D warnings
    @cargo test -p pomme-protocol
    @cargo test -p pomme-client -- net::azalea_compat world::block

# Regenerate a version's packet-id table from the decompiled reference.
protogen version="26.2":
    @cargo run -p protogen -- reference/{{ version }}/decompiled {{ version }} pomme-protocol/src/data/protocol-{{ version }}.json

# Regenerate a version's client-registry id table from the data-generator report.
registrygen version="26.2":
    @cargo run -p protogen -- registries reference/{{ version }} {{ version }} pomme-protocol/src/data/registries-{{ version }}.json

# Regenerate a version's block-state table from the data-generator report.
blockgen version="26.2":
    @cargo run -p blockgen -- blocks reference/{{ version }}/generated/reports/blocks.json {{ version }} pomme-client/src/world/block/data/blocks-{{ version }}.json

# JDK 25 bin dir for lightgen; override with `just jdk=<path> lightgen`.
jdk := "C:/Program Files/Amazon Corretto/jdk25.0.2_10/bin"

# Regenerate a version's light-property table by running vanilla's own code
# (tools/lightgen/LightDump.java) against the reference server jar, then
# compacting the dump with `blockgen light`. Uses the deobf server jar when
# one exists (pre-26.x); needs the Corretto JDK for 26.x class files.
lightgen version="26.2":
    #!/usr/bin/env bash
    set -euo pipefail
    v="{{ version }}"
    ref="reference/$v"
    jdk="{{ jdk }}"
    classes="$ref/server-$v.jar"
    if [ -f "$ref/server-$v-deobf.jar" ]; then classes="$ref/server-$v-deobf.jar"; fi
    if ! find "$ref/bundler" -name '*.jar' 2>/dev/null | grep -q .; then
        # This unzip build doesn't glob archive members; list them explicitly.
        unzip -Z1 "$ref/server.jar" | grep '^META-INF/libraries/.*\.jar$' \
            | xargs unzip -qn "$ref/server.jar" -d "$ref/bundler"
    fi
    libs=$(find "$ref/bundler" -name '*.jar' | tr '\n' ';')
    mkdir -p tools/lightgen/out
    "$jdk/javac.exe" --release 21 -d tools/lightgen/out tools/lightgen/LightDump.java
    "$jdk/java.exe" -cp "$classes;${libs}tools/lightgen/out" LightDump "$v" "$ref/generated/light.json"
    cargo run -p blockgen -- light "$ref/generated/light.json" pomme-client/src/world/block/data/blocks-"$v".json pomme-client/src/world/block/data/light-"$v".json
