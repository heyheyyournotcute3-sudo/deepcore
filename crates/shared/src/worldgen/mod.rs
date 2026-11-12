//! Deterministic world generation.
//!
//! One seed, one world. The generator fills a chunk from three ingredients:
//!
//! 1. **Layers** (`layers.ron`): which base rock and filler blocks belong at each depth.
//! 2. **Caves**: a 3D noise field carved out of the rock, giving tunnels and caverns in
//!    every layer so players can explore as well as dig.
//! 3. **Ore veins**: per-chunk seeded blobs, placed with the layer's frequency and vein
//!    size, so rare ores are small and sparse and common ores are large and frequent.
//!
//! Plus the surface: a flat camp at the centre of the map, gentle hills around it, trees
//! ringing the camp, and an unbreakable bedrock floor and wall.
//!
//! Everything is a pure function of `(seed, chunk position, content)`. No RNG state is
//! shared between chunks, so chunks can be generated in any order, on any thread, on any
//! machine, and the result is identical.

use crate::content::model::HazardRule;
use crate::content::registry::{LayerEntry, Registry};
use crate::voxel::chunk::{AIR, BlockId, Chunk};
use crate::voxel::coords::{
    CHUNK_SIZE, ChunkPos, LocalPos, WORLD_CHUNKS_X, WORLD_CHUNKS_Y, WORLD_CHUNKS_Z, WORLD_SIZE_X,
    WORLD_SIZE_Y, WORLD_SIZE_Z,
};

pub mod noise;

use self::noise::{Noise2, Noise3, Rng64, hash2, unit_float};

/// Salt constants so different decisions draw from independent random streams.
const SALT_ORES: u64 = 0x0A11_0001;
const SALT_TREES: u64 = 0x0A11_0002;
const SALT_CAVES: u64 = 0x0A11_0003;
const SALT_WATER: u64 = 0x0A11_0004;
const SALT_GAS: u64 = 0x0A11_0005;

/// Well-known block ids the generator needs. Resolved once and cached.
#[derive(Clone, Copy, Debug)]
pub struct GenBlocks {
    /// Air.
    pub air: BlockId,
    /// Bedrock floor and walls.
    pub bedrock: BlockId,
    /// The block used as the surface skin of the camp area.
    pub grass: BlockId,
    /// Tree trunk.
    pub log: BlockId,
    /// Tree canopy.
    pub leaves: BlockId,
    /// Water pocket fill.
    pub water: BlockId,
    /// Methane pocket fill.
    pub methane: BlockId,
}

impl GenBlocks {
    /// Resolves the ids from the registry, falling back to air for anything missing.
    pub fn resolve(registry: &Registry) -> Self {
        let id = |name: &str| registry.block_id(name).unwrap_or(AIR);
        Self {
            air: AIR,
            bedrock: id("bedrock"),
            grass: id("grass"),
            log: id("oak_log"),
            leaves: id("oak_leaves"),
            water: id("water"),
            methane: id("methane"),
        }
    }
}

/// The world generator: seed, content and derived noise fields.
pub struct WorldGen {
    seed: u64,
    registry: Registry,
    blocks: GenBlocks,
    caves: Noise3,
    surface: Noise2,
}

impl WorldGen {
    /// Creates a generator for a seed and a content registry.
    pub fn new(seed: u64, registry: Registry) -> Self {
        let blocks = GenBlocks::resolve(&registry);
        Self {
            seed,
            registry,
            blocks,
            caves: Noise3::new(seed ^ SALT_CAVES),
            surface: Noise2::new(seed ^ 0x5EED_5EED),
        }
    }

    /// The world seed.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The content registry this generator was built from.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// A copy that can be moved onto a worker thread.
    ///
    /// The registry is cloned (it is a few hundred small records) so the task owns everything
    /// it touches; this keeps chunk generation lock-free.
    pub fn clone_for_thread(&self) -> Self {
        Self {
            seed: self.seed,
            registry: self.registry.clone(),
            blocks: self.blocks,
            caves: self.caves,
            surface: self.surface,
        }
    }

