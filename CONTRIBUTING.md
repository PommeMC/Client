# Contributing to POMC

Thanks for your interest in contributing to POMC!

## Getting Started

1. Fork the repository
2. Clone your fork and set up the development environment:
   ```bash
   git clone https://github.com/<your-username>/POMC.git
   cd POMC
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

## Development Guidelines

- **Rust nightly** is required (due to `simdnbt` dependency)
- Run `cargo clippy` before submitting a PR
- Run `cargo fmt` to format your code
- No `unwrap()` outside of tests — use `thiserror` for error types
- Comments explain **why**, not **what**
- Keep changes focused — one feature or fix per PR

## Project Structure

```
src/
├── main.rs          # Entry point
├── args.rs          # CLI arguments
├── window/          # winit event loop, input handling
├── renderer/        # Vulkan rendering, chunk meshing, texture atlas
├── net/             # Server connection, packet handling
├── world/           # Chunk storage, block registry
├── physics/         # Movement, collision
├── player/          # Local player, inventory, interaction
└── ui/              # HUD, chat, menus
```

## Pull Requests

- Create a feature branch from `master`
- Write a clear PR title and description
- Keep PRs small and reviewable
- Ensure the project builds with no warnings

## Reporting Issues

Use the issue templates provided. Include reproduction steps and your system info (OS, GPU, Rust version) for bug reports.

## Code of Conduct

Be respectful. We're all here to build something cool.
