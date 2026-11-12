//! Procedural texture atlas generation.
//!
//! Deepcore ships no art. Every block texture is generated at startup from its `color`,
//! `pattern` and `emissive` fields in `blocks.ron`, packed into a single atlas image, and
//! sampled by the chunk shader with `floor(tile)`.
//!
//! The generator is deterministic: the same content produces the same atlas on every machine
//! (it hashes the block name for its speckle pattern rather than using any RNG state), so the
//! client and the server agree about what a block looks like.

use crate::content::model::Pattern;
use crate::content::registry::Registry;
use crate::worldgen::noise::hash2;

/// Size of one texture tile in pixels.
pub const TILE_SIZE: u32 = 16;
/// Number of tiles across the atlas.
pub const ATLAS_COLUMNS: u32 = 16;
/// Number of tiles down the atlas.
pub const ATLAS_ROWS: u32 = 16;
/// Atlas width in pixels.
pub const ATLAS_WIDTH: u32 = ATLAS_COLUMNS * TILE_SIZE;
/// Atlas height in pixels.
pub const ATLAS_HEIGHT: u32 = ATLAS_ROWS * TILE_SIZE;
/// Maximum number of blocks the atlas can hold.
pub const ATLAS_CAPACITY: u32 = ATLAS_COLUMNS * ATLAS_ROWS;

/// A generated RGBA atlas plus the tile index of every block.
pub struct TextureAtlas {
    /// RGBA8 pixels, row-major, `ATLAS_WIDTH * ATLAS_HEIGHT * 4` bytes.
    pub pixels: Vec<u8>,
    /// Atlas width.
    pub width: u32,
    /// Atlas height.
    pub height: u32,
    /// Tile index per block id.
    pub tiles: Vec<u32>,
    /// Emissive strength per block id, for the shader's glow term.
    pub emissive: Vec<f32>,
}

impl TextureAtlas {
    /// The atlas tile index for a block id.
    pub fn tile_of(&self, block: crate::voxel::BlockId) -> u32 {
        self.tiles.get(block as usize).copied().unwrap_or(0)
    }

    /// The emissive strength for a block id.
    pub fn emissive_of(&self, block: crate::voxel::BlockId) -> f32 {
        self.emissive.get(block as usize).copied().unwrap_or(0.0)
    }

    /// The pixel rectangle of a tile: `(x, y, size)`.
    pub fn tile_rect(tile: u32) -> (u32, u32, u32) {
        let column = tile % ATLAS_COLUMNS;
        let row = tile / ATLAS_COLUMNS;
        (column * TILE_SIZE, row * TILE_SIZE, TILE_SIZE)
    }
}

/// Generates the atlas for a content registry.
pub fn generate_atlas(registry: &Registry) -> TextureAtlas {
    let mut pixels = vec![0u8; (ATLAS_WIDTH * ATLAS_HEIGHT * 4) as usize];
    let mut tiles = Vec::with_capacity(registry.block_count());
    let mut emissive = Vec::with_capacity(registry.block_count());

    for (index, block) in registry.blocks().iter().enumerate() {
        let tile = index as u32 % ATLAS_CAPACITY;
        tiles.push(tile);
        emissive.push(block.def.emissive);

        if block.def.id == "air" {
            // Air is transparent; the mesher never emits faces for it anyway.
            continue;
        }

        let (origin_x, origin_y, _) = TextureAtlas::tile_rect(tile);
        let color = block.def.color;
        let seed = block_seed(&block.def.id);
        for py in 0..TILE_SIZE {
            for px in 0..TILE_SIZE {
                let rgb = pixel_color(&block.def.pattern, color, block.def.emissive, seed, px, py);
                let offset = (((origin_y + py) * ATLAS_WIDTH + origin_x + px) * 4) as usize;
                pixels[offset] = rgb[0];
                pixels[offset + 1] = rgb[1];
                pixels[offset + 2] = rgb[2];
                pixels[offset + 3] = 255;
            }
        }
    }

    TextureAtlas { pixels, width: ATLAS_WIDTH, height: ATLAS_HEIGHT, tiles, emissive }
}

