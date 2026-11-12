//! Chunk meshing tasks and chunk entities.
//!
//! Meshing runs on Bevy's `AsyncComputeTaskPool`; the main thread only uploads the finished
//! vertex buffer. Each chunk entity carries its position so it can be re-meshed when a block
//! in or next to it changes.

use std::collections::HashSet;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::pbr::ExtendedMaterial;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::content::model::BlockKind;
use deepcore_shared::mesh::{ChunkMeshData, mesh_chunk, neighbour_reader};
use deepcore_shared::voxel::coords::{CHUNK_SIZE, ChunkPos};
use deepcore_shared::voxel::world::VoxelWorld;

use super::material::{AtlasHandles, VoxelMaterial};
use crate::input::PlayerCamera;

/// Marker and bookkeeping for a rendered chunk entity.
#[derive(Component)]
pub struct ChunkRender {
    /// Which chunk this entity draws.
    pub pos: ChunkPos,
}

/// A chunk whose mesh is being built on a worker thread.
#[derive(Component)]
pub struct PendingMesh {
    /// The task handle.
    pub task: Task<ChunkMeshData>,
    /// The chunk this mesh belongs to.
    pub pos: ChunkPos,
}

/// How far the client draws chunks.
#[derive(Resource)]
pub struct ViewDistance {
    /// Horizontal radius in chunks.
    pub horizontal: i32,
    /// Vertical radius in chunks.
    pub vertical: i32,
}

impl Default for ViewDistance {
    fn default() -> Self {
        Self { horizontal: 5, vertical: 3 }
    }
}

/// Chunks that need a fresh mesh.
#[derive(Resource, Default)]
pub struct DirtyChunks {
    /// Chunks waiting to be re-meshed.
    pub chunks: HashSet<ChunkPos>,
}

/// Chunks that have already been meshed, including those whose mesh turned out to be empty
/// (all air). Without this set an empty chunk would be re-scheduled every frame and starve
/// the meshing budget, leaving holes in the world.
#[derive(Resource, Default)]
pub struct MeshedChunks {
    /// Positions that already have a finished mesh (or a known-empty one).
    pub chunks: HashSet<ChunkPos>,
}

/// Requests a re-mesh of the chunk containing a block, plus any neighbour whose border faces
/// may have changed.
pub fn mark_dirty(dirty: &mut DirtyChunks, block: deepcore_shared::voxel::BlockPos) {
    let chunk = block.chunk();
    dirty.chunks.insert(chunk);
    let local = block.local();
    if local.x == 0 {
        dirty.chunks.insert(chunk.offset(-1, 0, 0));
    }
    if local.x == CHUNK_SIZE - 1 {
        dirty.chunks.insert(chunk.offset(1, 0, 0));
    }
    if local.y == 0 {
        dirty.chunks.insert(chunk.offset(0, -1, 0));
    }
    if local.y == CHUNK_SIZE - 1 {
        dirty.chunks.insert(chunk.offset(0, 1, 0));
    }
    if local.z == 0 {
        dirty.chunks.insert(chunk.offset(0, 0, -1));
    }
    if local.z == CHUNK_SIZE - 1 {
        dirty.chunks.insert(chunk.offset(0, 0, 1));
    }
}

/// Spawns mesh tasks for chunks that are visible and not yet meshed, nearest first.
pub fn schedule_meshing(
    mut commands: Commands,
    world: Res<VoxelWorld>,
    view: Res<ViewDistance>,
    player: Query<&Transform, With<PlayerCamera>>,
    existing: Query<(Entity, &ChunkRender)>,
    pending: Query<&PendingMesh>,
    atlas: Option<Res<AtlasHandles>>,
    registry: Res<ContentRegistry>,
    mut dirty: ResMut<DirtyChunks>,
    meshed_chunks: Res<MeshedChunks>,
) {
    let Some(atlas) = atlas else {
        return;
    };
    let Ok(player_transform) = player.single() else {
        return;
    };
    let player_chunk = chunk_of(player_transform.translation);

    let mut meshed: std::collections::HashMap<ChunkPos, Entity> = std::collections::HashMap::new();
    for (entity, render) in &existing {
        meshed.insert(render.pos, entity);
    }
    let in_flight: HashSet<ChunkPos> = pending.iter().map(|job| job.pos).collect();

    // Per-block tables the worker thread needs; both are tiny.
    let opaque: Vec<bool> = registry
        .blocks()
        .iter()
        .map(|entry| matches!(entry.def.kind, BlockKind::Solid | BlockKind::Machine))
        .collect();
    let tiles = atlas.tiles.clone();

    let mut candidates: Vec<ChunkPos> = world
        .positions()
        .filter(|pos| {
            (pos.x - player_chunk.x).abs() <= view.horizontal
                && (pos.z - player_chunk.z).abs() <= view.horizontal
                && (pos.y - player_chunk.y).abs() <= view.vertical
        })
        .filter(|pos| !meshed_chunks.chunks.contains(pos) || dirty.chunks.contains(pos))
        .collect();
    candidates.sort_by_key(|pos| pos.chunk_distance(player_chunk));

    for pos in candidates.into_iter().take(8) {
        if in_flight.contains(&pos) {
            continue;
        }
        if let Some(entity) = meshed.get(&pos) {
            commands.entity(*entity).despawn();
        }
        dirty.chunks.remove(&pos);

        if !world.contains(pos) {
            continue;
        }
        let origin = pos.origin();
        let snapshot = world.clone_for_mesh();

        let opaque = opaque.clone();
        let tiles = tiles.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let block_at = neighbour_reader(origin, &snapshot);
            let is_opaque = move |block: deepcore_shared::voxel::BlockId| {
                opaque.get(block as usize).copied().unwrap_or(false)
            };
            let tile_of = move |block: deepcore_shared::voxel::BlockId| {
                tiles.get(block as usize).copied().unwrap_or(0)
            };
            mesh_chunk(block_at, is_opaque, tile_of)
        });

        commands.spawn(PendingMesh { task, pos });
    }
}

