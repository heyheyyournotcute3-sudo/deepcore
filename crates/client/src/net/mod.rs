//! Client networking: connecting, the handshake, receiving chunks and replicating players.

use std::net::{SocketAddr, UdpSocket};
use std::time::SystemTime;

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon_renet::renet::ConnectionConfig;
use bevy_replicon_renet::{
    RenetChannelsExt, RenetClient,
    netcode::{ClientAuthentication, NetcodeClientTransport},
};
use deepcore_shared::content::content_hash;
use deepcore_shared::protocol::*;
use deepcore_shared::voxel::world::VoxelWorld;

/// Where and how the client should connect.
#[derive(Resource, Clone, Debug)]
pub struct ConnectionOptions {
    /// Server address.
    pub address: SocketAddr,
    /// Display name.
    pub name: String,
    /// Host a server inside this process instead of connecting out.
    pub host: bool,
}

impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            address: SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT)),
            name: "Miner".to_owned(),
            host: false,
        }
    }
}

/// The connection state shown by the UI.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub enum ConnectionState {
    /// Not connected yet.
    #[default]
    Disconnected,
    /// Connecting and waiting for the handshake.
    Connecting,
    /// Handshake accepted; loading chunks.
    Loading,
    /// Playing.
    Playing,
    /// Refused, with a reason shown to the player.
    Rejected(String),
}

/// Data from the server's handshake, once accepted.
#[derive(Resource, Clone, Debug, Default)]
pub struct ServerInfo {
    /// The world seed.
    pub world_seed: u64,
    /// The spawn position.
    pub spawn: [f32; 3],
    /// True once a handshake was accepted.
    pub accepted: bool,
}

/// Creates the renet client and starts connecting.
pub fn start_client(world: &mut World) {
    let options = world.get_resource::<ConnectionOptions>().cloned().unwrap_or_default();

    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => socket,
        Err(error) => {
            error!("could not open a UDP socket: {error}");
            return;
        }
    };
    let current_time = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(time) => time,
        Err(_) => std::time::Duration::ZERO,
    };
    let client_id = current_time.as_millis() as u64;

    let authentication = ClientAuthentication::Unsecure {
        client_id,
        protocol_id: RENET_PROTOCOL_ID,
        server_addr: options.address,
        user_data: None,
    };
    let transport = match NetcodeClientTransport::new(current_time, authentication, socket) {
        Ok(transport) => transport,
        Err(error) => {
            error!("could not create the client transport: {error}");
            return;
        }
    };

    let channels = world.resource::<RepliconChannels>();
    let connection_config = ConnectionConfig {
        server_channels_config: channels.server_configs(),
        client_channels_config: channels.client_configs(),
        ..Default::default()
    };

    world.insert_resource(RenetClient::new(connection_config));
    world.insert_resource(transport);
    info!("connecting to {}", options.address);
}

/// Registers the client side of the protocol.
pub fn register_protocol(app: &mut App) {
    deepcore_shared::protocol::register_all(app);
}

/// Sends the hello once the connection is up.
pub fn send_hello(
    mut hellos: MessageWriter<ClientHello>,
    options: Res<ConnectionOptions>,
    mut state: ResMut<ConnectionState>,
    mut sent: Local<bool>,
    client: Option<Res<RenetClient>>,
) {
    if *sent {
        return;
    }
    let Some(client) = client else {
        return;
    };
    if client.is_connected() {
        info!("connected, sending hello");
        hellos.write(ClientHello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: content_hash(),
            name: options.name.clone(),
        });
        *state = ConnectionState::Connecting;
        *sent = true;
    }
}

/// Handles the server's handshake answer.
pub fn handle_handshake_result(
    mut results: MessageReader<HandshakeResult>,
    mut state: ResMut<ConnectionState>,
    mut info: ResMut<ServerInfo>,
) {
    for result in results.read() {
        debug!(
            "handshake result: accepted={} reason={:?} spawn={:?}",
            result.accepted, result.reason, result.spawn
        );
        if result.accepted {
            info.accepted = true;
            info.world_seed = result.world_seed;
            info.spawn = result.spawn;
            *state = ConnectionState::Loading;
            info!("handshake accepted (protocol v{})", result.protocol_version);
        } else {
            error!("server refused the connection: {}", result.reason);
            *state = ConnectionState::Rejected(result.reason.clone());
        }
    }
}

