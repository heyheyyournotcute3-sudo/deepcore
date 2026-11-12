//! Voxel storage: coordinates, chunks and the sparse world view used by gameplay systems.

pub mod chunk;
pub mod coords;
pub mod world;

pub use chunk::{AIR, BlockId, Chunk};
pub use coords::{
    BlockPos, CHUNK_SIZE, CHUNK_VOLUME, ChunkPos, LocalPos, WORLD_CHUNKS_X, WORLD_CHUNKS_Y,
    WORLD_CHUNKS_Z, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z,
};
pub use world::VoxelWorld;
