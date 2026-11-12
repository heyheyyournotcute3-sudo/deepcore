//! First-person camera, input and block targeting.
//!
//! In M1 the client owns the player: there is no server yet. The camera drives a
//! [`PlayerState`] through the *shared* movement code, so the behaviour is already the one the
//! server will later re-simulate for prediction.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::CursorOptions;

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::mining::{Target, raycast};
use deepcore_shared::movement::{
    BlockPhysics, MoveInput, MoveTuning, PLAYER_EYE_HEIGHT, PlayerState, horizontal_speed,
    step_player,
};
use deepcore_shared::voxel::world::VoxelWorld;

/// The local player's simulated state.
#[derive(Resource)]
pub struct LocalPlayer {
    /// Predicted/physical state.
    pub state: PlayerState,
    /// Input sequence counter, sent to the server in M2.
    pub seq: u32,
    /// Physics classification of blocks.
    pub physics: BlockPhysics,
    /// Movement tuning constants.
    pub tuning: MoveTuning,
}

impl LocalPlayer {
    /// Creates a player at a feet position with physics derived from content.
    pub fn new(position: Vec3, registry: &ContentRegistry) -> Self {
        Self {
            state: PlayerState::at(position),
            seq: 0,
            physics: BlockPhysics::from_registry(registry),
            tuning: MoveTuning::default(),
        }
    }
}

/// What the player is aiming at this frame.
#[derive(Resource, Default)]
pub struct Targeting {
    /// The block under the crosshair, if any.
    pub target: Option<Target>,
}

/// Spawns the camera and the player state.
pub fn setup_player(
    mut commands: Commands,
    registry: Res<ContentRegistry>,
    world: Res<VoxelWorld>,
) {
    let spawn = registry.world().spawn;
    let physics = BlockPhysics::from_registry(&registry);
    let feet = deepcore_shared::movement::settle_on_ground(
        &world,
        &physics,
        Vec3::new(spawn.0 as f32 + 0.5, spawn.1 as f32 + 2.0, spawn.2 as f32 + 0.5),
    );
    let mut player = LocalPlayer::new(feet, &registry);
    player.state.yaw = 0.0;
    player.state.pitch = -0.15;

    commands.spawn((
        Camera3d::default(),
        // Distance fog matched to the sky, so the streaming edge of the world is not obvious.
        DistanceFog {
            color: crate::render::sky_color(),
            falloff: FogFalloff::Linear {
                start: 80.0,
                end: crate::render::fog_distance(&crate::render::ViewDistance::default()),
            },
            ..default()
        },
        Transform::from_translation(feet + Vec3::Y * PLAYER_EYE_HEIGHT),
        PlayerCamera,
    ));
    commands.insert_resource(player);
    commands.insert_resource(Targeting::default());
}

/// Marker for the first-person camera.
#[derive(Component)]
pub struct PlayerCamera;

/// Reads mouse and keyboard and advances the shared player simulation.
pub fn player_input_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<AccumulatedMouseMotion>,
    mut player: ResMut<LocalPlayer>,
    world: Res<VoxelWorld>,
    mut camera: Query<&mut Transform, With<PlayerCamera>>,
    cursor_grab: Res<CursorGrab>,
) {
    // ---- mouse look
    if cursor_grab.grabbed {
        let sensitivity = 0.0022;
        player.state.yaw -= mouse.delta.x * sensitivity;
        player.state.pitch -= mouse.delta.y * sensitivity;
        player.state.pitch = player
            .state
            .pitch
            .clamp(-std::f32::consts::FRAC_PI_2 + 0.02, std::f32::consts::FRAC_PI_2 - 0.02);
    }

    // ---- movement axes
    let mut forward = 0.0;
    let mut right = 0.0;
    if keys.pressed(KeyCode::KeyW) {
        forward += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        forward -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        right += 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        right -= 1.0;
    }

    let input = MoveInput {
        seq: player.seq,
        forward,
        right,
        jump: keys.pressed(KeyCode::Space),
        sprint: keys.pressed(KeyCode::ShiftLeft),
        crouch: keys.pressed(KeyCode::ControlLeft),
        yaw: player.state.yaw,
        pitch: player.state.pitch,
    };
    player.seq = player.seq.wrapping_add(1);

    // ---- fixed-step simulation, so the client already runs at the server's rate
    let dt = 1.0 / deepcore_shared::SERVER_TICK_HZ as f32;
    let mut accumulated = time.delta_secs().min(0.1);
    while accumulated > 0.0 {
        let step = accumulated.min(dt);
        // Split the borrow so the physics tables and the state can be used together.
        let LocalPlayer { state, physics, tuning, .. } = &mut *player;
        step_player(&world, physics, state, input, step, tuning);
        accumulated -= step;
    }

    if let Ok(mut transform) = camera.single_mut() {
        let eye = player.state.position + Vec3::Y * eye_offset(&player.state);
        transform.translation = eye;
        transform.rotation =
            Quat::from_euler(EulerRot::YXZ, player.state.yaw, player.state.pitch, 0.0);
    }
}

fn eye_offset(state: &PlayerState) -> f32 {
    state.height() - 0.18
}

/// Whether the mouse is captured by the game.
#[derive(Resource, Default)]
pub struct CursorGrab {
    /// True when the cursor is hidden and captured.
    pub grabbed: bool,
}

/// Grabs the cursor on click and releases it on Escape.
pub fn cursor_grab_system(
    mut grab: ResMut<CursorGrab>,
    mut cursors: Query<&mut CursorOptions>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Ok(mut cursor) = cursors.single_mut() else {
        return;
    };
    if buttons.just_pressed(MouseButton::Left) && !grab.grabbed {
        grab.grabbed = true;
        cursor.visible = false;
        cursor.grab_mode = bevy::window::CursorGrabMode::Locked;
    }
    if keys.just_pressed(KeyCode::Escape) {
        grab.grabbed = false;
        cursor.visible = true;
        cursor.grab_mode = bevy::window::CursorGrabMode::None;
    }
}

/// Raycasts from the eye along the view direction to find the targeted block.
pub fn targeting_system(
    player: Res<LocalPlayer>,
    world: Res<VoxelWorld>,
    registry: Res<ContentRegistry>,
    mut targeting: ResMut<Targeting>,
) {
    let eye = player.state.position + Vec3::Y * PLAYER_EYE_HEIGHT;
    let direction = player.state.look_direction();
    let reach = 5.5;
    let passable = &player.physics.passable;
    targeting.target = raycast(&world, eye, direction, reach, |block| {
        passable.contains(&block) && registry.block_name(block) != "ladder"
    });
}

/// Draws a wireframe box around the targeted block.
pub fn draw_target_outline(
    mut gizmos: Gizmos,
    targeting: Res<Targeting>,
    registry: Res<ContentRegistry>,
) {
    let Some(target) = &targeting.target else {
        return;
    };
    let centre = target.block.center_f32();
    // A thin dark wireframe reads well against every block colour.
    gizmos.cube(
        Transform::from_translation(centre).with_scale(Vec3::splat(1.006)),
        Color::srgb(0.02, 0.02, 0.02),
    );
    let _ = registry;
}

/// Helper used by the debug overlay.
pub fn debug_line(player: &LocalPlayer) -> String {
    let state = &player.state;
    format!(
        "pos {:.1} {:.1} {:.1}  vel {:.1}  ground {}  ladder {}  water {}",
        state.position.x,
        state.position.y,
        state.position.z,
        horizontal_speed(state.velocity),
        state.on_ground,
        state.on_ladder,
        state.submerged
    )
}
