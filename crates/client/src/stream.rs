//! Chunk streaming for the local (M1) client.
//!
//! Until the server exists, the client generates its own chunks around the player on worker
//! threads. The generator is the same deterministic one the server will use, so M2 only has
//! to replace *where the chunks come from*.

use std::collections::HashSet;

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::voxel::coords::{CHUNK_SIZE, ChunkPos};
use deepcore_shared::voxel::world::VoxelWorld;
use deepcore_shared::worldgen::WorldGen;

use crate::input::PlayerCamera;
use crate::render::ViewDistance;

/// The world generator resource.
#[derive(Resource)]
pub struct ClientWorldGen(pub WorldGen);

/// A chunk generation job running on a worker thread.
pub struct GenJob {
    pub task: Task<deepcore_shared::voxel::Chunk>,
    pub pos: ChunkPos,
}

/// Generation jobs in flight.
#[derive(Resource, Default)]
pub struct GenQueue {
    in_flight: Vec<GenJob>,
    requested: HashSet<ChunkPos>,
}

/// The seed the client generates with in single-process mode.
pub const DEFAULT_SEED: u64 = 0x0DEE9C0DE;

/// Builds the generator and inserts the world resource.
///
/// The chunks immediately around the spawn point are generated synchronously here (nine
/// chunks, well under the 5 ms budget each) so the player is never dropped into empty space
/// while the streaming tasks spin up.
pub fn setup_world(mut commands: Commands, registry: Res<ContentRegistry>) {
    let generator = WorldGen::new(DEFAULT_SEED, registry.0.clone());
    info!("client world seed {:#x}", DEFAULT_SEED);

    let mut world = VoxelWorld::new();
    let spawn = generator.registry().world().spawn;
    let center = ChunkPos::new(
        spawn.0.div_euclid(CHUNK_SIZE),
        spawn.1.div_euclid(CHUNK_SIZE),
        spawn.2.div_euclid(CHUNK_SIZE),
    );
    for dy in -1..=1 {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let pos = center.offset(dx, dy, dz);
                if pos.in_world() {
                    world.insert(pos, generator.generate_chunk(pos));
                }
            }
        }
    }
    info!("generated {} spawn chunks around {center:?}", world.loaded_chunks());

    commands.insert_resource(ClientWorldGen(generator));
    commands.insert_resource(world);
    commands.insert_resource(GenQueue::default());
}

/// Requests generation of chunks around the player, nearest first, and applies finished ones.
pub fn stream_chunks(
    mut commands: Commands,
    mut world: ResMut<VoxelWorld>,
    generator: Res<ClientWorldGen>,
    view: Res<ViewDistance>,
    camera: Query<&Transform, With<PlayerCamera>>,
    mut queue: ResMut<GenQueue>,
) {
    let Ok(transform) = camera.single() else {
        return;
    };
    let position = transform.translation;
    let center = ChunkPos::new(
        (position.x.floor() as i32).div_euclid(CHUNK_SIZE),
        (position.y.floor() as i32).div_euclid(CHUNK_SIZE),
        (position.z.floor() as i32).div_euclid(CHUNK_SIZE),
    );

    // ---- collect finished jobs
    let mut finished = Vec::new();
    for (index, job) in queue.in_flight.iter_mut().enumerate() {
        if let Some(chunk) = block_on(poll_once(&mut job.task)) {
            finished.push((index, job.pos, chunk));
        }
    }
    for (index, pos, chunk) in finished.into_iter().rev() {
        queue.in_flight.swap_remove(index);
        queue.requested.remove(&pos);
        world.insert(pos, chunk);
    }

    // ---- request new chunks, nearest first
    let horizontal = view.horizontal;
    let vertical = view.vertical;
    let mut wanted: Vec<ChunkPos> = Vec::new();
    for dy in -vertical..=vertical {
        for dz in -horizontal..=horizontal {
            for dx in -horizontal..=horizontal {
                let pos = center.offset(dx, dy, dz);
                if !pos.in_world() || world.contains(pos) || queue.requested.contains(&pos) {
                    continue;
                }
                wanted.push(pos);
            }
        }
    }
    wanted.sort_by_key(|pos| pos.chunk_distance(center));

    let budget = 8usize.saturating_sub(queue.in_flight.len());
    for pos in wanted.into_iter().take(budget) {
        let generator = generator.0.clone_for_thread();
        let task = AsyncComputeTaskPool::get().spawn(async move { generator.generate_chunk(pos) });
        queue.in_flight.push(GenJob { task, pos });
        queue.requested.insert(pos);
    }

    let _ = &mut commands;
}

/// Unloads chunks that are far outside the view distance, to bound memory use.
pub fn unload_distant_chunks(
    mut world: ResMut<VoxelWorld>,
    view: Res<ViewDistance>,
    camera: Query<&Transform, With<PlayerCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    chunk_entities: Query<(Entity, &crate::render::ChunkRender)>,
    mut commands: Commands,
) {
    let Ok(transform) = camera.single() else {
        return;
    };
    let position = transform.translation;
    let center = ChunkPos::new(
        (position.x.floor() as i32).div_euclid(CHUNK_SIZE),
        (position.y.floor() as i32).div_euclid(CHUNK_SIZE),
        (position.z.floor() as i32).div_euclid(CHUNK_SIZE),
    );
    let limit = (view.horizontal + 2).max(view.vertical + 2);

    let mut to_remove = Vec::new();
    for pos in world.positions() {
        if pos.chunk_distance(center) > limit {
            to_remove.push(pos);
        }
    }
    for pos in to_remove {
        world.remove(pos);
    }

    for (entity, render) in &chunk_entities {
        if render.pos.chunk_distance(center) > limit {
            commands.entity(entity).despawn();
        }
    }
    let _ = &mut meshes;
}