/// Applies chunk data and unload messages to the local world.
pub fn receive_chunks(
    mut chunks: MessageReader<ChunkData>,
    mut unloads: MessageReader<ChunkUnload>,
    mut world: ResMut<VoxelWorld>,
    mut dirty: ResMut<crate::render::DirtyChunks>,
    mut meshed: ResMut<crate::render::MeshedChunks>,
) {
    for message in chunks.read() {
        debug!(
            "chunk {} {} {} ({} bytes)",
            message.pos[0],
            message.pos[1],
            message.pos[2],
            message.payload.len()
        );
        let pos =
            deepcore_shared::voxel::ChunkPos::new(message.pos[0], message.pos[1], message.pos[2]);
        match decode_chunk(&message.payload) {
            Ok(chunk) => {
                world.insert(pos, chunk);
                // A chunk that arrives (or arrives again) must be re-meshed, and its
                // neighbours may now have new visible faces.
                meshed.chunks.remove(&pos);
                dirty.chunks.insert(pos);
                for offset in [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)]
                {
                    let neighbour = pos.offset(offset.0, offset.1, offset.2);
                    if world.contains(neighbour) {
                        dirty.chunks.insert(neighbour);
                    }
                }
            }
            Err(error) => error!("bad chunk from server: {error}"),
        }
    }

    for message in unloads.read() {
        let pos =
            deepcore_shared::voxel::ChunkPos::new(message.pos[0], message.pos[1], message.pos[2]);
        world.remove(pos);
        meshed.chunks.remove(&pos);
    }
}

/// Applies authoritative block changes.
pub fn receive_block_changes(
    mut changes: MessageReader<BlockChanged>,
    mut world: ResMut<VoxelWorld>,
    mut dirty: ResMut<crate::render::DirtyChunks>,
) {
    for change in changes.read() {
        let pos = unpack_pos(change.position);
        if world.set_block(pos, change.block) {
            crate::render::mark_dirty(&mut dirty, pos);
        }
    }
}

/// Shows server notices in the HUD.
#[derive(Resource, Default)]
pub struct Notices {
    /// The latest notice text.
    pub latest: String,
    /// Seconds left to show it.
    pub timer: f32,
}

/// Picks up notices from the server.
pub fn receive_notices(mut notices: MessageReader<Notice>, mut state: ResMut<Notices>) {
    for notice in notices.read() {
        state.latest = notice.text.clone();
        state.timer = 3.0;
    }
}

/// Counts down the notice timer.
pub fn tick_notices(time: Res<Time>, mut state: ResMut<Notices>) {
    if state.timer > 0.0 {
        state.timer = (state.timer - time.delta_secs()).max(0.0);
        if state.timer == 0.0 {
            state.latest.clear();
        }
    }
}

/// Updates the connection state from the renet client's own view of the link.
pub fn track_connection(client: Option<Res<RenetClient>>, mut state: ResMut<ConnectionState>) {
    let Some(client) = client else {
        return;
    };
    if client.is_disconnected() && matches!(*state, ConnectionState::Connecting) {
        warn!("the connection dropped while connecting");
        *state = ConnectionState::Disconnected;
    }
}

/// Leaves the loading screen once chunks around the spawn have arrived.
pub fn finish_loading(
    mut state: ResMut<ConnectionState>,
    world: Res<VoxelWorld>,
    info: Res<ServerInfo>,
) {
    if !matches!(*state, ConnectionState::Loading) {
        return;
    }
    let spawn = Vec3::new(info.spawn[0], info.spawn[1], info.spawn[2]);
    let center = crate::render::chunk_of(spawn);
    // Wait for the 3x3 column of chunks around the spawn.
    let mut present = 0;
    for dz in -1..=1 {
        for dx in -1..=1 {
            if world.contains(center.offset(dx, 0, dz)) {
                present += 1;
            }
        }
    }
    if present >= 9 {
        *state = ConnectionState::Playing;
        info!("chunks loaded, entering the mine");
    } else {
        info!("waiting for spawn chunks: {present}/9 present around {center:?}");
    }
}

/// The spawn position to use for the camera: the replicated player if it has arrived, else the
/// handshake hint.
pub fn resolved_spawn(info: &ServerInfo, players: &Query<&NetPlayer>, own: &OwnPlayer) -> Vec3 {
    if let Some(entity) = own.entity
        && let Ok(net) = players.get(entity)
    {
        return net.feet();
    }
    Vec3::new(info.spawn[0], info.spawn[1], info.spawn[2])
}

/// The client's own player entity, once replication has created it.
#[derive(Resource, Default)]
pub struct OwnPlayer {
    /// The entity, if it has arrived.
    pub entity: Option<Entity>,
    /// The renet client id this client was assigned.
    pub client_id: Option<u64>,
}

/// Finds this client's player entity among the replicated ones.
pub fn find_own_player(
    mut own: ResMut<OwnPlayer>,
    players: Query<(Entity, &NetOwner)>,
    transport: Option<Res<NetcodeClientTransport>>,
) {
    if own.entity.is_some() {
        return;
    }
    let Some(transport) = transport else {
        return;
    };
    let client_id = transport.client_id();
    own.client_id = Some(client_id);
    for (entity, owner) in &players {
        if owner.client == client_id {
            own.entity = Some(entity);
            info!("own player entity is {entity}");
            return;
        }
    }
}
