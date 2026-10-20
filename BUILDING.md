# Building Pomme

## Layout

```bash
pomme-client/         # Minecraft client (Rust, Vulkan)
pomme-android/        # Android entry point and APK project (see its README)
pomme-ios/            # iOS entry point and Xcode project (see its README)
pomme-auth/           # Microsoft sign-in, shared by the client and launcher
pomme-install/        # Downloads a version's assets and Pomme's own releases, shared by the client and launcher
pomme-launcher/       # Launcher app (Tauri, React, TypeScript)
pomme-protocol/       # Per-version protocol data and wire encoding
pomme-dirs/           # The data folder layout the client and launcher share
pomme-block/          # Data-free stand-in for azalea-block; block tables are Pomme's own
pomme-singleplayer/   # Runs SteelMC in-process as the integrated server
pomme-gpu-allocator/  # Vendored fork of the gpu-allocator crate
third_party/SteelMC/  # SteelMC submodule
packaging/            # Release installers: the macOS dmg, Windows setup and Linux AppImage
tools/                # protogen, blockgen, stategen, and the OpenAL fetch script
```

The client is a standalone app: it signs in with a Microsoft account,
downloads a version's assets on first start, and release builds keep
themselves up to date. The launcher is being retired; it can still start the
client with launch arguments.

Before building, you must have [just](https://github.com/casey/just) installed.
The Rust toolchain is pinned in `rust-toolchain.toml`; rustup picks it up.

## Building the client

Requires the [Vulkan SDK](https://vulkan.lunarg.com/) and Python (to stage
OpenAL Soft next to the binary; without it the client runs with audio disabled).

On Linux, source the Vulkan SDK's `setup-env.sh` into the current Bash shell
before running client builds or `just client-pre-pr`.
For builds run from an editor or agent, launch it from the configured environment
so its build processes inherit the SDK variables.

Clone with submodules, since singleplayer builds SteelMC by default:

```bash
git clone --recurse-submodules https://github.com/PommeMC/Client.git
# or, on an existing clone
git submodule update --init
```

SteelMC's first build downloads the Minecraft server jar and generates registry
source, so expect it to take a while. To skip building it (the submodule must
still be checked out):

```bash
just client-build --no-default-features
```

Otherwise:

```bash
just client-build --release
```

## Building the launcher

Requires [Node.js](https://nodejs.org/) and [pnpm](https://pnpm.io/).

```bash
pnpm install
just launcher-build --release
```

## Running

### Client

The client needs Minecraft's assets, for which you have 2 options:

1. Let it download them on first start, into the data folder:

   ```bash
   just client-dev -- --username Steve
   ```

2. If you're on linux, extract the vanilla 26.3 assets from `.minecraft/` to `reference/`:

   ```bash
   mkdir -p reference/assets/indexes
   mkdir -p reference/assets/objects
   mkdir -p reference/versions/26.3/extracted
   mkdir -p reference/game-dir

   # 34 is the asset index id for 26.3
   cp ~/.minecraft/assets/indexes/34.json reference/assets/indexes/26.3.json
   cp -r ~/.minecraft/assets/objects/. reference/assets/objects/
   cp ~/.minecraft/versions/26.3/26.3.jar reference/versions/26.3/
   unzip reference/versions/26.3/26.3.jar 'assets/*' -d reference/versions/26.3/extracted/
   echo 26.3 > reference/versions/26.3/extracted/.extracted
   ```

   Then you can run the client with:

   ```bash
   just client-dev -- --version 26.3 \
     --assets-dir $PWD/reference/assets \
     --versions-dir $PWD/reference/versions \
     --game-dir $PWD/reference/game-dir
   ```

`--width` and `--height` set the window's starting size in logical pixels
(default 854 × 480), for example to test the UI at a fixed size:

```bash
just client-dev -- --width 1280 --height 720
```

They only apply on desktop, and don't change saved settings or fullscreen.

### Launcher

```bash
pnpm install
just launcher-dev
```

### Data folder

The client and launcher share one per-user folder: `%LOCALAPPDATA%\Pomme` on
Windows, `~/Library/Application Support/Pomme` on macOS and
`~/.local/share/pomme` on Linux. An older `.pomme` folder is moved into it on
first launch.

```
minecraft/assets/     asset index and objects
minecraft/versions/   each version's json, jar and extracted assets
pomme-assets/         Pomme's branded assets
profiles/<name>/      options, servers, saves, screenshots, logs, mods, packs
cache/<version>/      rebuildable; safe to delete
launcher/             launcher-only settings, installations and client builds
```

### Android

An early arm64 build with touch controls; setup and limitations are in
[`pomme-android/README.md`](pomme-android/README.md).

### iOS

An early iPhone and iPad build with the same touch controls, plus TestFlight
archives; setup and limitations are in [`pomme-ios/README.md`](pomme-ios/README.md).

## Development commands

Run `just` with no arguments to list every recipe. The common ones:

- `just client-dev` / `just client-build` / `just client-release`: run, build, or benchmark the client; flags forward after `--`, e.g. `just client-dev -- --username Steve`
- `just launcher-dev` / `just launcher-build`: run or bundle the launcher
- `just client-pre-pr` / `just launcher-pre-pr`: the fmt, clippy, and test checks CI enforces
- `just protogen` / `just registrygen` / `just knownpackgen` / `just blockgen` / `just stategen`: regenerate a version's packet-id, registry, known-pack, block-state, and per-state property tables from `reference/<version>/`; `stategen` runs vanilla's own code and needs JDK 25 (`just jdk=<bin dir> stategen`, Windows only)
