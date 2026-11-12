//! The Deepcore wire protocol.
//!
//! Everything that crosses the network lives here so server and client can never disagree
//! about a shape. The protocol is versioned by [`PROTOCOL_VERSION`]: bump it whenever a
//! replicated component, client intent or server message changes, and the handshake will
//! refuse clients built against a different version instead of desyncing halfway through a
//! session.
//!
//! Channels:
//! * **movement** — unordered/unreliable; a late input is worthless.
//! * **intents** — ordered/reliable; mining and placing must not be reordered.
//! * **chunks** — ordered/reliable; a chunk stream is a bulk transfer.
//! * **events** — ordered/reliable; chat, tech tree, notifications.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Version of the wire protocol. Bump on any protocol-visible change.
pub const PROTOCOL_VERSION: u32 = 2;

/// Default UDP port a hosted or dedicated server listens on.
pub const DEFAULT_PORT: u16 = 7777;

/// renet protocol id, separating Deepcore traffic from unrelated UDP on the same port.
pub const RENET_PROTOCOL_ID: u64 = 0x0D33_0C0F_E0C0_DE01;

/// Maximum number of simultaneous players on one world.
pub const MAX_PLAYERS: usize = 16;

/// Reach in blocks the server allows for mining and interacting.
pub const MAX_REACH: f32 = 6.0;

/// How many chunks horizontally the server streams around a player.
pub const VIEW_RADIUS_HORIZONTAL: i32 = 5;
/// How many chunks vertically the server streams around a player.
pub const VIEW_RADIUS_VERTICAL: i32 = 4;

// ---------------------------------------------------------------- replicated components

/// A player as seen by everyone else: position, look direction and the last input the server
/// processed (used by the owner for reconciliation).
#[derive(Component, Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetPlayer {
    /// Feet position.
    pub position: [f32; 3],
    /// Look yaw in radians.
    pub yaw: f32,
    /// Look pitch in radians.
    pub pitch: f32,
    /// Sequence number of the last client input the server applied.
    pub last_seq: u32,
    /// True while the player is crouching.
    pub crouching: bool,
}

impl NetPlayer {
    /// Builds the replicated form of a player state.
    pub fn from_state(state: &crate::movement::PlayerState, last_seq: u32) -> Self {
        Self {
            position: [state.position.x, state.position.y, state.position.z],
            yaw: state.yaw,
            pitch: state.pitch,
            last_seq,
            crouching: state.crouching,
        }
    }

    /// The feet position as a vector.
    pub fn feet(&self) -> Vec3 {
        Vec3::new(self.position[0], self.position[1], self.position[2])
    }
}

/// Which client owns a player entity. Replicated so each client can tell which entity is its
/// own (and therefore which one it predicts).
#[derive(Component, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetOwner {
    /// The renet client id, or `u64::MAX` for a server-side bot.
    pub client: u64,
}

/// A block entity (machine, crate) as seen by clients. Full slot contents are only sent to a
/// client with that machine's UI open, so this carries the coarse state only.
#[derive(Component, Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetMachine {
    /// Machine definition index.
    pub machine: u16,
    /// Block position.
    pub position: [i32; 3],
    /// Rotation around Y, in quarter turns.
    pub facing: u8,
    /// True when the machine has everything it needs and is running.
    pub running: bool,
    /// Progress through the current recipe, 0..1.
    pub progress: f32,
}

// ---------------------------------------------------------------- client → server intents

/// Client hello, sent once on connect.
#[derive(Message, Clone, Debug, Serialize, Deserialize)]
pub struct ClientHello {
    /// The client's protocol version.
    pub protocol_version: u32,
    /// Hash of the client's content files.
    pub content_hash: u64,
    /// Display name.
    pub name: String,
}

/// One tick of movement input.
#[derive(Message, Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct MoveIntent {
    /// The movement input itself.
    pub input: crate::movement::MoveInput,
}

/// Begin mining the targeted block.
#[derive(Message, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct StartMining {
    /// The block being mined.
    pub block: [i32; 3],
    /// Which face was hit, 0..5.
    pub face: u8,
}

/// Stop mining (button released or target lost).
#[derive(Message, Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct StopMining;

/// Place a block on the targeted face.
#[derive(Message, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PlaceBlock {
    /// The block to place into.
    pub position: [i32; 3],
    /// Which block id to place.
    pub block: u16,
}

/// Interact with a machine or container.
#[derive(Message, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Interact {
    /// The block being interacted with.
    pub block: [i32; 3],
}

// ---------------------------------------------------------------- server → client messages

/// Sent once per connection: the server's version and content identity, or a rejection.
#[derive(Message, Clone, Debug, Serialize, Deserialize)]
pub struct HandshakeResult {
    /// True if the client may play.
    pub accepted: bool,
    /// Why the client was rejected, if it was.
    pub reason: String,
    /// The server's protocol version.
    pub protocol_version: u32,
    /// The server's content hash.
    pub content_hash: u64,
    /// The world seed, for display and for client-side prediction of untouched terrain.
    pub world_seed: u64,
    /// The client's own entity position at spawn.
    pub spawn: [f32; 3],
}

