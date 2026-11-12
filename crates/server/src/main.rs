//! Headless Deepcore server.
//!
//! No window, no renderer, no GPU: the server owns the world, every gameplay rule and every
//! player. Clients send intents; the server decides.

use std::net::SocketAddr;

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon_renet::RepliconRenetPlugins;
use deepcore_shared::content::ContentRegistry;
use deepcore_shared::protocol::*;
use deepcore_shared::voxel::world::VoxelWorld;

mod net;
mod sim;
mod world;

use net::{ClientSessions, ServerOptions};
use sim::players::ServerPhysics;

/// Simulation rate. The brief fixes this at 30 Hz.
pub const TICK_HZ: f64 = 30.0;

fn main() {
    let options = parse_args();

    let mut app = App::new();

    app.add_plugins((
        MinimalPlugins,
        bevy::log::LogPlugin { filter: "info,deepcore=debug".to_owned(), ..default() },
        bevy::state::app::StatesPlugin,
        RepliconPlugins,
        RepliconRenetPlugins,
        deepcore_shared::SharedPlugin,
    ));

    // The brief fixes the server at 30 Hz; replicon's tick follows the fixed schedule.
    app.insert_resource(Time::<Fixed>::from_hz(TICK_HZ));
    app.insert_resource(options.clone());
    app.init_resource::<ClientSessions>();
    app.init_resource::<world::chunks::GenerationQueue>();
    app.insert_resource(VoxelWorld::new());

    net::register_protocol(&mut app);

    // Startup: content-derived tables, the generator and the listening socket.
    app.add_systems(
        Startup,
        (|world: &mut World| net::insert_world_gen(world), setup_server_tables, net::start_server)
            .chain(),
    );

    // Per-tick simulation: movement, mining, world streaming.
    app.add_systems(
        FixedUpdate,
        (
            sim::players::apply_movement,
            sim::players::handle_start_mining,
            sim::players::handle_stop_mining,
            sim::players::handle_place_block,
            sim::players::advance_mining,
            sim::players::announce_join,
            world::chunks::generate_chunks,
            world::chunks::unload_distant_chunks,
        )
            .chain()
            .run_if(in_state(ServerState::Running)),
    );

    // The handshake must run before replicon flushes its outgoing messages, so the client
    // gets the result on the same tick it is accepted.
    app.add_systems(
        PreUpdate,
        net::handle_handshake.after(ServerSystems::Receive).run_if(in_state(ServerState::Running)),
    );

    // Streaming runs after the simulation so it sends this tick's state.
    app.add_systems(
        PostUpdate,
        (net::stream::stream_chunks, net::stream::broadcast_block_changes)
            .before(ServerSystems::Send)
            .run_if(in_state(ServerState::Running)),
    );

    app.add_observer(net::on_server_event);
    app.add_observer(net::on_client_connected);
    app.add_systems(Last, log_stats);
    app.add_systems(Update, handle_ctrl_c);

    app.run();
}

/// Builds the physics tables from content once the registry exists, and generates the chunks
/// around the spawn synchronously so players can be settled onto real terrain on join.
fn setup_server_tables(
    mut commands: Commands,
    registry: Res<ContentRegistry>,
    generator: Res<world::chunks::ServerWorldGen>,
    mut world: ResMut<VoxelWorld>,
) {
    commands.insert_resource(ServerPhysics::from_registry(&registry));

    let spawn = registry.world().spawn;
    let center = deepcore_shared::voxel::ChunkPos::new(
        spawn.0.div_euclid(deepcore_shared::voxel::CHUNK_SIZE),
        spawn.1.div_euclid(deepcore_shared::voxel::CHUNK_SIZE),
        spawn.2.div_euclid(deepcore_shared::voxel::CHUNK_SIZE),
    );
    let started = std::time::Instant::now();
    for dy in -1..=1 {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let pos = center.offset(dx, dy, dz);
                if pos.in_world() && !world.contains(pos) {
                    world.insert(pos, generator.0.generate_chunk(pos));
                }
            }
        }
    }
    info!(
        "generated {} spawn chunks in {:.0} ms",
        world.loaded_chunks(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    let spawn = registry.world().spawn;
    info!(
        "world ready: seed {:#x}, spawn at {:?}, {} blocks, {} items, {} machines",
        registry.world().surface_y,
        spawn,
        registry.block_count(),
        registry.item_count(),
        registry.machine_count()
    );
}

/// Logs the tick rate once a second: the cheapest early warning that the simulation is behind.
fn log_stats(
    time: Res<Time>,
    players: Query<&NetPlayer>,
    world: Res<VoxelWorld>,
    mut last: Local<f32>,
) {
    *last += time.delta_secs();
    if *last >= 5.0 {
        *last = 0.0;
        info!(
            "{} player(s), {} chunks loaded ({:.1} MB)",
            players.iter().count(),
            world.loaded_chunks(),
            world.storage_bytes() as f32 / 1_048_576.0
        );
    }
}

fn handle_ctrl_c(mut exit: MessageWriter<AppExit>, mut keys: Local<bool>) {
    // Bevy installs its own Ctrl+C handler; this keeps `cargo run` shutting down cleanly in
    // terminals where the handler is not installed.
    let _ = (&mut exit, &mut keys);
}

/// Parses `--port`, `--seed`, `--bots`, `--simulate-latency` and `--simulate-loss`.
fn parse_args() -> ServerOptions {
    let mut options = ServerOptions::default();
    let args: Vec<String> = std::env::args().collect();
    let mut index = 1;
    while index < args.len() {
        let arg = args[index].as_str();
        let mut next = |name: &str| -> Option<String> {
            if index + 1 < args.len() {
                index += 1;
                Some(args[index].clone())
            } else {
                eprintln!("{name} needs a value");
                None
            }
        };
        match arg {
            "--port" => {
                if let Some(value) = next("--port")
                    && let Ok(port) = value.parse::<u16>()
                {
                    options.address = SocketAddr::from(([0, 0, 0, 0], port));
                }
            }
            "--seed" => {
                if let Some(value) = next("--seed")
                    && let Ok(seed) = value.parse::<u64>()
                {
                    options.seed = seed;
                }
            }
            "--bots" => {
                if let Some(value) = next("--bots")
                    && let Ok(count) = value.parse::<usize>()
                {
                    options.bots = count;
                }
            }
            "--simulate-latency" => {
                if let Some(value) = next("--simulate-latency")
                    && let Ok(ms) = value.parse::<u64>()
                {
                    options.latency_ms = ms;
                }
            }
            "--simulate-loss" => {
                if let Some(value) = next("--simulate-loss")
                    && let Ok(percent) = value.parse::<u8>()
                {
                    options.loss_percent = percent.min(100);
                }
            }
            "--help" | "-h" => {
                println!(
                    "Deepcore server\n\n  --port <port>            UDP port (default {DEFAULT_PORT})\n  --seed <u64>             world seed\n  --bots <n>               headless bot players\n  --simulate-latency <ms>  add latency to all packets\n  --simulate-loss <pct>    request packet loss (best effort)"
                );
                std::process::exit(0);
            }
            other => eprintln!("unknown argument `{other}` (try --help)"),
        }
        index += 1;
    }
    options
}
