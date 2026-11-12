//! A sparse map of loaded chunks with absolute-coordinate access.
//!
//! Both the server (which owns every chunk it has generated) and the client (which only
//! holds the chunks the server streamed to it) need the same question answered:
//! *"what block is at this absolute position?"* — by the mesher, the physics, the raycast
//! and the machine systems. [`VoxelWorld`] is that shared answer.
//!
//! Unloaded positions report [`AIR`] rather than an error: a missing chunk behaves like
//! empty space, which is the right behaviour for collision (you fall through terrain that
//! has not arrived yet instead of getting stuck inside it).

use std::collections::HashMap;

use super::chunk::{AIR, BlockId, Chunk};
use super::coords::{BlockPos, ChunkPos, LocalPos, WORLD_SIZE_Y};

/// A sparse collection of chunks keyed by chunk coordinate.
///
/// It is a Bevy [`Resource`] so both binaries can keep the loaded world in the ECS.
#[derive(Default, bevy::prelude::Resource)]
pub struct VoxelWorld {
    chunks: HashMap<ChunkPos, Chunk>,
}

impl VoxelWorld {
    /// Creates an empty world with no chunks loaded.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of chunks currently loaded.
    pub fn loaded_chunks(&self) -> usize {
        self.chunks.len()
    }

    /// Inserts a chunk, replacing any previous chunk at that position.
    pub fn insert(&mut self, pos: ChunkPos, chunk: Chunk) -> Option<Chunk> {
        self.chunks.insert(pos, chunk)
    }

    /// Removes a chunk, returning it if it was loaded.
    pub fn remove(&mut self, pos: ChunkPos) -> Option<Chunk> {
        self.chunks.remove(&pos)
    }

    /// True if the chunk is loaded.
    pub fn contains(&self, pos: ChunkPos) -> bool {
        self.chunks.contains_key(&pos)
    }

    /// Borrows a chunk.
    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    /// Mutably borrows a chunk.
    pub fn chunk_mut(&mut self, pos: ChunkPos) -> Option<&mut Chunk> {
        self.chunks.get_mut(&pos)
    }

    /// Iterates over loaded chunks and their positions.
    pub fn iter(&self) -> impl Iterator<Item = (ChunkPos, &Chunk)> {
        self.chunks.iter().map(|(&pos, chunk)| (pos, chunk))
    }

    /// Iterates over loaded chunk positions.
    pub fn positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks.keys().copied()
    }

    /// Returns the block at an absolute position, or [`AIR`] if the chunk is not loaded.
    pub fn block(&self, pos: BlockPos) -> BlockId {
        if !pos.in_world() {
            return AIR;
        }
        match self.chunks.get(&pos.chunk()) {
            Some(chunk) => chunk.get(pos.local()),
            None => AIR,
        }
    }

    /// Returns true if a chunk is loaded at this position.
    pub fn is_loaded(&self, pos: BlockPos) -> bool {
        self.chunks.contains_key(&pos.chunk())
    }

    /// Sets a block and marks its chunk as modified.
    ///
    /// Returns `true` if the block changed. Does nothing for out-of-world positions or
    /// positions in unloaded chunks, so a late-arriving intent can never resurrect
    /// terrain the server does not own.
    pub fn set_block(&mut self, pos: BlockPos, block: BlockId) -> bool {
        if !pos.in_world() {
            return false;
        }
        let local = pos.local();
        match self.chunks.get_mut(&pos.chunk()) {
            Some(chunk) => {
                let changed = chunk.set(local, block);
                if changed {
                    chunk.mark_modified();
                }
                changed
            }
            None => false,
        }
    }

    /// True if a block is solid enough to stop the player.
    ///
    /// Air is passable; everything else is solid. Blocks that should be walk-through
    /// (ladders, lamps) are marked `passable` in the content data and consulted through
    /// [`Self::is_passable_with`].
    pub fn is_solid(&self, pos: BlockPos) -> bool {
        self.block(pos) != AIR
    }

    /// True if the block at this position blocks movement, given a predicate that decides
    /// whether a specific block id is passable.
    pub fn is_solid_with(&self, pos: BlockPos, is_passable: impl Fn(BlockId) -> bool) -> bool {
        let block = self.block(pos);
        block != AIR && !is_passable(block)
    }

    /// Y of the highest non-air block in a column, if the column's chunk is loaded.
    pub fn surface_y(&self, x: i32, z: i32) -> Option<i32> {
        (0..WORLD_SIZE_Y).rev().find(|&y| self.block(BlockPos::new(x, y, z)) != AIR)
    }

    /// Total bytes of palette storage used by loaded chunks.
    pub fn storage_bytes(&self) -> usize {
        self.chunks.values().map(Chunk::storage_bytes).sum()
    }

    /// A snapshot of the loaded chunks for handing to a worker thread.
    ///
    /// Meshing reads the world (not just one chunk) so that faces on chunk borders are culled
    /// correctly. Cloning is the cheapest correct way to move that data into a task; chunk
    /// storage is palette-compressed, so a loaded view radius costs a few hundred kilobytes.
    pub fn clone_for_mesh(&self) -> Self {
        Self { chunks: self.chunks.clone() }
    }

    /// Marks a chunk as needing a save.
    pub fn mark_modified(&mut self, pos: ChunkPos) {
        if let Some(chunk) = self.chunks.get_mut(&pos) {
            chunk.mark_modified();
        }
    }

    /// Positions of all chunks that differ from generated terrain.
    pub fn modified_chunks(&self) -> Vec<ChunkPos> {
        self.chunks.iter().filter(|(_, chunk)| chunk.is_modified()).map(|(&pos, _)| pos).collect()
    }
}

