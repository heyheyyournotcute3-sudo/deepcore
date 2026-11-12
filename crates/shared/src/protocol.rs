//! Network protocol constants shared by server and client.
//!
//! Every replicated component, client intent and server event is versioned by
//! [`PROTOCOL_VERSION`]. Bump it whenever the shape of any of them changes; the server
//! refuses clients with a different value during the handshake, which is far easier to
//! debug than a mysterious deserialisation failure halfway into a session.

/// Version of the wire protocol. Bump on any protocol-visible change.
pub const PROTOCOL_VERSION: u32 = 1;

/// Default UDP port a hosted or dedicated server listens on.
pub const DEFAULT_PORT: u16 = 7777;

/// renet protocol id (an arbitrary constant that separates Deepcore traffic from
/// unrelated UDP traffic hitting the same port).
pub const RENET_PROTOCOL_ID: u64 = 0x0D33_0C0F_E0C0_DE01;

/// Maximum number of simultaneous players on one world.
pub const MAX_PLAYERS: usize = 16;
