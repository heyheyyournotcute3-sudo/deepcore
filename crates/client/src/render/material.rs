//! The Deepcore voxel material: a [`StandardMaterial`] extension that tiles the procedural
//! atlas per block.
//!
//! A greedy-merged quad can span dozens of blocks, so a plain UV per vertex would stretch one
//! texel across the whole face. The extension reads two extra vertex attributes — the tiling
//! factor (`ATTRIBUTE_UV_1`) and the atlas tile index (packed into `ATTRIBUTE_COLOR.r`) — and
//! repeats the tile across the quad in the fragment stage.

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;

use deepcore_shared::atlas::{ATLAS_COLUMNS, ATLAS_HEIGHT, ATLAS_ROWS, ATLAS_WIDTH, TILE_SIZE};

/// The material type used by every chunk mesh.
pub type VoxelMaterial = ExtendedMaterial<StandardMaterial, VoxelExtension>;

/// Uniform data for the voxel shader.
#[derive(Clone, Debug, ShaderType, Reflect)]
#[reflect(Default)]
pub struct VoxelUniform {
    /// Number of atlas columns.
    pub atlas_columns: f32,
    /// Number of atlas rows.
    pub atlas_rows: f32,
    /// Inset in UV units that keeps bilinear filtering inside a tile.
    pub uv_inset: f32,
    /// Emissive lift for glowing blocks.
    pub glow_strength: f32,
}

impl Default for VoxelUniform {
    fn default() -> Self {
        Self {
            atlas_columns: ATLAS_COLUMNS as f32,
            atlas_rows: ATLAS_ROWS as f32,
            // Half a texel of the 16px tile.
            uv_inset: 0.5 / TILE_SIZE as f32,
            glow_strength: 0.0,
        }
    }
}

/// The material extension itself.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct VoxelExtension {
    /// Shader uniforms.
    #[uniform(100)]
    pub uniform: VoxelUniform,
}

impl MaterialExtension for VoxelExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/voxel.wesl".into()
    }

    fn deferred_fragment_shader() -> ShaderRef {
        "shaders/voxel.wesl".into()
    }
}

/// Registers the material plugin and builds the atlas texture at startup.
pub struct VoxelRenderPlugin;

impl Plugin for VoxelRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<VoxelMaterial>::default());
        app.add_systems(Startup, setup_atlas);
    }
}

/// Builds the procedural atlas image once, so every chunk can share it.
fn setup_atlas(
    mut commands: Commands,
    registry: Res<deepcore_shared::content::ContentRegistry>,
    mut images: ResMut<Assets<Image>>,
) {
    let atlas = deepcore_shared::atlas::generate_atlas(&registry);

    let image = Image::new(
        Extent3d { width: ATLAS_WIDTH, height: ATLAS_HEIGHT, depth_or_array_layers: 1 },
        TextureDimension::D2,
        atlas.pixels.clone(),
        // sRGB so the generated colours look like the ones in blocks.ron.
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    let handle = images.add(image);

    commands.insert_resource(AtlasHandles { image: handle, tiles: atlas.tiles });
}

/// The atlas texture and per-block tile lookup, used when building chunk meshes.
#[derive(Resource)]
pub struct AtlasHandles {
    /// The atlas image.
    pub image: Handle<Image>,
    /// Tile index per block id.
    pub tiles: Vec<u32>,
}
