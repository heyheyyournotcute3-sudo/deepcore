# Deepcore — Progress

> A future session, possibly a different model, must be able to continue from this file
> alone. It records what is built, what is deferred, known bugs and the exact commands to
> run everything.

## How to run

```bash
# Quality gate (fmt + clippy -D warnings + tests). Run before closing any milestone.
./scripts/check.sh

# Dedicated server (headless, 30 Hz, logs ticks at debug level)
RUST_LOG=debug cargo run -p deepcore_server

# Client (opens a window)
cargo run -p deepcore_client
```

Toolchain: stable Rust (installed via rustup, currently 1.99.1). The workspace pins exact
crate versions in the root `Cargo.toml`; see `docs/PLAN.md` §4 for why.

In this development sandbox there is no GPU; the client was verified against Mesa's
`llvmpipe` software Vulkan driver:

```bash
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json cargo run -p deepcore_client
```

## Status by milestone

| Milestone | State |
|---|---|
| M0 Workspace and skeleton | **done** |
| M1 Voxel world | **done** |
| M2 Multiplayer foundation | not started |
| M3 Content, inventory, crafting (Era 0) | not started |
| M4 Machines, research, Era 1 | not started |
| M5 Era 2: steam + logistics | not started |
| M6 Era 3: electricity | not started |
| M7 Era 4: chemistry + deep metallurgy | not started |
| M8 Era 5: nuclear | not started |
| M9 Persistence and polish | not started |
| M10 Lobby, accounts, server browser | not started |

## M1 — Voxel world (done)

**Built**
- `shared::voxel`: `BlockPos`/`ChunkPos`/`LocalPos` newtypes with index maths, plus
  palette-compressed `Chunk` (Solid / 4-bit / 8-bit / 16-bit storage, grown automatically) and
  a sparse `VoxelWorld` resource. Unit tests cover index round-trips, palette growth past 256
  entries, postcard round-trips and world bounds.
- `shared::content`: the RON content model, registries with string→numeric ids, a validator
  that fails fast (unknown references, recipes with no machine, missing tech parents, tech
  cycles, layer gaps/overlaps, ores with no use, duplicate ids, bad numbers) and a stable
  `content_hash()` for the M2 handshake. Content is embedded with `include_str!`, so a fresh
  clone needs no asset paths.
- `assets/data/`: 101 blocks, 178 items, 153 recipes, 43 machines, 22 tech nodes, 6 layers and
  the world-generation knobs — the complete content set from the brief (every ore, structure
  and tool from Sections 4 and 5, all six Eras, the whole nuclear chain).
- `shared::worldgen`: deterministic splitmix64 hashing, value-noise/fBm, layers, surface with a
  flat camp, caves, wobbly-sphere ore veins, water/methane pockets, tree ring and the
  unbreakable bedrock shell. Tests prove determinism across instances, across generation order,
  and that a different seed gives a different world.
- `shared::movement`: AABB-vs-voxel controller with per-axis resolution and sub-stepping,
  gravity, jump, sprint, crouch, ladders, water, and input sanitising for untrusted client
  data. 13 tests, including "identical inputs produce identical positions" and "the player
  never ends up inside rock".
- `shared::mining`: the break-time formula (`hardness / (speed × power)`), tick rounding, tool
  tiers, an Amanatides–Woo voxel raycast with exact face normals, and 3×3 face targeting for
  area tools.
- `shared::mesh`: greedy meshing with cross-chunk face culling, tested (solid chunk → 6 quads).
- `shared::atlas`: procedural 256×256 texture atlas from `blocks.ron` (speckle, blotch, ore
  flecks, strata, glow, grain), plus a dependency-free PNG encoder.
- `shared::balance` + `cargo run -p deepcore_shared --bin balance_report`: expands the tech tree
  and machine costs through the recipe graph and prints raw ore per Era (Era 1 ≈ 226 raw items,
  Era 5 ≈ 1177), with a recursion guard for cyclic recipes.
