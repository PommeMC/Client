<p align="center">
  <h1 align="center">Pomme</h1>
  <p align="center">A high-performance Minecraft client written in Rust</p>
  <p align="center">
    <a href="https://discord.gg/ucBA55bHPR">Discord</a> · <a href="https://github.com/PommeMC/Client/issues">Issues</a> · <a href="https://github.com/PommeMC/Client/releases">Releases</a>
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

- **Vulkan rendering**: chunk meshing, GPU frustum and cave-occlusion culling, smooth lighting, water/lava, entities and mobs, block entities, weather, clouds, sky, block overlays, hand animation
- **Vanilla-exact physics**: sprinting, swimming, drowning, collision, all matched against decompiled source
- **Full protocol support**: connects to 26.2 servers via azalea-protocol, handles chunk streaming, block updates, chat
- **Microsoft authentication**: sign in with your Microsoft account, tokens stored in the OS keyring
- **HUD & menus**: health, hunger, air bubbles, hotbar, F3 debug, chat, pause menu, options, server list
- **Launcher**: Tauri-based launcher with frosted glass UI, multi-account management, Mojang patch notes, installation manager

## Architecture

```bash
pomme-client/    # Minecraft client (Rust, Vulkan)
pomme-launcher/  # Launcher app (Tauri, React, TypeScript)
```

The client is a standalone binary that receives launch arguments from
the launcher. The launcher handles authentication, asset downloading,
version management, and spawns the client with the appropriate flags.

## Building

Before building, you must have [just](https://github.com/casey/just) installed.

### Client

Requires the [Vulkan SDK](https://vulkan.lunarg.com/) and a Rust toolchain.

```bash
just client-build --release
```

### Launcher

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

2. If you're on linux, extract the vanilla 26.2 assets from `.minecraft/` to `reference/`:

   ```bash
   mkdir -p reference/assets/indexes
   mkdir -p reference/assets/objects
   mkdir -p reference/versions/26.2/extracted
   mkdir -p reference/game-dir

   # 32 is the asset index id for 26.2
   cp ~/.minecraft/assets/indexes/32.json reference/assets/indexes/26.2.json
   cp -r ~/.minecraft/assets/objects/. reference/assets/objects/
   cp ~/.minecraft/versions/26.2/26.2.jar reference/versions/26.2/
   unzip reference/versions/26.2/26.2.jar 'assets/*' -d reference/versions/26.2/extracted/
   ```

   Then you can run the client with:

   ```bash
   just client-dev -- --version 26.2 \
     --assets-dir $PWD/reference/assets \
     --versions-dir $PWD/reference/versions \
     --game-dir $PWD/reference/game-dir
   ```

## Contributing

Contributions are welcome.
Please open an issue first to discuss what you'd like to change.

## License

This project is licensed under the GNU General Public License v3.0 or later (GPL-3.0-or-later).

It is not affiliated with or endorsed by Mojang Studios or Microsoft.
Minecraft is a trademark of Mojang Studios.

The [allocator crate](./pomme-gpu-allocator) is licensed under the MIT License and is a port of the
[gpu-allocator crate](https://github.com/Traverse-Research/gpu-allocator) by Traverse Research.

## Third-Party Licenses

Portions of this project include third-party code under separate licenses.
See [THIRD_PARTY_LICENSES.md](./THIRD_PARTY_LICENSES.md) for details.

## Community

[![Discord](https://img.shields.io/discord/1483578136544280618?color=5865F2&label=Discord&logo=discord&logoColor=white)](https://discord.gg/ucBA55bHPR)
[![Sponsor](https://img.shields.io/badge/Sponsor-Purdze-ea4aaa?logo=githubsponsors&logoColor=white)](https://github.com/sponsors/Purdze)

<a href="https://www.star-history.com/?repos=PommeMC%2FClient&type=date&logscale&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=PommeMC/Client&type=date&theme=dark&logscale&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=PommeMC/Client&type=date&logscale&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=PommeMC/Client&type=date&logscale&legend=top-left" />
 </picture>
</a>
