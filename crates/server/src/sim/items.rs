//! Server-side inventory intents: moving items, crafting, hotbar selection.

use bevy::prelude::*;
use bevy_replicon::prelude::*;

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::content::registry::RecipeId;
use deepcore_shared::crafting::{CraftError, craft};
use deepcore_shared::inventory::{HOTBAR_SLOTS, Slot};
use deepcore_shared::protocol::*;

use crate::net::ClientSessions;
use crate::sim::inventory::PlayerInventory;
use crate::sim::machines::{BlockEntity, SharedTech};

/// Resolves a replicon client entity to the renet client id used by `NetOwner`.
fn renet_id(sessions: &ClientSessions, sender: Entity) -> Option<u64> {
    sessions.entities.iter().find(|(_, entity)| **entity == sender).map(|(id, _)| *id)
}

/// Handles `MoveItems`: player inventory <-> player inventory, and player <-> machine.
pub fn handle_move_items(
    mut requests: MessageReader<FromClient<MoveItems>>,
    mut players: Query<(&mut PlayerInventory, &NetOwner)>,
    mut machines: Query<&mut BlockEntity>,
    registry: Res<ContentRegistry>,
    sessions: Res<ClientSessions>,
) {
    for request in requests.read() {
        let Some(sender) = request.client_id.entity() else {
            continue;
        };
        let Some(client_id) = renet_id(&sessions, sender) else {
            continue;
        };
        let Some((mut player, _)) = players.iter_mut().find(|(_, owner)| owner.client == client_id)
        else {
            continue;
        };

        // Both ends are either the player (container 0) or a machine entity index.
        let count = if request.count == u32::MAX { u32::MAX } else { request.count };
        match (request.from_container, request.to_container) {
            (0, 0) => {
                let moved = player.inventory.move_between(
                    request.from_slot as usize,
                    request.to_slot as usize,
                    count,
                    &registry,
                );
                if moved > 0 {
                    player.dirty = true;
                }
            }
            (0, machine) => {
                let Some(mut entity) = machine_by_index(&mut machines, machine) else {
                    continue;
                };
                let slot =
                    *player.inventory.slot(request.from_slot as usize).unwrap_or(&Slot::EMPTY);
                if slot.is_empty() {
                    continue;
                }
                let moved = move_into_machine(
                    &mut player,
                    &mut entity,
                    slot,
                    request.to_slot,
                    count,
                    &registry,
                );
                if moved > 0 {
                    player.dirty = true;
                }
            }
            (machine, 0) => {
                let Some(mut entity) = machine_by_index(&mut machines, machine) else {
                    continue;
                };
                let moved = move_from_machine(
                    &mut player,
                    &mut entity,
                    request.from_slot,
                    request.to_slot as usize,
                    count,
                    &registry,
                );
                if moved > 0 {
                    player.dirty = true;
                }
            }
            _ => {}
        }
    }
}

fn machine_by_index<'a>(
    machines: &'a mut Query<&mut BlockEntity>,
    index: u64,
) -> Option<Mut<'a, BlockEntity>> {
    machines
        .iter_mut()
        .find(|machine| {
            let _ = machine;
            // The client refers to machines by their server entity index; the snapshot carries it.
            true
        })
        .filter(|_| index != u64::MAX)
}

fn move_into_machine(
    player: &mut PlayerInventory,
    machine: &mut BlockEntity,
    slot: Slot,
    to_slot: u16,
    count: u32,
    registry: &ContentRegistry,
) -> u32 {
    let Some(item) = slot.item else {
        return 0;
    };
    let movable = slot.count.min(count);
    // Machines accept inputs, or fuel if the target slot is past the inputs.
    let input_len = machine.state.inputs.len();
    let target_inventory = if (to_slot as usize) < input_len {
        &mut machine.state.inputs
    } else {
        &mut machine.state.fuel
    };
    let target_index = if (to_slot as usize) < input_len {
        to_slot as usize
    } else {
        to_slot as usize - input_len
    };
    if target_index >= target_inventory.len() {
        return 0;
    }
    let room = target_inventory
        .slot(target_index)
        .map(|s| s.room_for(item, stack_size(registry, item)))
        .unwrap_or(0);
    let moved = room.min(movable);
    if moved == 0 {
        return 0;
    }
    let target = &mut target_inventory.slots_mut()[target_index];
    match target.item {
        Some(existing) if existing == item => target.count += moved,
        _ => *target = Slot::new(item, moved),
    }
    player.inventory.take_from_slot(player.inventory.find(item).unwrap_or(0), moved);
    moved
}

fn move_from_machine(
    player: &mut PlayerInventory,
    machine: &mut BlockEntity,
    from_slot: u16,
    to_slot: usize,
    count: u32,
    registry: &ContentRegistry,
) -> u32 {
    let output_len = machine.state.outputs.len();
    let (source, index) = if (from_slot as usize) < output_len {
        (&mut machine.state.outputs, from_slot as usize)
    } else {
        (&mut machine.state.inputs, from_slot as usize - output_len)
    };
    let Some(slot) = source.slot(index).copied() else {
        return 0;
    };
    if slot.is_empty() {
        return 0;
    }
    let item = slot.item.unwrap_or(deepcore_shared::content::registry::ItemId(0));
    let movable = slot.count.min(count);
    // Move into the player slot directly.
    let mut moved = 0;
    if to_slot < player.inventory.len() {
        let stack = stack_size(registry, item);
        let target = player.inventory.slots()[to_slot];
        let room = target.room_for(item, stack);
        moved = room.min(movable);
        if moved > 0 {
            let destination = &mut player.inventory.slots_mut()[to_slot];
            match destination.item {
                Some(existing) if existing == item => destination.count += moved,
                _ => *destination = Slot::new(item, moved),
            }
        }
    }
    if moved == 0 {
        // Fall back to any free space in the player inventory.
        let leftover = player.inventory.insert(item, movable, registry);
        moved = movable - leftover;
    }
    source.take_from_slot(index, moved);
    moved
}

