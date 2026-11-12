//! Mining rules: break time, tool tiers, reach and voxel raycasting.
//!
//! The brief fixes the formula:
//!
//! ```text
//! break_time = hardness / (tool_speed * power_multiplier)
//! ```
//!
//! rounded to whole server ticks. Everything here is shared so the client's predicted crack
//! animation finishes at exactly the moment the server removes the block.

use bevy::math::Vec3;

use crate::content::model::{ItemCategory, ToolDef};
use crate::content::registry::{ItemId, Registry};
use crate::voxel::coords::{BlockPos, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};
use crate::voxel::world::VoxelWorld;

/// Reach in blocks for an ordinary tool.
pub const DEFAULT_REACH: f32 = 5.0;
/// Reach in blocks for area tools such as the nuclear drill.
pub const DRILL_REACH: f32 = 6.0;

/// What a player is currently holding, resolved for mining.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolProfile {
    /// Mining tier; must be >= a block's minimum tier.
    pub tier: u8,
    /// Speed multiplier.
    pub speed: f32,
    /// Face area mined per action.
    pub area: (u8, u8),
    /// Reach in blocks.
    pub reach: f32,
    /// Power multiplier from the tool's energy source being charged and full.
    pub power_multiplier: f32,
}

impl ToolProfile {
    /// The bare hands profile: tier 1, speed 1.0. Used when the hand slot is empty.
    pub const HANDS: ToolProfile = ToolProfile {
        tier: 1,
        speed: 1.0,
        area: (1, 1),
        reach: DEFAULT_REACH,
        power_multiplier: 1.0,
    };

    /// Builds a profile from a tool definition, given whether its energy source is charged.
    pub fn from_tool(tool: &ToolDef, charged: bool) -> Self {
        let power_multiplier = match tool.energy {
            crate::content::model::ToolEnergy::None => 1.0,
            _ if charged => 1.0,
            _ => 0.0,
        };
        Self {
            tier: tool.tier,
            speed: tool.speed,
            area: tool.area,
            reach: if tool.area.0 > 1 || tool.area.1 > 1 { DRILL_REACH } else { tool.reach },
            power_multiplier,
        }
    }

    /// The profile of whatever item is held, falling back to bare hands.
    pub fn for_item(registry: &Registry, item: Option<ItemId>, charged: bool) -> Self {
        match item.and_then(|id| registry.tool(id)) {
            Some(tool) => Self::from_tool(tool, charged),
            None => Self::HANDS,
        }
    }

    /// True if this tool mines more than one block per action.
    pub fn is_area_tool(&self) -> bool {
        self.area.0 > 1 || self.area.1 > 1
    }
}

/// Why a block cannot be mined right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MineBlocked {
    /// The block is unbreakable (bedrock, water).
    Unbreakable,
    /// The tool tier is too low. The client shows "Too hard — needs Tier N".
    TierTooLow {
        /// The tier the block requires.
        required: u8,
    },
    /// The tool has no energy (empty steam canister, flat battery, spent core cell).
    NoEnergy,
}

/// The result of evaluating a mining attempt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MineCheck {
    /// Mining is allowed; break time in seconds and in whole ticks.
    Allowed {
        /// Seconds to break the block.
        seconds: f32,
        /// Whole server ticks to break it (at [`crate::SERVER_TICK_HZ`]).
        ticks: u32,
    },
    /// Mining is not allowed, with the reason.
    Blocked(MineBlocked),
}

/// Break time in seconds for a block and tool.
///
/// `power_multiplier` is the multiplier from the tool's power source; it is folded into the
/// tool speed here so both sides compute the same number.
pub fn break_time(hardness: f32, tool_speed: f32, power_multiplier: f32) -> f32 {
    let divisor = tool_speed * power_multiplier;
    if divisor <= 0.0 || hardness <= 0.0 {
        return f32::INFINITY;
    }
    hardness / divisor
}

/// Converts seconds into whole server ticks, rounding up so a break never finishes early.
pub fn seconds_to_ticks(seconds: f32, tick_hz: f64) -> u32 {
    if !seconds.is_finite() {
        return u32::MAX;
    }
    (seconds as f64 * tick_hz).ceil().max(1.0) as u32
}

