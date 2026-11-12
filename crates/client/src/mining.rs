//! Mining and placing, client side.
//!
//! The client predicts the crack animation with the shared break-time formula and sends
//! intents; the server owns the real timer, the validation and the block change.

use bevy::prelude::*;

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::mining::{MineBlocked, MineCheck, ToolProfile, check_mine};
use deepcore_shared::protocol::{PlaceBlock, StartMining, StopMining, pack_pos};
use deepcore_shared::voxel::world::VoxelWorld;

use crate::input::{LocalPlayer, Targeting};

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
///
/// The client predicts the crack animation locally with the shared break-time formula, and
/// sends `StartMining`/`StopMining`; the server owns the real timer and the block change.
pub fn mining_system(
    time: Res<Time>,
    buttons: Res<ButtonInput<MouseButton>>,
    player: Res<LocalPlayer>,
    registry: Res<ContentRegistry>,
    targeting: Res<Targeting>,
    held: Res<HeldItem>,
    mut progress: ResMut<MiningProgress>,
    world: Res<VoxelWorld>,
    mut start: MessageWriter<StartMining>,
    mut stop: MessageWriter<StopMining>,
) {
    let profile = ToolProfile::for_item(&registry, held.tool, held.charged);

    if !buttons.pressed(MouseButton::Left) {
        if progress.block.is_some() {
            stop.write(StopMining);
        }
        progress.block = None;
        progress.elapsed = 0.0;
        return;
    }

    let Some(target) = targeting.target else {
        if progress.block.is_some() {
            stop.write(StopMining);
        }
        progress.block = None;
        progress.elapsed = 0.0;
        return;
    };

    // Reset the timer and tell the server when the player looks at a different block.
    if progress.block != Some(target.block) {
        if progress.block.is_some() {
            stop.write(StopMining);
        }
        progress.block = Some(target.block);
        progress.elapsed = 0.0;
        start.write(StartMining { block: pack_pos(target.block), face: target.face() });
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

    // Predicted progress only: the block disappears when the server's `BlockChanged` arrives.
    progress.elapsed += time.delta_secs();
    if progress.elapsed >= progress.required {
        progress.elapsed = 0.0;
        // Ask again so a long dig keeps the server's job alive.
        start.write(StartMining { block: pack_pos(target.block), face: target.face() });
    }
    let _ = &player;
}

/// Asks the server to place a block on right click.
pub fn placing_system(
    buttons: Res<ButtonInput<MouseButton>>,
    targeting: Res<Targeting>,
    held: Res<HeldItem>,
    mut place: MessageWriter<PlaceBlock>,
) {
    if !buttons.just_pressed(MouseButton::Right) {
        return;
    }
    let (Some(target), Some(block)) = (targeting.target, held.place_block) else {
        return;
    };
    place.write(PlaceBlock { position: pack_pos(target.placement), block });
}
