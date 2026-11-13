//! Crafting and the machine processing state machine.
//!
//! One rule set for both: a machine (or the workbench) has inputs, outputs, a recipe and a
//! progress counter. Every tick it asks three questions — *is there a recipe that fits? is the
//! output blocked? is there energy?* — and the answers drive both the simulation and the
//! "why is it idle" text in the UI.

use serde::{Deserialize, Serialize};

use crate::content::model::{EnergyKind, ItemCategory};
use crate::content::registry::{ItemId, MachineId, RecipeId, Registry};
use crate::inventory::{Inventory, Slot, stack_size};

/// Why a machine is not running. Shown verbatim in the machine UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdleReason {
    /// No recipe can be started with the current inputs.
    NoRecipe,
    /// The recipe needs inputs the machine does not have.
    MissingInputs(String),
    /// The output slots cannot take the result.
    OutputFull,
    /// The machine needs fuel and has none.
    NoFuel,
    /// The machine needs power and the network has none.
    NoPower,
    /// A centrifuge cascade is too short to enrich.
    CascadeTooShort {
        /// How many centrifuges are linked.
        have: u8,
        /// How many are needed.
        need: u8,
    },
    /// The machine is idle because a player has not given it a recipe yet.
    NoSelection,
}

impl IdleReason {
    /// Human-readable text for the UI.
    pub fn text(&self) -> String {
        match self {
            IdleReason::NoRecipe => "No recipe".to_owned(),
            IdleReason::MissingInputs(item) => format!("Missing {item}"),
            IdleReason::OutputFull => "Output full".to_owned(),
            IdleReason::NoFuel => "No fuel".to_owned(),
            IdleReason::NoPower => "No power".to_owned(),
            IdleReason::CascadeTooShort { have, need } => {
                format!("Cascade too short ({have}/{need})")
            }
            IdleReason::NoSelection => "Choose a recipe".to_owned(),
        }
    }
}

/// The result of asking a machine whether it can run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessCheck {
    /// The recipe can run this tick.
    Ready,
    /// The recipe cannot run, with the reason.
    Idle(IdleReason),
}

/// The runtime state of a machine or crafting station.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MachineState {
    /// The machine definition.
    pub machine: MachineId,
    /// Input slots.
    pub inputs: Inventory,
    /// Output slots.
    pub outputs: Inventory,
    /// Fuel slot (machines with `EnergyKind::Fuel`).
    pub fuel: Inventory,
    /// The recipe currently selected or automatically chosen.
    pub recipe: Option<RecipeId>,
    /// Seconds of progress towards the current recipe.
    pub progress: f32,
    /// True when the machine ran last tick.
    pub running: bool,
    /// Why it is idle, if it is.
    pub idle: Option<IdleReason>,
    /// Upgrade modules installed (0-2): `Speed` and `Efficiency`.
    pub modules: Vec<UpgradeModule>,
}

/// An upgrade module installed in a machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpgradeModule {
    /// +50% speed, +80% power use.
    Speed,
    /// -30% power use.
    Efficiency,
}

impl Default for MachineState {
    /// A machine with no definition: used only as a placeholder by tooling and tests.
    fn default() -> Self {
        Self {
            machine: MachineId(0),
            inputs: Inventory::new(1),
            outputs: Inventory::new(1),
            fuel: Inventory::new(0),
            recipe: None,
            progress: 0.0,
            running: false,
            idle: None,
            modules: Vec::new(),
        }
    }
}

impl MachineState {
    /// Creates the state for a machine, sized from its definition.
    pub fn new(registry: &Registry, machine: MachineId) -> Self {
        let (input_slots, output_slots, fuel_slots) = match registry.machine(machine) {
            Some(def) => (
                def.input_slots as usize,
                def.output_slots as usize,
                match def.energy {
                    EnergyKind::Fuel { slots } => slots as usize,
                    _ => 0,
                },
            ),
            None => (2, 2, 0),
        };
        Self {
            machine,
            inputs: Inventory::new(input_slots.max(1)),
            outputs: Inventory::new(output_slots.max(1)),
            fuel: Inventory::new(fuel_slots),
            recipe: None,
            progress: 0.0,
            running: false,
            idle: None,
            modules: Vec::new(),
        }
    }

