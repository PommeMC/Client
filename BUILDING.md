# Building Pomme

## Layout

```bash
pomme-client/         # Minecraft client (Rust, Vulkan)
pomme-launcher/       # Launcher app (Tauri, React, TypeScript)
pomme-protocol/       # Per-version protocol data and wire encoding
pomme-block/          # Data-free stand-in for azalea-block; block tables are Pomme's own
pomme-singleplayer/   # Runs SteelMC in-process as the integrated server
pomme-gpu-allocator/  # Vendored fork of the gpu-allocator crate
third_party/SteelMC/  # SteelMC submodule
tools/                # protogen, blockgen, stategen, and the OpenAL fetch script
```

The client is a standalone binary that receives launch arguments from
the launcher. The launcher handles authentication, asset downloading,
version management, and spawns the client with the appropriate flags.

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

### Via the launcher (recommended)

```bash
pnpm install
just launcher-dev
```

### Standalone client

Running the standalone client requires minecraft assets, for which you have 2 options:

1. Run the launcher and install the latest supported release. Then you can do:

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
   ```

   Then you can run the client with:

   ```bash
   just client-dev -- --version 26.3 \
     --assets-dir $PWD/reference/assets \
     --versions-dir $PWD/reference/versions \
     --game-dir $PWD/reference/game-dir
   ```

## Development commands

Run `just` with no arguments to list every recipe. The common ones:

- `just client-dev` / `just client-build` / `just client-release`: run, build, or benchmark the client; flags forward after `--`, e.g. `just client-dev -- --username Steve`
- `just launcher-dev` / `just launcher-build`: run or bundle the launcher
- `just client-pre-pr` / `just launcher-pre-pr`: the fmt, clippy, and test checks CI enforces
- `just protogen` / `just registrygen` / `just knownpackgen` / `just blockgen` / `just stategen`: regenerate a version's packet-id, registry, known-pack, block-state, and per-state property tables from `reference/<version>/`; `stategen` runs vanilla's own code and needs JDK 25 (`just jdk=<bin dir> stategen`, Windows only)