fn stack_size(registry: &ContentRegistry, item: deepcore_shared::content::registry::ItemId) -> u32 {
    registry.item(item).map(|entry| entry.def.stack).unwrap_or(64).max(1)
}

/// Handles `CraftRequest`: validates the recipe is unlocked, then crafts.
pub fn handle_craft(
    mut requests: MessageReader<FromClient<CraftRequest>>,
    mut players: Query<(&mut PlayerInventory, &NetOwner)>,
    tech: Res<SharedTech>,
    registry: Res<ContentRegistry>,
    sessions: Res<ClientSessions>,
    mut notices: MessageWriter<ToClients<Notice>>,
) {
    for request in requests.read() {
        let Some(sender) = request.client_id.entity() else {
            continue;
        };
        let Some(client_id) = renet_id(&sessions, sender) else {
            continue;
        };
        let Some((mut player, _)) = players.iter_mut().find(|(_, owner)| owner.client == client_id)
        else {
            continue;
        };

        let recipe = RecipeId(request.recipe);
        let unlocked = tech.state.available_recipes(&registry);
        if !unlocked.contains(&recipe) {
            notices.write(ToClients {
                targets: SendTargets::Single(ClientId::Client(sender)),
                message: Notice { text: "That recipe is not researched yet".to_owned() },
            });
            continue;
        }

        let repeats = request.count.clamp(1, 64);
        let mut crafted = 0;
        for _ in 0..repeats {
            match craft(&registry, &mut player.inventory, recipe) {
                Ok(_) => crafted += 1,
                Err(CraftError::NoRoom) => {
                    notices.write(ToClients {
                        targets: SendTargets::Single(ClientId::Client(sender)),
                        message: Notice { text: "Not enough room".to_owned() },
                    });
                    break;
                }
                Err(CraftError::MissingInputs(name)) => {
                    if crafted == 0 {
                        notices.write(ToClients {
                            targets: SendTargets::Single(ClientId::Client(sender)),
                            message: Notice { text: format!("Missing {name}") },
                        });
                    }
                    break;
                }
                Err(CraftError::NeedsMachine) => {
                    notices.write(ToClients {
                        targets: SendTargets::Single(ClientId::Client(sender)),
                        message: Notice { text: "That needs a machine".to_owned() },
                    });
                    break;
                }
                Err(CraftError::UnknownRecipe) => break,
            }
        }
        if crafted > 0 {
            player.dirty = true;
        }
    }
}

/// Handles hotbar selection.
pub fn handle_select_hotbar(
    mut requests: MessageReader<FromClient<SelectHotbar>>,
    mut players: Query<(&mut PlayerInventory, &NetOwner)>,
    sessions: Res<ClientSessions>,
) {
    for request in requests.read() {
        let Some(sender) = request.client_id.entity() else {
            continue;
        };
        let Some(client_id) = renet_id(&sessions, sender) else {
            continue;
        };
        let Some((mut player, _)) = players.iter_mut().find(|(_, owner)| owner.client == client_id)
        else {
            continue;
        };
        let slot = request.slot.min((HOTBAR_SLOTS - 1) as u8);
        if player.selected != slot {
            player.selected = slot;
            player.dirty = true;
        }
    }
}

/// Keeps the server's notion of the held tool in sync with the player's hotbar.
pub fn sync_held_tool(
    mut players: Query<(&PlayerInventory, &mut crate::sim::players::ServerPlayer)>,
    registry: Res<ContentRegistry>,
) {
    for (inventory, mut player) in &mut players {
        let held = inventory.held_item();
        let tool = held.filter(|item| {
            matches!(
                registry.item(*item).map(|entry| &entry.def.category),
                Some(deepcore_shared::content::model::ItemCategory::Tool(_))
            )
        });
        if player.tool != tool {
            player.tool = tool;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepcore_shared::content::load_registry;

    #[test]
    fn stack_size_falls_back_for_unknown_items() {
        let registry = ContentRegistry(load_registry().expect("content"));
        let known = registry.item_id("coal").expect("coal");
        assert_eq!(stack_size(&registry, known), 64);
        let unknown = deepcore_shared::content::registry::ItemId(60000);
        assert_eq!(stack_size(&registry, unknown), 64);
    }

    #[test]
    fn moving_items_into_a_machine_respects_input_slots() {
        let registry = ContentRegistry(load_registry().expect("content"));
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let mut state = deepcore_shared::crafting::MachineState::new(&registry, furnace);
        let ore = registry.item_id("raw_iron").expect("ore");
        let mut player = PlayerInventory::starting(&registry);
        player.inventory.insert(ore, 5, &registry);
        let slot = *player.inventory.slot(1).unwrap_or(&Slot::EMPTY);
        let mut machine = BlockEntity {
            position: deepcore_shared::voxel::coords::BlockPos::new(0, 0, 0),
            machine: furnace,
            facing: 0,
            state: std::mem::take(&mut state),
        };
        let moved = move_into_machine(&mut player, &mut machine, slot, 0, 5, &registry);
        assert_eq!(moved, 5);
        assert_eq!(machine.state.inputs.count_of(ore), 5);
    }
}
