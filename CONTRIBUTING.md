# Contributing to Pomme

Thanks for your interest in contributing to Pomme!

## Getting Started

1. Fork the repository
2. Clone your fork and set up the development environment:

   ```bash
   git clone https://github.com/<your-username>/Pomme.git
   cd Pomme
   ```

3. Build and run:

   ```bash
   pnpm client:build-debug
   pnpm launcher:dev
   ```

## Before Submitting a PR

All of these must pass. CI will reject your PR if they don't.

```bash
pnpm client:pre-pr          # Client (Rust)
pnpm launcher:pre-pr        # Launcher (Rust & TypeScript)
```

## Development Guidelines

- **Rust nightly** is required (due to `simdnbt` dependency)
- No unnecessary comments. Code should be self-explanatory
- No DRY violations. Don't duplicate logic, extract shared helpers
- No `unwrap()` outside of tests
- Keep changes focused. One feature or fix per PR
- Use `feat/`, `fix/`, `perf/`, `refactor/`, `chore/` branch prefixes

## Pull Request Format

Every PR must include:

```markdown
## Summary
- Brief bullet points of what changed and why

## Test plan
- [ ] Steps to verify the changes work
- [ ] Edge cases checked
```

For bug fixes, also include:

```markdown
- What the issue was
- What caused it
- How it was fixed
```

## Project Structure

```bash
Pomme/
├── pomme-client            # Minecraft client (Vulkan, Rust)
└── pomme-launcher          # Launcher app (Tauri, React, TypeScript)
```

### Pomme client

```bash
pomme-client/
└── src/
    ├── main.rs             # Entry point
    ├── args.rs             # CLI arguments
    ├── entity/             # Entity storage (item drops)
    ├── window/             # winit event loop, input handling
    ├── renderer/           # Vulkan rendering, chunk meshing, texture atlas
    │   ├── pipelines/      # GPU pipelines (chunk, sky, hand, overlay, etc.)
    │   ├── shaders/        # GLSL shaders
    │   └── chunk/          # Chunk buffer management, meshing, atlas
    ├── net/                # Server connection, packet handling
    ├── world/              # Chunk storage, block registry, models
    ├── physics/            # Movement, collision
    ├── player/             # Local player, inventory, interaction
    └── ui/                 # HUD, chat, menus, pause screen
```

### Pomme launcher

```bash
pomme-launcher/
├── src/                    # React frontend (TypeScript)
├── src-tauri/              # Tauri backend (Rust)
└── package.json            # Node dependencies
```

## Releases

Releases are cut by pushing tags:

- `client-v*` (e.g. `client-v0.1.1`) builds and publishes the client.
- `launcher-v*` (e.g. `launcher-v0.1.1`) builds and publishes the launcher.

A plain `v*` tag does nothing.

## Reporting Issues

Include reproduction steps and your system info (OS, GPU, Rust version)
for bug reports.

## Code of Conduct

Be respectful. We're all here to build something cool.
