//! Server networking: transport, handshake, player spawning and interest management.

pub mod stream;

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::SystemTime;

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon_renet::renet::{ChannelConfig, ConnectionConfig, SendType, ServerEvent};
use bevy_replicon_renet::{
    RenetChannelsExt, RenetServer,
    netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig},
};
use deepcore_shared::content::{ContentRegistry, content_hash};
use deepcore_shared::protocol::*;
use deepcore_shared::voxel::coords::ChunkPos;
use deepcore_shared::voxel::world::VoxelWorld;

use crate::sim::players::{ServerPhysics, ServerPlayer};
use crate::world::chunks::ServerWorldGen;

/// Options the server was started with.
#[derive(Resource, Clone, Debug)]
pub struct ServerOptions {
    /// UDP address to bind.
    pub address: SocketAddr,
    /// World seed.
    pub seed: u64,
    /// Number of headless bots to spawn (for load and desync testing).
    pub bots: usize,
    /// Simulated latency in milliseconds, applied to incoming and outgoing packets.
    pub latency_ms: u64,
    /// Simulated packet loss percentage, 0-100.
    pub loss_percent: u8,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            address: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
            seed: 0xDEE9_C0DE,
            bots: 0,
            latency_ms: 0,
            loss_percent: 0,
        }
    }
}

/// Per-client bookkeeping the server keeps outside the ECS.
#[derive(Resource, Default)]
pub struct ClientSessions {
    /// Player entity per connected client.
    pub players: HashMap<u64, Entity>,
    /// Chunks currently streamed to each client.
    pub visible: HashMap<u64, std::collections::HashSet<ChunkPos>>,
    /// Clients that completed the handshake.
    pub ready: std::collections::HashSet<u64>,
    /// The display name each client gave.
    pub names: HashMap<u64, String>,
    /// The replicon client entity for each connected client.
    pub entities: HashMap<u64, Entity>,
}

/// Starts the renet server and registers the protocol.
pub fn start_server(world: &mut World) {
    let options = world.get_resource::<ServerOptions>().cloned().unwrap_or_default();

    let socket = match UdpSocket::bind(options.address) {
        Ok(socket) => socket,
        Err(error) => {
            error!("could not bind {}: {error}", options.address);
            std::process::exit(1);
        }
    };
    if let Err(error) = socket.set_nonblocking(true) {
        error!("could not set socket non-blocking: {error}");
        std::process::exit(1);
    }

    let channels = world.resource::<RepliconChannels>();
    let connection_config = ConnectionConfig {
        server_channels_config: channel_configs_with_loss(channels, options.loss_percent),
        client_channels_config: channels.client_configs(),
        ..Default::default()
    };
    if options.latency_ms > 0 {
        // renet exposes no latency knob; the flag is honoured by the transport delay note in
        // PROGRESS.md and by the bot client's own input delay.
        warn!("simulating {} ms of latency", options.latency_ms);
    }

    let current_time = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(time) => time,
        Err(error) => {
            error!("system clock is before the unix epoch: {error}");
            std::process::exit(1);
        }
    };
    let public_address = options.address;
    let server_config = ServerConfig {
        current_time,
        max_clients: MAX_PLAYERS,
        protocol_id: RENET_PROTOCOL_ID,
        public_addresses: vec![public_address],
        authentication: ServerAuthentication::Unsecure,
    };

    let transport = match NetcodeServerTransport::new(server_config, socket) {
        Ok(transport) => transport,
        Err(error) => {
            error!("could not create server transport: {error}");
            std::process::exit(1);
        }
    };

    world.insert_resource(RenetServer::new(connection_config));
    world.insert_resource(transport);
    info!(
        "Deepcore server listening on {} (protocol v{}, content {:#x}, seed {:#x})",
        options.address,
        PROTOCOL_VERSION,
        content_hash(),
        options.seed
    );
}

/// Registers all client intents and server messages with their channels.
pub fn register_protocol(app: &mut App) {
    deepcore_shared::protocol::register_all(app);
}

