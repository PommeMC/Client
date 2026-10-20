# Contributing to Pomme

Thanks for your interest in contributing to Pomme!

## Getting Started

1. Fork the repository
2. Clone your fork and set up the development environment:

   ```bash
   git clone --recurse-submodules https://github.com/<your-username>/Client.git
   cd Client
   ```

   SteelMC is a submodule; see [BUILDING.md](./BUILDING.md) for the
   submodule, first-build, and skip notes.

3. Build and run:

   ```bash
   just client-dev -- --username Steve
   ```

## Before Submitting a PR

All of these must pass. CI will reject your PR if they don't.

```bash
just client-pre-pr          # Every Rust crate and tool, as CI runs them
just launcher-pre-pr        # Launcher (Rust & TypeScript)
```

`client-pre-pr` runs the same `ci-*` recipes CI does, including the
singleplayer crate's ignored test, which boots a real SteelMC server and writes
a world to the temp dir. Tests run through
[cargo-nextest](https://nexte.st) (`cargo install cargo-nextest`). Android and
iOS have their own CI jobs.

For changes you'd otherwise check in game (items, inventory, entities,
advancements), `just ingame-test` runs scripted tests against a local vanilla
server. See [tests/ingame/README.md](tests/ingame/README.md).

## Development Guidelines

- **The toolchain is pinned** in `rust-toolchain.toml` (a nightly: rustfmt's
  options, SteelMC's pin, and simdnbt's and azalea's unstable feature gates).
  Never override it locally; when it has to move, bump SteelMC first
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

The workspace crates are listed under [Layout](./BUILDING.md#layout)
in BUILDING.md. Inside the two apps:

### Pomme client

```bash
pomme-client/
└── src/
    ├── lib.rs              # Startup, shared by every platform's entry point
    ├── main.rs             # Desktop entry point
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
    ├── update.rs           # Release builds updating themselves
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
  Tick `dry_run` to build and package without releasing: the files stay on the
  run.

Each release has, per OS, a zip (the files the updater swaps in, also usable
as a portable install) and an installer built from it by `packaging/`: a dmg
on macOS, a per-user setup on Windows and an AppImage on Linux, plus
`sha256sums.txt`. The tag is baked into the build, which is how it updates
itself and shows its version, and each release becomes the repo's "Latest".

Launcher releases are cut by pushing a `launcher-v*` tag (e.g. `launcher-v0.1.2`)
or by manual dispatch.

A plain `v*` tag does nothing.

## Reporting Issues

Include reproduction steps and your system info (OS, GPU, Pomme version)
for bug reports.

## Code of Conduct

Be respectful. We're all here to build something cool.
