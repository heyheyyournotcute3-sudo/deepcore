//! Palette-compressed chunk storage.
//!
//! A chunk is 32³ = 32 768 blocks. Storing a raw `u16` per block costs 64 KiB per chunk,
//! and a chunk of solid stone would spend 64 KiB saying the same number 32 768 times.
//! Instead each chunk keeps a palette (the distinct block IDs it contains) and one small
//! index per block whose width is the smallest that can address the palette:
//!
//! * 1 distinct block → 0 bits per block (a `Solid` chunk, zero allocation);
//! * ≤ 16 distinct blocks → 4 bits per block (16 KiB);
//! * ≤ 256 distinct blocks → 8 bits per block (32 KiB);
//! * otherwise → 16 bits per block (64 KiB).
//!
//! That is the same scheme Minecraft uses, and it keeps the common cases (air, stone,
//! ore pockets) very cheap while never being worse than the raw representation.

use serde::{Deserialize, Serialize};

use super::coords::{CHUNK_VOLUME, LocalPos};

/// The `u16` block id used in content data and on the wire.
pub type BlockId = u16;

/// The id of air. Air is always palette entry 0 and is never rendered.
pub const AIR: BlockId = 0;

/// Number of bits used per block index at each storage width.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
enum Storage {
    /// A single block fills the whole chunk.
    Solid,
    /// 4-bit indices, at most 16 palette entries.
    Nibble,
    /// 8-bit indices, at most 256 palette entries.
    Byte,
    /// 16-bit indices, at most 65 536 palette entries.
    Word,
}

impl Storage {
    fn bits(self) -> usize {
        match self {
            Storage::Solid => 0,
            Storage::Nibble => 4,
            Storage::Byte => 8,
            Storage::Word => 16,
        }
    }

    fn for_palette_len(len: usize) -> Self {
        match len {
            0 | 1 => Storage::Solid,
            2..=16 => Storage::Nibble,
            17..=256 => Storage::Byte,
            _ => Storage::Word,
        }
    }
}

/// A 32³ block of the world, palette-compressed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    palette: Vec<BlockId>,
    storage: Storage,
    /// Packed indices. Empty when `storage == Storage::Solid`.
    data: Vec<u8>,
    /// Number of blocks that differ from the chunk's most common block. A chunk with
    /// `non_air == 0` is untouched and is never written to disk.
    non_air: u32,
    /// True if any block differs from what world generation would produce.
    modified: bool,
}

impl Default for Chunk {
    fn default() -> Self {
        Self::filled(AIR)
    }
}

impl Chunk {
    /// Creates a chunk completely filled with one block id.
    pub fn filled(block: BlockId) -> Self {
        Self {
            palette: vec![block],
            storage: Storage::Solid,
            data: Vec::new(),
            non_air: if block == AIR { 0 } else { CHUNK_VOLUME as u32 },
            modified: false,
        }
    }

    /// Creates a chunk from a flat block array in [`LocalPos::index`] order.
    pub fn from_blocks(blocks: &[BlockId]) -> Self {
        assert_eq!(blocks.len(), CHUNK_VOLUME, "chunk must be filled with CHUNK_VOLUME blocks");

        // Build the palette, keeping air first so it gets index 0.
        let mut palette: Vec<BlockId> = Vec::with_capacity(8);
        palette.push(AIR);
        let mut lookup = std::collections::HashMap::new();
        lookup.insert(AIR, 0u16);
        for &block in blocks {
            if let std::collections::hash_map::Entry::Vacant(entry) = lookup.entry(block) {
                // There can never be more than 65536 distinct u16 ids, so this cast is safe.
                let index = palette.len() as u16;
                entry.insert(index);
                palette.push(block);
            }
        }

        let storage = Storage::for_palette_len(palette.len());
        let non_air = blocks.iter().filter(|&&b| b != AIR).count() as u32;

        let mut chunk = Self { palette, storage, data: Vec::new(), non_air, modified: false };
        chunk.write_indices(blocks, &lookup);
        chunk
    }

    /// Packs indices for the current palette and storage width.
    fn write_indices(
        &mut self,
        blocks: &[BlockId],
        lookup: &std::collections::HashMap<BlockId, u16>,
    ) {
        let bits = self.storage.bits();
        if bits == 0 {
            self.data.clear();
            return;
        }
        let byte_len = (CHUNK_VOLUME * bits).div_ceil(8);
        self.data.clear();
        self.data.resize(byte_len, 0);

        let mask: u32 = (1u32 << bits) - 1;
        for (i, &block) in blocks.iter().enumerate() {
            // Every block was inserted into the lookup while building the palette.
            let index = u32::from(lookup[&block]) & mask;
            let bit_pos = i * bits;
            let byte = bit_pos / 8;
            let shift = bit_pos % 8;
            let packed = (index as u16) << shift;
            self.data[byte] |= packed as u8;
            if shift + bits > 8 {
                self.data[byte + 1] |= (packed >> 8) as u8;
            }
        }
    }