/// Uploads finished chunk meshes as entities.
pub fn apply_finished_meshes(
    mut commands: Commands,
    mut pending: Query<(Entity, &mut PendingMesh)>,
    atlas: Option<Res<AtlasHandles>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<VoxelMaterial>>,
    mut material_cache: ResMut<VoxelMaterialCache>,
    mut meshed_chunks: ResMut<MeshedChunks>,
) {
    let Some(atlas) = atlas else {
        return;
    };

    // One shared material instance for every chunk.
    let material = material_cache.handle(&mut materials, atlas.image.clone());

    for (entity, mut job) in &mut pending {
        let Some(mesh_data) = block_on(poll_once(&mut job.task)) else {
            continue;
        };
        commands.entity(entity).despawn();
        // Record the chunk as meshed even when the result is empty, so it is not retried.
        meshed_chunks.chunks.insert(job.pos);

        if mesh_data.is_empty() {
            continue;
        }

        let handle = meshes.add(build_bevy_mesh(&mesh_data));
        let origin = job.pos.origin();
        commands.spawn((
            Mesh3d(handle),
            MeshMaterial3d(material.clone()),
            Transform::from_xyz(origin.x as f32, origin.y as f32, origin.z as f32),
            ChunkRender { pos: job.pos },
        ));
    }
}

/// Caches the single shared voxel material.
#[derive(Resource, Default)]
pub struct VoxelMaterialCache {
    handle: Option<Handle<VoxelMaterial>>,
}

impl VoxelMaterialCache {
    fn handle(
        &mut self,
        materials: &mut Assets<VoxelMaterial>,
        atlas: Handle<Image>,
    ) -> Handle<VoxelMaterial> {
        if let Some(handle) = &self.handle {
            return handle.clone();
        }
        let handle = materials.add(ExtendedMaterial {
            base: StandardMaterial {
                base_color_texture: Some(atlas),
                base_color: Color::WHITE,
                perceptual_roughness: 0.95,
                metallic: 0.0,
                reflectance: 0.04,
                ..default()
            },
            extension: super::material::VoxelExtension::default(),
        });
        self.handle = Some(handle.clone());
        handle
    }
}

/// Converts our plain vertex data into a Bevy mesh with the three attributes the voxel shader
/// reads: position, normal, uv (tile-local), uv_b (tiling factor) and colour (tile index).
pub fn build_bevy_mesh(data: &ChunkMeshData) -> Mesh {
    let mut positions = Vec::with_capacity(data.vertices.len());
    let mut normals = Vec::with_capacity(data.vertices.len());
    let mut uvs = Vec::with_capacity(data.vertices.len());
    let mut uvs_b = Vec::with_capacity(data.vertices.len());
    let mut colors = Vec::with_capacity(data.vertices.len());

    for vertex in &data.vertices {
        positions.push(vertex.position);
        normals.push(vertex.normal);
        uvs.push(vertex.uv);
        uvs_b.push(vertex.uv_b);
        // The tile index rides in the red channel; the shader rounds it back to an integer.
        colors.push([vertex.tile, 0.0, 0.0, 1.0]);
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uvs_b);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(data.indices.clone()));
    mesh
}

/// Forgets meshes for chunks that are no longer loaded, so they are rebuilt if they return.
pub fn forget_unloaded_meshes(world: Res<VoxelWorld>, mut meshed_chunks: ResMut<MeshedChunks>) {
    if meshed_chunks.chunks.is_empty() {
        return;
    }
    meshed_chunks.chunks.retain(|pos| world.contains(*pos));
}

/// The chunk a world position belongs to.
pub fn chunk_of(position: Vec3) -> ChunkPos {
    ChunkPos::new(
        (position.x.floor() as i32).div_euclid(CHUNK_SIZE),
        (position.y.floor() as i32).div_euclid(CHUNK_SIZE),
        (position.z.floor() as i32).div_euclid(CHUNK_SIZE),
    )
}

/// Sky colour, matched by the camera fog.
pub fn sky_color() -> Color {
    Color::srgb(0.53, 0.68, 0.86)
}

/// Fog end distance for the current view radius, in blocks.
pub fn fog_distance(view: &ViewDistance) -> f32 {
    (view.horizontal * CHUNK_SIZE) as f32 * 0.9
}
