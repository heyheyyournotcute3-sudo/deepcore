//! Greedy chunk meshing.
//!
//! A 32³ chunk has 32 768 blocks. Emitting one quad per visible face costs up to 196 608
//! quads per chunk; greedy meshing merges coplanar faces of the same block into rectangles
//! and typically cuts that by 10-50x for solid rock. The output is a plain vertex/index
//! buffer with no Bevy types, so it can be produced on a worker thread and tested headlessly.
//!
//! Face culling works across chunk borders: the mesher is handed a `get` closure that reads
//! the *world*, so a face is only emitted when the neighbouring block is transparent — even
//! when that neighbour lives in the next chunk.

use crate::voxel::coords::{CHUNK_SIZE, LocalPos};

/// One vertex of a voxel face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelVertex {
    /// Position in chunk-local space (0..32 per axis).
    pub position: [f32; 3],
    /// Face normal.
    pub normal: [f32; 3],
    /// Tile-local texture coordinates. `uv` holds the fraction inside one atlas tile in
    /// `0..1`; `uv_b` holds how many tiles the (possibly greedy-merged) quad spans, which
    /// the voxel shader uses to repeat the tile across the quad.
    pub uv: [f32; 2],
    /// Number of atlas tiles the quad spans in each direction.
    pub uv_b: [f32; 2],
    /// Atlas tile index, passed to the shader to compute the final UV.
    pub tile: f32,
    /// Block id, used for ambient occlusion and future shader effects.
    pub block: f32,
}

/// A mesh for one chunk.
#[derive(Clone, Debug, Default)]
pub struct ChunkMeshData {
    /// Vertices, 4 per quad.
    pub vertices: Vec<VoxelVertex>,
    /// Triangle indices, 6 per quad.
    pub indices: Vec<u32>,
    /// Number of quads emitted, for diagnostics.
    pub quad_count: usize,
}

impl ChunkMeshData {
    /// True if nothing was emitted (an empty chunk).
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Approximate memory footprint in bytes.
    pub fn byte_size(&self) -> usize {
        self.vertices.len() * std::mem::size_of::<VoxelVertex>()
            + self.indices.len() * std::mem::size_of::<u32>()
    }
}

/// Face directions, in the order the mesher walks them.
const FACE_NORMALS: [[i32; 3]; 6] = [
    [1, 0, 0],  // +X
    [-1, 0, 0], // -X
    [0, 1, 0],  // +Y
    [0, -1, 0], // -Y
    [0, 0, 1],  // +Z
    [0, 0, -1], // -Z
];

/// Builds the mesh for one chunk.
///
/// * `block_at` reads a block in chunk-local coordinates, where coordinates outside
///   `0..CHUNK_SIZE` are resolved in the neighbouring chunk (this is what makes border
///   faces cull correctly).
/// * `is_opaque` decides whether a block hides the face behind it.
/// * `tile_of` maps a block id to its atlas tile index.
pub fn mesh_chunk(
    block_at: impl Fn(i32, i32, i32) -> crate::voxel::BlockId,
    is_opaque: impl Fn(crate::voxel::BlockId) -> bool,
    tile_of: impl Fn(crate::voxel::BlockId) -> u32,
) -> ChunkMeshData {
    let mut mesh = ChunkMeshData::default();

    // For each of the six directions, sweep the chunk with a 2D mask over each slice and
    // greedily merge equal, visible faces into rectangles.
    for (face_index, normal) in FACE_NORMALS.iter().enumerate() {
        // The axis the faces point along, and the two axes spanning the face plane.
        let axis = if normal[0] != 0 {
            0
        } else if normal[1] != 0 {
            1
        } else {
            2
        };
        let (u_axis, v_axis) = match axis {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        let positive = normal[axis] > 0;

        for slice in 0..CHUNK_SIZE {
            // Mask of (block, tile) for faces visible at this slice, or None if hidden.
            let mut mask: Vec<Option<(crate::voxel::BlockId, u32)>> =
                vec![None; (CHUNK_SIZE * CHUNK_SIZE) as usize];

            for v in 0..CHUNK_SIZE {
                for u in 0..CHUNK_SIZE {
                    let mut pos = [0i32; 3];
                    pos[axis] = slice;
                    pos[u_axis] = u;
                    pos[v_axis] = v;
                    let block = block_at(pos[0], pos[1], pos[2]);
                    if !is_opaque(block) {
                        continue;
                    }
                    // The neighbour in the face direction: if it is opaque too, no face here.
                    let mut neighbour = pos;
                    neighbour[axis] += if positive { 1 } else { -1 };
                    if is_opaque(block_at(neighbour[0], neighbour[1], neighbour[2])) {
                        continue;
                    }
                    mask[(v * CHUNK_SIZE + u) as usize] = Some((block, tile_of(block)));
                }
            }

            // Greedy merge over the mask.
            let mut v = 0;
            while v < CHUNK_SIZE {
                let mut u = 0;
                while u < CHUNK_SIZE {
                    let Some(entry) = mask[(v * CHUNK_SIZE + u) as usize] else {
                        u += 1;
                        continue;
                    };
                    // Extend along u.
                    let mut width = 1;
                    while u + width < CHUNK_SIZE
                        && mask[(v * CHUNK_SIZE + u + width) as usize] == Some(entry)
                    {
                        width += 1;
                    }
                    // Extend along v while the whole row matches.
                    let mut height = 1;
                    'grow: while v + height < CHUNK_SIZE {
                        for du in 0..width {
                            if mask[((v + height) * CHUNK_SIZE + u + du) as usize] != Some(entry) {
                                break 'grow;
                            }
                        }
                        height += 1;
                    }

                    emit_quad(
                        &mut mesh, face_index, axis, u_axis, v_axis, slice, u, v, width, height,
                        positive, entry.0, entry.1,
                    );

                    // Clear the consumed cells.
                    for dv in 0..height {
                        for du in 0..width {
                            mask[((v + dv) * CHUNK_SIZE + u + du) as usize] = None;
                        }
                    }
                    u += width;
                }
                v += 1;
            }
        }
    }

    mesh
}

