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

# Stage Minecraft's OpenAL Soft next to the dev binaries, as the release job does.
openal:
    #!/usr/bin/env bash
    python=$(command -v python3 || command -v python) || {
        echo "warning: no python on PATH, skipping OpenAL staging (audio will be disabled)" >&2
        exit 0
    }
    "$python" tools/fetch_openal.py target/debug target/release

client-dev *args: openal
    @cargo run -p pomme-client {{ args }}

# Optimized release client for accurate benchmarking. Like any release build,
# it plays as your signed-in account (it asks the first time).
client-release *args: openal
    @cargo run --release -p pomme-client -- {{ args }}

# Headless CPU benchmarks (mesher, fade rebuild, occlusion), no window or GPU.
# An optional filter runs only matching benches, e.g. `just bench mesh`.
bench *args:
    @cargo run --release -p pomme-client --features cpu-bench -- --cpu-bench {{ args }}

client-build *args: openal
    @cargo build -p pomme-client {{ args }}

# In-game test scripts (tests/ingame/README.md) against a local vanilla
# 26.2 server, e.g. `just ingame-test player/armor_equip` or a whole folder like
# `just ingame-test entities`; no name runs them all.
ingame-test *scripts: openal
    @cargo run -p ingame-test -- {{ scripts }}

# Just the local test server, for driving a client over the harness socket.
ingame-server:
    @cargo run -p ingame-test -- --server-only

# Android (pomme-android/README.md): the arm64 debug APK.
android_app := "com.pommemc.client"

android-build:
    @python3 tools/build_openal.py android pomme-android/app/src/main/jniLibs/arm64-v8a
    @cargo ndk -t arm64-v8a -o pomme-android/app/src/main/jniLibs build -p pomme-android
    @cd pomme-android && ./gradlew -q assembleDebug

android-install: android-build
    @adb install -r pomme-android/app/build/outputs/apk/debug/app-debug.apk

android-run:
    @adb shell am start -n {{ android_app }}/android.app.NativeActivity

# Copies a desktop install's `minecraft/` assets and version files into the
# app's own storage, through a staging dir `run-as` can read.
android-push-assets src=(home_directory() / "Library/Application Support/Pomme/minecraft"):
    #!/usr/bin/env bash
    set -e
    stage=/data/local/tmp/pomme
    adb shell rm -rf "$stage" && adb shell mkdir -p "$stage/versions"
    adb push "{{ src }}/assets" "$stage/"
    adb push "{{ src }}/versions/26.2" "$stage/versions/"
    adb shell run-as {{ android_app }} sh -c "'mkdir -p files/minecraft && cp -r $stage/. files/minecraft/'"
    adb shell rm -rf "$stage"

# The launch arguments, as on a command line, e.g.
# `just android-config --version 26.2 --username Tester --quick-access-multiplayer 10.0.2.2:25565`.
android-config *args:
    #!/usr/bin/env bash
    set -e
    echo "{{ args }}" | adb shell run-as {{ android_app }} sh -c "'mkdir -p files && cat > files/launch.txt'"

android-log:
    @adb logcat -s pomme:V RustStdoutStderr:V AndroidRuntime:E DEBUG:V

# iOS (pomme-ios/README.md). `team` is your Apple Developer team id.

# Builds, installs and runs it with its log on the connected iPhone or iPad.
ios-device team:
    @python3 tools/ios.py device {{ team }}

# On the booted simulator, which builds and starts but can't render yet.
ios-sim:
    @python3 tools/ios.py sim

# The launch arguments, as on a command line; `--sim` first for the simulator.
ios-config *args:
    @python3 tools/ios.py config {{ args }}

# Copies a desktop install's `minecraft/` assets in; `--sim` for the simulator.
ios-push-assets *args:
    @python3 tools/ios.py push-assets {{ args }}

# A Release archive and its App Store .ipa, for TestFlight.
ios-archive team:
    @python3 tools/ios.py archive {{ team }}

# Uploads the archive to App Store Connect, where TestFlight picks it up.
ios-upload team:
    @python3 tools/ios.py upload {{ team }}

# Everything CI checks for the Rust crates; CI runs the same recipes below,
# so the two can't drift.
client-pre-pr: ci-fmt ci-lint ci-test ci-singleplayer

# The client's optional features clippy covers; `ttf-fonts` needs the system
# FreeType.
lint_features := "test-harness,cpu-bench"

# Formatting, for every crate.
ci-fmt:
    @cargo fmt --all --check

