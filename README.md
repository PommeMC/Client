<p align="center">
  <h1 align="center">Pomme</h1>
  <p align="center">A high-performance Minecraft: Java Edition client, written from scratch in Rust and Vulkan</p>
  <p align="center">
    <a href="https://github.com/PommeMC/Client/actions/workflows/ci.yml"><img src="https://github.com/PommeMC/Client/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
    <img src="https://img.shields.io/badge/Minecraft-1.20%20to%2026.3-5D9C3F" alt="Minecraft 1.20 to 26.3">
    <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-2A2470" alt="Windows, macOS and Linux">
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-blue" alt="GPL-3.0 license"></a>
  </p>
  <p align="center">
    <a href="#download"><b>Download</b></a> · <a href="https://discord.gg/ucBA55bHPR"><b>Discord</b></a> · <a href="./BUILDING.md"><b>Build from source</b></a> · <a href="./CONTRIBUTING.md"><b>Contributing</b></a>
  </p>
</p>

---

Pomme contains no Mojang code, and joins vanilla servers from 1.20 through 26.3.

https://github.com/user-attachments/assets/0bc2379f-322b-4566-a9d0-935d72cedf0d

<p align="center">
  <img width="1920" height="1080" alt="The Pomme launcher" src="https://github.com/user-attachments/assets/b8353f51-a23b-45c5-9f3d-457e498a5253" />
</p>

## Download

Get the **Pomme launcher** from the
[latest launcher release](https://github.com/PommeMC/Client/releases?q=launcher&expanded=true),
for Windows x64, macOS (Apple Silicon) or Linux x64 (`.AppImage`, `.deb` or
`.rpm`). It signs you in with your Microsoft account, installs and updates the
client, and launches it. You need to own Minecraft: Java Edition.

## Why Pomme?

- **No JVM**: a native binary, so there's no Java to install or tune.
- **Vanilla behaviour**: physics, rendering, menus and networking are ported line
  by line from the decompiled game, and checked against it.
- **Tested against real servers**: scripted [in-game tests](./tests/ingame/README.md)
  run the client against a vanilla 26.2 server and screenshot the results.
- **Many versions, one client**: per-version protocol tables are generated from
  each version's decompiled source, and older versions are translated to 26.2.

> [!WARNING]
> Pomme is in active development. Expect missing features and bugs, and please
> report them in [Issues](https://github.com/PommeMC/Client/issues) or on Discord.

## Features

- **Rendering**: chunk meshing, GPU frustum and cave-occlusion culling, smooth lighting, water and lava, mobs with skins, particles, weather, clouds and sky
- **Singleplayer**: worlds run on [SteelMC](https://github.com/Steel-Foundation/SteelMC), a Rust server embedded in the client
- **Audio**: vanilla sound events, music and subtitles through OpenAL Soft
- **Resource packs**: local and server-sent packs, applied live
- **Launcher**: multiple accounts, patch notes and installation management

<details>
<summary>Everything else</summary>

- Vanilla-exact movement, collision, swimming and drowning
- Block entities, dropped items, block overlays and hand animation
- HUD and menus: health, hunger, air, status effects, hotbar, boss bars, titles, waypoints, F3 debug, chat, pause menu, options, server list, world list
- Create, edit and delete singleplayer worlds from the menu
- Microsoft account tokens stored in the OS keyring
- Discord presence for the menu, servers and worlds

</details>

## Building from source

See [BUILDING.md](./BUILDING.md) for the client and launcher builds, running
the client standalone, and the development commands.

## Contributing

Contributions are welcome.
Please open an issue first to discuss what you'd like to change, and see
[CONTRIBUTING.md](./CONTRIBUTING.md) for the workflow.

## License

Pomme is licensed under the [GNU General Public License v3.0 or later](./LICENSE)
(GPL-3.0-or-later). Copyright (C) 2026 Purdze.

`pomme-singleplayer` links [SteelMC](./third_party/SteelMC) and is therefore
AGPL-3.0-or-later. The [allocator crate](./pomme-gpu-allocator) is MIT, a port
of the [gpu-allocator crate](https://github.com/Traverse-Research/gpu-allocator)
by Traverse Research, and [pomme-block](./pomme-block) is likewise MIT. See
[THIRD_PARTY_LICENSES.md](./THIRD_PARTY_LICENSES.md) for the other third-party code.

Pomme is not affiliated with or endorsed by Mojang Studios or Microsoft.
Minecraft is a trademark of Mojang Studios.

## Community

[![Discord](https://img.shields.io/discord/1483578136544280618?color=5865F2&label=Discord&logo=discord&logoColor=white)](https://discord.gg/ucBA55bHPR)
[![Sponsor](https://img.shields.io/badge/Sponsor-Purdze-ea4aaa?logo=githubsponsors&logoColor=white)](https://github.com/sponsors/Purdze)

<details>
<summary>Star history</summary>

<a href="https://www.star-history.com/?repos=PommeMC%2FClient&type=date&logscale=&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=PommeMC/Client&type=date&theme=dark&logscale&legend=top-left&sealed_token=r7ib_n7xrIE4aUaRqlz9KCFjr5-Cpocxw6LPtk06-CcFfxLAEjzgPfz5bSzzGcSnbhRnkbN6KwRem4jwXnRJzcsuZYIZX5tyIAmYKFIc60c_2HBXRhGHA4rlvmyr2MXfeZDe2thqz-maf8Tdh5FxHAWJUFflGQP1JjoIfbyUZ0GW6pMxxJ8qfga5eta0" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=PommeMC/Client&type=date&logscale&legend=top-left&sealed_token=r7ib_n7xrIE4aUaRqlz9KCFjr5-Cpocxw6LPtk06-CcFfxLAEjzgPfz5bSzzGcSnbhRnkbN6KwRem4jwXnRJzcsuZYIZX5tyIAmYKFIc60c_2HBXRhGHA4rlvmyr2MXfeZDe2thqz-maf8Tdh5FxHAWJUFflGQP1JjoIfbyUZ0GW6pMxxJ8qfga5eta0" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=PommeMC/Client&type=date&logscale&legend=top-left&sealed_token=r7ib_n7xrIE4aUaRqlz9KCFjr5-Cpocxw6LPtk06-CcFfxLAEjzgPfz5bSzzGcSnbhRnkbN6KwRem4jwXnRJzcsuZYIZX5tyIAmYKFIc60c_2HBXRhGHA4rlvmyr2MXfeZDe2thqz-maf8Tdh5FxHAWJUFflGQP1JjoIfbyUZ0GW6pMxxJ8qfga5eta0" />
 </picture>
</a>

</details>
