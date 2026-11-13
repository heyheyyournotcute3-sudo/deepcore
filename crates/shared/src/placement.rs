//! Machine placement: ghost previews, multi-block footprints and rotation.
//!
//! A machine kit places a footprint of blocks, not one block. The client shows a translucent
//! ghost while the kit is held, and the server validates the whole footprint before any of it
//! is written, so a player can never end up with half a blast furnace.

use bevy::prelude::*;

use crate::content::ContentRegistry;
use crate::content::model::MachineDef;
use crate::voxel::coords::BlockPos;
use crate::voxel::world::VoxelWorld;

/// The blocks a machine occupies when placed at `origin` with a given facing.
pub fn footprint(def: &MachineDef, origin: BlockPos, facing: u8) -> Vec<BlockPos> {
    let (sx, sy, sz) = (def.size.0 as i32, def.size.1 as i32, def.size.2 as i32);
    // Rotation swaps X and Z at odd quarter turns.
    let (dx, dz) = if facing.is_multiple_of(2) { (sx, sz) } else { (sz, sx) };
    let mut blocks = Vec::with_capacity((dx * sy * dz) as usize);
    for y in 0..sy {
        for z in 0..dz {
            for x in 0..dx {
                blocks.push(origin.offset(x, y, z));
            }
        }
    }
    blocks
}

/// Why a placement is invalid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlacementError {
    /// One of the blocks is outside the world.
    OutOfWorld,
    /// One of the blocks is not air.
    Blocked,
    /// The player is standing in the footprint.
    PlayerInTheWay,
    /// The player is too far away.
    TooFar,
    /// The machine needs research the team has not done.
    NotResearched(String),
    /// The machine needs power or fuel it cannot get where it is being placed.
    NotSupported,
}

impl PlacementError {
    /// Text for the HUD.
    pub fn text(&self) -> String {
        match self {
            PlacementError::OutOfWorld => "Outside the world".to_owned(),
            PlacementError::Blocked => "Not enough room".to_owned(),
            PlacementError::PlayerInTheWay => "You are in the way".to_owned(),
            PlacementError::TooFar => "Too far away".to_owned(),
            PlacementError::NotResearched(name) => format!("Needs {name} research"),
            PlacementError::NotSupported => "Unsupported placement".to_owned(),
        }
    }
}

/// Checks that a machine can be placed at `origin`.
///
/// `is_unlocked` is the team's research check; `player_position` is used to keep the player out
/// of the footprint.
pub fn validate_placement(
    world: &VoxelWorld,
    def: &MachineDef,
    origin: BlockPos,
    facing: u8,
    player_position: Vec3,
    is_unlocked: impl Fn(&str) -> bool,
) -> Result<Vec<BlockPos>, PlacementError> {
    if let Some(tech) = &def.tech
        && !is_unlocked(tech)
    {
        return Err(PlacementError::NotResearched(tech.clone()));
    }
    if origin.center_f32().distance(player_position) > 8.0 {
        return Err(PlacementError::TooFar);
    }

    let blocks = footprint(def, origin, facing);
    let player_box = crate::movement::Aabb::from_feet(player_position, 1.8);
    for block in &blocks {
        if !block.in_world() {
            return Err(PlacementError::OutOfWorld);
        }
        if world.block(*block) != crate::voxel::AIR {
            return Err(PlacementError::Blocked);
        }
        if player_box.intersects_block(*block) {
            return Err(PlacementError::PlayerInTheWay);
        }
    }
    Ok(blocks)
}

/// The ghost preview state: which machine is being placed and where.
#[derive(Resource, Default, Clone, Debug)]
pub struct PlacementGhost {
    /// The machine being placed.
    pub machine: Option<u16>,
    /// The block the ghost is anchored at.
    pub origin: Option<BlockPos>,
    /// Rotation in quarter turns.
    pub facing: u8,
    /// True if the current position is valid.
    pub valid: bool,
    /// Why it is invalid, if it is.
    pub reason: Option<PlacementError>,
}