#[allow(clippy::too_many_arguments)]
fn emit_quad(
    mesh: &mut ChunkMeshData,
    face_index: usize,
    axis: usize,
    u_axis: usize,
    v_axis: usize,
    slice: i32,
    u: i32,
    v: i32,
    width: i32,
    height: i32,
    positive: bool,
    block: crate::voxel::BlockId,
    tile: u32,
) {
    let normal = FACE_NORMALS[face_index];
    // The face plane sits on the boundary of the block cell in the normal direction.
    let plane = if positive { slice as f32 + 1.0 } else { slice as f32 };

    let corner = |du: f32, dv: f32| -> [f32; 3] {
        let mut position = [0.0f32; 3];
        position[axis] = plane;
        position[u_axis] = u as f32 + du * width as f32;
        position[v_axis] = v as f32 + dv * height as f32;
        position
    };

    let base = mesh.vertices.len() as u32;
    // Texture coordinates are per block, not per merged quad, so a merged face still shows
    // the right number of tile repeats.
    let uv_scale = [width as f32, height as f32];

    let corners = [
        (corner(0.0, 0.0), [0.0, 0.0]),
        (corner(1.0, 0.0), [uv_scale[0], 0.0]),
        (corner(1.0, 1.0), [uv_scale[0], uv_scale[1]]),
        (corner(0.0, 1.0), [0.0, uv_scale[1]]),
    ];

    for (position, uv) in corners {
        mesh.vertices.push(VoxelVertex {
            position,
            normal: [normal[0] as f32, normal[1] as f32, normal[2] as f32],
            // Keep the tile-local coordinate in 0..1 so the shader can clamp to the tile,
            // and carry the tiling factor separately.
            uv: [uv[0] / uv_scale[0], uv[1] / uv_scale[1]],
            uv_b: uv_scale,
            tile: tile as f32,
            block: block as f32,
        });
    }

    // Wind the triangles so the face points outward, matching the normal.
    let flip = should_flip(axis, positive);
    if flip {
        mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    } else {
        mesh.indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
    }
    mesh.quad_count += 1;
}

/// Chooses triangle winding so that, in a right-handed Y-up coordinate system with clockwise
/// front faces (Bevy's default), every face is visible from outside the chunk.
fn should_flip(axis: usize, positive: bool) -> bool {
    match axis {
        // X faces: the +X face needs the opposite winding from the -X face.
        0 => !positive,
        1 => positive,
        _ => !positive,
    }
}

/// Maps a chunk-local coordinate to the block id in the world, resolving neighbours.
pub fn neighbour_reader(
    chunk_origin: crate::voxel::BlockPos,
    world: &crate::voxel::VoxelWorld,
) -> impl Fn(i32, i32, i32) -> crate::voxel::BlockId + '_ {
    move |x, y, z| {
        // Coordinates outside the chunk map onto the neighbouring chunk's blocks, which is
        // exactly what `BlockPos::in_world` + `VoxelWorld::block` already do.
        world.block(chunk_origin.offset(x, y, z))
    }
}

