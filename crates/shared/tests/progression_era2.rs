//! Era 2 progression: steam power, ore processing yields and the Steam Drill.

use deepcore_shared::content::load_registry;
use deepcore_shared::content::registry::{ItemId, RecipeId, Registry};
use deepcore_shared::crafting::{MachineState, ProcessCheck, craft};
use deepcore_shared::inventory::Inventory;
use deepcore_shared::power::{NetworkKind, NetworkNode, PowerGrids};

fn all_recipes(registry: &Registry) -> Vec<RecipeId> {
    (0..registry.recipe_count()).map(|index| RecipeId(index as u16)).collect()
}

fn item(registry: &Registry, name: &str) -> ItemId {
    registry.item_id(name).unwrap_or_else(|| panic!("item `{name}` exists"))
}

#[test]
fn ore_processing_increases_yield_per_ore() {
    let registry = load_registry().expect("content");
    let unlocked = all_recipes(&registry);
    let ore = item(&registry, "raw_iron");
    let ingot = item(&registry, "iron_ingot");

    // Baseline: smelting raw ore directly gives one ingot per ore.
    let furnace = registry.machine_id("stone_furnace").expect("furnace");
    let mut direct = MachineState::new(&registry, furnace);
    direct.inputs.insert(ore, 4, &registry);
    direct.fuel.insert(item(&registry, "coal"), 8, &registry);
    let mut produced_direct = 0;
    for _ in 0..600 {
        if direct.check(&registry, &unlocked, 1.0) == ProcessCheck::Ready
            && direct.advance(&registry, 0.1, 1.0)
        {
            produced_direct = direct.outputs.count_of(ingot);
        }
    }
    assert_eq!(produced_direct, 4, "4 ore -> 4 ingots without processing");

    // Crushing first: 2 ore -> 3 crushed, which smelt into 3 ingots (1.5x).
    let crusher = registry.machine_id("crusher").expect("crusher");
    let mut crushing = MachineState::new(&registry, crusher);
    crushing.inputs.insert(ore, 2, &registry);
    let crushed = item(&registry, "crushed_iron");
    let mut crushed_out = 0;
    for _ in 0..400 {
        if crushing.check(&registry, &unlocked, 1.0) == ProcessCheck::Ready
            && crushing.advance(&registry, 0.1, 1.0)
        {
            crushed_out = crushing.outputs.count_of(crushed);
        }
    }
    assert_eq!(crushed_out, 3, "crushing 2 ore yields 3 crushed ore");

    // Washing the crushed ore doubles the ingots per ore again (2.0x).
    let washer = registry.machine_id("ore_washer").expect("washer");
    let mut washing = MachineState::new(&registry, washer);
    washing.inputs.insert(crushed, 2, &registry);
    let washed = item(&registry, "washed_iron");
    let mut washed_out = 0;
    for _ in 0..400 {
        if washing.check(&registry, &unlocked, 1.0) == ProcessCheck::Ready
            && washing.advance(&registry, 0.1, 1.0)
        {
            washed_out = washing.outputs.count_of(washed);
        }
    }
    assert_eq!(washed_out, 2, "washing 2 crushed ore yields 2 concentrate");

    // And the washed concentrate smelts into 4 ingots from 2 ore's worth of concentrate.
    let mut smelting = MachineState::new(&registry, furnace);
    smelting.inputs.insert(washed, 2, &registry);
    smelting.fuel.insert(item(&registry, "coal"), 4, &registry);
    let mut produced_washed = 0;
    for _ in 0..600 {
        if smelting.check(&registry, &unlocked, 1.0) == ProcessCheck::Ready
            && smelting.advance(&registry, 0.1, 1.0)
        {
            produced_washed = smelting.outputs.count_of(ingot);
        }
    }
    assert_eq!(produced_washed, 4, "washed ore doubles the ingots: 2 ore -> 4 ingots");
    assert!(produced_washed > produced_direct / 2, "washing beats direct smelting");
}

#[test]
fn steam_boiler_powers_a_steam_engine() {
    let mut grids = PowerGrids::new();
    let boiler = deepcore_shared::voxel::BlockPos::new(0, 64, 0);
    let engine = deepcore_shared::voxel::BlockPos::new(2, 64, 0);
    let pipe_a = deepcore_shared::voxel::BlockPos::new(1, 64, 0);
    // Pipe between them, with the boiler and engine adjacent to it.
    let conductors = vec![(NetworkKind::Steam, pipe_a)];
    let machines = vec![NetworkNode::producer(boiler, 20.0), NetworkNode::consumer(engine, 4.0)];
    grids.rebuild(&conductors, &machines);
    let network = grids.network_at(NetworkKind::Steam, pipe_a).expect("the pipe forms a network");
    assert!(network.supply >= 20.0);
    assert!(network.demand >= 4.0);
    assert_eq!(network.satisfaction(), 1.0, "the boiler easily drives the engine");
}

#[test]
fn the_steam_drill_is_craftable_and_opens_layer_4() {
    let registry = load_registry().expect("content");
    let mut inventory = Inventory::player();
    // Its parts, as they would come from the Era 2 machines.
    inventory.insert(item(&registry, "steel_ingot"), 12, &registry);
    inventory.insert(item(&registry, "brass_ingot"), 6, &registry);
    inventory.insert(item(&registry, "copper_pipe"), 6, &registry);
    inventory.insert(item(&registry, "bronze_gear"), 2, &registry);

    let recipe = registry.recipe_id("craft_steam_drill").expect("recipe");
    craft(&registry, &mut inventory, recipe).expect("craft the steam drill");
    let drill = item(&registry, "steam_drill");
    assert_eq!(inventory.count_of(drill), 1);

    let tool = registry.tool(drill).expect("tool");
    assert_eq!(tool.tier, 3, "the steam drill is tier 3");
    assert!((tool.speed - 3.0).abs() < 1e-6);

    // Tier 3 opens the Basalt Furnace (layer 4).
    let basalt = registry.block_id("basalt").expect("basalt");
    let entry = registry.block(basalt).expect("block");
    assert_eq!(entry.def.min_tier, 3);
    assert!(tool.tier >= entry.def.min_tier);

    // And it needs steam to run.
    assert!(matches!(tool.energy, deepcore_shared::content::model::ToolEnergy::Steam { .. }));
}

#[test]
fn era_two_machines_exist_and_are_gated_by_steam_research() {
    let registry = load_registry().expect("content");
    let steam = registry.tech_id("steam_power").expect("steam_power node");
    let unlocked: Vec<&str> =
        registry.tech(steam).expect("node").def.unlocks.iter().map(String::as_str).collect();
    for machine in ["steam_boiler", "steam_engine", "hand_pump"] {
        assert!(unlocked.contains(&machine), "{machine} should be unlocked by steam_power");
        assert!(registry.machine_id(machine).is_some(), "{machine} exists");
    }
    let processing = registry.tech_id("ore_processing").expect("ore_processing node");
    let unlocked: Vec<&str> =
        registry.tech(processing).expect("node").def.unlocks.iter().map(String::as_str).collect();
    for machine in ["crusher", "ore_washer"] {
        assert!(unlocked.contains(&machine), "{machine} should be unlocked by ore_processing");
    }
}