/// Evaluates whether a tool can mine a block, and how long it takes.
pub fn check_mine(
    registry: &Registry,
    block: crate::voxel::BlockId,
    tool: &ToolProfile,
) -> MineCheck {
    let Some(entry) = registry.block(block) else {
        return MineCheck::Blocked(MineBlocked::Unbreakable);
    };
    if !entry.def.breakable {
        return MineCheck::Blocked(MineBlocked::Unbreakable);
    }
    if tool.power_multiplier <= 0.0 {
        return MineCheck::Blocked(MineBlocked::NoEnergy);
    }
    if tool.tier < entry.def.min_tier {
        return MineCheck::Blocked(MineBlocked::TierTooLow { required: entry.def.min_tier });
    }
    let seconds = break_time(entry.def.hardness, tool.speed, tool.power_multiplier);
    MineCheck::Allowed { seconds, ticks: seconds_to_ticks(seconds, crate::SERVER_TICK_HZ) }
}

/// A targeted block: which block, which face was hit, and where on the face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    /// The block that was hit.
    pub block: BlockPos,
    /// The face normal, pointing back towards the player.
    pub normal: [i32; 3],
    /// Distance from the ray origin.
    pub distance: f32,
    /// The block adjacent to the hit face, i.e. where a placed block would go.
    pub placement: BlockPos,
}

impl Target {
    /// The face index 0..6 used by the protocol.
    pub fn face(&self) -> u8 {
        match self.normal {
            [1, 0, 0] => 0,
            [-1, 0, 0] => 1,
            [0, 1, 0] => 2,
            [0, -1, 0] => 3,
            [0, 0, 1] => 4,
            _ => 5,
        }
    }
}

/// Face normal for a protocol face index.
pub fn face_normal(face: u8) -> [i32; 3] {
    match face {
        0 => [1, 0, 0],
        1 => [-1, 0, 0],
        2 => [0, 1, 0],
        3 => [0, -1, 0],
        4 => [0, 0, 1],
        _ => [0, 0, -1],
    }
}

/// Casts a ray through the voxel world and returns the first non-air block it hits.
///
/// This is an Amanatides–Woo voxel traversal: it visits every block the ray passes through
/// in order, so it is exact (no missed blocks, no false hits) and cheap.
pub fn raycast(
    world: &VoxelWorld,
    origin: Vec3,
    direction: Vec3,
    max_distance: f32,
    skip_passable: impl Fn(crate::voxel::BlockId) -> bool,
) -> Option<Target> {
    if direction.length_squared() < 1e-9 {
        return None;
    }
    let direction = direction.normalize();

    let mut x = origin.x.floor() as i32;
    let mut y = origin.y.floor() as i32;
    let mut z = origin.z.floor() as i32;

    let step_x = if direction.x > 0.0 { 1 } else { -1 };
    let step_y = if direction.y > 0.0 { 1 } else { -1 };
    let step_z = if direction.z > 0.0 { 1 } else { -1 };

    let t_delta_x =
        if direction.x.abs() < 1e-9 { f32::INFINITY } else { (1.0 / direction.x).abs() };
    let t_delta_y =
        if direction.y.abs() < 1e-9 { f32::INFINITY } else { (1.0 / direction.y).abs() };
    let t_delta_z =
        if direction.z.abs() < 1e-9 { f32::INFINITY } else { (1.0 / direction.z).abs() };

    let boundary = |cell: i32, step: i32, origin: f32| -> f32 {
        if step > 0 { (cell + 1) as f32 - origin } else { origin - cell as f32 }
    };
    let mut t_max_x = boundary(x, step_x, origin.x) * t_delta_x;
    let mut t_max_y = boundary(y, step_y, origin.y) * t_delta_y;
    let mut t_max_z = boundary(z, step_z, origin.z) * t_delta_z;

    let mut normal = [0, 0, 0];
    let mut travelled = 0.0f32;

    // The origin block itself can be solid (e.g. the player's head inside a placed block).
    loop {
        let pos = BlockPos::new(x, y, z);
        if pos.in_world() {
            let block = world.block(pos);
            if block != crate::voxel::AIR && !skip_passable(block) {
                return Some(Target {
                    block: pos,
                    normal,
                    distance: travelled,
                    placement: pos.offset(normal[0], normal[1], normal[2]),
                });
            }
        } else if travelled > 0.0 {
            // Left the world: nothing to hit.
            return None;
        }

        if t_max_x < t_max_y && t_max_x < t_max_z {
            x += step_x;
            travelled = t_max_x;
            t_max_x += t_delta_x;
            normal = [-step_x, 0, 0];
        } else if t_max_y < t_max_z {
            y += step_y;
            travelled = t_max_y;
            t_max_y += t_delta_y;
            normal = [0, -step_y, 0];
        } else {
            z += step_z;
            travelled = t_max_z;
            t_max_z += t_delta_z;
            normal = [0, 0, -step_z];
        }

        if travelled > max_distance {
            return None;
        }
        if x < -1 || y < -1 || z < -1 || x > WORLD_SIZE_X || y > WORLD_SIZE_Y || z > WORLD_SIZE_Z {
            return None;
        }
    }
}