/// A stand-alone reader over one chunk, treating everything outside it as air. Useful for
/// tests and for meshing a chunk whose neighbours have not arrived yet.
pub fn chunk_reader(
    chunk: &crate::voxel::Chunk,
) -> impl Fn(i32, i32, i32) -> crate::voxel::BlockId + '_ {
    move |x, y, z| {
        if x < 0 || y < 0 || z < 0 || x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE {
            crate::voxel::AIR
        } else {
            chunk.get(LocalPos::new(x, y, z))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voxel::chunk::Chunk;

    const STONE: crate::voxel::BlockId = 1;

    fn opaque(block: crate::voxel::BlockId) -> bool {
        block != crate::voxel::AIR
    }

    #[test]
    fn empty_chunk_produces_no_geometry() {
        let chunk = Chunk::filled(crate::voxel::AIR);
        let mesh = mesh_chunk(chunk_reader(&chunk), opaque, |_| 0);
        assert!(mesh.is_empty());
        assert_eq!(mesh.quad_count, 0);
    }

    #[test]
    fn single_block_produces_six_quads() {
        let mut chunk = Chunk::filled(crate::voxel::AIR);
        chunk.set(LocalPos::new(5, 5, 5), STONE);
        let mesh = mesh_chunk(chunk_reader(&chunk), opaque, |_| 0);
        assert_eq!(mesh.quad_count, 6, "a lone cube has six visible faces");
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
    }

    #[test]
    fn interior_faces_are_culled() {
        let mut chunk = Chunk::filled(crate::voxel::AIR);
        // A 2x2x2 cube: 24 quads would be emitted without culling; the shell is 6 faces of
        // 2x2 = 6 quads after greedy merging.
        for y in 4..6 {
            for z in 4..6 {
                for x in 4..6 {
                    chunk.set(LocalPos::new(x, y, z), STONE);
                }
            }
        }
        let mesh = mesh_chunk(chunk_reader(&chunk), opaque, |_| 0);
        assert_eq!(mesh.quad_count, 6, "a 2x2x2 cube should merge into 6 quads");
    }

    #[test]
    fn solid_chunk_merges_into_six_quads() {
        let chunk = Chunk::filled(STONE);
        let mesh = mesh_chunk(chunk_reader(&chunk), opaque, |_| 0);
        assert_eq!(mesh.quad_count, 6, "a solid 32^3 chunk is six big quads");
        assert_eq!(mesh.vertices.len(), 24);
    }

    #[test]
    fn greedy_meshing_beats_naive_face_count() {
        let chunk = Chunk::filled(STONE);
        let naive = 6 * CHUNK_SIZE * CHUNK_SIZE; // one quad per surface block face
        let mesh = mesh_chunk(chunk_reader(&chunk), opaque, |_| 0);
        assert!(
            (mesh.quad_count as i32) < naive / 100,
            "greedy mesh emitted {} quads vs naive {naive}",
            mesh.quad_count
        );
    }

    #[test]
    fn different_blocks_are_not_merged_together() {
        let mut chunk = Chunk::filled(crate::voxel::AIR);
        for z in 0..4 {
            for x in 0..4 {
                chunk.set(LocalPos::new(x, 0, z), if x < 2 { 1 } else { 2 });
            }
        }
        let mesh = mesh_chunk(chunk_reader(&chunk), opaque, |b| b as u32);
        // Top faces: two strips of 2x4 (2 quads) plus sides and bottom; the key check is
        // that the two block types did not merge into one quad.
        let top_quads =
            mesh.vertices.chunks(4).filter(|quad| quad[0].normal == [0.0, 1.0, 0.0]).count();
        assert_eq!(top_quads, 2, "each block type needs its own top quad");
    }

    #[test]
    fn faces_between_chunks_are_culled_by_the_world_reader() {
        use crate::voxel::coords::{BlockPos, ChunkPos};
        use crate::voxel::world::VoxelWorld;

        let mut world = VoxelWorld::new();
        world.insert(ChunkPos::new(0, 0, 0), Chunk::filled(STONE));
        world.insert(ChunkPos::new(1, 0, 0), Chunk::filled(STONE));

        let left = world.chunk(ChunkPos::new(0, 0, 0)).expect("chunk");
        let mesh = mesh_chunk(neighbour_reader(BlockPos::new(0, 0, 0), &world), opaque, |_| 0);
        // With the neighbour loaded, the shared +X wall must not be emitted: a fully solid
        // pair of chunks exposes 5 faces each (4 sides + top + bottom, minus the shared one).
        assert_eq!(mesh.quad_count, 5, "shared wall should be culled, chunk mesh: {mesh:?}");
        assert_eq!(left.palette()[0], STONE);
    }

    #[test]
    fn normals_are_axis_aligned_and_unit_length() {
        let mut chunk = Chunk::filled(crate::voxel::AIR);
        chunk.set(LocalPos::new(3, 3, 3), STONE);
        let mesh = mesh_chunk(chunk_reader(&chunk), opaque, |_| 0);
        for vertex in &mesh.vertices {
            let length =
                (vertex.normal[0].powi(2) + vertex.normal[1].powi(2) + vertex.normal[2].powi(2))
                    .sqrt();
            assert!((length - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn merged_quads_keep_texture_scale() {
        let chunk = Chunk::filled(STONE);
        let mesh = mesh_chunk(chunk_reader(&chunk), opaque, |_| 0);
        // Every face is a full 32x32 quad: the tile-local UVs stay inside one tile, and the
        // tiling factor tells the shader to repeat the tile 32 times.
        for quad in mesh.vertices.chunks(4) {
            for vertex in quad {
                assert!((0.0..=1.0).contains(&vertex.uv[0]));
                assert!((0.0..=1.0).contains(&vertex.uv[1]));
                assert_eq!(vertex.uv_b, [32.0, 32.0]);
            }
        }
    }
}
