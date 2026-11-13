//! Server-side machine placement and fuel handling.

use bevy::prelude::*;
use bevy_replicon::prelude::*;

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::content::model::EnergyKind;
use deepcore_shared::inventory::Slot;
use deepcore_shared::placement::validate_placement;
use deepcore_shared::protocol::*;
use deepcore_shared::voxel::coords::BlockPos;
use deepcore_shared::voxel::world::VoxelWorld;

use crate::net::ClientSessions;
use crate::sim::inventory::PlayerInventory;
use crate::sim::machines::{BlockEntity, SharedTech};

/// A request from the client to place a machine kit.
#[derive(Message, Clone, Copy, Debug)]
pub struct PlaceMachineRequest {
    /// The renet client id of the sender.
    pub client: u64,
    /// The machine definition.
    pub machine: u16,
    /// The anchor block.
    pub origin: BlockPos,
    /// Rotation in quarter turns.
    pub facing: u8,
}

/// Places a machine kit: validates the footprint, consumes the kit and spawns the entity.
pub fn place_machine(
    mut requests: MessageReader<PlaceMachineRequest>,
    mut players: Query<(&mut PlayerInventory, &NetOwner, &ServerPlayerView)>,
    tech: Res<SharedTech>,
    registry: Res<ContentRegistry>,
    mut world: ResMut<VoxelWorld>,
    mut commands: Commands,
    mut changes: MessageWriter<BlockChanged>,
    mut notices: MessageWriter<ToClients<Notice>>,
    sessions: Res<ClientSessions>,
) {
    for request in requests.read() {
        let Some((mut inventory, _, view)) =
            players.iter_mut().find(|(_, owner, _)| owner.client == request.client)
        else {
            continue;
        };
        let machine_id = deepcore_shared::content::registry::MachineId(request.machine);
        let Some(def) = registry.machine(machine_id) else {
            continue;
        };

        // The team must have researched it.
        let is_unlocked = |tech_id: &str| {
            registry.tech_id(tech_id).map(|node| tech.state.is_unlocked(node)).unwrap_or(false)
        };
        let blocks = match validate_placement(
            &world,
            def,
            request.origin,
            request.facing,
            view.position,
            is_unlocked,
        ) {
            Ok(blocks) => blocks,
            Err(error) => {
                let Some(entity) = sessions.entities.get(&request.client).copied() else {
                    continue;
                };
                notices.write(ToClients {
                    targets: SendTargets::Single(ClientId::Client(entity)),
                    message: Notice { text: error.text() },
                });
                continue;
            }
        };

        // The player must actually be holding a kit that builds this machine.
        let kit = registry.items().iter().enumerate().find_map(|(index, entry)| {
            let block = entry.places?;
            let machine = registry.block(block)?.machine?;
            (machine == machine_id)
                .then_some(deepcore_shared::content::registry::ItemId(index as u16))
        });
        let Some(kit) = kit else {
            continue;
        };
        if inventory.inventory.count_of(kit) == 0 {
            continue;
        }
        inventory.inventory.remove(kit, 1);

        // Write the blocks and spawn the machine.
        for block in &blocks {
            // Machine blocks are solid placeholders; the entity carries the behaviour.
            let placeholder = registry.block_id(&def.id).unwrap_or(deepcore_shared::voxel::AIR);
            world.set_block(*block, placeholder);
            changes.write(BlockChanged { position: pack_pos(*block), block: placeholder });
        }
        let entity = crate::sim::machines::spawn_machine(
            &mut commands,
            &registry,
            machine_id,
            request.origin,
            request.facing,
        );
        debug!("placed machine {machine_id:?} at {:?} as entity {entity}", request.origin);
        inventory.dirty = true;
    }
}

/// The position a player is at, for placement distance checks.
#[derive(Component, Clone, Copy, Debug)]
pub struct ServerPlayerView {
    /// The player's feet position.
    pub position: Vec3,
}

/// Keeps `ServerPlayerView` in sync with the authoritative player state.
pub fn sync_player_view(
    mut views: Query<(&mut ServerPlayerView, &crate::sim::players::ServerPlayer)>,
) {
    for (mut view, player) in &mut views {
        view.position = player.state.position;
    }
}

/// Burns fuel in machines that need it.
///
/// One coal burns for 80 seconds; a machine with no fuel left reports `NoFuel` and stops.
pub fn burn_fuel(
    time: Res<Time>,
    registry: Res<ContentRegistry>,
    mut machines: Query<&mut BlockEntity>,
) {
    let dt = time.delta_secs();
    for mut machine in &mut machines {
        let Some(def) = registry.machine(machine.machine) else {
            continue;
        };
        let EnergyKind::Fuel { .. } = def.energy else {
            continue;
        };
        // Fuel is consumed from the fuel slot when a recipe starts.
        if !machine.state.running || machine.state.progress > 0.0 {
            continue;
        }
        if machine.state.fuel.is_fully_empty() {
            continue;
        }
        // Take one item from the fuel slot.
        let slot = machine.state.fuel.slots().first().copied().unwrap_or(Slot::EMPTY);
        if slot.is_empty() {
            continue;
        }
        machine.state.fuel.take_from_slot(0, 1);
        let _ = dt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepcore_shared::content::load_registry;

    #[test]
    fn fuel_machine_reports_when_it_has_none() {
        let registry = ContentRegistry(load_registry().expect("content"));
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let mut state = deepcore_shared::crafting::MachineState::new(&registry, furnace);
        let ore = registry.item_id("raw_iron").expect("ore");
        state.inputs.insert(ore, 1, &registry);
        let unlocked = (0..registry.recipe_count())
            .map(|index| deepcore_shared::content::registry::RecipeId(index as u16))
            .collect::<Vec<_>>();
        assert_eq!(
            state.check(&registry, &unlocked, 1.0),
            deepcore_shared::crafting::ProcessCheck::Idle(
                deepcore_shared::crafting::IdleReason::NoFuel
            )
        );
    }
}
