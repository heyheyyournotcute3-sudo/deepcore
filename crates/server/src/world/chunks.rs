//! Chunk generation and storage on the server.
//!
//! Chunks are generated on the `AsyncComputeTaskPool`, inserted into the shared [`VoxelWorld`]
//! and then streamed to clients by the interest-management systems. Generation is
//! deterministic, so a chunk that no player has modified is never written to disk.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};

use deepcore_shared::voxel::coords::{CHUNK_SIZE, ChunkPos, WORLD_CHUNKS_Y};
use deepcore_shared::voxel::world::VoxelWorld;
use deepcore_shared::worldgen::WorldGen;

/// The server's world generator.
#[derive(Resource)]
pub struct ServerWorldGen(pub WorldGen);

impl ServerWorldGen {
    /// Creates a generator for a seed and content registry.
    pub fn new(seed: u64, registry: deepcore_shared::content::registry::Registry) -> Self {
        Self(WorldGen::new(seed, registry))
    }
}

struct GenerationJob {
    task: Task<deepcore_shared::voxel::Chunk>,
    pos: ChunkPos,
}

/// Generation work in flight.
#[derive(Resource, Default)]
pub struct GenerationQueue {
    jobs: Vec<GenerationJob>,
    requested: HashSet<ChunkPos>,
}

/// How many chunk generation tasks may run at once.
const MAX_JOBS_IN_FLIGHT: usize = 16;

/// Collects the chunks each player needs and generates the missing ones.
pub fn generate_chunks(
    mut world: ResMut<VoxelWorld>,
    generator: Res<ServerWorldGen>,
    players: Query<&deepcore_shared::protocol::NetPlayer>,
    mut queue: ResMut<GenerationQueue>,
) {
    // ---- collect finished jobs
    let mut finished = Vec::new();
    for (index, job) in queue.jobs.iter_mut().enumerate() {
        if let Some(chunk) = block_on(poll_once(&mut job.task)) {
            finished.push((index, job.pos, chunk));
        }
    }
    for (index, pos, chunk) in finished.into_iter().rev() {
        queue.jobs.swap_remove(index);
        queue.requested.remove(&pos);
        // Do not overwrite a chunk that already arrived (e.g. from a save in M9).
        if !world.contains(pos) {
            world.insert(pos, chunk);
        }
    }

    // ---- decide what is missing
    let mut wanted: HashSet<ChunkPos> = HashSet::new();
    for player in &players {
        let position = player.feet();
        let center = ChunkPos::new(
            (position.x.floor() as i32).div_euclid(CHUNK_SIZE),
            (position.y.floor() as i32).div_euclid(CHUNK_SIZE),
            (position.z.floor() as i32).div_euclid(CHUNK_SIZE),
        );
        for dy in -deepcore_shared::protocol::VIEW_RADIUS_VERTICAL
            ..=deepcore_shared::protocol::VIEW_RADIUS_VERTICAL
        {
            for dz in -deepcore_shared::protocol::VIEW_RADIUS_HORIZONTAL
                ..=deepcore_shared::protocol::VIEW_RADIUS_HORIZONTAL
            {
                for dx in -deepcore_shared::protocol::VIEW_RADIUS_HORIZONTAL
                    ..=deepcore_shared::protocol::VIEW_RADIUS_HORIZONTAL
                {
                    let pos = center.offset(dx, dy, dz);
                    if pos.in_world() && pos.y < WORLD_CHUNKS_Y {
                        wanted.insert(pos);
                    }
                }
            }
        }
    }

    let mut missing: Vec<ChunkPos> = wanted
        .iter()
        .copied()
        .filter(|pos| !world.contains(*pos) && !queue.requested.contains(pos))
        .collect();
    // Nearest to any player first: sort by distance to the closest player chunk.
    missing.sort_by_key(|pos| {
        players
            .iter()
            .map(|player| {
                let center = ChunkPos::new(
                    (player.feet().x.floor() as i32).div_euclid(CHUNK_SIZE),
                    (player.feet().y.floor() as i32).div_euclid(CHUNK_SIZE),
                    (player.feet().z.floor() as i32).div_euclid(CHUNK_SIZE),
                );
                pos.chunk_distance(center)
            })
            .min()
            .unwrap_or(0)
    });

    let budget = MAX_JOBS_IN_FLIGHT.saturating_sub(queue.jobs.len());
    for pos in missing.into_iter().take(budget) {
        let generator = generator.0.clone_for_thread();
        let task = AsyncComputeTaskPool::get().spawn(async move { generator.generate_chunk(pos) });
        queue.jobs.push(GenerationJob { task, pos });
        queue.requested.insert(pos);
    }
}

/// Unloads chunks nobody is near, so memory stays bounded on a long session.
pub fn unload_distant_chunks(
    mut world: ResMut<VoxelWorld>,
    players: Query<&deepcore_shared::protocol::NetPlayer>,
    mut unloaded: MessageWriter<deepcore_shared::protocol::ChunkUnload>,
) {
    if players.is_empty() {
        return;
    }
    let mut keep: HashSet<ChunkPos> = HashSet::new();
    let mut centres: HashMap<ChunkPos, i32> = HashMap::new();
    for player in &players {
        let position = player.feet();
        let center = ChunkPos::new(
            (position.x.floor() as i32).div_euclid(CHUNK_SIZE),
            (position.y.floor() as i32).div_euclid(CHUNK_SIZE),
            (position.z.floor() as i32).div_euclid(CHUNK_SIZE),
        );
        centres.insert(center, 0);
        let radius = deepcore_shared::protocol::VIEW_RADIUS_HORIZONTAL + 2;
        for dy in -deepcore_shared::protocol::VIEW_RADIUS_VERTICAL - 1
            ..=deepcore_shared::protocol::VIEW_RADIUS_VERTICAL + 1
        {
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    let pos = center.offset(dx, dy, dz);
                    if pos.in_world() {
                        keep.insert(pos);
                    }
                }
            }
        }
    }
    let _ = centres;

    let stale: Vec<ChunkPos> = world.positions().filter(|pos| !keep.contains(pos)).collect();
    for pos in stale {
        // Modified chunks are kept for saving in M9; for now everything unloads.
        world.remove(pos);
        unloaded.write(deepcore_shared::protocol::ChunkUnload { pos: [pos.x, pos.y, pos.z] });
    }
}