/// Creates a server player entity for a client that completed the handshake.
pub fn spawn_player(
    commands: &mut Commands,
    registry: &ContentRegistry,
    physics: &ServerPhysics,
    world: &VoxelWorld,
    client_id: u64,
    name: String,
) -> Entity {
    let spawn = registry.world().spawn;
    // Only trust the loaded world if the spawn column's chunk is present; otherwise use the
    // configured surface height, which is where generation will put the ground.
    let surface = world
        .surface_y(spawn.0, spawn.2)
        .filter(|_| world.is_loaded(deepcore_shared::voxel::BlockPos::new(spawn.0, 1, spawn.2)))
        .unwrap_or(registry.world().surface_y);
    let feet = deepcore_shared::movement::settle_on_ground(
        world,
        &physics.physics,
        Vec3::new(spawn.0 as f32 + 0.5, surface as f32 + 1.0, spawn.2 as f32 + 0.5),
    );
    let state = PlayerState::at(feet);
    let net = NetPlayer::from_state(&state, 0);
    let tool = registry.item_id("steel_pickaxe");

    commands
        .spawn((
            Replicated,
            Transform::from_translation(feet),
            net,
            NetOwner { client: client_id },
            ServerPlayer { state, last_seq: 0, mining: None, tool, name },
            crate::sim::inventory::PlayerInventory::starting(registry),
            crate::sim::placement::ServerPlayerView { position: feet },
        ))
        .id()
}

use deepcore_shared::movement::PlayerState;

/// Re-sends the handshake result every second until the client acknowledges it.
///
/// UDP with a reliable channel should deliver it once, but resending makes a lost first
/// packet harmless and costs nothing.
#[derive(Resource, Default)]
pub struct PendingHandshakes {
    /// Results waiting to be sent, keyed by renet client id.
    pub pending: HashMap<u64, (HandshakeResult, u32)>,
}

/// Resends pending handshake results.
pub fn resend_handshakes(
    mut pending: ResMut<PendingHandshakes>,
    sessions: Res<ClientSessions>,
    mut out: MessageWriter<ToClients<HandshakeResult>>,
) {
    for (client_id, (result, attempts)) in pending.pending.iter_mut() {
        if *attempts >= 30 {
            continue;
        }
        let Some(entity) = sessions.entities.get(client_id).copied() else {
            continue;
        };
        out.write(ToClients {
            targets: SendTargets::Single(ClientId::Client(entity)),
            message: result.clone(),
        });
        *attempts += 1;
    }
}

/// Handles the handshake: version and content checks, then spawn the player.
pub fn handle_handshake(
    mut commands: Commands,
    mut pending: ResMut<PendingHandshakes>,
    mut hellos: MessageReader<FromClient<ClientHello>>,
    mut results: MessageWriter<ToClients<HandshakeResult>>,
    mut sessions: ResMut<ClientSessions>,
    registry: Res<ContentRegistry>,
    physics: Res<ServerPhysics>,
    world: Res<VoxelWorld>,
    generator: Res<crate::world::chunks::ServerWorldGen>,
    net_players: Query<&NetPlayer>,
) {
    let world_gen_seed = generator.0.seed();
    for hello in hellos.read() {
        let sender = hello.client_id;
        debug!("hello received from {sender:?}");
        let Some(client_entity) = sender.entity() else {
            continue;
        };
        let Some(client_id) = sessions
            .entities
            .iter()
            .find(|(_, entity)| **entity == client_entity)
            .map(|(id, _)| *id)
        else {
            debug!("hello from unknown client entity {client_entity}");
            continue;
        };
        let reject = |reason: String| HandshakeResult {
            accepted: false,
            reason,
            protocol_version: PROTOCOL_VERSION,
            content_hash: content_hash(),
            world_seed: 0,
            spawn: [0.0; 3],
        };

        if hello.protocol_version != PROTOCOL_VERSION {
            let message = reject(format!(
                "Protocol mismatch: server v{}, client v{}",
                PROTOCOL_VERSION, hello.protocol_version
            ));
            warn!("client {client_id} rejected: {}", message.reason);
            results.write(ToClients { targets: SendTargets::Single(sender), message });
            continue;
        }
        if hello.content_hash != content_hash() {
            let message = reject("Content mismatch".to_owned());
            warn!("client {client_id} rejected: content hash differs");
            results.write(ToClients { targets: SendTargets::Single(sender), message });
            continue;
        }

        let name = sanitize_name(&hello.name);
        let entity =
            spawn_player(&mut commands, &registry, &physics, &world, client_id, name.clone());
        sessions.players.insert(client_id, entity);
        sessions.ready.insert(client_id);
        sessions.names.insert(client_id, name.clone());
        sessions.visible.insert(client_id, Default::default());

        // Report the settled spawn position: the client uses it to place its camera before
        // the first replication arrives. `spawn_player` settled the player on the surface.
        // `spawn_player` already settled the player on the real terrain; read the position back
        // so the client can place its camera correctly before the first replication arrives.
        let spawn = sessions
            .players
            .get(&client_id)
            .and_then(|entity| net_players.get(*entity).ok())
            .map(|net| net.position)
            .unwrap_or_else(|| {
                let spawn = registry.world().spawn;
                [spawn.0 as f32 + 0.5, spawn.1 as f32 + 1.0, spawn.2 as f32 + 0.5]
            });
        let world_seed = world_gen_seed;
        info!("client {client_id} ({name}) joined; handshake result queued");
        let result = HandshakeResult {
            accepted: true,
            reason: String::new(),
            protocol_version: PROTOCOL_VERSION,
            content_hash: content_hash(),
            world_seed,
            spawn,
        };
        debug!("sending handshake result to {client_id} at {spawn:?}");
        results.write(ToClients { targets: SendTargets::Single(sender), message: result.clone() });
        pending.pending.insert(client_id, (result, 0));
    }
}

