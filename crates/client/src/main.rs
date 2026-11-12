//! Deepcore client.
//!
//! M1 scope: a window, a deterministic voxel world generated and meshed around the player, a
//! first-person controller using the shared movement code, mining with tier rules, block
//! placing, and a HUD with a depth meter.

use bevy::prelude::*;
use bevy::render::RenderPlugin;

mod hud;
mod input;
mod mining;
mod render;
mod stream;

fn main() {
    let mut app = App::new();

    // Assets (the voxel shader) live in the workspace `assets/` directory. Bevy looks next to
    // the executable by default; point it at the source tree so `cargo run` and a plain binary
    // both find the shader. The path must be absolute: Bevy resolves it against the process
    // working directory otherwise.
    let asset_path = asset_root();
    info!("asset root: {asset_path}");

    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Deepcore".to_owned(),
                    resolution: (1280, 720).into(),
                    ..default()
                }),
                ..default()
            })
            .set(RenderPlugin {
                // Synchronous pipelines are fine here; the world is small and this avoids a
                // first-frame hitch while shaders compile.
                synchronous_pipeline_compilation: true,
                ..default()
            })
            .set(ImagePlugin::default_nearest())
            .set(AssetPlugin { file_path: asset_path, ..default() }),
    );

    app.add_plugins(deepcore_shared::SharedPlugin);
    app.add_plugins(render::VoxelRenderPlugin);

    // Resources.
    app.init_resource::<render::ViewDistance>();
    app.init_resource::<render::DirtyChunks>();
    app.init_resource::<render::MeshedChunks>();
    app.init_resource::<input::CursorGrab>();
    app.init_resource::<mining::MiningProgress>();
    app.init_resource::<mining::HeldItem>();
    app.init_resource::<render::chunk::VoxelMaterialCache>();

    // World + renderer startup.
    app.add_systems(
        Startup,
        (stream::setup_world, setup_environment, input::setup_player, hud::setup_hud).chain(),
    );

    // Frame systems.
    app.add_systems(
        Update,
        (
            input::cursor_grab_system,
            input::player_input_system,
            input::targeting_system,
            mining::mining_system,
            mining::placing_system,
            hud::toggle_debug,
            hud::update_depth_text,
            hud::update_debug_text,
            render::schedule_meshing,
            render::apply_finished_meshes,
            render::forget_unloaded_meshes,
            input::draw_target_outline,
        )
            .chain(),
    );

    // Streaming runs after the player has moved.
    app.add_systems(
        Update,
        (stream::stream_chunks, stream::unload_distant_chunks).after(input::player_input_system),
    );

    app.run();
}

/// Finds the workspace `assets` directory relative to the executable or the crate.
fn asset_root() -> String {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        // target/debug/deepcore_client -> ../../../assets
        candidates.push(dir.join("../../../assets"));
        candidates.push(dir.join("assets"));
    }
    candidates.push(std::path::PathBuf::from("assets"));
    candidates.push(std::path::PathBuf::from("../../assets"));
    for candidate in candidates {
        if candidate.join("shaders/voxel.wesl").exists() {
            // `canonicalize` also proves the path is usable; fall back to the raw path.
            return std::fs::canonicalize(&candidate)
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|_| candidate.to_string_lossy().to_string());
        }
    }
    "assets".to_owned()
}

/// Sky, sunlight and ambient fill for the surface.
fn setup_environment(mut commands: Commands) {
    commands.insert_resource(ClearColor(render::sky_color()));
    // Daylight fill: bright enough that shaded surfaces still read, dark enough that the
    // mine below layer 1 genuinely needs a lamp.
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.62, 0.70, 0.86),
        brightness: 420.0,
        ..default()
    });

    commands.spawn((
        DirectionalLight {
            illuminance: 14_000.0,
            color: Color::srgb(1.0, 0.97, 0.90),
            shadow_maps_enabled: true,
            ..default()
        },
        // A high, slightly south-west sun: mostly top-down light, short shadows.
        Transform::from_xyz(-60.0, 200.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}
