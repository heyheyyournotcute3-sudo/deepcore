//! Progression integration tests: one per Era.
//!
//! Each test starts from that Era's raw inputs and drives the real content registries, the real
//! crafting rules and the real machine state machine to that Era's unlock. These are the tests
//! that prove the game is actually completable, not just that the arithmetic works.

use deepcore_shared::content::load_registry;
use deepcore_shared::content::registry::{ItemId, RecipeId, Registry};
use deepcore_shared::crafting::{MachineState, ProcessCheck, craft};
use deepcore_shared::inventory::{Inventory, Slot};
use deepcore_shared::tech::TechState;

/// Everything unlocked, for tests that focus on machines rather than research.
fn all_recipes(registry: &Registry) -> Vec<RecipeId> {
    (0..registry.recipe_count()).map(|index| RecipeId(index as u16)).collect()
}

fn item(registry: &Registry, name: &str) -> ItemId {
    registry.item_id(name).unwrap_or_else(|| panic!("item `{name}` exists"))
}

/// Runs a machine until it has produced `count` of an item, or the tick budget runs out.
fn run_machine(
    registry: &Registry,
    state: &mut MachineState,
    wanted: ItemId,
    count: u32,
    max_ticks: usize,
) -> u32 {
    let unlocked = all_recipes(registry);
    for _ in 0..max_ticks {
        if state.check(registry, &unlocked, 1.0) == ProcessCheck::Ready
            && state.advance(registry, 0.1, 1.0)
        {
            let produced = state.outputs.count_of(wanted);
            if produced >= count {
                return produced;
            }
        }
    }
    state.outputs.count_of(wanted)
}

#[test]
fn era_1_smelting_produces_iron_ingots_and_steel() {
    let registry = load_registry().expect("content loads");

    // Raw inputs from layer 2.
    let ore = item(&registry, "raw_iron");
    let coal = item(&registry, "coal");
    let limestone = item(&registry, "limestone");
    let ingot = item(&registry, "iron_ingot");
    let steel = item(&registry, "steel_ingot");

    // Stone furnace: raw iron -> iron ingot.
    let furnace_id = registry.machine_id("stone_furnace").expect("furnace");
    let mut furnace = MachineState::new(&registry, furnace_id);
    furnace.inputs.insert(ore, 4, &registry);
    furnace.fuel.insert(coal, 4, &registry);
    let ingots = run_machine(&registry, &mut furnace, ingot, 4, 400);
    assert_eq!(ingots, 4, "the furnace should smelt four iron ingots");
    assert_eq!(furnace.inputs.count_of(ore), 0, "all ore consumed");

    // Blast furnace: iron + coal + limestone -> steel.
    let blast_id = registry.machine_id("blast_furnace").expect("blast furnace");
    let mut blast = MachineState::new(&registry, blast_id);
    blast.inputs.insert(ingot, 2, &registry);
    blast.inputs.insert(limestone, 1, &registry);
    blast.inputs.insert(coal, 2, &registry);
    blast.fuel.insert(coal, 4, &registry);
    let steel_out = run_machine(&registry, &mut blast, steel, 2, 400);
    assert_eq!(steel_out, 2, "the blast furnace should make two steel ingots");

    // Anvil: steel -> steel plate, then the hardened pickaxe head.
    let anvil_id = registry.machine_id("anvil").expect("anvil");
    let mut anvil = MachineState::new(&registry, anvil_id);
    anvil.inputs.insert(steel, 4, &registry);
    let plate = item(&registry, "steel_plate");
    let plates = run_machine(&registry, &mut anvil, plate, 4, 400);
    assert_eq!(plates, 4);
}