    /// Reads the palette index of a block.
    fn read_index(&self, index: usize) -> usize {
        let bits = self.storage.bits();
        if bits == 0 {
            return 0;
        }
        let bit_pos = index * bits;
        let byte = bit_pos / 8;
        let shift = bit_pos % 8;
        let mut value = self.data[byte] as u16;
        if shift + bits > 8 {
            value |= (self.data[byte + 1] as u16) << 8;
        }
        ((value >> shift) as u32 & ((1u32 << bits) - 1)) as usize
    }

    /// Writes the packed index of a block.
    fn write_index(&mut self, index: usize, palette_index: usize) {
        let bits = self.storage.bits();
        if bits == 0 {
            return;
        }
        let mask: u32 = (1u32 << bits) - 1;
        let value = (palette_index as u32) & mask;
        let bit_pos = index * bits;
        let byte = bit_pos / 8;
        let shift = bit_pos % 8;

        self.data[byte] &= !(((mask as u16) << shift) as u8);
        if shift + bits > 8 {
            self.data[byte + 1] &= !(((mask as u16) << shift) >> 8) as u8;
        }

        self.data[byte] |= ((value << shift) as u16) as u8;
        if shift + bits > 8 {
            self.data[byte + 1] |= ((value << shift) >> 8) as u8;
        }
    }

    /// Returns the block at a chunk-local position.
    pub fn get(&self, pos: LocalPos) -> BlockId {
        let palette_index = self.read_index(pos.index());
        // Indices are always written from the palette, so this cannot be out of range;
        // `get` returning AIR rather than panicking keeps a corrupt chunk from crashing.
        self.palette.get(palette_index).copied().unwrap_or(AIR)
    }

    /// Sets the block at a chunk-local position, growing the palette if needed.
    ///
    /// Returns `true` if the block actually changed.
    pub fn set(&mut self, pos: LocalPos, block: BlockId) -> bool {
        let index = pos.index();
        let current_palette_index = self.read_index(index);
        let current = self.palette.get(current_palette_index).copied().unwrap_or(AIR);
        if current == block {
            return false;
        }

        let target = match self.palette.iter().position(|&b| b == block) {
            Some(entry) => entry,
            None => {
                let entry = self.palette.len();
                let needed = Storage::for_palette_len(entry + 1);
                if needed != self.storage {
                    self.grow_storage(needed);
                }
                self.palette.push(block);
                entry
            }
        };

        self.write_index(index, target);
        if current == AIR && block != AIR {
            self.non_air += 1;
        } else if current != AIR && block == AIR {
            self.non_air = self.non_air.saturating_sub(1);
        }
        true
    }

    /// Widens the index storage and rewrites every index at the new width.
    fn grow_storage(&mut self, needed: Storage) {
        let bits = needed.bits();
        let byte_len = (CHUNK_VOLUME * bits).div_ceil(8);
        let mut data = vec![0u8; byte_len];

        let mask: u32 = (1u32 << bits) - 1;
        for i in 0..CHUNK_VOLUME {
            let index = (self.read_index(i) as u32) & mask;
            let bit_pos = i * bits;
            let byte = bit_pos / 8;
            let shift = bit_pos % 8;
            let packed = (index as u16) << shift;
            data[byte] |= packed as u8;
            if shift + bits > 8 {
                data[byte + 1] |= (packed >> 8) as u8;
            }
        }

        self.storage = needed;
        self.data = data;
    }

    /// Number of blocks in this chunk that are not air.
    pub fn non_air_count(&self) -> u32 {
        self.non_air
    }

    /// True if the chunk is entirely air.
    pub fn is_empty(&self) -> bool {
        self.non_air == 0
    }

    /// True if the chunk differs from freshly generated terrain and must be persisted.
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Marks the chunk as player-modified so it will be saved.
    pub fn mark_modified(&mut self) {
        self.modified = true;
    }

    /// Clears the modified flag, e.g. right after a successful save.
    pub fn clear_modified(&mut self) {
        self.modified = false;
    }

    /// The chunk's palette, index 0 is always air.
    pub fn palette(&self) -> &[BlockId] {
        &self.palette
    }

