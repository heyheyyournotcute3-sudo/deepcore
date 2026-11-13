//! Server-side power simulation: builds pipe and cable networks from the world and drives
//! machines from them.

use bevy::prelude::*;

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::content::model::EnergyKind;
use deepcore_shared::power::{NetworkKind, NetworkNode, PowerGrids};
use deepcore_shared::voxel::coords::BlockPos;
use deepcore_shared::voxel::world::VoxelWorld;

use crate::sim::machines::BlockEntity;

/// The live power grids, rebuilt when the world changes.
#[derive(Resource, Default)]
pub struct ServerPower {
    /// The grids.
    pub grids: PowerGrids,
    /// Set when a pipe, cable or machine changed and the topology must be rebuilt.
    pub needs_rebuild: bool,
}

/// Block names that conduct each network kind.
const STEAM_CONDUCTOR: &str = "copper_pipe_block";
const ELECTRIC_CONDUCTOR: &str = "copper_cable_block";

/// Rebuilds the networks from the world and the machine list.
///
/// This is the expensive step, so it only runs when `needs_rebuild` is set: placing a pipe, a
/// cable or a machine, or removing one.
pub fn rebuild_power_networks(
    mut power: ResMut<ServerPower>,
    world: Res<VoxelWorld>,
    machines: Query<&BlockEntity>,
    registry: Res<ContentRegistry>,
) {
    if !power.needs_rebuild {
        return;
    }
    power.needs_rebuild = false;

    let steam_block = registry.block_id(STEAM_CONDUCTOR);
    let electric_block = registry.block_id(ELECTRIC_CONDUCTOR);

    // Collect conductor blocks. Chunks are palette-compressed, so scanning loaded chunks is
    // cheap; only chunks that actually contain the conductor ids are walked in full.
    let mut conductors: Vec<(NetworkKind, BlockPos)> = Vec::new();
    for (pos, chunk) in world.iter() {
        let palette = chunk.palette();
        let has_steam = steam_block.is_some_and(|id| palette.contains(&id));
        let has_electric = electric_block.is_some_and(|id| palette.contains(&id));
        if !has_steam && !has_electric {
            continue;
        }
        let origin = pos.origin();
        for y in 0..deepcore_shared::voxel::CHUNK_SIZE {
            for z in 0..deepcore_shared::voxel::CHUNK_SIZE {
                for x in 0..deepcore_shared::voxel::CHUNK_SIZE {
                    let block = chunk.get(deepcore_shared::voxel::LocalPos::new(x, y, z));
                    if Some(block) == steam_block {
                        conductors.push((NetworkKind::Steam, origin.offset(x, y, z)));
                    } else if Some(block) == electric_block {
                        conductors.push((NetworkKind::Electric, origin.offset(x, y, z)));
                    }
                }
            }
        }
    }

    // Machines contribute supply and demand.
    let mut nodes = Vec::new();
    for machine in &machines {
        let Some(def) = registry.machine(machine.machine) else {
            continue;
        };
        let (kind, supply, demand) = match def.energy {
            EnergyKind::Electric { kw } => (NetworkKind::Electric, 0.0, kw),
            EnergyKind::Steam { per_second } => (NetworkKind::Steam, 0.0, per_second),
            EnergyKind::Shaft { per_second } => (NetworkKind::Steam, 0.0, per_second),
            EnergyKind::Fuel { .. } | EnergyKind::None => continue,
        };
        // Generators and boilers produce instead of consuming.
        let is_producer = matches!(
            def.id.as_str(),
            "steam_boiler" | "generator" | "steam_turbine" | "heat_exchanger"
        );
        let node = if is_producer {
            let output = match def.id.as_str() {
                "steam_boiler" => 20.0,
                "steam_turbine" => 40.0,
                "generator" => 60.0,
                "heat_exchanger" => 100.0,
                _ => 0.0,
            };
            NetworkNode::producer(machine.position, output)
        } else {
            NetworkNode::consumer(machine.position, demand)
        };
        let _ = supply;
        let _ = kind;
        nodes.push((machine.position, node));
    }

    // A battery bank stores energy on whichever network it touches.
    let node_list: Vec<NetworkNode> = nodes.iter().map(|(_, node)| *node).collect();
    power.grids.rebuild(&conductors, &node_list);

    // Give battery banks their capacity.
    for machine in &machines {
        let Some(def) = registry.machine(machine.machine) else {
            continue;
        };
        if def.id != "battery_bank" {
            continue;
        }
        for network in &mut power.grids.networks {
            if network.nodes.iter().any(|node| node.position == machine.position) {
                network.capacity = 500.0;
                network.stored = network.stored.min(network.capacity);
            }
        }
    }

    debug!(
        "power: {} steam networks, {} electric networks",
        power.grids.network_count(NetworkKind::Steam),
        power.grids.network_count(NetworkKind::Electric)
    );
}

/// Advances the grids one tick.
pub fn tick_power(mut power: ResMut<ServerPower>, time: Res<Time>) {
    power.grids.tick(time.delta_secs().min(0.1));
}

/// The power available to a machine, 0..1, used to scale its speed (brownouts).
pub fn power_for(power: &ServerPower, machine: &BlockEntity, registry: &ContentRegistry) -> f32 {
    let Some(def) = registry.machine(machine.machine) else {
        return 1.0;
    };
    match def.energy {
        EnergyKind::Electric { .. } => {
            power.grids.satisfaction_at(NetworkKind::Electric, machine.position)
        }
        EnergyKind::Steam { .. } | EnergyKind::Shaft { .. } => {
            power.grids.satisfaction_at(NetworkKind::Steam, machine.position)
        }
        EnergyKind::Fuel { .. } | EnergyKind::None => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepcore_shared::power::Network;

    #[test]
    fn a_steam_network_with_a_boiler_and_an_engine_runs() {
        let mut network = Network { kind: NetworkKind::Steam, ..Default::default() };
        // A boiler produces 20 steam/s, a steam engine draws 4.
        network.nodes.push(NetworkNode::producer(BlockPos::new(0, 64, 0), 20.0));
        network.nodes.push(NetworkNode::consumer(BlockPos::new(1, 64, 0), 4.0));
        network.recompute_totals();
        assert_eq!(network.satisfaction(), 1.0);
    }

    #[test]
    fn an_overloaded_electric_network_browns_out() {
        let mut network = Network { kind: NetworkKind::Electric, ..Default::default() };
        network.nodes.push(NetworkNode::producer(BlockPos::new(0, 64, 0), 60.0));
        network.nodes.push(NetworkNode::consumer(BlockPos::new(1, 64, 0), 45.0));
        network.nodes.push(NetworkNode::consumer(BlockPos::new(2, 64, 0), 30.0));
        network.recompute_totals();
        // 60 supply against 75 demand: every consumer runs at 80% speed.
        assert!((network.satisfaction() - 0.8).abs() < 1e-6);
    }
}