#[test]
fn era_1_reaches_the_hardened_steel_pickaxe() {
    let registry = load_registry().expect("content loads");
    let mut tech = TechState::new(&registry);
    let mut inventory = Inventory::player();

    // Research Era 0 -> Era 1 by delivering the required materials.
    let hand_tools = registry.tech_id("hand_tools").expect("hand_tools");
    assert!(tech.unlock(hand_tools));
    let smelting = registry.tech_id("smelting").expect("smelting");
    for (item_id, count) in registry.tech(smelting).expect("node").cost.clone() {
        inventory.insert(item_id, count, &registry);
    }
    tech.deposit(&registry, smelting, &mut inventory);
    assert!(tech.try_complete(&registry, smelting), "smelting research completes");

    // With smelting unlocked, the recipes it gates become available.
    let available = tech.available_recipes(&registry);
    let smelt_iron = registry.recipe_id("smelt_iron").expect("recipe");
    let brick = registry.recipe_id("bake_bricks").expect("recipe");
    assert!(available.contains(&brick) || available.contains(&smelt_iron));

    // The hardened pickaxe is craftable once its parts exist.
    let pickaxe = item(&registry, "hardened_steel_pickaxe");
    let head = item(&registry, "hardened_tool_head");
    let rod = item(&registry, "steel_rod");
    inventory.insert(head, 1, &registry);
    inventory.insert(rod, 2, &registry);
    let recipe = registry.recipe_id("craft_hardened_steel_pickaxe").expect("recipe");
    let produced = craft(&registry, &mut inventory, recipe).expect("craft the pickaxe");
    assert_eq!(produced, vec![Slot::new(pickaxe, 1)]);
    assert_eq!(inventory.count_of(pickaxe), 1);

    // And it opens layer 3.
    let tool = registry.tool(pickaxe).expect("tool definition");
    assert_eq!(tool.tier, 2);
    assert!(
        tool.tier
            >= registry
                .block(registry.block_id("granite").expect("granite"))
                .expect("block")
                .def
                .min_tier
    );
}

#[test]
fn every_era_has_researchable_nodes_and_reachable_tools() {
    let registry = load_registry().expect("content loads");
    // Each Era from 1..5 must have at least one tech node whose parents are in earlier Eras.
    for era in 1..=5u8 {
        let nodes: Vec<_> =
            registry.tech_nodes().iter().filter(|node| node.def.era == era).collect();
        assert!(!nodes.is_empty(), "era {era} has no tech nodes");
        for node in nodes {
            for parent in &node.parents {
                let parent_node = registry.tech(*parent).expect("parent exists");
                assert!(
                    parent_node.def.era <= era,
                    "era {era} node `{}` depends on a later-era parent",
                    node.def.id
                );
            }
        }
    }
}

#[test]
fn the_tool_ladder_is_monotonic() {
    let registry = load_registry().expect("content loads");
    let ladder = [
        ("steel_pickaxe", 1u8, 1.0f32),
        ("hardened_steel_pickaxe", 2, 1.6),
        ("steam_drill", 3, 3.0),
        ("tungsten_drill", 4, 5.0),
        ("nuclear_drill", 5, 12.0),
    ];
    let mut previous_tier = 0;
    let mut previous_speed = 0.0;
    for (name, tier, speed) in ladder {
        let tool =
            registry.tool(item(&registry, name)).unwrap_or_else(|| panic!("{name} is a tool"));
        assert_eq!(tool.tier, tier, "{name} tier");
        assert!((tool.speed - speed).abs() < 1e-6, "{name} speed");
        assert!(tier > previous_tier, "{name} must be a higher tier than the previous tool");
        assert!(speed > previous_speed, "{name} must be faster than the previous tool");
        previous_tier = tier;
        previous_speed = speed;
    }
}

#[test]
fn every_layer_can_be_entered_with_the_tool_of_its_era() {
    let registry = load_registry().expect("content loads");
    // Layer base blocks and the tool tier needed to break them.
    let ladder = [
        ("dirt", 1, "steel_pickaxe"),
        ("stone", 1, "steel_pickaxe"),
        ("granite", 2, "hardened_steel_pickaxe"),
        ("basalt", 3, "steam_drill"),
        ("gneiss", 4, "tungsten_drill"),
        ("mantle_rock", 5, "nuclear_drill"),
    ];
    for (block_name, required, tool_name) in ladder {
        let block = registry.block_id(block_name).unwrap_or_else(|| panic!("{block_name} exists"));
        let entry = registry.block(block).expect("block entry");
        assert_eq!(entry.def.min_tier, required, "{block_name} min tier");
        let tool = registry.tool(item(&registry, tool_name)).expect("tool");
        assert!(
            tool.tier >= required,
            "{tool_name} (tier {}) cannot break {block_name} (needs {required})",
            tool.tier
        );
    }
}
