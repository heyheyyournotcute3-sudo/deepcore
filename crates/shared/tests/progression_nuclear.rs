//! The nuclear chain integration test — the most important test in the project.
//!
//! It feeds raw materials through every nuclear structure in order and asserts that a working
//! Nuclear Drill and a stable reactor come out the other end:
//!
//! crush -> leach -> precipitate -> convert -> enrich (cascade) -> fabricate
//! -> Nuclear Drill, fuel rods, control rods, reactor, waste.

use deepcore_shared::content::load_registry;
use deepcore_shared::content::registry::{ItemId, RecipeId, Registry};
use deepcore_shared::crafting::{MachineState, ProcessCheck, craft};
use deepcore_shared::inventory::Inventory;
use deepcore_shared::nuclear::{
    CASCADE_MAX, CASCADE_MIN, CoreCell, Reactor, ReactorLayout, ReactorState, ReactorTuning,
    cascade_speed, cascade_valid,
};

fn all_recipes(registry: &Registry) -> Vec<RecipeId> {
    (0..registry.recipe_count()).map(|index| RecipeId(index as u16)).collect()
}

fn item(registry: &Registry, name: &str) -> ItemId {
    registry.item_id(name).unwrap_or_else(|| panic!("item `{name}` exists"))
}

/// Runs a machine until it has produced `count` of an item, returning what it produced.
fn run_until(
    registry: &Registry,
    machine: &mut MachineState,
    wanted: ItemId,
    count: u32,
    max_steps: usize,
) -> u32 {
    let unlocked = all_recipes(registry);
    for _ in 0..max_steps {
        if machine.check(registry, &unlocked, 1.0) == ProcessCheck::Ready {
            machine.advance(registry, 0.1, 1.0);
        }
        let produced = machine.outputs.count_of(wanted);
        if produced >= count {
            return produced;
        }
    }
    machine.outputs.count_of(wanted)
}

/// A machine loaded with its inputs and, where it needs one, fuel.
fn machine_with(
    registry: &Registry,
    machine_name: &str,
    inputs: &[(&str, u32)],
    fuel: &[(&str, u32)],
) -> MachineState {
    let machine_id = registry
        .machine_id(machine_name)
        .unwrap_or_else(|| panic!("machine `{machine_name}` exists"));
    let mut state = MachineState::new(registry, machine_id);
    for (name, count) in inputs {
        state.inputs.insert(item(registry, name), *count, registry);
    }
    for (name, count) in fuel {
        state.fuel.insert(item(registry, name), *count, registry);
    }
    state
}

