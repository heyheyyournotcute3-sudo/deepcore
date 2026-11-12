# Deepcore

A co-op, first-person, 3D voxel mining game written entirely in Rust with
[Bevy](https://bevy.org).

You and up to 15 friends spawn at a surface camp above a 1024 × 1024 × 512 block mine.
Everyone starts with a steel pickaxe. Dig down, haul ore home, build machines, unlock better
tools, go deeper — through six Eras of technology, ending in a full nuclear fuel chain, a
Nuclear Drill that bores 3×3 tunnels, and a reactor that powers the base.

> **Status:** in active development. See [`PROGRESS.md`](PROGRESS.md) for exactly what works
> today and [`docs/PLAN.md`](docs/PLAN.md) for the architecture and milestone plan.

## Quick start

```bash
cargo run -p deepcore_server   # headless authoritative server, 30 Hz
cargo run -p deepcore_client   # windowed client
```

## Layout

| Path | What lives there |
|---|---|
| `crates/shared` | every rule of the game: voxels, worldgen, movement, inventory, crafting, power, hazards, protocol |
| `crates/server` | headless authoritative server: world, simulation, networking |
| `crates/client` | windowed client: rendering, input, prediction, UI, HUD, audio |
| `crates/lobby` | axum service for accounts and the server browser (M10) |
| `assets/data` | all content as RON: blocks, items, recipes, machines, tech tree, layers, hazards |
| `tools/atlas_gen` | generates the block texture atlas from `blocks.ron` — no downloaded art |

## Development

```bash
./scripts/check.sh   # fmt + clippy -D warnings + tests
```

All crate versions are pinned exactly in the workspace `Cargo.toml` because the Bevy
ecosystem changes API between releases. Bump them deliberately and re-verify against the
crate sources.
