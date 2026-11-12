//! On-screen HUD: crosshair, hotbar, depth meter, block info and the debug overlay.
//!
//! Deliberately drawn with plain Bevy UI (not egui) so it costs almost nothing per frame.

use bevy::prelude::*;
use bevy::text::FontSize;

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::mining::MineBlocked;

use crate::input::{LocalPlayer, Targeting};
use crate::mining::{HeldItem, MiningProgress};
use crate::render::ViewDistance;

/// Marker for the crosshair container.
#[derive(Component)]
pub struct Crosshair;

/// Marker for the depth/position text.
#[derive(Component)]
pub struct DepthText;

/// Marker for the message line ("Too hard — needs Tier 2").
#[derive(Component)]
pub struct MessageText;

/// Marker for the debug overlay text.
#[derive(Component)]
pub struct DebugText;

/// Whether the debug overlay is visible.
#[derive(Resource, Default)]
pub struct DebugOverlay {
    /// Toggle state, bound to F3.
    pub visible: bool,
}

/// Builds the HUD once at startup.
pub fn setup_hud(mut commands: Commands) {
    commands
        .spawn((Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            position_type: PositionType::Absolute,
            ..default()
        },))
        .with_children(|parent| {
            // Crosshair: a small dot.
            let _ = parent
                .spawn((
                    Crosshair,
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Percent(50.0),
                        top: Val::Percent(50.0),
                        width: Val::Px(4.0),
                        height: Val::Px(4.0),
                        margin: UiRect::new(Val::Px(-2.0), Val::Auto, Val::Px(-2.0), Val::Auto),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
                ))
                .id();

            // Depth meter, bottom left.
            parent.spawn((
                DepthText,
                Text::new(""),
                TextFont { font_size: FontSize::Px(16.0), ..default() },
                TextColor(Color::srgb(0.92, 0.94, 0.88)),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(16.0),
                    bottom: Val::Px(16.0),
                    ..default()
                },
            ));

            // Message line, above the crosshair.
            parent.spawn((
                MessageText,
                Text::new(""),
                TextFont { font_size: FontSize::Px(20.0), ..default() },
                TextColor(Color::srgb(1.0, 0.35, 0.3)),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(50.0),
                    top: Val::Percent(56.0),
                    margin: UiRect::new(Val::Auto, Val::Auto, Val::Auto, Val::Auto),
                    ..default()
                },
            ));

            // Debug overlay, top left.
            parent.spawn((
                DebugText,
                Text::new(""),
                TextFont { font_size: FontSize::Px(14.0), ..default() },
                TextColor(Color::srgb(0.75, 1.0, 0.8)),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(16.0),
                    top: Val::Px(16.0),
                    ..default()
                },
            ));
        });

    commands.insert_resource(DebugOverlay::default());
}

/// Toggles the debug overlay with F3.
pub fn toggle_debug(mut overlay: ResMut<DebugOverlay>, keys: Res<ButtonInput<KeyCode>>) {
    if keys.just_pressed(KeyCode::F3) {
        overlay.visible = !overlay.visible;
    }
}

/// Updates the depth meter and the crosshair colour.
pub fn update_depth_text(
    player: Res<LocalPlayer>,
    registry: Res<ContentRegistry>,
    targeting: Res<Targeting>,
    progress: Res<MiningProgress>,
    mut crosshair: Query<&mut BackgroundColor, With<Crosshair>>,
    mut depth: Query<&mut Text, (With<DepthText>, Without<MessageText>)>,
    mut message: Query<&mut Text, (With<MessageText>, Without<DepthText>)>,
) {
    let position = player.state.position;
    let y = position.y as i32;
    let depth_metres = registry.depth_of(y);
    let layer = registry
        .layer_at_y(y)
        .and_then(|id| registry.layer(id))
        .map(|layer| layer.def.name.as_str())
        .unwrap_or("Surface");

    let _ = &targeting;

    if let Ok(mut text) = depth.single_mut() {
        **text = format!(
            "{}  |  depth {} m  |  x {:.0} z {:.0}",
            layer, depth_metres, position.x, position.z
        );
    }

    // Crosshair turns red when the targeted block cannot be mined.
    let blocked = progress.blocked.is_some();
    if let Ok(mut color) = crosshair.single_mut() {
        *color = BackgroundColor(if blocked {
            Color::srgba(1.0, 0.25, 0.2, 0.95)
        } else {
            Color::srgba(1.0, 1.0, 1.0, 0.85)
        });
    }

    if let Ok(mut text) = message.single_mut() {
        **text = match progress.blocked {
            Some(MineBlocked::TierTooLow { required }) => {
                format!("Too hard — needs Tier {required}")
            }
            Some(MineBlocked::NoEnergy) => "Out of power".to_owned(),
            Some(MineBlocked::Unbreakable) => "Unbreakable".to_owned(),
            None => String::new(),
        };
    }
}

/// Updates the F3 overlay.
pub fn update_debug_text(
    overlay: Res<DebugOverlay>,
    player: Res<LocalPlayer>,
    world: Res<deepcore_shared::voxel::world::VoxelWorld>,
    view: Res<ViewDistance>,
    held: Res<HeldItem>,
    mut text: Query<&mut Text, With<DebugText>>,
    material: Option<Res<crate::render::AtlasHandles>>,
) {
    let Ok(mut text) = text.single_mut() else {
        return;
    };
    if !overlay.visible {
        **text = String::new();
        return;
    }
    let tiles = material.as_ref().map(|m| m.tiles.len()).unwrap_or(0);
    **text = format!(
        "{}\nchunks loaded {}  ({:.1} MB palette)  atlas tiles {}\nview radius {} h / {} v\nheld tool: {}",
        crate::input::debug_line(&player),
        world.loaded_chunks(),
        world.storage_bytes() as f32 / 1_048_576.0,
        tiles,
        view.horizontal,
        view.vertical,
        held.tool.map(|_id| "tool".to_owned()).unwrap_or_else(|| "none".to_owned())
    );
}