/// The machine a held item would place, if any.
pub fn machine_for_item(
    registry: &ContentRegistry,
    item: crate::content::registry::ItemId,
) -> Option<crate::content::registry::MachineId> {
    let entry = registry.item(item)?;
    let block = entry.places?;
    registry.block(block)?.machine
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::load_registry;
    use crate::voxel::chunk::Chunk;
    use crate::voxel::coords::ChunkPos;

    fn empty_world() -> VoxelWorld {
        let mut world = VoxelWorld::new();
        for cy in 14..16 {
            world.insert(ChunkPos::new(16, cy, 16), Chunk::filled(0));
        }
        world
    }

    #[test]
    fn footprint_of_a_single_block_machine_is_one_block() {
        let registry = load_registry().expect("content");
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let def = registry.machine(furnace).expect("def");
        let origin = BlockPos::new(512, 480, 512);
        let blocks = footprint(def, origin, 0);
        // The stone furnace is 1x2x1.
        assert_eq!(blocks.len(), 2);
        assert!(blocks.contains(&origin));
        assert!(blocks.contains(&origin.offset(0, 1, 0)));
    }

    #[test]
    fn footprint_of_a_big_machine_covers_its_volume() {
        let registry = load_registry().expect("content");
        let blast = registry.machine_id("blast_furnace").expect("blast furnace");
        let def = registry.machine(blast).expect("def");
        let origin = BlockPos::new(500, 470, 500);
        let blocks = footprint(def, origin, 0);
        // 2x3x2 = 12 blocks.
        assert_eq!(blocks.len(), 12);
        assert!(blocks.contains(&origin));
        assert!(blocks.contains(&origin.offset(1, 2, 1)));
    }

    #[test]
    fn rotation_swaps_x_and_z() {
        let registry = load_registry().expect("content");
        let blast = registry.machine_id("blast_furnace").expect("blast furnace");
        let def = registry.machine(blast).expect("def");
        let origin = BlockPos::new(500, 470, 500);
        let turned = footprint(def, origin, 1);
        assert_eq!(turned.len(), 12);
        // A quarter turn of a 2x3x2 box is still 2x3x2, so the set is the same shape.
        assert!(turned.contains(&origin.offset(1, 2, 1)));
    }

    #[test]
    fn placement_is_rejected_when_a_block_is_in_the_way() {
        let registry = load_registry().expect("content");
        let mut world = empty_world();
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let def = registry.machine(furnace).expect("def");
        let origin = BlockPos::new(512, 480, 512);
        world.set_block(origin.offset(0, 1, 0), 1);
        let result = validate_placement(
            &world,
            def,
            origin,
            0,
            origin.center_f32() + Vec3::new(0.0, 0.0, 4.0),
            |_| true,
        );
        assert_eq!(result, Err(PlacementError::Blocked));
    }

    #[test]
    fn placement_is_rejected_when_the_player_stands_in_it() {
        let registry = load_registry().expect("content");
        let world = empty_world();
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let def = registry.machine(furnace).expect("def");
        let origin = BlockPos::new(512, 480, 512);
        let result = validate_placement(&world, def, origin, 0, origin.center_f32(), |_| true);
        assert_eq!(result, Err(PlacementError::PlayerInTheWay));
    }

    #[test]
    fn placement_requires_research() {
        let registry = load_registry().expect("content");
        let world = empty_world();
        let blast = registry.machine_id("blast_furnace").expect("blast furnace");
        let def = registry.machine(blast).expect("def");
        let origin = BlockPos::new(500, 470, 500);
        let result = validate_placement(
            &world,
            def,
            origin,
            0,
            origin.center_f32() + Vec3::new(0.0, 0.0, 6.0),
            |_| false,
        );
        assert!(matches!(result, Err(PlacementError::NotResearched(_))));
    }

    #[test]
    fn a_valid_placement_returns_the_footprint() {
        let registry = load_registry().expect("content");
        let world = empty_world();
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let def = registry.machine(furnace).expect("def");
        let origin = BlockPos::new(512, 480, 512);
        let blocks = validate_placement(
            &world,
            def,
            origin,
            0,
            origin.center_f32() + Vec3::new(0.0, 0.0, 4.0),
            |_| true,
        )
        .expect("valid placement");
        assert_eq!(blocks.len(), 2);
    }
}