/// A stable per-block seed derived from its name.
fn block_seed(id: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Converts a linear RGB triple into sRGB bytes.
fn to_srgb_bytes(rgb: (f32, f32, f32)) -> [u8; 3] {
    let encode = |value: f32| -> u8 {
        let v = value.clamp(0.0, 1.0);
        let srgb = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (srgb * 255.0).round() as u8
    };
    [encode(rgb.0), encode(rgb.1), encode(rgb.2)]
}

/// Computes one pixel of a tile for a pattern.
fn pixel_color(
    pattern: &Pattern,
    base: (f32, f32, f32),
    emissive: f32,
    seed: u64,
    x: u32,
    y: u32,
) -> [u8; 3] {
    let noise = hash_noise(seed, x, y);
    let mut color = base;

    match pattern {
        Pattern::Flat => {}
        Pattern::Speckle => {
            // Fine per-pixel variation, plus a slightly darker border so tiles read apart.
            let shade = 0.85 + noise * 0.3;
            color = (base.0 * shade, base.1 * shade, base.2 * shade);
            if x == 0 || y == 0 || x == TILE_SIZE - 1 || y == TILE_SIZE - 1 {
                color = (color.0 * 0.82, color.1 * 0.82, color.2 * 0.82);
            }
        }
        Pattern::Blotch => {
            // Coarse 4x4 blobs.
            let blotch = hash_noise(seed ^ 0xB10C, x / 4, y / 4);
            let shade = 0.78 + blotch * 0.44;
            color = (base.0 * shade, base.1 * shade, base.2 * shade);
        }
        Pattern::OreFleck(fleck) => {
            // Dark rock with bright ore flecks.
            let shade = 0.8 + noise * 0.25;
            color = (base.0 * shade, base.1 * shade, base.2 * shade);
            // Flecks: clustered bright pixels.
            let cluster = hash_noise(seed ^ 0xF1EC, x / 3, y / 3);
            if cluster > 0.62 && noise > 0.35 {
                let mix = 0.55 + cluster * 0.45;
                color = (
                    color.0 * (1.0 - mix) + fleck.0 * mix,
                    color.1 * (1.0 - mix) + fleck.1 * mix,
                    color.2 * (1.0 - mix) + fleck.2 * mix,
                );
            }
        }
        Pattern::Stratified => {
            // Horizontal strata with slight wobble.
            let band = hash_noise(seed ^ 0x57AA_u64, 0, y / 2);
            let shade = 0.82 + band * 0.32 + noise * 0.06;
            color = (base.0 * shade, base.1 * shade, base.2 * shade);
        }
        Pattern::Glow => {
            // Bright centre fading to the edges, plus emissive lift.
            let cx = x as f32 - (TILE_SIZE as f32 - 1.0) / 2.0;
            let cy = y as f32 - (TILE_SIZE as f32 - 1.0) / 2.0;
            let distance = (cx * cx + cy * cy).sqrt() / (TILE_SIZE as f32 * 0.7);
            let lift = (1.0 - distance).clamp(0.0, 1.0) * (0.5 + emissive * 0.8);
            color = (
                (base.0 + lift * 0.6).min(1.0),
                (base.1 + lift * 0.6).min(1.0),
                (base.2 + lift * 0.6).min(1.0),
            );
        }
        Pattern::Grain => {
            // Vertical wood grain.
            let column = hash_noise(seed ^ 0x6AA1_u64, x, 0);
            let shade = 0.8 + column * 0.3 + noise * 0.08;
            color = (base.0 * shade, base.1 * shade, base.2 * shade);
        }
    }

    // Emissive blocks are lifted a little in the base texture too, so they read as glowing
    // even before the shader adds its own term.
    if emissive > 0.0 {
        let lift = emissive * 0.25;
        color = ((color.0 + lift).min(1.0), (color.1 + lift).min(1.0), (color.2 + lift).min(1.0));
    }

    to_srgb_bytes(color)
}

/// A stable per-pixel noise value in `0..1`.
fn hash_noise(seed: u64, x: u32, y: u32) -> f32 {
    crate::worldgen::noise::unit_float(hash2(seed, x as i32, y as i32))
}

/// Writes the atlas as a PNG byte stream. Used by `tools/atlas_gen`.
pub fn encode_png(atlas: &TextureAtlas) -> Result<Vec<u8>, String> {
    // Minimal PNG encoder: one IHDR, one IDAT (zlib stored blocks), one IEND. Writing our own
    // keeps the tool dependency-free, and the atlas is tiny (256x256).
    let mut raw = Vec::with_capacity((atlas.width * atlas.height * 4 + atlas.height) as usize);
    for y in 0..atlas.height {
        raw.push(0); // filter type: none
        let start = (y * atlas.width * 4) as usize;
        raw.extend_from_slice(&atlas.pixels[start..start + (atlas.width * 4) as usize]);
    }

    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&atlas.width.to_be_bytes());
    ihdr.extend_from_slice(&atlas.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, deflate, no filter, no interlace
    push_chunk(&mut png, b"IHDR", &ihdr);

    let zlib = zlib_stored(&raw);
    push_chunk(&mut png, b"IDAT", &zlib);
    push_chunk(&mut png, b"IEND", &[]);
    Ok(png)
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// Wraps data in a zlib stream made of uncompressed deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut remaining = data;
    loop {
        let chunk_len = remaining.len().min(65_535);
        let final_block = chunk_len == remaining.len();
        out.push(if final_block { 1 } else { 0 });
        let len = chunk_len as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(&remaining[..chunk_len]);
        remaining = &remaining[chunk_len..];
        if final_block {
            break;
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::load_registry;

    #[test]
    fn atlas_has_a_tile_for_every_block() {
        let registry = load_registry().expect("content");
        let atlas = generate_atlas(&registry);
        assert_eq!(atlas.tiles.len(), registry.block_count());
        assert_eq!(atlas.pixels.len(), (ATLAS_WIDTH * ATLAS_HEIGHT * 4) as usize);
        // Every block gets a distinct tile.
        let unique: std::collections::HashSet<u32> = atlas.tiles.iter().copied().collect();
        assert_eq!(unique.len(), registry.block_count());
    }

    #[test]
    fn atlas_is_deterministic() {
        let registry = load_registry().expect("content");
        let a = generate_atlas(&registry);
        let b = generate_atlas(&registry);
        assert_eq!(a.pixels, b.pixels);
    }

    #[test]
    fn generated_tiles_are_not_uniform() {
        let registry = load_registry().expect("content");
        let atlas = generate_atlas(&registry);
        let stone = registry.block_id("stone").expect("stone");
        let (x, y, size) = TextureAtlas::tile_rect(atlas.tile_of(stone));
        let mut colors = std::collections::HashSet::new();
        for py in 0..size {
            for px in 0..size {
                let offset = (((y + py) * ATLAS_WIDTH + x + px) * 4) as usize;
                colors.insert((
                    atlas.pixels[offset],
                    atlas.pixels[offset + 1],
                    atlas.pixels[offset + 2],
                ));
            }
        }
        assert!(
            colors.len() > 4,
            "stone texture should have speckle, had {} colours",
            colors.len()
        );
    }

    #[test]
    fn uranium_tile_is_green_and_emissive() {
        let registry = load_registry().expect("content");
        let atlas = generate_atlas(&registry);
        let uranium = registry.block_id("pitchblende").expect("pitchblende");
        assert!(atlas.emissive_of(uranium) > 0.0);
        let (x, y, _) = TextureAtlas::tile_rect(atlas.tile_of(uranium));
        let mut greenest = 0i32;
        for py in 0..TILE_SIZE {
            for px in 0..TILE_SIZE {
                let offset = (((y + py) * ATLAS_WIDTH + x + px) * 4) as usize;
                let green = atlas.pixels[offset + 1] as i32 - atlas.pixels[offset] as i32;
                greenest = greenest.max(green);
            }
        }
        assert!(greenest > 30, "pitchblende should have green flecks, best delta {greenest}");
    }

    #[test]
    fn png_encoder_produces_a_valid_header() {
        let registry = load_registry().expect("content");
        let atlas = generate_atlas(&registry);
        let png = encode_png(&atlas).expect("encode");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..20], &ATLAS_WIDTH.to_be_bytes());
        assert_eq!(&png[20..24], &ATLAS_HEIGHT.to_be_bytes());
    }

    #[test]
    fn crc32_matches_known_value() {
        // The CRC of "IEND" with empty data, from the PNG specification.
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }
}
