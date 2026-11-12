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
| M1 Voxel world | not started |
| M2 Multiplayer foundation | not started |
| M3 Content, inventory, crafting (Era 0) | not started |
| M4 Machines, research, Era 1 | not started |
| M5 Era 2: steam + logistics | not started |
| M6 Era 3: electricity | not started |
| M7 Era 4: chemistry + deep metallurgy | not started |
| M8 Era 5: nuclear | not started |
| M9 Persistence and polish | not started |
| M10 Lobby, accounts, server browser | not started |

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
