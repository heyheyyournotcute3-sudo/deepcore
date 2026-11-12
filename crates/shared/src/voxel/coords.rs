//! Chunk and block coordinate maths.
//!
//! Deepcore uses a right-handed, Y-up coordinate system. The world is
//! [`WORLD_SIZE_X`] × [`WORLD_SIZE_Z`] blocks wide and [`WORLD_SIZE_Y`] blocks deep.
//!
//! * `y = 0` is the bedrock floor, `y = 511` is the highest point of the surface.
//! * A chunk is [`CHUNK_SIZE`]³ blocks.
//! * [`BlockPos`] is an absolute block coordinate.
//! * [`ChunkPos`] is an absolute chunk coordinate.
//! * [`LocalPos`] is a block coordinate inside a chunk (0..CHUNK_SIZE per axis).
//!
//! Keeping these as distinct newtypes is what stops the classic voxel bug of silently
//! mixing up absolute and chunk-local coordinates.

use serde::{Deserialize, Serialize};

/// Edge length of a chunk, in blocks.
pub const CHUNK_SIZE: i32 = 32;
/// Number of blocks in one chunk.
pub const CHUNK_VOLUME: usize = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize;

/// Number of chunks along the X and Z axes.
pub const WORLD_CHUNKS_X: i32 = 32;
/// Number of chunks along the Y axis (depth).
pub const WORLD_CHUNKS_Y: i32 = 16;
/// Number of chunks along the X and Z axes (alias for clarity at call sites).
pub const WORLD_CHUNKS_Z: i32 = 32;

/// World width and depth in blocks.
pub const WORLD_SIZE_X: i32 = WORLD_CHUNKS_X * CHUNK_SIZE;
/// World height in blocks.
pub const WORLD_SIZE_Y: i32 = WORLD_CHUNKS_Y * CHUNK_SIZE;
/// World depth in blocks.
pub const WORLD_SIZE_Z: i32 = WORLD_CHUNKS_Z * CHUNK_SIZE;

/// Absolute block coordinate inside the world.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct BlockPos {
    /// East/west axis, `0..WORLD_SIZE_X`.
    pub x: i32,
    /// Vertical axis, `0..WORLD_SIZE_Y`, where 0 is bedrock.
    pub y: i32,
    /// North/south axis, `0..WORLD_SIZE_Z`.
    pub z: i32,
}

impl BlockPos {
    /// Creates a block position.
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Returns true if the position lies inside the world bounds.
    pub const fn in_world(self) -> bool {
        self.x >= 0
            && self.x < WORLD_SIZE_X
            && self.y >= 0
            && self.y < WORLD_SIZE_Y
            && self.z >= 0
            && self.z < WORLD_SIZE_Z
    }

    /// The chunk that contains this block.
    pub const fn chunk(self) -> ChunkPos {
        ChunkPos::new(
            self.x.div_euclid(CHUNK_SIZE),
            self.y.div_euclid(CHUNK_SIZE),
            self.z.div_euclid(CHUNK_SIZE),
        )
    }

    /// The block's coordinate inside its chunk.
    pub const fn local(self) -> LocalPos {
        LocalPos::new(
            self.x.rem_euclid(CHUNK_SIZE),
            self.y.rem_euclid(CHUNK_SIZE),
            self.z.rem_euclid(CHUNK_SIZE),
        )
    }

    /// Offset by a vector.
    pub const fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.z + dz)
    }

    /// The six axis-aligned neighbours of this block.
    pub const fn neighbours(self) -> [BlockPos; 6] {
        [
            self.offset(1, 0, 0),
            self.offset(-1, 0, 0),
            self.offset(0, 1, 0),
            self.offset(0, -1, 0),
            self.offset(0, 0, 1),
            self.offset(0, 0, -1),
        ]
    }

    /// Centre of the block in world space.
    pub fn center_f32(self) -> bevy::math::Vec3 {
        bevy::math::Vec3::new(self.x as f32 + 0.5, self.y as f32 + 0.5, self.z as f32 + 0.5)
    }

    /// Squared distance to a floating point world position.
    pub fn distance_squared_to(self, point: bevy::math::Vec3) -> f32 {
        self.center_f32().distance_squared(point)
    }
}

/// Absolute chunk coordinate.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct ChunkPos {
    /// Chunk index along X, `0..WORLD_CHUNKS_X`.
    pub x: i32,
    /// Chunk index along Y, `0..WORLD_CHUNKS_Y`, where 0 is the bottom of the world.
    pub y: i32,
    /// Chunk index along Z, `0..WORLD_CHUNKS_Z`.
    pub z: i32,
}

impl ChunkPos {
    /// Creates a chunk position.
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Returns true if the chunk lies inside the world.
    pub const fn in_world(self) -> bool {
        self.x >= 0
            && self.x < WORLD_CHUNKS_X
            && self.y >= 0
            && self.y < WORLD_CHUNKS_Y
            && self.z >= 0
            && self.z < WORLD_CHUNKS_Z
    }

    /// Lowest (most negative) corner block of the chunk.
    pub const fn origin(self) -> BlockPos {
        BlockPos::new(self.x * CHUNK_SIZE, self.y * CHUNK_SIZE, self.z * CHUNK_SIZE)
    }