/// The blocks an area tool breaks when hitting a face.
///
/// A 3x3 drill hitting the floor breaks a 3x3 patch of floor; hitting a wall breaks a 3x3
/// patch of that wall. The patch is centred on the hit block and lies in the plane of the
/// face.
pub fn area_blocks(center: BlockPos, normal: [i32; 3], area: (u8, u8)) -> Vec<BlockPos> {
    let (width, height) = (area.0.max(1) as i32, area.1.max(1) as i32);
    if width == 1 && height == 1 {
        return vec![center];
    }

    // Two tangent axes spanning the face plane.
    let (u, v) = match normal {
        [1, 0, 0] | [-1, 0, 0] => ([0, 0, 1], [0, 1, 0]),
        [0, 1, 0] | [0, -1, 0] => ([1, 0, 0], [0, 0, 1]),
        _ => ([1, 0, 0], [0, 1, 0]),
    };

    let mut blocks = Vec::with_capacity((width * height) as usize);
    let half_u = (width - 1) / 2;
    let half_v = (height - 1) / 2;
    for dv in 0..height {
        for du in 0..width {
            let offset_u = du - half_u;
            let offset_v = dv - half_v;
            blocks.push(center.offset(
                u[0] * offset_u + v[0] * offset_v,
                u[1] * offset_u + v[1] * offset_v,
                u[2] * offset_u + v[2] * offset_v,
            ));
        }
    }
    blocks
}

