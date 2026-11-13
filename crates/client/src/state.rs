//! Client-side mirrors of the server's inventory, machine and tech state, plus the intents the
//! UI sends back.

use bevy::prelude::*;

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::content::registry::RecipeId;
use deepcore_shared::crafting::{CraftError, can_craft, craft};
use deepcore_shared::inventory::{Inventory, Slot};
use deepcore_shared::protocol::*;
use deepcore_shared::tech::TechState;

/// The local player's inventory, as last reported by the server.
#[derive(Resource, Clone, Debug)]
pub struct ClientInventory {
    /// The 45 slots, hotbar first.
    pub inventory: Inventory,
    /// The selected hotbar slot.
    pub selected: u8,
}

impl Default for ClientInventory {
    fn default() -> Self {
        Self { inventory: Inventory::player(), selected: 0 }
    }
}

impl ClientInventory {
    /// The item in the selected hotbar slot.
    pub fn held(&self) -> Option<deepcore_shared::content::registry::ItemId> {
        self.inventory.slot(self.selected as usize).and_then(|slot| slot.item)
    }
}

/// The team tech tree, as last reported by the server.
#[derive(Resource, Clone, Debug, Default)]
pub struct ClientTech {
    /// The tree state.
    pub state: TechState,
}

/// The machine the player currently has open, with its last snapshot.
#[derive(Resource, Clone, Debug, Default)]
pub struct OpenMachine {
    /// The machine entity id used by the protocol.
    pub entity: Option<u64>,
    /// The block position, for the title.
    pub position: [i32; 3],
    /// Input slots.
    pub inputs: Vec<Slot>,
    /// Output slots.
    pub outputs: Vec<Slot>,
    /// Fuel slots.
    pub fuel: Vec<Slot>,
    /// Progress, 0..1.
    pub progress: f32,
    /// Why it is idle.
    pub idle: Option<String>,
    /// The recipe being run.
    pub recipe: Option<u16>,
}

/// Applies inventory snapshots from the server.
pub fn receive_inventory(
    mut snapshots: MessageReader<InventorySnapshot>,
    mut inventory: ResMut<ClientInventory>,
) {
    for snapshot in snapshots.read() {
        let mut slots = snapshot.slots.clone();
        slots.resize(deepcore_shared::inventory::PLAYER_SLOTS, Slot::EMPTY);
        inventory.inventory.set_slots(&slots);
        inventory.selected = snapshot.selected;
    }
}

/// Applies tech snapshots from the server.
pub fn receive_tech(
    mut snapshots: MessageReader<TechSnapshot>,
    mut tech: ResMut<ClientTech>,
    registry: Res<ContentRegistry>,
) {
    for snapshot in snapshots.read() {
        let mut state = TechState::new(&registry);
        for (index, unlocked) in snapshot.unlocked.iter().enumerate() {
            if let Some(slot) = state.unlocked.get_mut(index) {
                *slot = *unlocked;
            }
        }
        for (node, item, count) in &snapshot.deposits {
            if let Some(deposits) = state.deposits.get_mut(*node as usize) {
                let item = deepcore_shared::content::registry::ItemId(*item);
                match deposits.iter_mut().find(|(id, _)| *id == item) {
                    Some((_, existing)) => *existing = *count,
                    None => deposits.push((item, *count)),
                }
            }
        }
        tech.state = state;
    }
}

/// Applies machine snapshots to the open machine.
pub fn receive_machine(
    mut snapshots: MessageReader<MachineSnapshot>,
    mut machine: ResMut<OpenMachine>,
) {
    for snapshot in snapshots.read() {
        if machine.entity != Some(snapshot.entity) && machine.entity.is_some() {
            // A snapshot for a machine we are not looking at: ignore it.
            continue;
        }
        machine.entity = Some(snapshot.entity);
        machine.inputs = snapshot.inputs.clone();
        machine.outputs = snapshot.outputs.clone();
        machine.fuel = snapshot.fuel.clone();
        machine.progress = snapshot.progress;
        machine.idle = snapshot.idle.clone();
        machine.recipe = snapshot.recipe;
    }
}

/// Tracks which machine the player last asked to open.
///
/// The intent itself is sent by the interaction system in `mining`; this only remembers the
/// position so the panel can be titled correctly.
pub fn track_open_machine(mut requests: MessageReader<Interact>, mut open: ResMut<OpenMachine>) {
    for request in requests.read() {
        open.position = request.block;
    }
}

/// A local, optimistic craft used by the UI to give instant feedback.
///
/// Returns the same errors the server would produce, so the two never disagree.
pub fn predict_craft(
    registry: &ContentRegistry,
    inventory: &Inventory,
    recipe: RecipeId,
) -> Result<(), CraftError> {
    if !can_craft(registry, inventory, recipe) {
        let entry = registry.recipe(recipe).ok_or(CraftError::UnknownRecipe)?;
        let missing = entry
            .inputs
            .iter()
            .find(|(item, count)| inventory.count_of(*item) < *count)
            .map(|(item, _)| registry.item_name(*item).to_owned())
            .unwrap_or_default();
        return Err(CraftError::MissingInputs(missing));
    }
    let mut copy = inventory.clone();
    craft(registry, &mut copy, recipe).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepcore_shared::content::load_registry;

    #[test]
    fn prediction_matches_the_server_craft_rules() {
        let registry = ContentRegistry(load_registry().expect("content"));
        let recipe = registry.recipe_id("craft_ladder").expect("recipe");
        let mut inventory = Inventory::player();
        assert!(predict_craft(&registry, &inventory, recipe).is_err());

        let timber = registry.item_id("timber").expect("timber");
        inventory.insert(timber, 4, &registry);
        assert!(predict_craft(&registry, &inventory, recipe).is_ok());
    }

    #[test]
    fn default_inventory_is_empty_and_selects_the_first_slot() {
        let inventory = ClientInventory::default();
        assert!(inventory.inventory.is_fully_empty());
        assert_eq!(inventory.selected, 0);
        assert_eq!(inventory.held(), None);
    }

    #[test]
    fn the_hotbar_is_the_first_nine_slots() {
        let inventory = ClientInventory::default();
        assert_eq!(inventory.inventory.slots().len(), deepcore_shared::inventory::PLAYER_SLOTS);
        assert_eq!(
            &inventory.inventory.slots()[..deepcore_shared::inventory::HOTBAR_SLOTS].len(),
            &deepcore_shared::inventory::HOTBAR_SLOTS
        );
    }
}
