<p align="center">
  <h1 align="center">Pomme</h1>
  <p align="center">A high-performance Minecraft client written in Rust</p>
  <p align="center">
    <a href="https://discord.gg/ucBA55bHPR">Discord</a> · <a href="https://github.com/PommeMC/Pomme-Client/issues">Issues</a> · <a href="https://github.com/PommeMC/Pomme-Client/releases">Releases</a>
  </p>
</p>

---

Pomme is a from-scratch Minecraft: Java Edition client built entirely in Rust. It connects to vanilla servers, renders the world through Vulkan, and handles physics, networking, and UI without any Mojang code. The goal is a lightweight, performant alternative to the official Java client.

<img width="957" height="535" alt="launcher" src="https://github.com/user-attachments/assets/5d7240d3-80fb-49fb-9eb4-7e497f61a603" />

## Features

- **Vulkan rendering** -chunk meshing, frustum culling, water/lava, sky, block overlays, hand animation
- **Vanilla-exact physics** -sprinting, swimming, drowning, collision, all matched against decompiled source
- **Full protocol support** -connects to 26.1 servers via azalea-protocol, handles chunk streaming, block updates, chat
- **Microsoft authentication** -sign in with your Microsoft account, tokens stored in the OS keyring
- **HUD & menus** -health, hunger, air bubbles, hotbar, F3 debug, chat, pause menu, options, server list
- **Launcher** -Tauri-based launcher with frosted glass UI, multi-account management, Mojang patch notes, installation manager

## Architecture

```
pomme/             Minecraft client (Rust, Vulkan)
launcher/         Launcher app (Tauri, React, TypeScript)
```

The client is a standalone binary that receives launch arguments from the launcher. The launcher handles authentication, asset downloading, version management, and spawns the client with the appropriate flags.

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
```bash
cargo run --release -- --dev --server localhost:25565 --username Steve
```

## Contributing

Contributions are welcome. Please open an issue first to discuss what you'd like to change.

## License

GPL-3.0-or-later. This project is not affiliated with Mojang or Microsoft.

## Community

[![Discord](https://img.shields.io/discord/1351635498498867313?color=5865F2&label=Discord&logo=discord&logoColor=white)](https://discord.gg/ucBA55bHPR)
[![Sponsor](https://img.shields.io/badge/Sponsor-Purdze-ea4aaa?logo=githubsponsors&logoColor=white)](https://github.com/sponsors/Purdze)
