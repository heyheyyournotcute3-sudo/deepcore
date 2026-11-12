//! Deepcore windowed client.
//!
//! M0 scope: open a window, light a test cube and fly a camera around it. The voxel
//! renderer, UI and networking arrive in later milestones.

use bevy::prelude::*;

fn main() {
    let mut app = App::new();

    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Deepcore".to_owned(),
            resolution: (1280, 720).into(),
            ..default()
        }),
        ..default()
    }));

    app.add_plugins(deepcore_shared::SharedPlugin);
    app.add_systems(Startup, setup_scene);
    app.add_systems(Update, orbit_camera);

    app.run();
}

/// Marks the camera the scene orbits around.
#[derive(Component)]
struct OrbitCamera {
    angle: f32,
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Ground plane.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(24.0, 24.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.24, 0.26, 0.22),
            perceptual_roughness: 0.95,
            ..default()
        })),
    ));

    // The lit test cube.
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.55, 0.52, 0.48),
            perceptual_roughness: 0.7,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.5, 0.0),
    ));

    commands.spawn((
        DirectionalLight { illuminance: 12_000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(-3.0, 2.5, 4.0).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
        OrbitCamera { angle: 0.0 },
    ));
}

/// Slowly orbits the camera so the M0 build visibly renders and animates.
fn orbit_camera(time: Res<Time>, mut camera: Single<(&mut Transform, &mut OrbitCamera)>) {
    let (transform, orbit) = &mut *camera;
    orbit.angle += time.delta_secs() * 0.25;
    let radius = 5.0;
    transform.translation = Vec3::new(orbit.angle.sin() * radius, 2.5, orbit.angle.cos() * radius);
    transform.look_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y);
}