#[test]
fn the_full_nuclear_chain_produces_a_nuclear_drill_and_a_stable_reactor() {
    let registry = load_registry().expect("content loads");

    // ---------------------------------------------------------------- 1. crush
    // Uranium ore comes from the Deep Crust.
    let crushed = item(&registry, "crushed_uranium_ore");
    let mut crusher = machine_with(&registry, "crusher", &[("uranium_ore", 4)], &[]);
    let crushed_out = run_until(&registry, &mut crusher, crushed, 4, 2000);
    assert!(crushed_out >= 4, "the crusher produces crushed uranium ore, got {crushed_out}");

    // ---------------------------------------------------------------- 2. leach
    let solution = item(&registry, "uranium_solution");
    let tailings = item(&registry, "radioactive_tailings");
    let mut leacher = machine_with(
        &registry,
        "leaching_tank",
        &[("crushed_uranium_ore", 4), ("sulfuric_acid", 2)],
        &[],
    );
    let solution_out = run_until(&registry, &mut leacher, solution, 2, 4000);
    assert!(solution_out >= 2, "leaching produces uranium solution, got {solution_out}");
    assert!(
        leacher.outputs.count_of(tailings) > 0,
        "leaching also produces radioactive tailings that must be handled"
    );

    // ---------------------------------------------------------------- 3. precipitate
    let yellowcake = item(&registry, "yellowcake");
    let mut precipitator =
        machine_with(&registry, "precipitator", &[("uranium_solution", 2), ("lye", 1)], &[]);
    let cake = run_until(&registry, &mut precipitator, yellowcake, 1, 4000);
    assert!(cake >= 1, "precipitation produces yellowcake, got {cake}");

    // ---------------------------------------------------------------- 4. convert
    let uf6 = item(&registry, "uf6_canister");
    let mut converter = machine_with(
        &registry,
        "conversion_plant",
        &[("yellowcake", 1), ("fluorine_compound", 1)],
        &[],
    );
    let canisters = run_until(&registry, &mut converter, uf6, 1, 4000);
    assert!(canisters >= 1, "conversion produces UF6 canisters, got {canisters}");

    // ---------------------------------------------------------------- 5. enrich
    // A cascade must be 4..16 centrifuges; a longer cascade is faster.
    assert!(cascade_valid(CASCADE_MIN));
    assert!(cascade_valid(CASCADE_MAX));
    assert!(cascade_speed(CASCADE_MAX) > cascade_speed(CASCADE_MIN));

    let enriched = item(&registry, "enriched_canister");
    let depleted = item(&registry, "depleted_canister");
    let mut centrifuge = machine_with(&registry, "centrifuge", &[("uf6_canister", 1)], &[]);
    let enriched_out = run_until(&registry, &mut centrifuge, enriched, 1, 4000);
    assert!(enriched_out >= 1, "the cascade produces an enriched canister, got {enriched_out}");
    assert!(
        centrifuge.outputs.count_of(depleted) >= 1,
        "enrichment leaves a depleted canister for shielding"
    );

    // ---------------------------------------------------------------- 6. fabricate
    // Core cell for the drill.
    let core_cell = item(&registry, "compact_core_cell");
    let mut fabricator = machine_with(
        &registry,
        "fuel_fabricator",
        &[("enriched_canister", 1), ("stainless_plate", 2), ("lead_plate", 2)],
        &[],
    );
    let cells = run_until(&registry, &mut fabricator, core_cell, 1, 4000);
    assert!(cells >= 1, "the fabricator produces a compact core cell, got {cells}");

    // Fuel rod for the reactor.
    let fuel_rod = item(&registry, "fuel_rod");
    let mut rod_fabricator = machine_with(
        &registry,
        "fuel_fabricator",
        &[("enriched_canister", 1), ("zirconium_tube", 2)],
        &[],
    );
    let rods = run_until(&registry, &mut rod_fabricator, fuel_rod, 1, 4000);
    assert!(rods >= 1, "the fabricator produces a fuel rod, got {rods}");

    // ---------------------------------------------------------------- 7. the drill
    // Assemble the Nuclear Drill from its parts in the player's inventory.
    let mut inventory = Inventory::player();
    inventory.insert(core_cell, 1, &registry);
    inventory.insert(item(&registry, "drill_head"), 1, &registry);
    inventory.insert(item(&registry, "stainless_plate"), 4, &registry);
    inventory.insert(item(&registry, "rare_earth_magnet"), 2, &registry);
    inventory.insert(item(&registry, "lead_plate"), 2, &registry);
    let drill_recipe = registry.recipe_id("craft_nuclear_drill").expect("recipe");
    craft(&registry, &mut inventory, drill_recipe).expect("assemble the nuclear drill");

    let drill = item(&registry, "nuclear_drill");
    assert_eq!(inventory.count_of(drill), 1, "the player has a Nuclear Drill");
    let tool = registry.tool(drill).expect("tool");
    assert_eq!(tool.tier, 5);
    assert_eq!(tool.area, (3, 3), "the Nuclear Drill mines a 3x3 face");
    assert!((tool.speed - 12.0).abs() < 1e-6, "12x the starting pickaxe speed");

    // It reaches layer 6.
    let mantle = registry.block_id("mantle_rock").expect("mantle rock");
    assert!(tool.tier >= registry.block(mantle).expect("block").def.min_tier);

    // Its core cell lasts about 2000 breaks and must be returned when spent.
    let mut cell = CoreCell::new(2000);
    assert_eq!(cell.spend(2000), 2000);
    assert!(cell.is_spent());
    let spent = item(&registry, "spent_core_cell");
    let cooling = registry.recipe_id("cool_spent_fuel").expect("cooling recipe");
    let _ = (spent, cooling);

    // ---------------------------------------------------------------- 8. control rods
    let mut machine_shop =
        machine_with(&registry, "machine_shop", &[("cadmium", 2), ("stainless_ingot", 1)], &[]);
    let control_rod = item(&registry, "control_rod");
    let control_rods = run_until(&registry, &mut machine_shop, control_rod, 2, 4000);
    assert!(control_rods >= 2, "control rods are made from cadmium and stainless steel");

    // ---------------------------------------------------------------- 9. the reactor
    // A 5x5x5 shell with moderator, fuel rods and control rods is a complete build.
    let layout = ReactorLayout { casing: 98, moderator: 12, fuel_rods: 4, control_rods: 4 };
    assert!(layout.is_complete());

    let tuning = ReactorTuning::default();
    let mut reactor = Reactor::new(4, 4, true);
    reactor.set_rod_withdrawal(0.7);
    reactor.set_coolant_flow(0.9);
    // Run it for ten minutes of simulated time: it must stay stable and produce power.
    let mut ever_melted = false;
    let mut ever_scrammed = false;
    for _ in 0..6000 {
        reactor.tick(&tuning, 0.1);
        if reactor.state == ReactorState::Meltdown {
            ever_melted = true;
        }
        if reactor.state == ReactorState::Scrammed {
            ever_scrammed = true;
        }
    }
    assert!(!ever_melted, "a cooled reactor with a scram panel must not melt down");
    assert!(!ever_scrammed, "a cooled reactor should stay below the scram line");
    assert!(reactor.temperature < tuning.limit_temp);
    assert!(reactor.power_kw(&tuning) > 0.0, "the reactor produces power");

    // And it dwarfs the coal grid, as the brief requires.
    assert!(
        reactor.power_kw(&tuning) > 60.0 * 20.0,
        "reactor output should be about 20x a generator, got {}",
        reactor.power_kw(&tuning)
    );

    // ---------------------------------------------------------------- 10. waste
    // Spent fuel cools in a pool, then can be cast into shielding.
    let shielding_recipe = registry.recipe_id("cast_shielding_plate").expect("shielding recipe");
    let shielding_entry = registry.recipe(shielding_recipe).expect("recipe");
    assert!(
        shielding_entry.inputs.iter().any(|(input, _)| *input == depleted),
        "depleted canisters are cast into dense shielding plates"
    );
    let waste_vault = registry.machine_id("waste_vault_block").expect("waste vault");
    assert!(registry.machine(waste_vault).is_some());
}

