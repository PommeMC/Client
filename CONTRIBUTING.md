# Contributing to Pomme

Thanks for your interest in contributing to Pomme!

## Getting Started

1. Fork the repository
2. Clone your fork and set up the development environment:
   ```bash
   git clone https://github.com/<your-username>/Pomme-Client.git
   cd Pomme-Client
   rustup override set nightly
   ```
3. Extract vanilla 1.21.11 assets into `reference/assets/`:
   ```bash
   unzip ~/.minecraft/versions/1.21.11/1.21.11.jar -d reference/assets/
   ```
4. Build and run:
   ```bash
   cargo build
   cargo run -- --server localhost:25565 --username Steve
   ```

## Before Submitting a PR

All of these must pass. CI will reject your PR if they don't.

### Client (Rust)

```bash
cargo fmt -- --check
cargo clippy --release --all-targets --all-features -- -D warnings
cargo build --release
```

### Launcher Backend (Rust)

```bash
cd launcher/src-tauri
cargo fmt -- --check
cargo clippy --release --all-targets --all-features -- -D warnings
```

### Launcher Frontend (TypeScript)

```bash
cd launcher
pnpm install
pnpm format:check
pnpm lint
pnpm exec tsc --noEmit
pnpm exec vite build
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
- What the issue was
- What caused it
- How it was fixed

## Project Structure

```
src/
├── main.rs          # Entry point
├── args.rs          # CLI arguments
├── entity/          # Entity storage (item drops)
├── window/          # winit event loop, input handling
├── renderer/        # Vulkan rendering, chunk meshing, texture atlas
│   ├── pipelines/   # GPU pipelines (chunk, sky, hand, overlay, etc.)
│   ├── shaders/     # GLSL shaders
│   └── chunk/       # Chunk buffer management, meshing, atlas
├── net/             # Server connection, packet handling
├── world/           # Chunk storage, block registry, models
├── physics/         # Movement, collision
├── player/          # Local player, inventory, interaction
└── ui/              # HUD, chat, menus, pause screen

launcher/
├── src/             # React frontend (TypeScript)
├── src-tauri/       # Tauri backend (Rust)
└── package.json     # Node dependencies
```

## Reporting Issues

Include reproduction steps and your system info (OS, GPU, Rust version) for bug reports.

## Code of Conduct

Be respectful. We're all here to build something cool.