    /// Generates one chunk.
    pub fn generate_chunk(&self, pos: ChunkPos) -> Chunk {
        let mut blocks = vec![AIR; crate::voxel::CHUNK_VOLUME];
        let origin = pos.origin();

        // ---- 1. rock layers
        for ly in 0..CHUNK_SIZE {
            let world_y = origin.y + ly;
            let Some(layer_id) = self.registry.layer_at_y(world_y) else {
                continue;
            };
            let Some(layer) = self.registry.layer(layer_id) else {
                continue;
            };
            for lz in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    let world_x = origin.x + lx;
                    let world_z = origin.z + lz;
                    let index = LocalPos::new(lx, ly, lz).index();
                    blocks[index] = self.rock_at(layer, world_x, world_y, world_z);
                }
            }
        }

        // ---- 2. surface shaping (only near the top of the world)
        self.shape_surface(pos, &mut blocks);

        // ---- 3. caves
        self.carve_caves(pos, &mut blocks);

        // ---- 4. bedrock shell
        self.apply_bedrock(pos, &mut blocks);

        // ---- 5. ore veins
        self.place_ores(pos, &mut blocks);

        // ---- 6. hazards: water and gas pockets
        self.place_pockets(pos, &mut blocks);

        // ---- 7. trees, on surface chunks near the camp ring
        self.place_trees(pos, &mut blocks);

        Chunk::from_blocks(&blocks)
    }

    /// The rock block at a world position inside a layer, before caves and ores.
    fn rock_at(&self, layer: &LayerEntry, x: i32, y: i32, z: i32) -> BlockId {
        // Filler blocks are chosen with low-frequency noise so they form patches rather
        // than salt-and-pepper noise.
        let patch = Noise3::new(self.seed ^ 0xF111).sample(x as f32, y as f32, z as f32, 18.0);
        let mut accumulated = 0.0;
        for (block, share) in &layer.filler_blocks {
            accumulated += share;
            if patch < accumulated {
                return *block;
            }
        }
        layer.base_block
    }

    /// The height of the terrain surface at a column.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        let world = self.registry.world();
        let centre = WORLD_SIZE_X as f32 / 2.0;
        let distance = ((x as f32 - centre).powi(2) + (z as f32 - centre).powi(2)).sqrt();
        let hills = self.surface.sample(x as f32, z as f32, 90.0) - 0.5;
        let amplitude = world.surface_amplitude as f32;

        // The camp is flat; the hills fade in outside the camp radius.
        let blend = ((distance - world.camp_radius as f32) / 40.0).clamp(0.0, 1.0);
        let height = world.surface_y as f32 + hills * amplitude * 2.0 * blend;
        height.round() as i32
    }

    fn shape_surface(&self, pos: ChunkPos, blocks: &mut [BlockId]) {
        let origin = pos.origin();
        let world = self.registry.world();
        let centre = WORLD_SIZE_X as f32 / 2.0;
        // Only chunks that can contain the surface need this pass.
        let top_of_chunk = origin.y + CHUNK_SIZE - 1;
        let min_surface = world.surface_y - world.surface_amplitude * 2;
        if top_of_chunk < min_surface {
            return;
        }

        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let x = origin.x + lx;
                let z = origin.z + lz;
                let surface = self.surface_height(x, z);
                let in_camp = ((x as f32 - centre).powi(2) + (z as f32 - centre).powi(2)).sqrt()
                    < world.camp_radius as f32;

                for ly in 0..CHUNK_SIZE {
                    let y = origin.y + ly;
                    if y > surface + 4 {
                        // Well above the surface: sky.
                        let index = LocalPos::new(lx, ly, lz).index();
                        blocks[index] = AIR;
                        continue;
                    }
                    let index = LocalPos::new(lx, ly, lz).index();
                    if y > surface {
                        blocks[index] = AIR;
                    } else if y == surface {
                        // The camp is flat grass; outside it the same grass skin sits on
                        // hills, which keeps the surface readable from a distance.
                        let _ = in_camp;
                        blocks[index] = self.blocks.grass;
                    } else if y >= surface - 3 {
                        // A few blocks of dirt under the grass, then the layer rock resumes.
                        blocks[index] = self.registry.block_id("dirt").unwrap_or(blocks[index]);
                    }
                }
            }
        }
    }

    fn carve_caves(&self, pos: ChunkPos, blocks: &mut [BlockId]) {
        let origin = pos.origin();
        let world = self.registry.world();
        let threshold = world.cave_threshold;
        let scale = world.cave_scale;

        for ly in 0..CHUNK_SIZE {
            let y = origin.y + ly;
            // Never carve the camp floor: the base needs solid ground.
            let depth = self.registry.depth_of(y);
            let layer = self.registry.layer_at_y(y);
            let density = layer
                .and_then(|id| self.registry.layer(id))
                .map(|l| l.def.cave_density)
                .unwrap_or(1.0);
            for lz in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    let index = LocalPos::new(lx, ly, lz).index();
                    if blocks[index] == AIR || blocks[index] == self.blocks.bedrock {
                        continue;
                    }
                    let x = origin.x + lx;
                    let z = origin.z + lz;
                    if depth < 8 {
                        continue;
                    }
                    let value = self.caves.fbm(x as f32, y as f32, z as f32, scale);
                    // Higher cave_density lowers the effective threshold, carving more.
                    let local_threshold = threshold - (density - 1.0) * 0.15;
                    if value > local_threshold {
                        blocks[index] = AIR;
                    }
                }
            }
        }
    }

    fn apply_bedrock(&self, pos: ChunkPos, blocks: &mut [BlockId]) {
        let origin = pos.origin();
        let world = self.registry.world();
        let floor = world.bedrock_y + world.bedrock_thickness;
        let max_x = WORLD_SIZE_X - world.bedrock_thickness;
        let max_z = WORLD_SIZE_Z - world.bedrock_thickness;

        for ly in 0..CHUNK_SIZE {
            let y = origin.y + ly;
            for lz in 0..CHUNK_SIZE {
                let z = origin.z + lz;
                for lx in 0..CHUNK_SIZE {
                    let x = origin.x + lx;
                    if y < floor
                        || x < world.bedrock_thickness
                        || x >= max_x
                        || z < world.bedrock_thickness
                        || z >= max_z
                    {
                        blocks[LocalPos::new(lx, ly, lz).index()] = self.blocks.bedrock;
                    }
                }
            }
        }
    }

    fn place_ores(&self, pos: ChunkPos, blocks: &mut [BlockId]) {
        let origin = pos.origin();
        let mut rng = Rng64::for_chunk(self.seed, pos, SALT_ORES);

        for ly in 0..CHUNK_SIZE {
            let y = origin.y + ly;
            let Some(layer_id) = self.registry.layer_at_y(y) else {
                continue;
            };
            let Some(layer) = self.registry.layer(layer_id) else {
                continue;
            };
            let depth = self.registry.depth_of(y);

            for ore in &layer.ores {
                if let Some((low, high)) = ore.depth_band
                    && (depth < low || depth > high)
                {
                    continue;
                }
                // Frequency is per chunk; roll once per ore per chunk.
                if !rng.chance(ore.frequency) {
                    continue;
                }
                let size = rng.next_range(ore.vein_size.0 as i32, ore.vein_size.1 as i32);
                // Pick a seed point inside this chunk and grow a blob around it.
                let sx = origin.x + rng.next_range(0, CHUNK_SIZE - 1);
                let sy = origin.y + ly;
                let sz = origin.z + rng.next_range(0, CHUNK_SIZE - 1);
                self.grow_vein(
                    blocks,
                    VeinSeed { chunk: pos, seed: (sx, sy, sz), size, ore: ore.block },
                );
            }
        }
    }

    /// Grows a roughly spherical ore blob of `size` blocks around a seed point, clipped to
    /// the chunk being generated.
    fn grow_vein(&self, blocks: &mut [BlockId], vein: VeinSeed) {
        let VeinSeed { chunk, seed: (sx, sy, sz), size, ore } = vein;
        let ore_scale = self.registry.world().ore_scale;
        // Radius that yields roughly `size` blocks in a sphere.
        let radius = ((size as f32 * 3.0 / (4.0 * std::f32::consts::PI)).cbrt() * 1.35).max(1.0);
        let r = radius.ceil() as i32;
        let origin = chunk.origin();

        for dy in -r..=r {
            for dz in -r..=r {
                for dx in -r..=r {
                    let x = sx + dx;
                    let y = sy + dy;
                    let z = sz + dz;
                    // Chunk-local bounds: veins never write outside their own chunk.
                    let (lx, ly, lz) = (x - origin.x, y - origin.y, z - origin.z);
                    if !in_chunk_bounds(lx, ly, lz) {
                        continue;
                    }
                    // Wobbly sphere: perturb the radius with noise so veins are not balls.
                    let jitter = self.caves.sample(
                        x as f32 * 3.0,
                        y as f32 * 3.0,
                        z as f32 * 3.0,
                        ore_scale * 0.5,
                    );
                    let effective = radius * (0.65 + jitter * 0.7);
                    let distance = ((dx * dx + dy * dy + dz * dz) as f32).sqrt();
                    if distance > effective {
                        continue;
                    }
                    let index = LocalPos::new(lx, ly, lz).index();
                    if blocks[index] == AIR || blocks[index] == self.blocks.bedrock {
                        continue;
                    }
                    blocks[index] = ore;
                }
            }
        }
    }

    fn place_pockets(&self, pos: ChunkPos, blocks: &mut [BlockId]) {
        let origin = pos.origin();
        let mut rng = Rng64::for_chunk(self.seed, pos, SALT_WATER);
        let mut gas_rng = Rng64::for_chunk(self.seed, pos, SALT_GAS);

        for ly in 0..CHUNK_SIZE {
            let y = origin.y + ly;
            let Some(layer_id) = self.registry.layer_at_y(y) else {
                continue;
            };
            let Some(layer) = self.registry.layer(layer_id) else {
                continue;
            };

            for hazard in &layer.def.hazards {
                let (chance, volume, fill) = match hazard {
                    HazardRule::Flooding { pocket_chance, max_volume } => {
                        (*pocket_chance, *max_volume, self.blocks.water)
                    }
                    HazardRule::Gas { pocket_chance, volume } => {
                        (*pocket_chance, *volume, self.blocks.methane)
                    }
                    _ => continue,
                };
                let rng = match hazard {
                    HazardRule::Flooding { .. } => &mut rng,
                    _ => &mut gas_rng,
                };
                if !rng.chance(chance) {
                    continue;
                }
                let sx = origin.x + rng.next_range(1, CHUNK_SIZE - 2);
                let sy = y;
                let sz = origin.z + rng.next_range(1, CHUNK_SIZE - 2);
                let radius = ((volume as f32 * 3.0 / (4.0 * std::f32::consts::PI)).cbrt()).max(1.0);
                let r = radius.ceil() as i32;
                for dy in -r..=r {
                    for dz in -r..=r {
                        for dx in -r..=r {
                            let distance = ((dx * dx + dy * dy + dz * dz) as f32).sqrt();
                            if distance > radius {
                                continue;
                            }
                            let (lx, ly2, lz) =
                                (sx + dx - origin.x, sy + dy - origin.y, sz + dz - origin.z);
                            if !in_chunk_bounds(lx, ly2, lz) {
                                continue;
                            }
                            let index = LocalPos::new(lx, ly2, lz).index();
                            if blocks[index] != AIR && blocks[index] != self.blocks.bedrock {
                                blocks[index] = fill;
                            }
                        }
                    }
                }
            }
        }
    }

    fn place_trees(&self, pos: ChunkPos, blocks: &mut [BlockId]) {
        let origin = pos.origin();
        let world = self.registry.world();
        let centre = WORLD_SIZE_X as f32 / 2.0;

        for ly in 0..CHUNK_SIZE {
            let y = origin.y + ly;
            for lz in 0..CHUNK_SIZE {
                let z = origin.z + lz;
                for lx in 0..CHUNK_SIZE {
                    let x = origin.x + lx;
                    let index = LocalPos::new(lx, ly, lz).index();
                    if blocks[index] != self.blocks.grass {
                        continue;
                    }
                    let surface = self.surface_height(x, z);
                    if y != surface {
                        continue;
                    }
                    let distance =
                        ((x as f32 - centre).powi(2) + (z as f32 - centre).powi(2)).sqrt();
                    // Trees ring the camp: outside the flat area, inside a wider ring.
                    if distance < world.camp_radius as f32 + 2.0
                        || distance > world.camp_radius as f32 + 90.0
                    {
                        continue;
                    }
                    // Deterministic per-column tree chance, so trees do not depend on chunk order.
                    let roll = unit_float(hash2(self.seed ^ SALT_TREES, x, z));
                    if roll > 0.012 {
                        continue;
                    }
                    self.build_tree(blocks, pos, lx, ly, lz);
                }
            }
        }
    }

    /// Places a small tree with its trunk base at the given chunk-local position.
    fn build_tree(&self, blocks: &mut [BlockId], pos: ChunkPos, lx: i32, ly: i32, lz: i32) {
        let origin = pos.origin();
        let trunk_height = 4 + (hash2(self.seed, origin.x + lx, origin.z + lz) % 3) as i32;
        let set = |blocks: &mut [BlockId],
                   wx: i32,
                   wy: i32,
                   wz: i32,
                   block: BlockId,
                   replace_air_only: bool| {
            let (cx, cy, cz) = (wx - origin.x, wy - origin.y, wz - origin.z);
            if !in_chunk_bounds(cx, cy, cz) {
                return;
            }
            let index = LocalPos::new(cx, cy, cz).index();
            if replace_air_only && blocks[index] != AIR {
                return;
            }
            blocks[index] = block;
        };

        let world_x = origin.x + lx;
        let world_z = origin.z + lz;
        let base_y = origin.y + ly;

        for dy in 1..=trunk_height {
            set(blocks, world_x, base_y + dy, world_z, self.blocks.log, true);
        }
        let canopy_y = base_y + trunk_height;
        for dy in -1..=1 {
            let radius: i32 = if dy == 1 { 1 } else { 2 };
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs() == radius && dz.abs() == radius && dy == 1 {
                        continue;
                    }
                    set(
                        blocks,
                        world_x + dx,
                        canopy_y + dy,
                        world_z + dz,
                        self.blocks.leaves,
                        true,
                    );
                }
            }
        }
    }

    /// The block a fresh, untouched world has at a position. Used to tell whether a chunk
    /// has been modified by players and therefore needs saving.
    pub fn is_inside_world(x: i32, y: i32, z: i32) -> bool {
        (0..WORLD_SIZE_X).contains(&x)
            && (0..WORLD_SIZE_Y).contains(&y)
            && (0..WORLD_SIZE_Z).contains(&z)
    }

    /// Total number of chunks in the world.
    pub fn chunk_count() -> usize {
        (WORLD_CHUNKS_X * WORLD_CHUNKS_Y * WORLD_CHUNKS_Z) as usize
    }
}

