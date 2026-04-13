<p align="center">
  <h1 align="center">Pomme</h1>
  <p align="center">A high-performance Minecraft client written in Rust</p>
  <p align="center">
    <a href="https://discord.gg/ucBA55bHPR">Discord</a> · <a href="https://github.com/PommeMC/Pomme-Client/issues">Issues</a> · <a href="https://github.com/PommeMC/Pomme-Client/releases">Releases</a>
  </p>
</p>

---

Pomme is a from-scratch Minecraft: Java Edition client built entirely in Rust.
It connects to vanilla servers, renders the world through Vulkan,
and handles physics, networking, and UI without any Mojang code.
The goal is a lightweight, performant alternative to the official Java client.

<p align="center">
  <img width="1920" height="1080" alt="pomme-launcher" src="https://github.com/user-attachments/assets/b8353f51-a23b-45c5-9f3d-457e498a5253" />
</p>

## Features

- **Vulkan rendering**
-chunk meshing, frustum culling, water/lava, sky, block overlays, hand animation

- **Vanilla-exact physics**
-sprinting, swimming, drowning, collision, all matched against decompiled source

- **Full protocol support**
-connects to 26.1 servers via azalea-protocol, handles chunk streaming,
block updates, chat

- **Microsoft authentication**
-sign in with your Microsoft account, tokens stored in the OS keyring

- **HUD & menus**
-health, hunger, air bubbles, hotbar, F3 debug, chat, pause menu,
options, server list

- **Launcher**
-Tauri-based launcher with frosted glass UI, multi-account management,
Mojang patch notes, installation manager

## Architecture

```text
pomme/            Minecraft client (Rust, Vulkan)
launcher/         Launcher app (Tauri, React, TypeScript)
```

The client is a standalone binary that receives launch arguments from
the launcher. The launcher handles authentication, asset downloading,
version management, and spawns the client with the appropriate flags.

## Building

### Client

Requires the [Vulkan SDK](https://vulkan.lunarg.com/) and a Rust toolchain.

```bash
cargo build --release
```

### Launcher

Requires [Node.js](https://nodejs.org/) and [pnpm](https://pnpm.io/).

```bash
cd launcher
pnpm install
pnpm tauri build
```

## Running

**Via the launcher** (recommended):

```bash
cd launcher && pnpm tauri dev
```

**Standalone client**:
Running the standalone client requires assets, for which you have 2 options:

1. Run the launcher and install the latest supported release. Then you can do:

   ```bash
   cargo run -- --username Steve --quick-access-server localhost
   ```

2. If you're on linux, extract the vanilla 26.1.1 assets to `reference/assets/`:

   ```bash
   mkdir -p reference/assets/indexes 
   mkdir -p reference/assets/objects
   mkdir -p reference/versions/26.1.1/extracted
   mkdir -p reference/game-dir
   
    # 30 is the asset index id for 26.1.1
   cp ~/.minecraft/assets/indexes/30.json reference/assets/indexes/26.1.1.json
   cp -r ~/.minecraft/assets/objects/. reference/assets/objects/
   cp ~/.minecraft/versions/26.1.1/26.1.1.jar reference/versions/26.1.1/
   unzip reference/versions/26.1.1/26.1.1.jar 'assets/*' -d reference/versions/26.1.1/extracted/
   
   cargo run -- --username Steve --quick-access-server localhost \
     --version 26.1.1 \
     --assets-dir $PWD/reference/assets \
     --versions-dir $PWD/reference/versions \
     --game-dir $PWD/reference/game-dir
   ```

## Contributing

Contributions are welcome.
Please open an issue first to discuss what you'd like to change.

## License

GPL-3.0-or-later. This project is not affiliated with Mojang or Microsoft.

## Community

[![Discord](https://img.shields.io/discord/1483578136544280618?color=5865F2&label=Discord&logo=discord&logoColor=white)](https://discord.gg/ucBA55bHPR)
[![Sponsor](https://img.shields.io/badge/Sponsor-Purdze-ea4aaa?logo=githubsponsors&logoColor=white)](https://github.com/sponsors/Purdze)

<a href="https://www.star-history.com/?repos=PommeMC%2FPomme-Client&type=date&logscale&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=PommeMC/Pomme-Client&type=date&theme=dark&logscale&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=PommeMC/Pomme-Client&type=date&logscale&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=PommeMC/Pomme-Client&type=date&logscale&legend=top-left" />
 </picture>
</a>
