//! Local mining and placing (M1 single-process scope).
//!
//! In M2 these become intents sent to the server. The *rules* already live in
//! `deepcore_shared::mining`; this module only drives the timer, applies the result to the
//! local world and requests re-meshes.

use bevy::prelude::*;

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::mining::{MineBlocked, MineCheck, ToolProfile, area_blocks, check_mine};
use deepcore_shared::voxel::world::VoxelWorld;

use crate::input::{LocalPlayer, Targeting};
use crate::render::{DirtyChunks, mark_dirty};

/// The block the player is currently breaking, and how far along they are.
#[derive(Resource, Default)]
pub struct MiningProgress {
    /// Block being mined.
    pub block: Option<deepcore_shared::voxel::BlockPos>,
    /// Seconds of mining accumulated.
    pub elapsed: f32,
    /// Seconds required, for the HUD crack overlay.
    pub required: f32,
    /// Why the last attempt was refused, for the red crosshair and message.
    pub blocked: Option<MineBlocked>,
}

/// The item the player is holding in the hotbar. M3 replaces this with a real inventory.
#[derive(Resource)]
pub struct HeldItem {
    /// The tool item id, if any.
    pub tool: Option<deepcore_shared::content::registry::ItemId>,
    /// Whether the tool's energy source is charged.
    pub charged: bool,
    /// The block id placed by the right mouse button, if any.
    pub place_block: Option<deepcore_shared::voxel::BlockId>,
}

impl Default for HeldItem {
    fn default() -> Self {
        Self { tool: None, charged: true, place_block: None }
    }
}

/// Advances mining while the left mouse button is held.
pub fn mining_system(
    time: Res<Time>,
    buttons: Res<ButtonInput<MouseButton>>,
    player: Res<LocalPlayer>,
    registry: Res<ContentRegistry>,
    targeting: Res<Targeting>,
    held: Res<HeldItem>,
    mut progress: ResMut<MiningProgress>,
    mut world: ResMut<VoxelWorld>,
    mut dirty: ResMut<DirtyChunks>,
) {
    let profile = ToolProfile::for_item(&registry, held.tool, held.charged);

    if !buttons.pressed(MouseButton::Left) {
        progress.block = None;
        progress.elapsed = 0.0;
        return;
    }

    let Some(target) = targeting.target else {
        progress.block = None;
        progress.elapsed = 0.0;
        return;
    };

    // Reset the timer when the player looks at a different block.
    if progress.block != Some(target.block) {
        progress.block = Some(target.block);
        progress.elapsed = 0.0;
    }

    let check = check_mine(&registry, world.block(target.block), &profile);
    match check {
        MineCheck::Blocked(reason) => {
            progress.blocked = Some(reason);
            progress.elapsed = 0.0;
            progress.required = f32::INFINITY;
            return;
        }
        MineCheck::Allowed { seconds, .. } => {
            progress.blocked = None;
            progress.required = seconds;
        }
    }

    progress.elapsed += time.delta_secs();
    if progress.elapsed < progress.required {
        return;
    }

    // Break it. Area tools break the whole face in one action.
    let blocks = area_blocks(target.block, target.normal, profile.area);
    let mut broke_any = false;
    for pos in blocks {
        let block = world.block(pos);
        if block == deepcore_shared::voxel::AIR {
            continue;
        }
        if !matches!(check_mine(&registry, block, &profile), MineCheck::Allowed { .. }) {
            continue;
        }
        if world.set_block(pos, deepcore_shared::voxel::AIR) {
            mark_dirty(&mut dirty, pos);
            broke_any = true;
        }
    }

    let _ = player;
    if broke_any {
        progress.elapsed = 0.0;
        progress.block = None;
    }
}

/// Places a block on right click, if the target face is free and the player has room.
pub fn placing_system(
    buttons: Res<ButtonInput<MouseButton>>,
    player: Res<LocalPlayer>,
    targeting: Res<Targeting>,
    held: Res<HeldItem>,
    mut world: ResMut<VoxelWorld>,
    mut dirty: ResMut<DirtyChunks>,
) {
    if !buttons.just_pressed(MouseButton::Right) {
        return;
    }
    let (Some(target), Some(block)) = (targeting.target, held.place_block) else {
        return;
    };
    let pos = target.placement;
    if !pos.in_world() || world.block(pos) != deepcore_shared::voxel::AIR {
        return;
    }
    // Do not place a block inside the player.
    let player_box =
        deepcore_shared::movement::Aabb::from_feet(player.state.position, player.state.height());
    if player_box.intersects_block(pos) {
        return;
    }
    if world.set_block(pos, block) {
        mark_dirty(&mut dirty, pos);
    }
}
