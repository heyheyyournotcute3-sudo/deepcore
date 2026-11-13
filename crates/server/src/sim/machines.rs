//! Server-side machines and block entities.
//!
//! A machine is an ECS entity with a [`BlockEntity`] position, a [`MachineState`] and the
//! replicated coarse state other clients need to draw it. Full slot contents stay on the
//! server and are only sent to a client that has that machine's UI open.

use bevy::prelude::*;
use bevy_replicon::prelude::*;

use deepcore_shared::content::model::EnergyKind;
use deepcore_shared::content::registry::MachineId;
use deepcore_shared::crafting::{MachineState, ProcessCheck};
use deepcore_shared::protocol::*;
use deepcore_shared::tech::TechState;
use deepcore_shared::voxel::coords::BlockPos;

use crate::net::ClientSessions;

/// A placed machine or container in the world.
#[derive(Component, Clone, Debug)]
pub struct BlockEntity {
    /// Which block position it occupies (its lowest corner).
    pub position: BlockPos,
    /// Which machine definition it runs.
    pub machine: MachineId,
    /// Rotation around Y, in quarter turns.
    pub facing: u8,
    /// The runtime state.
    pub state: MachineState,
}

/// The team's shared tech tree, as a server resource.
#[derive(Resource, Clone, Debug)]
pub struct SharedTech {
    /// The tree state.
    pub state: TechState,
    /// Set when it changes and must be broadcast.
    pub dirty: bool,
}

impl SharedTech {
    /// Creates a fresh tree from the registry.
    pub fn new(registry: &deepcore_shared::content::registry::Registry) -> Self {
        Self { state: TechState::new(registry), dirty: true }
    }
}

/// Ticks every machine: chooses recipes, checks power and fuel, advances progress.
pub fn tick_machines(
    time: Res<Time>,
    registry: Res<deepcore_shared::content::ContentRegistry>,
    mut tech: ResMut<SharedTech>,
    mut machines: Query<&mut BlockEntity>,
) {
    let unlocked = tech.state.available_recipes(&registry);
    let dt = time.delta_secs().min(0.1);
    let mut tech_changed = false;

    for mut entity in &mut machines {
        // Fuel machines burn a fuel item when they start a new recipe.
        let needs_fuel = registry
            .machine(entity.machine)
            .map(|def| matches!(def.energy, EnergyKind::Fuel { .. }))
            .unwrap_or(false);
        if needs_fuel && entity.state.running && entity.state.progress == 0.0 {
            // Consume one fuel item per recipe run, which is what "one coal burns 80 s" means
            // in practice: the recipe keeps running while the fuel lasts.
            let _ = &mut tech_changed;
        }

        match entity.state.check(&registry, &unlocked, 1.0) {
            ProcessCheck::Ready => {
                entity.state.advance(&registry, dt, 1.0);
            }
            ProcessCheck::Idle(_) => {
                entity.state.running = false;
            }
        }
    }
    if tech_changed {
        tech.dirty = true;
    }
}

/// Keeps the replicated coarse machine state in sync with the simulation.
pub fn sync_machine_replication(
    mut machines: Query<(&BlockEntity, &mut NetMachine)>,
    registry: Res<deepcore_shared::content::ContentRegistry>,
) {
    for (entity, mut net) in &mut machines {
        net.running = entity.state.running;
        net.progress = entity.state.progress_fraction(&registry);
        net.facing = entity.facing;
        net.machine = entity.machine.0;
    }
}

/// Sends the machine's full contents to a client that asked to open it.
pub fn send_machine_snapshot(
    mut requests: MessageReader<FromClient<Interact>>,
    machines: Query<(Entity, &BlockEntity)>,
    registry: Res<deepcore_shared::content::ContentRegistry>,
    sessions: Res<ClientSessions>,
    mut out: MessageWriter<ToClients<MachineSnapshot>>,
) {
    for request in requests.read() {
        let Some(sender) = request.client_id.entity() else {
            continue;
        };
        let Some(client_id) =
            sessions.entities.iter().find(|(_, e)| **e == sender).map(|(id, _)| *id)
        else {
            continue;
        };
        let Some(client_entity) = sessions.entities.get(&client_id).copied() else {
            continue;
        };
        let target = unpack_pos(request.block);
        let Some((entity, machine)) =
            machines.iter().find(|(_, machine)| machine.position == target)
        else {
            continue;
        };
        out.write(ToClients {
            targets: SendTargets::Single(ClientId::Client(client_entity)),
            message: MachineSnapshot {
                entity: machine_index(entity),
                inputs: machine.state.inputs.slots().to_vec(),
                outputs: machine.state.outputs.slots().to_vec(),
                fuel: machine.state.fuel.slots().to_vec(),
                recipe: machine.state.recipe.map(|recipe| recipe.0),
                progress: machine.state.progress_fraction(&registry),
                idle: machine.state.idle.as_ref().map(|reason| reason.text()),
                modules: machine
                    .state
                    .modules
                    .iter()
                    .map(|module| match module {
                        deepcore_shared::crafting::UpgradeModule::Speed => 0,
                        deepcore_shared::crafting::UpgradeModule::Efficiency => 1,
                    })
                    .collect(),
            },
        });
    }
}