# Lints that run on every OS. SteelMC is left to `ci-singleplayer`, so it isn't
# built three times over.
ci-lint:
    @cargo clippy -p pomme-client --release --all-targets --no-default-features --features {{ lint_features }} -- -D warnings
    @cargo clippy -p pomme-protocol -p pomme-auth --release --all-targets --all-features -- -D warnings
    @cargo clippy -p pomme-dirs -p pomme-install -p azalea-block -p ingame-test -p protogen -p blockgen --release --all-targets -- -D warnings

# Tests, in the dev profile (dependencies are optimised there).
ci-test:
    @cargo nextest run -p pomme-protocol -p pomme-dirs -p pomme-auth -p pomme-install -p ingame-test
    @cargo nextest run -p pomme-client --no-default-features

# SteelMC's crate, the client with it, and the test that boots a real world.
ci-singleplayer:
    @cargo clippy -p pomme-singleplayer --release --all-targets -- -D warnings
    @cargo clippy -p pomme-client --release --all-targets --features {{ lint_features }} -- -D warnings
    @cargo nextest run -p pomme-singleplayer --run-ignored all

# Regenerate a version's packet-id table from the decompiled reference.
protogen version="26.2":
    @cargo run -p protogen -- reference/{{ version }}/decompiled {{ version }} pomme-protocol/src/data/protocol-{{ version }}.json

# Regenerate a version's client-registry id table from the data-generator report.
registrygen version="26.2":
    @cargo run -p protogen -- registries reference/{{ version }} {{ version }} pomme-protocol/src/data/registries-{{ version }}.json

# Regenerate a version's known-pack table from the extracted reference data.
knownpackgen version="26.2":
    @cargo run -p protogen -- knownpacks reference/{{ version }} {{ version }} pomme-protocol/src/data/known-packs-{{ version }}.json

# Regenerate a version's block-state table from the data-generator report.
blockgen version="26.2":
    @cargo run -p blockgen -- blocks reference/{{ version }}/generated/reports/blocks.json {{ version }} pomme-client/src/world/block/data/blocks-{{ version }}.json

# Regenerate the vanilla codec fixtures: packets encoded by the 26.2 server's own
# STREAM_CODECs (tools/codec-oracle/CodecOracle.java), which pomme-protocol's
# decoders are tested against. Needs Java 25 on PATH or JAVA_HOME.
codec-fixtures:
    #!/usr/bin/env bash
    set -euo pipefail
    jar=$(cargo run -q -p ingame-test -- --fetch-server)
    out=target/codec-oracle
    sep=":"; if [ "${OS:-}" = "Windows_NT" ]; then sep=";"; fi
    bin=""; if [ -n "${JAVA_HOME:-}" ]; then bin="$JAVA_HOME/bin/"; fi
    rm -rf "$out" && mkdir -p "$out/lib"
    # This unzip build doesn't glob archive members; list them explicitly.
    unzip -Z1 "$jar" | grep -E '^META-INF/(versions|libraries)/.*\.jar$' \
        | xargs unzip -qn "$jar" -d "$out/lib"
    cp=$(find "$out/lib" -name '*.jar' | tr '\n' "$sep")
    "${bin}javac" --release 25 -nowarn -cp "$cp" -d "$out/classes" tools/codec-oracle/CodecOracle.java
    "${bin}java" -cp "$cp$out/classes" CodecOracle pomme-protocol/src/data/codec-fixtures-26.2.json

# JDK 25 bin dir for stategen; override with `just jdk=<path> stategen`.
jdk := "C:/Program Files/Amazon Corretto/jdk25.0.2_10/bin"

# TODO: Windows only (javac.exe, ';' classpath separator); make portable.

# Regenerate a version's per-state property table by running vanilla's own code
# (tools/stategen/StateDump.java) against the reference server jar, then
# compacting the dump with `blockgen state`. Uses the deobf server jar when
# one exists (pre-26.x); needs the Corretto JDK for 26.x class files. Also
# writes the version's per-block piston table (piston-<version>.json).
stategen version="26.2":
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
    mkdir -p tools/stategen/out
    "$jdk/javac.exe" --release 21 -d tools/stategen/out tools/stategen/StateDump.java
    "$jdk/java.exe" -cp "$classes;${libs}tools/stategen/out" StateDump "$v" "$ref/generated/state.json" pomme-client/src/world/block/data/piston-"$v".json
    cargo run -p blockgen -- state "$ref/generated/state.json" pomme-client/src/world/block/data/blocks-"$v".json pomme-client/src/world/block/data/state-"$v".json