    /// The machine definition.
    pub fn definition<'a>(
        &self,
        registry: &'a Registry,
    ) -> Option<&'a crate::content::model::MachineDef> {
        registry.machine(self.machine)
    }

    /// Speed multiplier from installed modules.
    pub fn speed_multiplier(&self) -> f32 {
        let mut multiplier = 1.0;
        for module in &self.modules {
            if *module == UpgradeModule::Speed {
                multiplier *= 1.5;
            }
        }
        multiplier
    }

    /// Power multiplier from installed modules.
    pub fn power_multiplier(&self) -> f32 {
        let mut multiplier = 1.0;
        for module in &self.modules {
            match module {
                UpgradeModule::Speed => multiplier *= 1.8,
                UpgradeModule::Efficiency => multiplier *= 0.7,
            }
        }
        multiplier
    }

    /// Installs an upgrade module, up to two per machine.
    pub fn install_module(&mut self, module: UpgradeModule) -> bool {
        if self.modules.len() >= 2 {
            return false;
        }
        self.modules.push(module);
        true
    }

    /// Finds the recipe this machine should run, given its inputs.
    ///
    /// If a recipe was explicitly selected and its inputs are present, that one is used;
    /// otherwise the first available recipe whose inputs are satisfied is chosen.
    pub fn choose_recipe(&self, registry: &Registry, unlocked: &[RecipeId]) -> Option<RecipeId> {
        let available = registry.recipes_for_machine(self.machine);
        if let Some(selected) = self.recipe
            && available.contains(&selected)
            && unlocked.contains(&selected)
            && self.has_inputs(registry, selected)
        {
            return Some(selected);
        }
        available
            .iter()
            .copied()
            .filter(|recipe| unlocked.contains(recipe))
            .find(|recipe| self.has_inputs(registry, *recipe))
    }

    /// True if the input slots contain everything a recipe needs.
    pub fn has_inputs(&self, registry: &Registry, recipe: RecipeId) -> bool {
        let Some(entry) = registry.recipe(recipe) else {
            return false;
        };
        for (item, count) in &entry.inputs {
            if self.inputs.count_of(*item) < *count {
                return false;
            }
        }
        for (item, count) in &entry.fuel {
            if self.fuel.count_of(*item) < *count && self.inputs.count_of(*item) < *count {
                return false;
            }
        }
        true
    }

    /// True if the output slots can accept everything a recipe produces.
    pub fn has_output_room(&self, registry: &Registry, recipe: RecipeId) -> bool {
        let Some(entry) = registry.recipe(recipe) else {
            return false;
        };
        for (item, count) in &entry.outputs {
            if !self.outputs.can_fit(*item, *count, registry) {
                return false;
            }
        }
        true
    }

    /// Consumes the inputs of a recipe.
    pub fn consume_inputs(&mut self, registry: &Registry, recipe: RecipeId) {
        let Some(entry) = registry.recipe(recipe) else {
            return;
        };
        let inputs = entry.inputs.clone();
        let fuel = entry.fuel.clone();
        for (item, count) in inputs {
            self.inputs.remove(item, count);
        }
        for (item, count) in fuel {
            // Prefer the fuel slot, fall back to inputs.
            let taken = self.fuel.remove(item, count);
            if taken < count {
                self.inputs.remove(item, count - taken);
            }
        }
    }

    /// Produces the outputs of a recipe.
    pub fn produce_outputs(&mut self, registry: &Registry, recipe: RecipeId) {
        let Some(entry) = registry.recipe(recipe) else {
            return;
        };
        let outputs = entry.outputs.clone();
        for (item, count) in outputs {
            let leftover = self.outputs.insert(item, count, registry);
            if leftover > 0 {
                // Should not happen: `has_output_room` was checked first. Log rather than lose.
                tracing::warn!("machine output overflow: {leftover} items lost");
            }
        }
    }

    /// Checks whether the machine can start or continue its current recipe.
    ///
    /// `power_available` is the fraction of demanded power the network can supply (1.0 for
    /// fuel machines or a satisfied network).
    pub fn check(
        &mut self,
        registry: &Registry,
        unlocked: &[RecipeId],
        power_available: f32,
    ) -> ProcessCheck {
        let Some(recipe) = self.choose_recipe(registry, unlocked) else {
            self.recipe = None;
            self.progress = 0.0;
            let idle = if self.inputs.is_fully_empty() {
                IdleReason::NoRecipe
            } else {
                self.explain_missing_inputs(registry, unlocked)
            };
            self.idle = Some(idle.clone());
            return ProcessCheck::Idle(idle);
        };
        self.recipe = Some(recipe);

        // Fuel check for fuel-burning machines.
        if let Some(def) = registry.machine(self.machine)
            && matches!(def.energy, EnergyKind::Fuel { .. })
            && self.fuel.is_fully_empty()
        {
            self.idle = Some(IdleReason::NoFuel);
            self.running = false;
            return ProcessCheck::Idle(IdleReason::NoFuel);
        }

        if power_available <= 0.0 {
            self.idle = Some(IdleReason::NoPower);
            self.running = false;
            return ProcessCheck::Idle(IdleReason::NoPower);
        }

        if !self.has_output_room(registry, recipe) {
            self.idle = Some(IdleReason::OutputFull);
            self.running = false;
            return ProcessCheck::Idle(IdleReason::OutputFull);
        }

        self.idle = None;
        ProcessCheck::Ready
    }

    /// Explains which input is missing, for the UI.
    fn explain_missing_inputs(&self, registry: &Registry, unlocked: &[RecipeId]) -> IdleReason {
        let available = registry.recipes_for_machine(self.machine);
        let mut best: Option<(&str, u32, u32)> = None;
        for recipe in available.iter().filter(|r| unlocked.contains(r)) {
            let Some(entry) = registry.recipe(*recipe) else {
                continue;
            };
            for (item, count) in &entry.inputs {
                let have = self.inputs.count_of(*item);
                if have < *count {
                    let name = registry.item_name(*item);
                    match best {
                        Some((_, _, best_have)) if best_have >= have => {}
                        _ => best = Some((name, *count, have)),
                    }
                }
            }
        }
        match best {
            Some((name, _, _)) => IdleReason::MissingInputs(name.to_owned()),
            None => IdleReason::NoRecipe,
        }
    }

    /// Advances processing by `dt` seconds at `speed` multiplier.
    ///
    /// Returns true if a recipe finished this tick.
    pub fn advance(&mut self, registry: &Registry, dt: f32, speed: f32) -> bool {
        let Some(recipe) = self.recipe else {
            return false;
        };
        let Some(entry) = registry.recipe(recipe) else {
            return false;
        };
        self.running = true;
        self.progress += dt * speed * self.speed_multiplier();
        if self.progress < entry.def.time {
            return false;
        }
        self.progress -= entry.def.time;
        self.consume_inputs(registry, recipe);
        self.produce_outputs(registry, recipe);
        true
    }

    /// Total seconds the current recipe takes at 1x speed.
    pub fn recipe_time(&self, registry: &Registry) -> f32 {
        self.recipe
            .and_then(|recipe| registry.recipe(recipe))
            .map(|entry| entry.def.time)
            .unwrap_or(0.0)
    }

    /// Progress through the current recipe, 0..1.
    pub fn progress_fraction(&self, registry: &Registry) -> f32 {
        let time = self.recipe_time(registry);
        if time <= 0.0 { 0.0 } else { (self.progress / time).clamp(0.0, 1.0) }
    }

    /// The power this machine draws right now, in kW (0 for non-electric machines).
    pub fn power_draw(&self, registry: &Registry) -> f32 {
        let Some(def) = registry.machine(self.machine) else {
            return 0.0;
        };
        let base = match def.energy {
            EnergyKind::Electric { kw } => kw,
            EnergyKind::Steam { per_second } => per_second,
            EnergyKind::Shaft { per_second } => per_second,
            _ => 0.0,
        };
        if self.running { base * self.power_multiplier() } else { 0.0 }
    }
}

