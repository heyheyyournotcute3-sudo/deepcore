//! M1 acceptance test: a player can dig from the surface to the bottom of layer 2, and is
//! stopped by layer 3 granite while still holding the steel pickaxe.
//!
//! This drives the *real* generator, the *real* chunk storage and the *real* mining rules —
//! no mocks — so it is a genuine progression test rather than a unit test of arithmetic.

use deepcore_shared::content::load_registry;
use deepcore_shared::mining::{MineBlocked, MineCheck, ToolProfile, check_mine};
use deepcore_shared::voxel::coords::{BlockPos, CHUNK_SIZE, ChunkPos, WORLD_CHUNKS_Y};
use deepcore_shared::voxel::world::VoxelWorld;
use deepcore_shared::worldgen::WorldGen;

/// Generates every chunk in the column above and below a position, so the test can inspect
/// the whole vertical slice.
fn column_world(generator: &WorldGen, x: i32, z: i32) -> VoxelWorld {
    let mut world = VoxelWorld::new();
    let cx = x.div_euclid(CHUNK_SIZE);
    let cz = z.div_euclid(CHUNK_SIZE);
    for cy in 0..WORLD_CHUNKS_Y {
        world
            .insert(ChunkPos::new(cx, cy, cz), generator.generate_chunk(ChunkPos::new(cx, cy, cz)));
    }
    world
}

#[test]
fn player_can_dig_to_layer_three_but_not_through_it() {
    let registry = load_registry().expect("content loads");
    let generator = WorldGen::new(0xDEE9_C0DE, registry.clone());
    let spawn = registry.world().spawn;
    let mut world = column_world(&generator, spawn.0, spawn.2);

    let steel = ToolProfile::for_item(&registry, registry.item_id("steel_pickaxe"), true);
    let hardened =
        ToolProfile::for_item(&registry, registry.item_id("hardened_steel_pickaxe"), true);
    assert_eq!(steel.tier, 1);
    assert_eq!(hardened.tier, 2);

    // Dig a shaft straight down from the surface.
    let surface = world.surface_y(spawn.0, spawn.2).expect("surface exists");
    let mut dug = 0;
    let mut blocked_at = None;
    for y in (0..surface).rev() {
        let pos = BlockPos::new(spawn.0, y, spawn.2);
        let block = world.block(pos);
        if block == deepcore_shared::voxel::AIR {
            continue;
        }
        match check_mine(&registry, block, &steel) {
            MineCheck::Allowed { .. } => {
                world.set_block(pos, deepcore_shared::voxel::AIR);
                dug += 1;
            }
            MineCheck::Blocked(MineBlocked::TierTooLow { required }) => {
                blocked_at = Some((y, required, registry.block_name(block).to_owned()));
                break;
            }
            MineCheck::Blocked(other) => panic!("unexpected block reason {other:?} at y={y}"),
        }
    }

    let (blocked_y, required_tier, block_name) =
        blocked_at.expect("the steel pickaxe must be stopped somewhere on the way down");
    assert_eq!(required_tier, 2, "the block that stops tier 1 must require tier 2");
    assert_eq!(block_name, "Granite", "layer 3 is the granite layer");

    let depth = registry.depth_of(blocked_y);
    assert!(
        (64..=160).contains(&depth),
        "granite should sit in the Granite Depths (64..160), stopped at depth {depth}"
    );
    // The shaft is at least the whole of layer 2 (depth 16..64) plus layer 1.
    assert!(dug > 40, "the player should have dug a real shaft, only broke {dug} blocks");

    // The hardened steel pickaxe can break that same block.
    match check_mine(&registry, registry.block_id("granite").expect("granite"), &hardened) {
        MineCheck::Allowed { seconds, .. } => {
            assert!(
                seconds < 2.0,
                "granite should take under 2s with hardened steel, got {seconds}"
            );
        }
        other => panic!("hardened steel must mine granite, got {other:?}"),
    }

    // And digging resumes: a hardened pickaxe gets past the granite into layer 3.
    let granite = registry.block_id("granite").expect("granite");
    let mut dug_into_layer_three = 0;
    for y in (0..blocked_y).rev() {
        let pos = BlockPos::new(spawn.0, y, spawn.2);
        let block = world.block(pos);
        if block == deepcore_shared::voxel::AIR {
            continue;
        }
        if matches!(check_mine(&registry, block, &hardened), MineCheck::Allowed { .. }) {
            world.set_block(pos, deepcore_shared::voxel::AIR);
            if block == granite {
                dug_into_layer_three += 1;
            }
        } else {
            break;
        }
    }
    assert!(dug_into_layer_three > 0, "hardened steel should break granite blocks");
}

#[test]
fn every_layer_is_reachable_with_its_tool_tier() {
    let registry = load_registry().expect("content loads");
    // The tier ladder from the brief: each layer's base rock needs exactly its tool tier.
    let ladder = [
        ("dirt", 1),
        ("stone", 1),
        ("granite", 2),
        ("basalt", 3),
        ("gneiss", 4),
        ("mantle_rock", 5),
    ];
    for (block_name, expected_tier) in ladder {
        let block = registry.block_id(block_name).unwrap_or_else(|| panic!("{block_name} exists"));
        let entry = registry.block(block).expect("block entry");
        assert_eq!(
            entry.def.min_tier, expected_tier,
            "{block_name} should require tier {expected_tier}"
        );
    }
}

#[test]
fn bedrock_is_a_hard_floor_at_the_bottom_of_the_world() {
    let registry = load_registry().expect("content loads");
    let generator = WorldGen::new(7, registry.clone());
    let chunk = generator.generate_chunk(ChunkPos::new(10, 0, 10));
    let bedrock = registry.block_id("bedrock").expect("bedrock");
    // The bottom chunk is the bedrock shell: floor plus the unbreakable wall ring.
    let thickness = registry.world().bedrock_thickness;
    let blocks = chunk.to_blocks();
    for y in 0..thickness {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let index = deepcore_shared::voxel::LocalPos::new(x, y, z).index();
                assert_eq!(blocks[index], bedrock, "bedrock floor missing at ({x},{y},{z})");
            }
        }
    }

    let nuclear = ToolProfile::for_item(&registry, registry.item_id("nuclear_drill"), true);
    assert_eq!(
        check_mine(&registry, bedrock, &nuclear),
        MineCheck::Blocked(MineBlocked::Unbreakable)
    );
}