/// True if an item is a tool, used by the inventory to decide the hand slot.
pub fn is_tool(registry: &Registry, item: ItemId) -> bool {
    matches!(registry.item(item).map(|e| &e.def.category), Some(ItemCategory::Tool(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::load_registry;
    use crate::voxel::chunk::Chunk;
    use crate::voxel::coords::ChunkPos;

    #[test]
    fn break_time_matches_the_formula() {
        // hardness 2.5 with a tier-1 tool at speed 1.0 -> 2.5 seconds.
        assert!((break_time(2.5, 1.0, 1.0) - 2.5).abs() < 1e-6);
        // Hardened steel pickaxe: speed 1.6 -> 2.5 / 1.6 seconds.
        assert!((break_time(2.5, 1.6, 1.0) - 1.5625).abs() < 1e-4);
        // Nuclear drill: 12x speed on granite.
        assert!((break_time(2.5, 12.0, 1.0) - 0.2083).abs() < 1e-3);
        // No power means it never breaks.
        assert!(break_time(2.5, 3.0, 0.0).is_infinite());
    }

    #[test]
    fn ticks_round_up_so_breaks_never_finish_early() {
        assert_eq!(seconds_to_ticks(1.0, 30.0), 30);
        assert_eq!(seconds_to_ticks(0.01, 30.0), 1);
        assert_eq!(seconds_to_ticks(1.01, 30.0), 31);
        assert_eq!(seconds_to_ticks(f32::INFINITY, 30.0), u32::MAX);
    }

    #[test]
    fn tier_gate_blocks_lower_tools() {
        let registry = load_registry().expect("content");
        let granite = registry.block_id("granite").expect("granite");
        let steel =
            ToolProfile { tier: 1, speed: 1.0, area: (1, 1), reach: 5.0, power_multiplier: 1.0 };
        assert_eq!(
            check_mine(&registry, granite, &steel),
            MineCheck::Blocked(MineBlocked::TierTooLow { required: 2 })
        );

        let hardened = ToolProfile { tier: 2, speed: 1.6, ..steel };
        match check_mine(&registry, granite, &hardened) {
            MineCheck::Allowed { seconds, ticks } => {
                assert!((seconds - 1.5625).abs() < 1e-4);
                assert_eq!(ticks, 47);
            }
            other => panic!("expected allowed, got {other:?}"),
        }
    }

    #[test]
    fn bedrock_is_unbreakable_even_with_the_best_tool() {
        let registry = load_registry().expect("content");
        let bedrock = registry.block_id("bedrock").expect("bedrock");
        let nuclear =
            ToolProfile { tier: 5, speed: 12.0, area: (3, 3), reach: 6.0, power_multiplier: 1.0 };
        assert_eq!(
            check_mine(&registry, bedrock, &nuclear),
            MineCheck::Blocked(MineBlocked::Unbreakable)
        );
    }

    #[test]
    fn unpowered_tool_reports_no_energy() {
        let registry = load_registry().expect("content");
        let dirt = registry.block_id("dirt").expect("dirt");
        let flat =
            ToolProfile { tier: 3, speed: 3.0, area: (1, 1), reach: 5.0, power_multiplier: 0.0 };
        assert_eq!(check_mine(&registry, dirt, &flat), MineCheck::Blocked(MineBlocked::NoEnergy));
    }

    #[test]
    fn tool_profiles_come_from_content() {
        let registry = load_registry().expect("content");
        let drill = registry.item_id("nuclear_drill").expect("nuclear drill");
        let profile = ToolProfile::for_item(&registry, Some(drill), true);
        assert_eq!(profile.tier, 5);
        assert_eq!(profile.area, (3, 3));
        assert_eq!(profile.reach, DRILL_REACH);
        assert!(profile.is_area_tool());

        let none = ToolProfile::for_item(&registry, None, true);
        assert_eq!(none, ToolProfile::HANDS);
    }

    fn single_block_world(block: BlockPos, id: crate::voxel::BlockId) -> VoxelWorld {
        let mut world = VoxelWorld::new();
        let chunk_pos = block.chunk();
        world.insert(chunk_pos, Chunk::filled(0));
        world.set_block(block, id);
        world
    }

    #[test]
    fn raycast_hits_the_expected_block_and_face() {
        let world = single_block_world(BlockPos::new(5, 5, 5), 1);
        let origin = Vec3::new(2.5, 5.5, 5.5);
        let target = raycast(&world, origin, Vec3::X, 10.0, |_| false).expect("hit");
        assert_eq!(target.block, BlockPos::new(5, 5, 5));
        assert_eq!(target.normal, [-1, 0, 0]);
        assert_eq!(target.placement, BlockPos::new(4, 5, 5));
        assert!((target.distance - 2.5).abs() < 1e-3);
    }

    #[test]
    fn raycast_misses_when_out_of_range() {
        let world = single_block_world(BlockPos::new(5, 5, 5), 1);
        let origin = Vec3::new(2.5, 5.5, 5.5);
        assert!(raycast(&world, origin, Vec3::X, 2.0, |_| false).is_none());
    }

    #[test]
    fn raycast_skips_passable_blocks() {
        let mut world = VoxelWorld::new();
        world.insert(ChunkPos::new(0, 0, 0), Chunk::filled(0));
        world.set_block(BlockPos::new(3, 5, 5), 2); // passable
        world.set_block(BlockPos::new(5, 5, 5), 1); // solid
        let target =
            raycast(&world, Vec3::new(2.5, 5.5, 5.5), Vec3::X, 10.0, |b| b == 2).expect("hit");
        assert_eq!(target.block, BlockPos::new(5, 5, 5));
    }

    #[test]
    fn raycast_handles_all_six_faces() {
        let block = BlockPos::new(5, 5, 5);
        // Face index convention: 0 = +X, 1 = -X, 2 = +Y, 3 = -Y, 4 = +Z, 5 = -Z, where the
        // index names the face that was hit, and `normal` is that face's outward normal.
        for (direction, expected_face) in [
            (Vec3::NEG_X, 0u8),
            (Vec3::X, 1),
            (Vec3::NEG_Y, 2),
            (Vec3::Y, 3),
            (Vec3::NEG_Z, 4),
            (Vec3::Z, 5),
        ] {
            let world = single_block_world(block, 1);
            let origin = block.center_f32() - direction * 3.0;
            let target = raycast(&world, origin, direction, 10.0, |_| false).expect("hit");
            assert_eq!(target.face(), expected_face, "wrong face for {direction:?}");
            assert_eq!(face_normal(expected_face), target.normal);
            assert_ne!(target.placement, target.block);
            assert_eq!(
                (target.placement.x - target.block.x).abs()
                    + (target.placement.y - target.block.y).abs()
                    + (target.placement.z - target.block.z).abs(),
                1
            );
        }
    }

    #[test]
    fn area_blocks_form_a_3x3_face() {
        let center = BlockPos::new(10, 10, 10);
        // Hitting the top face breaks a 3x3 patch of floor around the hit block.
        let floor = area_blocks(center, [0, 1, 0], (3, 3));
        assert_eq!(floor.len(), 9);
        assert!(floor.contains(&center));
        assert!(floor.iter().all(|p| p.y == center.y));
        assert!(floor.contains(&center.offset(1, 0, 1)));
        assert!(floor.contains(&center.offset(-1, 0, -1)));

        // Hitting a wall keeps the patch in the wall plane.
        let wall = area_blocks(center, [1, 0, 0], (3, 3));
        assert_eq!(wall.len(), 9);
        assert!(wall.iter().all(|p| p.x == center.x));

        // A 1x1 tool is just the one block.
        assert_eq!(area_blocks(center, [0, 1, 0], (1, 1)), vec![center]);
    }
}