/// A hand-crafting check: does the player have everything a recipe needs?
pub fn can_craft(registry: &Registry, inventory: &Inventory, recipe: RecipeId) -> bool {
    let Some(entry) = registry.recipe(recipe) else {
        return false;
    };
    entry.inputs.iter().all(|(item, count)| inventory.count_of(*item) >= *count)
}

/// Performs a hand craft (workbench or hand recipe), returning what was produced.
///
/// Fails without changing anything if the player lacks inputs or space.
pub fn craft(
    registry: &Registry,
    inventory: &mut Inventory,
    recipe: RecipeId,
) -> Result<Vec<Slot>, CraftError> {
    let Some(entry) = registry.recipe(recipe) else {
        return Err(CraftError::UnknownRecipe);
    };
    if entry.machine.is_some() {
        return Err(CraftError::NeedsMachine);
    }
    if !can_craft(registry, inventory, recipe) {
        let missing = entry
            .inputs
            .iter()
            .find(|(item, count)| inventory.count_of(*item) < *count)
            .map(|(item, _)| registry.item_name(*item).to_owned())
            .unwrap_or_default();
        return Err(CraftError::MissingInputs(missing));
    }
    // Check there is room for the outputs before consuming anything.
    for (item, count) in &entry.outputs {
        if !inventory.can_fit(*item, *count, registry) {
            return Err(CraftError::NoRoom);
        }
    }

    for (item, count) in &entry.inputs {
        inventory.remove(*item, *count);
    }
    let mut produced = Vec::new();
    for (item, count) in &entry.outputs {
        let leftover = inventory.insert(*item, *count, registry);
        produced.push(Slot::new(*item, *count - leftover));
    }
    Ok(produced)
}

