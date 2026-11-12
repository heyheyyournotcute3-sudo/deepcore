# Deepcore — Plan

> Status: living document. `PROGRESS.md` records what is actually done; this file
> records what we are building and why.

## 1. What the game is

**Deepcore** is a co-op, first-person, 3D voxel mining game written entirely in Rust.

A group of up to 16 players spawns at a surface camp above a 1024 × 1024 × 512 block
procedurally generated mine. Everyone starts with a steel pickaxe and an empty backpack.
The loop is:

```
dig deeper  ->  haul ore home  ->  build a machine  ->  it unlocks a better tool
     ^                                                          |
     +----------------------------------------------------------+
```

The end of the ladder is **nuclear power**: uranium is found in the Deep Crust, and the
full refining chain (crush → leach → precipitate → convert → enrich → fabricate) ends in a
**Nuclear Drill** that mines a 3×3 face at 12× speed, and a reactor that powers the Base.
Every step of that chain is a structure the players physically place, power and feed.

## 2. Architectural rules

1. `shared` holds every rule of the game, `server` decides, `client` shows.
2. The server is authoritative for everything except the local player's predicted movement.
3. All content (blocks, items, recipes, machines, tech, layers, hazards) lives in RON under
   `assets/data/`. Adding an ore must never require touching Rust.
4. One plugin per feature, files under ~400 lines, named by purpose.
5. Nothing is deferred silently: anything not built is listed in `PROGRESS.md`.
6. No downloaded art. Textures are generated from data at runtime by `atlas_gen`.

## 3. Crate layout

```
deepcore/
├── Cargo.toml            workspace, exact pins, profiles
├── assets/data/*.ron     the single source of truth for content
├── crates/shared/        pure rules + protocol (no rendering, no sockets)
│   ├── content/          RON model, registries, validator, content hash
│   ├── voxel/            ChunkPos/BlockPos maths, palette-compressed Chunk
│   ├── worldgen/         layers, caves, ore veins, deterministic per seed
│   ├── movement/         AABB-vs-voxel collision, player controller (both sides)
│   ├── inventory/        stacks, slots, transfers
│   ├── crafting/         recipe matching, machine processing state machine
│   ├── power/            fuel/steam/electric network solving
│   ├── hazards/          darkness, flooding, gas, heat, cave-ins, radiation
│   └── protocol/         replicated components, intents, events, PROTOCOL_VERSION
├── crates/server/        headless authoritative binary
│   ├── net/              renet transport, handshake, interest management
│   ├── world/            chunk store, generation, persistence
│   └── sim/              mining validation, machines, power, hazards
├── crates/client/        windowed binary
│   ├── render/           atlas, greedy mesher, chunk entities, lighting
│   ├── input/            FPS controls, raycast targeting
│   ├── prediction/       local movement prediction + reconciliation
│   ├── ui/               egui: inventory, crafting, machines, tech tree
│   ├── hud/              hotbar, crosshair, vitals, depth meter, Geiger
│   └── audio/            procedural audio
├── crates/lobby/         axum service: accounts, tokens, server browser (M10)
└── tools/atlas_gen/      generates the block texture atlas PNG
```

`shared` is deliberately engine-light: it depends on Bevy only for `App`, `Component`,
`Resource` and `Plugin` plumbing, so its logic is testable in a plain `App` with
`MinimalPlugins`.

## 4. Dependency decisions (verified against crates.io, 2026-11-12)

| Crate | Version | Note |
|---|---|---|
| `bevy` | `=0.20.1` | newest stable; all deps below support it |
| `bevy_replicon` | `=0.45.5` | supports Bevy 0.20 |
| `bevy_replicon_renet` | `=0.21.0` | renet 2.0.1 UDP transport |
| `bevy_egui` | `=0.43.1` | egui 0.36 |
| `ron` | `=0.12.3` | content files |
| `postcard` | `=1.1.3` | replication + save encoding |
| `zstd` | `=0.14.3` | chunk + save compression |
| `rusqlite` | `=0.42.0` | bundled SQLite (players, machines, tech tree) |
| `axum` | `=0.8.9` | lobby service |
| `argon2` | `=0.6.0` | lobby password hashing |
| `proptest` | `=1.12.0` | property tests for inventory/palette |

**Deviation from the brief:** the brief lists `sqlx` for the lobby. `sqlx` and `rusqlite`
both declare `links = "sqlite3"`, and Cargo refuses to build a workspace that links the
same native library twice — so the two cannot coexist in one dependency graph. Rather than
drop `rusqlite` (which the *server* needs for synchronous, transaction-friendly saves), the
lobby uses `rusqlite` too, behind `tokio::task::spawn_blocking`. Queries are written in
plain portable SQL so a Postgres move only means swapping the driver. Recorded in
`PROGRESS.md`.

Bevy is built with `default-features = false` and an explicit feature list per crate, so
the headless server does not pull in a renderer, and the client does not pull in audio
backends it does not use.

## 5. Runtime model

- **Server tick:** fixed 30 Hz (`Time<Fixed>` at 30 Hz; replicon's `ServerTick` follows it).
  Machines, power and hazards tick in `FixedUpdate`. Replication runs in `PostUpdate`.
- **Client frame:** uncapped. Local movement is predicted each frame with the shared
  controller and reconciled against the last acknowledged input sequence.
- **Chunks:** 32³ blocks, palette-compressed `u16` IDs, zstd-compressed for transport.
  Unmodified chunks are never saved — they regenerate deterministically from the seed.
- **Block entities:** machines/chests are ECS entities carrying a `BlockPos`, never voxels.
- **Hosting:** the client can run the server plugins in-process ("Host game"), which is how
  most groups will play.

## 6. Milestones

Order is fixed by the brief. Each one ends with a runnable build, green tests, and a
`PROGRESS.md` entry.

| # | Milestone | Acceptance summary |
|---|---|---|
| M0 | Workspace and skeleton | client window + lit cube; server headless 30 Hz; `scripts/check.sh` green |
| M1 | Voxel world (single process) | deterministic 6-layer world, greedy meshing, mining, digging to layer 3 blocked by tier |
| M2 | Multiplayer foundation | 2 clients see each other dig; responsive at 150 ms; 8 bots 10 min no desync |
| M3 | Content, inventory, crafting (Era 0) | Era 0 playable in MP; validator passes |
| M4 | Machines, research, Era 1 | team reaches Hardened Steel Pickaxe and layer 3 |
| M5 | Era 2: steam + logistics | automated ore processing; Steam Drill reaches layer 4 |
| M6 | Era 3: electricity | stable coal/steam plant, brownouts correct, gas cleared |
| M7 | Era 4: chemistry + deep metallurgy | layer 5 reached, uranium found, survived |
| M8 | Era 5: nuclear | full chain test produces Nuclear Drill + stable reactor; 3×3 tunnel into layer 6 |
| M9 | Persistence and polish | restart restores everything; objectives; audio; menus |
| M10 | Lobby, accounts, server browser | Docker lobby + dedicated server; register/login/browse/play |

## 7. Risks and how we handle them

| Risk | Mitigation |
|---|---|
| Bevy/replicon API drift | exact pins; APIs verified against the installed crate source before use |
| Scope: 10 milestones is a lot of game | build breadth-first per milestone and keep every rule data-driven so content grows without code |
| Multiplayer retrofits are painful | networking lands in M2, before any content systems |
| Balance is guesswork | all numbers in RON + `balance_report` to print raw-ore cost per Era |
| No GPU in CI | gameplay logic lives in `shared`/`server` and is tested headless; the client is verified by compiling and by running against a software rasteriser |