/// A single ore vein waiting to be grown.
struct VeinSeed {
    chunk: ChunkPos,
    seed: (i32, i32, i32),
    size: i32,
    ore: BlockId,
}

/// True if a chunk-local coordinate is inside the chunk.
fn in_chunk_bounds(x: i32, y: i32, z: i32) -> bool {
    (0..CHUNK_SIZE).contains(&x) && (0..CHUNK_SIZE).contains(&y) && (0..CHUNK_SIZE).contains(&z)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::content::load_registry;

    fn test_generator() -> WorldGen {
        WorldGen::new(0xDEAD_BEEF, load_registry().expect("content"))
    }

    #[test]
    fn generation_is_deterministic() {
        let a = test_generator();
        let b = test_generator();
        for chunk in [
            ChunkPos::new(0, 14, 0),
            ChunkPos::new(16, 12, 16),
            ChunkPos::new(31, 0, 31),
            ChunkPos::new(5, 9, 27),
        ] {
            let ca = a.generate_chunk(chunk);
            let cb = b.generate_chunk(chunk);
            assert_eq!(ca.to_blocks(), cb.to_blocks(), "chunk {chunk:?} differs between runs");
        }
    }

    #[test]
    fn different_seeds_give_different_worlds() {
        let a = WorldGen::new(1, load_registry().expect("content"));
        let b = WorldGen::new(2, load_registry().expect("content"));
        let chunk = ChunkPos::new(10, 10, 10);
        assert_ne!(a.generate_chunk(chunk).to_blocks(), b.generate_chunk(chunk).to_blocks());
    }

    #[test]
    fn chunk_generation_order_does_not_matter() {
        let generator = test_generator();
        let first = generator.generate_chunk(ChunkPos::new(4, 12, 9));
        // Generating other chunks in between must not change the result.
        let _ = generator.generate_chunk(ChunkPos::new(5, 12, 9));
        let _ = generator.generate_chunk(ChunkPos::new(0, 0, 0));
        let again = generator.generate_chunk(ChunkPos::new(4, 12, 9));
        assert_eq!(first.to_blocks(), again.to_blocks());
    }

    #[test]
    fn bedrock_floor_is_solid_and_air_is_above_surface() {
        let generator = test_generator();
        let bedrock = generator.blocks.bedrock;
        let thickness = generator.registry.world().bedrock_thickness;
        let floor = generator.generate_chunk(ChunkPos::new(8, 0, 8));
        for y in 0..thickness {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    assert_eq!(
                        floor.get(LocalPos::new(x, y, z)),
                        bedrock,
                        "bedrock floor missing at ({x},{y},{z})"
                    );
                }
            }
        }

        // The topmost chunk (y 480..512) sits above surface_y = 480, so it is sky apart
        // from the tallest hills and their trees.
        let sky = generator.generate_chunk(ChunkPos::new(8, WORLD_CHUNKS_Y - 1, 8));
        let solid = sky.to_blocks().iter().filter(|&&b| b != AIR).count();
        assert!(solid < 6000, "top chunk should be mostly sky, had {solid} solid blocks");
    }

    #[test]
    fn camp_area_is_flat_and_walkable() {
        let generator = test_generator();
        let spawn = generator.registry.world().spawn;
        let surface = generator.surface_height(spawn.0, spawn.2);
        for (dx, dz) in [(0, 0), (10, 0), (0, 10), (-12, 5), (20, 20)] {
            let height = generator.surface_height(spawn.0 + dx, spawn.2 + dz);
            assert_eq!(height, surface, "camp should be flat at offset ({dx},{dz})");
        }
        // Hills exist outside the camp.
        let mut varied = false;
        for i in 0..200 {
            let height = generator.surface_height(100 + i * 3, 100);
            if height != surface {
                varied = true;
                break;
            }
        }
        assert!(varied, "terrain outside the camp should vary");
    }

    #[test]
    fn layers_contain_their_base_rock_at_depth() {
        let generator = test_generator();
        let registry = &generator.registry;
        // Chunk Y for each layer, derived from surface_y = 480 and 32-block chunks:
        // topsoil 464..480, shale 416..464, granite 320..416, basalt 192..320,
        // gneiss 32..192, mantle 0..32.
        for (chunk_y, layer_id) in [(14, 0), (13, 1), (11, 2), (8, 3), (4, 4), (0, 5)] {
            let layer = registry.layer(crate::content::registry::LayerId(layer_id)).expect("layer");
            let chunk = generator.generate_chunk(ChunkPos::new(16, chunk_y, 16));
            let blocks = chunk.to_blocks();
            let has_base_or_filler = blocks.iter().any(|&b| {
                b == layer.base_block || layer.filler_blocks.iter().any(|(f, _)| *f == b)
            });
            assert!(has_base_or_filler, "layer {layer_id} chunk has none of its own rock");
        }
    }

    #[test]
    fn ores_appear_in_their_layer() {
        let generator = test_generator();
        let registry = &generator.registry;
        let pitchblende = registry.block_id("pitchblende").expect("pitchblende");
        // Deep Crust is depth 288..448 -> y 192..32 (surface 480). Scan several chunks.
        let mut found = false;
        for cy in 4..7 {
            for cx in 0..6 {
                let blocks = generator.generate_chunk(ChunkPos::new(cx * 3, cy, 7)).to_blocks();
                if blocks.contains(&pitchblende) {
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "uranium should generate in the Deep Crust");
    }

    #[test]
    fn caves_carve_air_underground() {
        let generator = test_generator();
        let mut air_underground = 0;
        for cx in 0..4 {
            let blocks = generator.generate_chunk(ChunkPos::new(cx, 9, 3)).to_blocks();
            air_underground += blocks.iter().filter(|&&b| b == AIR).count();
        }
        assert!(air_underground > 1000, "expected caves, found only {air_underground} air blocks");
    }

    #[test]
    fn trees_ring_the_camp() {
        let generator = test_generator();
        let log = generator.blocks.log;
        let mut logs = 0;
        // The camp centre is at 512,512; the tree ring is around it.
        for angle_step in 0..40 {
            let angle = angle_step as f32 * 0.157;
            let x = (512.0 + angle.cos() * 45.0) as i32;
            let z = (512.0 + angle.sin() * 45.0) as i32;
            let chunk = ChunkPos::new(
                x / CHUNK_SIZE,
                (generator.surface_height(x, z) + 2) / CHUNK_SIZE,
                z / CHUNK_SIZE,
            );
            logs +=
                generator.generate_chunk(chunk).to_blocks().iter().filter(|&&b| b == log).count();
        }
        assert!(logs > 0, "expected trees around the camp");
    }

    #[test]
    fn generation_is_fast_enough_for_a_worker_thread() {
        let generator = test_generator();
        let start = std::time::Instant::now();
        for i in 0..8 {
            let _ = generator.generate_chunk(ChunkPos::new(i, 8, i));
        }
        let per_chunk = start.elapsed().as_secs_f64() / 8.0;
        assert!(per_chunk < 0.05, "chunk generation took {per_chunk:.3}s, budget is 5ms");
    }
}