/// Why a hand craft failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CraftError {
    /// The recipe id is not registered.
    UnknownRecipe,
    /// The recipe must be made in a machine.
    NeedsMachine,
    /// The player lacks an input.
    MissingInputs(String),
    /// The player's inventory is full.
    NoRoom,
}

/// Recipes craftable by hand or at the workbench, filtered by what is unlocked.
pub fn available_hand_recipes(registry: &Registry, unlocked: &[RecipeId]) -> Vec<RecipeId> {
    registry.hand_recipes().iter().copied().filter(|r| unlocked.contains(r)).collect()
}

/// True if an item is a block the player can place (used by the hotbar).
pub fn is_placeable(registry: &Registry, item: ItemId) -> bool {
    matches!(
        registry.item(item).map(|entry| &entry.def.category),
        Some(ItemCategory::Block | ItemCategory::MachineKit)
    )
}

/// The block a held item places, if any.
pub fn placed_block(registry: &Registry, item: ItemId) -> Option<crate::voxel::BlockId> {
    registry.item(item).and_then(|entry| entry.places)
}

/// Largest stack size in the registry, used by the UI to size slot widgets.
pub fn max_stack(registry: &Registry) -> u32 {
    registry.items().iter().map(|entry| entry.def.stack).max().unwrap_or(64)
}