    /// Bytes used by the packed index data, for the memory budget display.
    pub fn storage_bytes(&self) -> usize {
        self.data.len() + self.palette.len() * std::mem::size_of::<BlockId>()
    }

    /// Copies the chunk into a flat array in [`LocalPos::index`] order.
    pub fn to_blocks(&self) -> Vec<BlockId> {
        (0..CHUNK_VOLUME).map(|i| self.palette[self.read_index(i)]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(x: i32, y: i32, z: i32) -> LocalPos {
        LocalPos::new(x, y, z)
    }

    #[test]
    fn solid_chunk_uses_no_index_storage() {
        let chunk = Chunk::filled(7);
        assert_eq!(chunk.storage, Storage::Solid);
        assert_eq!(chunk.storage_bytes(), 2);
        assert_eq!(chunk.get(pos(0, 0, 0)), 7);
        assert_eq!(chunk.get(pos(31, 31, 31)), 7);
        assert_eq!(chunk.non_air_count(), CHUNK_VOLUME as u32);
    }

    #[test]
    fn air_chunk_is_empty() {
        let chunk = Chunk::default();
        assert!(chunk.is_empty());
        assert_eq!(chunk.non_air_count(), 0);
        assert_eq!(chunk.get(pos(5, 5, 5)), AIR);
    }

    #[test]
    fn set_and_get_round_trip_across_storage_widths() {
        let mut chunk = Chunk::filled(AIR);
        // Fill with a pattern that forces 1, then 2, then 3 distinct blocks and therefore
        // walks Solid -> Nibble -> Byte transitions.
        for (n, block) in [(1u16, 5u16), (2, 6), (3, 300), (4, 700), (5, 1000)] {
            let p = pos(n as i32, 1, 1);
            chunk.set(p, block);
            assert_eq!(chunk.get(p), block);
        }
        assert_eq!(chunk.get(pos(1, 1, 1)), 5);
        assert_eq!(chunk.get(pos(5, 1, 1)), 1000);
        assert_eq!(chunk.get(pos(9, 9, 9)), AIR);
    }

    #[test]
    fn palette_grows_past_256_entries() {
        let mut chunk = Chunk::filled(AIR);
        for i in 0..400u16 {
            let p = LocalPos::from_index(i as usize);
            chunk.set(p, i + 10);
        }
        assert_eq!(chunk.storage, Storage::Word);
        for i in 0..400u16 {
            let p = LocalPos::from_index(i as usize);
            assert_eq!(chunk.get(p), i + 10, "block {i} lost during palette growth");
        }
    }

    #[test]
    fn modified_flag_starts_false_and_can_be_set() {
        let mut chunk = Chunk::filled(1);
        assert!(!chunk.is_modified());
        chunk.set(pos(0, 0, 0), 2);
        assert!(!chunk.is_modified(), "set() alone must not mark a chunk as saved-state");
        chunk.mark_modified();
        assert!(chunk.is_modified());
        chunk.clear_modified();
        assert!(!chunk.is_modified());
    }

    #[test]
    fn to_blocks_matches_from_blocks() {
        let mut blocks = vec![AIR; CHUNK_VOLUME];
        for (i, b) in blocks.iter_mut().enumerate() {
            *b = (i % 7) as BlockId;
        }
        let chunk = Chunk::from_blocks(&blocks);
        assert_eq!(chunk.to_blocks(), blocks);
    }

    #[test]
    fn set_to_same_value_reports_no_change() {
        let mut chunk = Chunk::filled(3);
        assert!(!chunk.set(pos(1, 2, 3), 3));
        assert!(chunk.set(pos(1, 2, 3), 4));
        assert!(chunk.set(pos(1, 2, 3), 3));
    }

    #[test]
    fn non_air_count_tracks_edits() {
        let mut chunk = Chunk::default();
        chunk.set(pos(0, 0, 0), 4);
        chunk.set(pos(1, 0, 0), 4);
        assert_eq!(chunk.non_air_count(), 2);
        chunk.set(pos(0, 0, 0), AIR);
        assert_eq!(chunk.non_air_count(), 1);
    }

    #[test]
    fn postcard_round_trip() {
        let mut chunk = Chunk::filled(AIR);
        chunk.set(pos(3, 4, 5), 42);
        chunk.set(pos(6, 7, 8), 65535);
        let bytes = postcard::to_allocvec(&chunk).expect("serialise chunk");
        let back: Chunk = postcard::from_bytes(&bytes).expect("deserialise chunk");
        assert_eq!(chunk, back);
    }
}