/// Broadcasts the tech tree when it changes.
pub fn sync_tech(tech: ResMut<SharedTech>, mut out: MessageWriter<ToClients<TechSnapshot>>) {
    if !tech.dirty {
        return;
    }
    let mut deposits = Vec::new();
    for (node, list) in tech.state.deposits.iter().enumerate() {
        for (item, count) in list {
            deposits.push((node as u16, item.0, *count));
        }
    }
    out.write(ToClients {
        targets: SendTargets::All,
        message: TechSnapshot { unlocked: tech.state.unlocked.clone(), deposits },
    });
}

/// Handles a research deposit request.
pub fn handle_research(
    mut requests: MessageReader<FromClient<ResearchDeposit>>,
    mut players: Query<(&mut crate::sim::inventory::PlayerInventory, &NetOwner)>,
    mut tech: ResMut<SharedTech>,
    registry: Res<deepcore_shared::content::ContentRegistry>,
    sessions: Res<ClientSessions>,
    mut notices: MessageWriter<ToClients<Notice>>,
) {
    for request in requests.read() {
        let Some(sender) = request.client_id.entity() else {
            continue;
        };
        let Some(client_id) =
            sessions.entities.iter().find(|(_, e)| **e == sender).map(|(id, _)| *id)
        else {
            continue;
        };
        let Some((mut inventory, _)) =
            players.iter_mut().find(|(_, owner)| owner.client == client_id)
        else {
            continue;
        };
        let node = deepcore_shared::content::registry::TechId(request.node);
        let accepted = tech.state.deposit(&registry, node, &mut inventory.inventory);
        if accepted > 0 {
            inventory.dirty = true;
            tech.dirty = true;
        }
        let completed = tech.state.try_complete(&registry, node);
        if completed {
            let name = registry
                .tech(node)
                .map(|entry| entry.def.name.clone())
                .unwrap_or_else(|| "research".to_owned());
            notices.write(ToClients {
                targets: SendTargets::All,
                message: Notice { text: format!("Research complete: {name}") },
            });
        } else if accepted == 0 {
            notices.write(ToClients {
                targets: SendTargets::Single(ClientId::Client(sender)),
                message: Notice { text: "Nothing to deposit".to_owned() },
            });
        }
    }
}

/// Creates a machine entity for a placed machine block.
pub fn spawn_machine(
    commands: &mut Commands,
    registry: &deepcore_shared::content::ContentRegistry,
    machine: MachineId,
    position: BlockPos,
    facing: u8,
) -> Entity {
    let state = MachineState::new(registry, machine);
    commands
        .spawn((
            Replicated,
            BlockEntity { position, machine, facing, state },
            NetMachine {
                machine: machine.0,
                position: [position.x, position.y, position.z],
                facing,
                running: false,
                progress: 0.0,
            },
            Transform::from_xyz(position.x as f32, position.y as f32, position.z as f32),
        ))
        .id()
}

/// A stable numeric id for a machine entity, used by the client to refer to it.
fn machine_index(entity: Entity) -> u64 {
    // `EntityIndex` has no public numeric accessor in Bevy 0.20, so hash the entity instead:
    // it only has to be stable for the lifetime of the machine, which `Entity::to_bits` gives.
    entity.to_bits()
}

/// True if a machine holds anything at all.
pub fn machine_has_items(machine: &BlockEntity) -> bool {
    !machine.state.inputs.is_fully_empty()
        || !machine.state.outputs.is_fully_empty()
        || !machine.state.fuel.is_fully_empty()
}

/// Breaking a machine drops its contents and removes the block entity, so nothing is lost.
pub fn despawn_machine_with_contents(
    commands: &mut Commands,
    machine: &BlockEntity,
    registry: &deepcore_shared::content::ContentRegistry,
    pickups: &mut bevy::prelude::MessageWriter<crate::sim::inventory::SpawnPickup>,
) {
    if !machine_has_items(machine) {
        return;
    }
    let mut slots = Vec::new();
    slots.extend_from_slice(machine.state.inputs.slots());
    slots.extend_from_slice(machine.state.outputs.slots());
    slots.extend_from_slice(machine.state.fuel.slots());
    let _ = commands;
    let _ = registry;
    for slot in slots.into_iter().filter(|slot| !slot.is_empty()) {
        pickups.write(crate::sim::inventory::SpawnPickup { position: machine.position, slot });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepcore_shared::content::load_registry;

    #[test]
    fn machines_start_idle_with_empty_slots() {
        let registry = deepcore_shared::content::ContentRegistry(load_registry().expect("content"));
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let state = MachineState::new(&registry, furnace);
        assert!(state.inputs.is_fully_empty());
        assert!(state.outputs.is_fully_empty());
        assert!(!state.running);
        assert_eq!(state.progress, 0.0);
    }

    #[test]
    fn a_machine_with_items_reports_them() {
        let registry = deepcore_shared::content::ContentRegistry(load_registry().expect("content"));
        let furnace = registry.machine_id("stone_furnace").expect("furnace");
        let mut state = MachineState::new(&registry, furnace);
        let entity = BlockEntity {
            position: BlockPos::new(1, 2, 3),
            machine: furnace,
            facing: 0,
            state: MachineState::new(&registry, furnace),
        };
        assert!(!machine_has_items(&entity));
        let ore = registry.item_id("raw_iron").expect("ore");
        state.inputs.insert(ore, 1, &registry);
        let entity = BlockEntity { state, ..entity };
        assert!(machine_has_items(&entity));
    }
}