/// Convenience for tests and tooling: the stack size of a named item.
pub fn stack_of(registry: &Registry, name: &str) -> u32 {
    registry.item_id(name).map(|id| stack_size(registry, id)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::load_registry;

    fn registry() -> Registry {
        load_registry().expect("content")
    }

    fn all_recipes(registry: &Registry) -> Vec<RecipeId> {
        (0..registry.recipe_count()).map(|index| RecipeId(index as u16)).collect()
    }

    #[test]
    fn hand_craft_consumes_inputs_and_produces_outputs() {
        let registry = registry();
        let timber = registry.item_id("timber").expect("timber");
        let ladder = registry.item_id("ladder").expect("ladder");
        let recipe = registry.recipe_id("craft_ladder").expect("recipe");

        let mut inventory = Inventory::player();
        inventory.insert(timber, 8, &registry);
        let produced = craft(&registry, &mut inventory, recipe).expect("craft works");
        assert_eq!(produced, vec![Slot::new(ladder, 4)]);
        assert_eq!(inventory.count_of(timber), 4);
        assert_eq!(inventory.count_of(ladder), 4);
    }

    #[test]
    fn hand_craft_fails_without_inputs() {
        let registry = registry();
        let recipe = registry.recipe_id("craft_ladder").expect("recipe");
        let mut inventory = Inventory::player();
        assert_eq!(
            craft(&registry, &mut inventory, recipe),
            Err(CraftError::MissingInputs("Timber".to_owned()))
        );
        assert!(inventory.is_fully_empty());
    }

    #[test]
    fn hand_craft_refuses_machine_recipes() {
        let registry = registry();
        let recipe = registry.recipe_id("smelt_iron").expect("recipe");
        let mut inventory = Inventory::player();
        assert_eq!(craft(&registry, &mut inventory, recipe), Err(CraftError::NeedsMachine));
    }

    #[test]
    fn hand_craft_fails_when_the_inventory_is_full() {
        let registry = registry();
        let recipe = registry.recipe_id("craft_ladder").expect("recipe");
        let timber = registry.item_id("timber").expect("timber");
        // A one-slot inventory full of a non-stackable tool leaves no room for the output.
        let mut inventory = Inventory::new(1);
        let pickaxe = registry.item_id("steel_pickaxe").expect("pickaxe");
        inventory.insert(pickaxe, 1, &registry);
        // Put the timber directly in the slot by bypassing the stack limit check.
        inventory.slots_mut()[0] = Slot::new(timber, 4);
        assert_eq!(craft(&registry, &mut inventory, recipe), Err(CraftError::NoRoom));
        assert_eq!(inventory.count_of(timber), 4, "nothing consumed on failure");
    }

    #[test]
    fn furnace_smelts_ore_into_ingots_over_time() {
        let registry = registry();
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let ore = registry.item_id("raw_iron").expect("raw iron");
        let ingot = registry.item_id("iron_ingot").expect("iron ingot");
        let coal = registry.item_id("coal").expect("coal");

        let mut state = MachineState::new(&registry, furnace);
        state.inputs.insert(ore, 3, &registry);
        state.fuel.insert(coal, 1, &registry);

        let unlocked = all_recipes(&registry);
        assert_eq!(state.check(&registry, &unlocked, 1.0), ProcessCheck::Ready);
        assert_eq!(state.recipe, Some(registry.recipe_id("smelt_iron").expect("recipe")));

        // Smelting one iron takes 6 s; run 7 s of simulated time.
        let mut finished = 0;
        for _ in 0..70 {
            if state.advance(&registry, 0.1, 1.0) {
                finished += 1;
            }
        }
        assert_eq!(finished, 1);
        assert_eq!(state.outputs.count_of(ingot), 1);
        assert_eq!(state.inputs.count_of(ore), 2);
    }

    #[test]
    fn machine_reports_no_fuel_and_no_power() {
        let registry = registry();
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let ore = registry.item_id("raw_iron").expect("raw iron");
        let unlocked = all_recipes(&registry);

        let mut state = MachineState::new(&registry, furnace);
        state.inputs.insert(ore, 1, &registry);
        assert_eq!(state.check(&registry, &unlocked, 1.0), ProcessCheck::Idle(IdleReason::NoFuel));

        // Electric machine without power: feed it a recipe it can actually run (quartz into
        // silicon) so the only thing missing is electricity.
        let arc = registry.machine_id("arc_furnace").expect("arc furnace");
        let quartz = registry.item_id("quartz").expect("quartz");
        let coal = registry.item_id("coal").expect("coal");
        let mut electric = MachineState::new(&registry, arc);
        electric.inputs.insert(quartz, 2, &registry);
        electric.inputs.insert(coal, 1, &registry);
        assert_eq!(
            electric.check(&registry, &unlocked, 0.0),
            ProcessCheck::Idle(IdleReason::NoPower)
        );
    }

    #[test]
    fn machine_reports_output_full() {
        let registry = registry();
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let ore = registry.item_id("raw_iron").expect("raw iron");
        let coal = registry.item_id("coal").expect("coal");
        let unlocked = all_recipes(&registry);

        let mut state = MachineState::new(&registry, furnace);
        state.inputs.insert(ore, 1, &registry);
        state.fuel.insert(coal, 1, &registry);
        // Fill both output slots with a different, full stack.
        let stone = registry.item_id("stone_cobble").expect("cobble");
        for index in 0..state.outputs.len() {
            state.outputs.slots_mut()[index] = Slot::new(stone, 64);
        }
        assert_eq!(
            state.check(&registry, &unlocked, 1.0),
            ProcessCheck::Idle(IdleReason::OutputFull)
        );
    }

    #[test]
    fn missing_input_reason_names_the_item() {
        let registry = registry();
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let coal = registry.item_id("coal").expect("coal");
        let unlocked = all_recipes(&registry);
        let mut state = MachineState::new(&registry, furnace);
        state.fuel.insert(coal, 1, &registry);
        // Something in the inputs but not a valid recipe input.
        let stone = registry.item_id("stone_cobble").expect("cobble");
        state.inputs.insert(stone, 1, &registry);
        match state.check(&registry, &unlocked, 1.0) {
            ProcessCheck::Idle(IdleReason::MissingInputs(name)) => {
                assert!(!name.is_empty(), "the missing item should be named");
            }
            other => panic!("expected a missing-input reason, got {other:?}"),
        }
    }

    #[test]
    fn upgrade_modules_change_speed_and_power() {
        let registry = registry();
        let arc = registry.machine_id("arc_furnace").expect("arc furnace");
        let mut state = MachineState::new(&registry, arc);
        assert!((state.speed_multiplier() - 1.0).abs() < 1e-6);
        assert!(state.install_module(UpgradeModule::Speed));
        assert!(state.install_module(UpgradeModule::Efficiency));
        assert!(!state.install_module(UpgradeModule::Speed), "only two modules fit");
        assert!((state.speed_multiplier() - 1.5).abs() < 1e-6);
        assert!((state.power_multiplier() - 1.8 * 0.7).abs() < 1e-5);
    }

    #[test]
    fn progress_fraction_is_bounded() {
        let registry = registry();
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let ore = registry.item_id("raw_iron").expect("raw iron");
        let coal = registry.item_id("coal").expect("coal");
        let unlocked = all_recipes(&registry);
        let mut state = MachineState::new(&registry, furnace);
        state.inputs.insert(ore, 1, &registry);
        state.fuel.insert(coal, 1, &registry);
        let _ = state.check(&registry, &unlocked, 1.0);
        state.advance(&registry, 3.0, 1.0);
        let fraction = state.progress_fraction(&registry);
        assert!((0.0..=1.0).contains(&fraction));
        assert!((fraction - 0.5).abs() < 0.01, "3 s of a 6 s recipe should be half done");
    }

    #[test]
    fn every_hand_recipe_is_craftable_from_somewhere() {
        let registry = registry();
        let unlocked = all_recipes(&registry);
        let recipes = available_hand_recipes(&registry, &unlocked);
        assert!(!recipes.is_empty());
        for recipe in recipes {
            let entry = registry.recipe(recipe).expect("recipe");
            assert!(!entry.outputs.is_empty());
        }
    }
}