/// One chunk of the world, compressed.
#[derive(Message, Clone, Debug, Serialize, Deserialize)]
pub struct ChunkData {
    /// Chunk coordinate.
    pub pos: [i32; 3],
    /// postcard-encoded [`crate::voxel::Chunk`], zstd-compressed.
    pub payload: Vec<u8>,
}

/// A chunk leaving the client's view.
#[derive(Message, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ChunkUnload {
    /// Chunk coordinate.
    pub pos: [i32; 3],
}

/// One block changed, broadcast to everyone with that chunk loaded.
#[derive(Message, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct BlockChanged {
    /// Block position.
    pub position: [i32; 3],
    /// The new block id (0 = air).
    pub block: u16,
}

/// A short message for the player, e.g. "Too hard — needs Tier 2" or a research result.
#[derive(Message, Clone, Debug, Serialize, Deserialize)]
pub struct Notice {
    /// The text to show.
    pub text: String,
}

/// Chat line.
#[derive(Message, Clone, Debug, Serialize, Deserialize)]
pub struct ChatLine {
    /// Sender display name.
    pub from: String,
    /// The message.
    pub text: String,
}

// ---------------------------------------------------------------- helpers

/// Packs a block position for the wire.
pub fn pack_pos(position: crate::voxel::BlockPos) -> [i32; 3] {
    [position.x, position.y, position.z]
}

/// Unpacks a wire block position.
pub fn unpack_pos(position: [i32; 3]) -> crate::voxel::BlockPos {
    crate::voxel::BlockPos::new(position[0], position[1], position[2])
}

/// Compresses a chunk for transport.
pub fn encode_chunk(chunk: &crate::voxel::Chunk) -> Result<Vec<u8>, String> {
    let encoded = postcard::to_allocvec(chunk).map_err(|error| format!("chunk encode: {error}"))?;
    zstd::bulk::compress(&encoded, 3).map_err(|error| format!("chunk compress: {error}"))
}

/// Decompresses a chunk received from the server.
pub fn decode_chunk(payload: &[u8]) -> Result<crate::voxel::Chunk, String> {
    let raw = zstd::bulk::decompress(payload, 1024 * 1024)
        .map_err(|error| format!("chunk decompress: {error}"))?;
    postcard::from_bytes(&raw).map_err(|error| format!("chunk decode: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voxel::chunk::{AIR, Chunk};
    use crate::voxel::coords::LocalPos;

    #[test]
    fn chunk_transport_round_trips() {
        let mut chunk = Chunk::filled(1);
        chunk.set(LocalPos::new(1, 2, 3), 42);
        let payload = encode_chunk(&chunk).expect("encode");
        let decoded = decode_chunk(&payload).expect("decode");
        assert_eq!(chunk.to_blocks(), decoded.to_blocks());
    }

    #[test]
    fn chunk_transport_is_small_for_simple_chunks() {
        let chunk = Chunk::filled(1);
        let payload = encode_chunk(&chunk).expect("encode");
        assert!(payload.len() < 200, "solid chunk compressed to {} bytes", payload.len());
        let air = Chunk::filled(AIR);
        let payload = encode_chunk(&air).expect("encode");
        assert!(payload.len() < 200);
    }

    #[test]
    fn messages_round_trip() {
        let hello = ClientHello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: 0xDEAD_BEEF,
            name: "digger".to_owned(),
        };
        let bytes = postcard::to_allocvec(&hello).expect("encode");
        let back: ClientHello = postcard::from_bytes(&bytes).expect("decode");
        assert_eq!(back.name, "digger");
        assert_eq!(back.content_hash, 0xDEAD_BEEF);

        let result = HandshakeResult {
            accepted: true,
            reason: String::new(),
            protocol_version: PROTOCOL_VERSION,
            content_hash: 1,
            world_seed: 7,
            spawn: [1.0, 2.0, 3.0],
        };
        let bytes = postcard::to_allocvec(&result).expect("encode");
        let back: HandshakeResult = postcard::from_bytes(&bytes).expect("decode");
        assert!(back.accepted);
    }

    #[test]
    fn net_player_mirrors_a_player_state() {
        let mut state = crate::movement::PlayerState::at(Vec3::new(10.0, 20.0, 30.0));
        state.yaw = 1.5;
        state.pitch = -0.25;
        state.crouching = true;
        let net = NetPlayer::from_state(&state, 42);
        assert_eq!(net.feet(), Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(net.last_seq, 42);
        assert!(net.crouching);
        assert!((net.yaw - 1.5).abs() < 1e-6);
    }

    #[test]
    fn position_packing_round_trips() {
        let pos = crate::voxel::BlockPos::new(5, 400, 1000);
        assert_eq!(unpack_pos(pack_pos(pos)), pos);
    }
}
