//! Voxel rendering: the atlas material, chunk meshing and chunk entities.

pub mod chunk;
pub mod material;

pub use chunk::{
    DirtyChunks, MeshedChunks, ViewDistance, apply_finished_meshes, chunk_of, fog_distance,
    forget_unloaded_meshes, mark_dirty, schedule_meshing, sky_color,
};
pub use material::{AtlasHandles, VoxelRenderPlugin};
