//! Server streaming: sending chunks and block changes to the clients that need them.

use std::collections::HashSet;

use bevy::prelude::*;
use bevy_replicon::prelude::*;

use deepcore_shared::protocol::*;
use deepcore_shared::voxel::coords::{CHUNK_SIZE, ChunkPos};
use deepcore_shared::voxel::world::VoxelWorld;

use crate::net::ClientSessions;

/// Maps a raw renet client id to its replicon client entity.
fn client_entity(sessions: &ClientSessions, client_id: u64) -> Option<Entity> {
    sessions.entities.get(&client_id).copied()
}

/// How many chunks may be sent to one client per tick. Chunk streaming is a burst; the steady
/// state budget only applies once the view is filled.
const CHUNKS_PER_TICK: usize = 4;

/// Streams chunk data to each player, nearest first, and sends unload messages.
pub fn stream_chunks(
    world: Res<VoxelWorld>,
    players: Query<(&NetPlayer, &NetOwner)>,
    mut sessions: ResMut<ClientSessions>,
    mut chunks_out: MessageWriter<ToClients<ChunkData>>,
    mut unload_out: MessageWriter<ToClients<ChunkUnload>>,
) {
    for (player, owner) in &players {
        let client_id = owner.client;
        if !sessions.players.contains_key(&client_id) {
            continue;
        }
        let position = player.feet();
        let center = ChunkPos::new(
            (position.x.floor() as i32).div_euclid(CHUNK_SIZE),
            (position.y.floor() as i32).div_euclid(CHUNK_SIZE),
            (position.z.floor() as i32).div_euclid(CHUNK_SIZE),
        );

        let Some(entity) = client_entity(&sessions, client_id) else {
            continue;
        };
        let visible = sessions.visible.entry(client_id).or_default();

        // ---- work out what should be visible
        let mut wanted: HashSet<ChunkPos> = HashSet::new();
        for dy in -VIEW_RADIUS_VERTICAL..=VIEW_RADIUS_VERTICAL {
            for dz in -VIEW_RADIUS_HORIZONTAL..=VIEW_RADIUS_HORIZONTAL {
                for dx in -VIEW_RADIUS_HORIZONTAL..=VIEW_RADIUS_HORIZONTAL {
                    let pos = center.offset(dx, dy, dz);
                    if pos.in_world() && world.contains(pos) {
                        wanted.insert(pos);
                    }
                }
            }
        }

        // ---- unload what left the view
        let stale: Vec<ChunkPos> = visible.difference(&wanted).copied().collect();
        for pos in stale {
            visible.remove(&pos);
            unload_out.write(ToClients {
                targets: SendTargets::Single(ClientId::Client(entity)),
                message: ChunkUnload { pos: [pos.x, pos.y, pos.z] },
            });
        }

        // ---- send what is new, nearest first
        let mut missing: Vec<ChunkPos> = wanted.difference(visible).copied().collect();
        missing.sort_by_key(|pos| pos.chunk_distance(center));
        for pos in missing.into_iter().take(CHUNKS_PER_TICK) {
            let Some(chunk) = world.chunk(pos) else {
                continue;
            };
            match encode_chunk(chunk) {
                Ok(payload) => {
                    chunks_out.write(ToClients {
                        targets: SendTargets::Single(ClientId::Client(entity)),
                        message: ChunkData { pos: [pos.x, pos.y, pos.z], payload },
                    });
                    visible.insert(pos);
                }
                Err(error) => error!("could not encode chunk {pos:?}: {error}"),
            }
        }
    }
}

/// Broadcasts block changes to every client that has the chunk loaded.
pub fn broadcast_block_changes(
    mut changes: MessageReader<BlockChanged>,
    sessions: Res<ClientSessions>,
    mut out: MessageWriter<ToClients<BlockChanged>>,
) {
    for change in changes.read() {
        let pos = unpack_pos(change.position).chunk();
        for (client_id, visible) in &sessions.visible {
            if visible.contains(&pos) {
                out.write(ToClients {
                    targets: match client_entity(&sessions, *client_id) {
                        Some(entity) => SendTargets::Single(ClientId::Client(entity)),
                        None => continue,
                    },
                    message: *change,
                });
            }
        }
    }
}