impl FromIterator<(ChunkPos, Chunk)> for VoxelWorld {
    fn from_iter<T: IntoIterator<Item = (ChunkPos, Chunk)>>(iter: T) -> Self {
        Self { chunks: iter.into_iter().collect() }
    }
}

/// Converts a block position to the chunk-local position it addresses.
pub fn local_of(pos: BlockPos) -> LocalPos {
    pos.local()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unloaded_positions_are_air() {
        let world = VoxelWorld::new();
        assert_eq!(world.block(BlockPos::new(5, 5, 5)), AIR);
        assert!(!world.is_solid(BlockPos::new(5, 5, 5)));
        assert!(!world.is_loaded(BlockPos::new(5, 5, 5)));
    }

    #[test]
    fn out_of_world_positions_are_air_and_not_settable() {
        let mut world = VoxelWorld::new();
        let chunk = ChunkPos::new(0, 0, 0);
        world.insert(chunk, Chunk::filled(AIR));
        assert_eq!(world.block(BlockPos::new(-1, 0, 0)), AIR);
        assert!(!world.set_block(BlockPos::new(-1, 0, 0), 5));
        assert!(!world.set_block(BlockPos::new(0, 512, 0), 5));
    }

    #[test]
    fn set_block_writes_through_to_the_chunk_and_marks_it_modified() {
        let mut world = VoxelWorld::new();
        let chunk_pos = ChunkPos::new(1, 2, 3);
        world.insert(chunk_pos, Chunk::filled(AIR));

        let block = chunk_pos.origin().offset(4, 5, 6);
        assert!(world.set_block(block, 9));
        assert_eq!(world.block(block), 9);
        assert!(world.chunk(chunk_pos).expect("chunk loaded").is_modified());
        assert_eq!(world.modified_chunks(), vec![chunk_pos]);

        // Setting the same value again reports no change.
        assert!(!world.set_block(block, 9));
    }

    #[test]
    fn set_block_on_unloaded_chunk_is_a_no_op() {
        let mut world = VoxelWorld::new();
        assert!(!world.set_block(BlockPos::new(100, 100, 100), 9));
        assert_eq!(world.loaded_chunks(), 0);
    }

    #[test]
    fn surface_y_finds_the_highest_solid_block() {
        let mut world = VoxelWorld::new();
        world.insert(ChunkPos::new(0, 0, 0), Chunk::filled(1));
        assert_eq!(world.surface_y(0, 0), Some(31));
        // An air chunk above the solid one does not change the surface height.
        world.insert(ChunkPos::new(0, 1, 0), Chunk::filled(AIR));
        assert_eq!(world.surface_y(0, 0), Some(31));
        // A column whose chunk is not loaded has no known surface.
        assert_eq!(world.surface_y(0, 32), None);
    }

    #[test]
    fn is_solid_with_respects_passable_blocks() {
        let mut world = VoxelWorld::new();
        world.insert(ChunkPos::new(0, 0, 0), Chunk::filled(AIR));
        let pos = BlockPos::new(1, 1, 1);
        world.set_block(pos, 42);
        assert!(world.is_solid_with(pos, |_| false));
        assert!(!world.is_solid_with(pos, |b| b == 42));
    }
}