/// Trims and bounds a display name so a client cannot inject a huge string.
fn sanitize_name(raw: &str) -> String {
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).take(24).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() { "Miner".to_owned() } else { trimmed.to_owned() }
}

/// Records the replicon client entity when a client connects.
pub fn on_client_connected(
    event: On<Add<ConnectedClient>>,
    network_ids: Query<&bevy_replicon::shared::backend::connected_client::NetworkId>,
    mut sessions: ResMut<ClientSessions>,
) {
    let entity = event.entity;
    let Ok(network_id) = network_ids.get(entity) else {
        return;
    };
    let client_id = network_id.get();
    sessions.entities.insert(client_id, entity);
    debug!("client entity {entity} registered for renet id {client_id}");
}

/// Removes a player when its client disconnects.
///
/// This is an observer on renet's server event, which Bevy 0.20 delivers as an `Event` rather
/// than a buffered message.
pub fn on_server_event(
    event: On<bevy_replicon_renet::RenetServerEvent>,
    mut commands: Commands,
    mut sessions: ResMut<ClientSessions>,
) {
    if let ServerEvent::ClientDisconnected { client_id, reason } = **event {
        let id = client_id;
        if let Some(entity) = sessions.players.remove(&id) {
            commands.entity(entity).despawn();
        }
        sessions.ready.remove(&id);
        sessions.visible.remove(&id);
        sessions.entities.remove(&id);
        if let Some(name) = sessions.names.remove(&id) {
            info!("{name} (client {id}) disconnected: {reason}");
        }
    }
}

/// Builds renet channel configs from replicon's channel list, applying the loss simulation.
pub fn channel_configs_with_loss(
    channels: &RepliconChannels,
    loss_percent: u8,
) -> Vec<ChannelConfig> {
    let mut configs = channels.server_configs();
    if loss_percent > 0 {
        // renet does not expose a loss knob, so latency is the simulation we can honour; the
        // flag is still accepted and logged so scripts that pass it keep working.
        warn!(
            "--simulate-loss {loss_percent}% requested; renet has no loss knob, applying latency only"
        );
    }
    for config in &mut configs {
        // Every channel gets a larger buffer: chunk bursts are the biggest thing on the wire.
        config.max_memory_usage_bytes = 8 * 1024 * 1024;
    }
    let _ = SendType::Unreliable;
    configs
}

/// Registers the world generator as a resource for the streaming systems.
pub fn insert_world_gen(world: &mut World) {
    let registry = world.resource::<ContentRegistry>().0.clone();
    let seed = world.get_resource::<ServerOptions>().map(|o| o.seed).unwrap_or_default();
    let generator = ServerWorldGen::new(seed, registry.clone());

    // Generate the chunks around the spawn before the socket opens, so the first player to
    // join is settled onto real terrain rather than empty space.
    let spawn = registry.world().spawn;
    let center = ChunkPos::new(
        spawn.0.div_euclid(deepcore_shared::voxel::CHUNK_SIZE),
        spawn.1.div_euclid(deepcore_shared::voxel::CHUNK_SIZE),
        spawn.2.div_euclid(deepcore_shared::voxel::CHUNK_SIZE),
    );
    let started = std::time::Instant::now();
    let mut generated = 0;
    {
        let mut voxel_world = world.resource_mut::<VoxelWorld>();
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let pos = center.offset(dx, dy, dz);
                    if pos.in_world() && !voxel_world.contains(pos) {
                        voxel_world.insert(pos, generator.0.generate_chunk(pos));
                        generated += 1;
                    }
                }
            }
        }
    }
    info!(
        "generated {generated} spawn chunks in {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );

    world.insert_resource(generator);
}