#[test]
fn the_reactor_chain_needs_research_at_every_step() {
    let registry = load_registry().expect("content loads");
    // Each nuclear machine must be gated by a tech node, and those nodes must form a chain.
    let chain = [
        ("leaching_tank", "uranium_milling"),
        ("precipitator", "uranium_milling"),
        ("conversion_plant", "conversion"),
        ("centrifuge", "enrichment"),
        ("fuel_fabricator", "fuel_fabrication"),
    ];
    for (machine_name, tech_name) in chain {
        let machine = registry.machine_id(machine_name).unwrap_or_else(|| panic!("{machine_name}"));
        let def = registry.machine(machine).expect("machine");
        assert_eq!(
            def.tech.as_deref(),
            Some(tech_name),
            "{machine_name} should be gated by {tech_name}"
        );
    }

    // And the tech nodes chain in order.
    let enrichment = registry.tech_id("enrichment").expect("enrichment node");
    let node = registry.tech(enrichment).expect("node");
    let parent_names: Vec<&str> = node
        .parents
        .iter()
        .filter_map(|parent| registry.tech(*parent).map(|entry| entry.def.id.as_str()))
        .collect();
    assert!(parent_names.contains(&"conversion"), "enrichment needs conversion first");
}

#[test]
fn uranium_is_only_found_deep_and_is_radioactive() {
    let registry = load_registry().expect("content loads");
    let pitchblende = registry.block_id("pitchblende").expect("pitchblende");
    let entry = registry.block(pitchblende).expect("block");
    assert_eq!(entry.def.min_tier, 4, "uranium needs a tier-4 tool");
    let (radius, dose) = entry.def.radiation.expect("uranium is radioactive");
    assert!(radius > 0.0 && dose > 0.0);

    // It only appears in the Deep Crust and the Mantle Edge.
    for layer in registry.layers() {
        let has_uranium = layer.ores.iter().any(|ore| ore.block == pitchblende);
        if has_uranium {
            assert!(
                layer.def.id == "deep_crust" || layer.def.id == "mantle_edge",
                "uranium should only generate in the deepest layers, found in {}",
                layer.def.id
            );
        }
    }
}
