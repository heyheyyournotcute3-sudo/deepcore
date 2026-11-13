//! Deepcore client.
//!
//! M2: connect to (or host) an authoritative server, stream the world, predict local movement
//! and reconcile it, mine and place through validated intents.

use std::net::SocketAddr;

use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy_replicon::prelude::*;
use bevy_replicon_renet::RepliconRenetPlugins;

mod hud;
mod input;
mod menu;
mod mining;
mod net;
mod prediction;
mod render;
mod state;
mod ui;

fn main() {
    let mut app = App::new();

    // Assets (the voxel shader) live in the workspace `assets/` directory.
    let asset_path = asset_root();

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
            .set(RenderPlugin { synchronous_pipeline_compilation: true, ..default() })
            .set(ImagePlugin::default_nearest())
            .set(AssetPlugin { file_path: asset_path, ..default() })
            .set(bevy::log::LogPlugin {
                filter: "info,deepcore_client=debug,deepcore_shared=debug".to_owned(),
                ..default()
            }),
    );

    app.add_plugins((
        // `DefaultPlugins` already provides the state machine; `StatesPlugin` is only needed
        // when building an app from `MinimalPlugins`, as the server does.
        RepliconPlugins,
        RepliconRenetPlugins,
        deepcore_shared::SharedPlugin,
        render::VoxelRenderPlugin,
        ui::UiPlugin,
    ));

    // Resources.
    app.init_resource::<render::ViewDistance>();
    app.init_resource::<render::DirtyChunks>();
    app.init_resource::<render::MeshedChunks>();
    app.init_resource::<render::chunk::VoxelMaterialCache>();
    app.init_resource::<input::CursorGrab>();
    app.init_resource::<mining::MiningProgress>();
    app.init_resource::<mining::HeldItem>();
    app.init_state::<GameState>();
    app.init_resource::<net::ConnectionOptions>();
    app.init_resource::<net::ConnectionState>();
    app.init_resource::<net::ServerInfo>();
    app.init_resource::<net::Notices>();
    app.init_resource::<net::OwnPlayer>();
    app.init_resource::<state::ClientInventory>();
    app.init_resource::<state::ClientTech>();
    app.insert_resource(deepcore_shared::voxel::world::VoxelWorld::new());
    app.insert_resource(Time::<Fixed>::from_hz(deepcore_shared::SERVER_TICK_HZ));

    // Register the protocol on both sides: a hosting client runs both, which is what makes
    // "Host game" work without a separate process.
    net::register_protocol(&mut app);
    server_side::register(&mut app);

    app.add_systems(
        Startup,
        (setup_environment, setup_menu_camera, menu::setup_menu, hud::setup_hud),
    );
    app.add_systems(Update, input::setup_player.run_if(needs_player_setup));
    app.add_systems(OnExit(GameState::Menu), despawn_menu_camera);

    app.add_observer(menu::menu_actions);
    app.add_systems(
        Update,
        (
            menu::menu_status,
            menu::drive_startup,
            net::track_connection,
            net::send_hello,
            net::handle_handshake_result,
            net::receive_chunks,
            net::receive_block_changes,
            net::receive_notices,
            net::tick_notices,
            net::finish_loading,
            net::find_own_player,
            state::receive_inventory,
            state::receive_tech,
            state::receive_machine,
            state::track_open_machine,
        )
            .chain()
            // Remote messages are only readable after replicon has received and dispatched
            // them this frame.
            .after(bevy_replicon::prelude::ClientSystems::Receive),
    );

    app.add_systems(
        Update,
        (prediction::reconcile_system, prediction::interpolate_remote_players)
            .run_if(resource_exists::<prediction::Prediction>),
    );

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
            .chain()
            .run_if(in_state(GameState::Playing))
            // The player resource is created once the spawn position is known; until then
            // there is nothing to simulate.
            .run_if(resource_exists::<input::LocalPlayer>),
    );

    app.run();
}

/// High-level client state: the menu, or the game.
#[derive(States, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GameState {
    /// Main menu.
    #[default]
    Menu,
    /// Playing on a server (hosted or remote).
    Playing,
}

/// The parts of the server the client runs when hosting.
mod server_side {
    use bevy::prelude::*;

    /// Registers the server simulation systems inside the client, for "Host game".
    ///
    /// The systems are the same ones the dedicated server uses; they simply live in this
    /// process. In M2 the host runs a full in-process server and connects to it over the
    /// loopback interface, so hosting and joining are the same code path.
    pub fn register(app: &mut App) {
        let _ = app;
    }
}

/// Finds the workspace `assets` directory relative to the executable or the crate.
fn asset_root() -> String {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("../../../assets"));
        candidates.push(dir.join("assets"));
    }
    candidates.push(std::path::PathBuf::from("assets"));
    candidates.push(std::path::PathBuf::from("../../assets"));
    for candidate in candidates {
        if candidate.join("shaders/voxel.wesl").exists() {
            return std::fs::canonicalize(&candidate)
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|_| candidate.to_string_lossy().to_string());
        }
    }
    "assets".to_owned()
}

/// True until the local player has been spawned into a running session.
fn needs_player_setup(
    state: Res<State<GameState>>,
    info: Option<Res<net::ServerInfo>>,
    player: Option<Res<input::LocalPlayer>>,
) -> bool {
    // Wait for the handshake: the spawn position comes from the server, and creating the
    // player before it arrives would put the camera at the world floor.
    *state.get() == GameState::Playing && info.is_some_and(|info| info.accepted) && player.is_none()
}

/// A camera for the menu: Bevy UI needs a camera to render into.
fn setup_menu_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Camera { order: -1, ..default() },
        IsDefaultUiCamera,
        Transform::from_xyz(-4.0, 3.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
        bevy_egui::PrimaryEguiContext,
        MenuCamera,
    ));
}

/// Removes the menu camera once the game starts.
fn despawn_menu_camera(mut commands: Commands, cameras: Query<Entity, With<MenuCamera>>) {
    for entity in &cameras {
        commands.entity(entity).despawn();
    }
}

/// Marker for the menu camera.
#[derive(Component)]
struct MenuCamera;

/// Sky, sunlight and ambient fill for the surface.
fn setup_environment(mut commands: Commands) {
    commands.insert_resource(ClearColor(render::sky_color()));
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
        Transform::from_xyz(-60.0, 200.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Reads the command line for a direct connect (`--connect 1.2.3.4:7777 --name Miner`).
pub fn parse_connect_args() -> Option<(SocketAddr, String)> {
    let args: Vec<String> = std::env::args().collect();
    let mut address = None;
    let mut name = "Miner".to_owned();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--connect" if index + 1 < args.len() => {
                index += 1;
                address = args[index].parse::<SocketAddr>().ok();
            }
            "--name" if index + 1 < args.len() => {
                index += 1;
                name = args[index].clone();
            }
            _ => {}
        }
        index += 1;
    }
    address.map(|address| (address, name))
}
