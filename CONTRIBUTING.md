# Contributing to Pomme

Thanks for your interest in contributing to Pomme!

## Getting Started

1. Fork the repository
2. Clone your fork and set up the development environment:

   ```bash
   git clone --recurse-submodules https://github.com/<your-username>/Client.git
   cd Client
   ```

   SteelMC is a submodule and the client builds it by default for singleplayer.
   On an existing clone, `git submodule update --init` fetches it. Its first
   build downloads the Minecraft server jar and generates registry source, so
   expect that one to take a while. `cargo build -p pomme-client
   --no-default-features` skips it entirely.

3. Build and run:

   ```bash
   just client-build
   just launcher-dev
   ```

## Before Submitting a PR

All of these must pass. CI will reject your PR if they don't.

```bash
just client-pre-pr          # Client, protocol, and singleplayer (Rust)
just launcher-pre-pr        # Launcher (Rust & TypeScript)
```

`client-pre-pr` includes the singleplayer crate's ignored test, which boots a
real SteelMC server and writes a world to the temp dir, so it takes a minute.

## Development Guidelines

- **The toolchain is pinned** in `rust-toolchain.toml` (a nightly, for rustfmt's
  options and to match SteelMC's pin). Never override it locally; when it has to
  move, bump SteelMC first
- No unnecessary comments. Code should be self-explanatory
- No DRY violations. Don't duplicate logic, extract shared helpers
- No `unwrap()` outside of tests
- Keep changes focused. One feature or fix per PR
- Use `feat/`, `fix/`, `perf/`, `refactor/`, `chore/`, `docs/` branch prefixes

## Pull Request Format

Opening a PR prefills a template with the sections we expect. Fill them in rather
than deleting them.

Ticking a box under **Type of change** applies the matching label automatically,
and unticking it removes the label again.

Under **Reference**, name the classes in `reference/26.2/decompiled` you checked
the change against. This is what reviewers compare against, so it saves a round
trip. Put `n/a` for launcher, tooling and chore PRs.

For bug fixes, also include:

```markdown
- What the issue was
- What caused it
- How it was fixed
```

## Project Structure

The workspace crates are listed under [Architecture](./README.md#architecture)
in the README. Inside the two apps:

### Pomme client

```bash
pomme-client/
└── src/
    ├── main.rs             # Entry point
    ├── app/                # Winit event loop, input handling, state machine
    ├── args.rs             # CLI arguments
    ├── audio/              # OpenAL playback, sound events, music, subtitles
    ├── entity/             # Entities, mobs, villagers, item drops
    ├── net/                # Connection, framing, packet handling, per-version translation
    ├── physics/            # Movement, collision
    ├── player/             # Local player, inventory, interaction
    ├── renderer/           # Vulkan rendering, chunk meshing, texture atlas
    │   ├── pipelines/      # GPU pipelines (chunk, sky, hand, overlay, etc.)
    │   ├── shaders/        # GLSL shaders
    │   └── chunk/          # Chunk buffer management, meshing, atlas
    ├── ui/                 # HUD, chat, menus, pause screen
    ├── world/              # Chunk storage, block registry, models, lighting
    ├── singleplayer.rs     # The integrated server as the client sees it
    ├── resource_pack.rs    # Local and server-sent resource packs
    ├── particle.rs         # Particle simulation
    ├── mob_effect.rs       # Status effects
    ├── lang.rs             # Vanilla translation strings
    └── discord.rs          # Discord rich presence
```

### Pomme launcher

```bash
pomme-launcher/
├── src/                    # React frontend (TypeScript)
├── src-tauri/              # Tauri backend (Rust)
└── package.json            # Node dependencies
```

## Releases

Client releases are built by `.github/workflows/release-client.yml`:

- Pushing a `client-v*` tag (e.g. `client-v0.2.2+26.2`; the suffix is the
  Minecraft version the build targets).
- A weekly run from `master` every Sunday, skipped when nothing landed since the
  last tag or when that commit's CI is not green.
- A manual dispatch, optionally with a version core; blank bumps the patch.

Launcher releases are cut by pushing a `launcher-v*` tag (e.g. `launcher-v0.1.2`).

A plain `v*` tag does nothing.

## Reporting Issues

Include reproduction steps and your system info (OS, GPU, Pomme version)
for bug reports.

## Code of Conduct

Be respectful. We're all here to build something cool.