    /// Offset by a chunk vector.
    pub const fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.z + dz)
    }

    /// Chebyshev distance in chunks to another chunk.
    pub fn chunk_distance(self, other: ChunkPos) -> i32 {
        (self.x - other.x).abs().max((self.y - other.y).abs()).max((self.z - other.z).abs())
    }

    /// Stable linear index of the chunk within the world, used for region files.
    pub fn linear_index(self) -> usize {
        (self.y as usize * WORLD_CHUNKS_Z as usize + self.z as usize) * WORLD_CHUNKS_X as usize
            + self.x as usize
    }
}

/// Block coordinate inside a chunk, each axis in `0..CHUNK_SIZE`.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct LocalPos {
    /// X inside the chunk.
    pub x: i32,
    /// Y inside the chunk.
    pub y: i32,
    /// Z inside the chunk.
    pub z: i32,
}

impl LocalPos {
    /// Creates a chunk-local position.
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Linear index inside the chunk's block array.
    ///
    /// Layout is `x + z * 32 + y * 1024`, so blocks along X are contiguous, which is the
    /// order the mesher walks and the order the palette compresses best.
    pub const fn index(self) -> usize {
        debug_assert!(self.x >= 0 && self.x < CHUNK_SIZE);
        debug_assert!(self.y >= 0 && self.y < CHUNK_SIZE);
        debug_assert!(self.z >= 0 && self.z < CHUNK_SIZE);
        (self.y * CHUNK_SIZE * CHUNK_SIZE + self.z * CHUNK_SIZE + self.x) as usize
    }

    /// Inverse of [`LocalPos::index`].
    pub const fn from_index(index: usize) -> Self {
        let index = index as i32;
        Self {
            x: index % CHUNK_SIZE,
            y: index / (CHUNK_SIZE * CHUNK_SIZE),
            z: (index / CHUNK_SIZE) % CHUNK_SIZE,
        }
    }

    /// Returns true if all axes are inside `0..CHUNK_SIZE`.
    pub const fn in_chunk(self) -> bool {
        self.x >= 0
            && self.x < CHUNK_SIZE
            && self.y >= 0
            && self.y < CHUNK_SIZE
            && self.z >= 0
            && self.z < CHUNK_SIZE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_dimensions_match_the_brief() {
        assert_eq!(WORLD_SIZE_X, 1024);
        assert_eq!(WORLD_SIZE_Z, 1024);
        assert_eq!(WORLD_SIZE_Y, 512);
        assert_eq!(CHUNK_VOLUME, 32768);
    }

    #[test]
    fn block_to_chunk_and_back() {
        for pos in [
            BlockPos::new(0, 0, 0),
            BlockPos::new(31, 31, 31),
            BlockPos::new(32, 0, 0),
            BlockPos::new(1023, 511, 1023),
            BlockPos::new(517, 300, 12),
        ] {
            let chunk = pos.chunk();
            let local = pos.local();
            let rebuilt = chunk.origin().offset(local.x, local.y, local.z);
            assert_eq!(rebuilt, pos, "round trip failed for {pos:?}");
            assert!(local.in_chunk());
        }
    }

    #[test]
    fn local_index_round_trips() {
        for i in 0..CHUNK_VOLUME {
            let pos = LocalPos::from_index(i);
            assert_eq!(pos.index(), i);
        }
        // Spot-check the documented layout.
        assert_eq!(LocalPos::new(1, 0, 0).index(), 1);
        assert_eq!(LocalPos::new(0, 0, 1).index(), 32);
        assert_eq!(LocalPos::new(0, 1, 0).index(), 1024);
    }

    #[test]
    fn local_index_is_unique() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    assert!(seen.insert(LocalPos::new(x, y, z).index()));
                }
            }
        }
        assert_eq!(seen.len(), CHUNK_VOLUME);
    }

    #[test]
    fn bounds_checks() {
        assert!(BlockPos::new(0, 0, 0).in_world());
        assert!(BlockPos::new(1023, 511, 1023).in_world());
        assert!(!BlockPos::new(1024, 0, 0).in_world());
        assert!(!BlockPos::new(0, 512, 0).in_world());
        assert!(!BlockPos::new(-1, 0, 0).in_world());

        assert!(ChunkPos::new(31, 15, 31).in_world());
        assert!(!ChunkPos::new(32, 0, 0).in_world());
        assert!(!ChunkPos::new(0, 16, 0).in_world());
    }

    #[test]
    fn chunk_linear_index_is_stable_and_unique() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for y in 0..WORLD_CHUNKS_Y {
            for z in 0..WORLD_CHUNKS_Z {
                for x in 0..WORLD_CHUNKS_X {
                    assert!(seen.insert(ChunkPos::new(x, y, z).linear_index()));
                }
            }
        }
        assert_eq!(seen.len(), (WORLD_CHUNKS_X * WORLD_CHUNKS_Y * WORLD_CHUNKS_Z) as usize);
    }

    #[test]
    fn neighbours_are_adjacent_and_distinct() {
        let pos = BlockPos::new(10, 10, 10);
        let mut seen = std::collections::HashSet::new();
        for n in pos.neighbours() {
            assert!(seen.insert(n));
            let d = (n.x - pos.x).abs() + (n.y - pos.y).abs() + (n.z - pos.z).abs();
            assert_eq!(d, 1);
        }
    }

    #[test]
    fn chunk_distance_is_chebyshev() {
        let a = ChunkPos::new(0, 0, 0);
        assert_eq!(a.chunk_distance(ChunkPos::new(3, 0, 0)), 3);
        assert_eq!(a.chunk_distance(ChunkPos::new(3, 2, 0)), 3);
        assert_eq!(a.chunk_distance(ChunkPos::new(-3, 0, 1)), 3);
    }
}