- Client: chunk streaming around the player, async greedy meshing on `AsyncComputeTaskPool`,
  a `StandardMaterial` extension (`assets/shaders/voxel.wesl`) that tiles the atlas per block
  across merged quads, first-person controls on the shared movement code, mining with the
  server-side rules, block placing, target outline, depth meter, crosshair that turns red with
  "Too hard — needs Tier N", and an F3 overlay.
- `crates/shared/tests/progression_m1.rs`: the M1 acceptance test — generates the real world,
  digs a shaft with the steel pickaxe, asserts it is stopped by granite in the Granite Depths
  (depth 64..160) and that the hardened pickaxe gets through; plus a bedrock-is-unbreakable test
  and a check that every layer's base rock requires its stated tier.

**Deferred**
- No networking yet (M2). The client generates its own chunks; M2 replaces the source.
- No inventory, hotbar contents or crafting UI (M3); the held tool is a resource placeholder.
- No audio, no particles, no persistence.

**Known issues**
- On the software rasteriser used in this sandbox (`llvmpipe`) the first frames are slow while
  shaders compile and chunks stream; on real hardware the meshing budget is 8 chunks/frame.
- `FogFalloff` distance is fixed at startup from the default view distance; it will follow the
  settings value once settings exist (M9).

## M0 — Workspace and skeleton (done)

**Built**
- Cargo workspace with four crates: `deepcore_shared`, `deepcore_server`,
  `deepcore_client`, `deepcore_lobby`, plus the excluded `tools/atlas_gen`.
- Exact dependency pins verified against crates.io: Bevy `=0.20.1`, `bevy_replicon`
  `=0.45.5`, `bevy_replicon_renet` `=0.21.0`, `bevy_egui` `=0.43.1`, `ron` `=0.12.3`,
  `postcard` `=1.1.3`, `zstd` `=0.14.3`, `rusqlite` `=0.42.0`, `axum` `=0.8.9`.
- Build profiles: `[profile.dev] opt-level = 1`, `[profile.dev.package."*"] opt-level = 3`,
  thin-LTO release.
- `rustfmt.toml`, `clippy.toml`, `rust-toolchain.toml`, `.gitignore`.
- `scripts/check.sh` runs `cargo fmt --check`, `cargo clippy --workspace --all-targets --
  -D warnings` and `cargo test --workspace`.
- `shared`: `SharedPlugin` (sets the shared 30 Hz `Time<Fixed>`), `protocol` constants
  (`PROTOCOL_VERSION`, `DEFAULT_PORT`, `RENET_PROTOCOL_ID`, `MAX_PLAYERS`) and a test that
  the fixed step really is 1/30 s.
- `server`: headless app (`MinimalPlugins` + log + states), banner on startup, tick counter
  that logs at debug level. Verified: 30 ticks per second, 33.33 ms fixed step.
- `client`: window "Deepcore", directional light with shadows, ground plane, lit test cube,
  slowly orbiting camera. Verified to open a window and render under llvmpipe.

**Deferred (deliberately, per milestone order)**
- `lobby` is a stub binary that prints a notice; it is built in M10.
- No networking, no voxels, no UI yet.

**Decisions / deviations**
- The brief specifies `sqlx` for the lobby. `sqlx` and `rusqlite` both declare
  `links = "sqlite3"`, and Cargo refuses to link the same native library twice in one
  workspace, so they cannot coexist. `rusqlite` (bundled) was kept because the *server*
  needs it for synchronous saves; the lobby will use `rusqlite` behind
  `spawn_blocking` with portable SQL. Documented in `docs/PLAN.md` §4.
- Bevy is built with `default-features = false` and explicit per-crate feature lists. The
  server pulls in no renderer; the client skips audio backends it does not use yet.
- `default_platform` (which enables the gamepad feature and needs `libudev` on Linux) is
  not used; the client lists the platform features it actually needs.

**Known bugs / warnings**
- The client logs `XDG_RUNTIME_DIR is invalid or not set` and an XSETTINGS warning in this
  headless sandbox. Harmless; not present on a desktop.
- `tools/atlas_gen` is created in M1.
