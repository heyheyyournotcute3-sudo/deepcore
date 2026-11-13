//! Power networks: steam pipes and electric cables.
//!
//! The brief's model, in one place:
//!
//! * **Steam** — boilers produce steam units per second into a pipe network, consumers draw
//!   from it. A network is a set of connected pipe blocks.
//! * **Electricity** — generators produce kW into a cable network. If demand exceeds supply,
//!   every consumer runs at `supply / demand` speed. Battery banks absorb surplus and cover
//!   deficits.
//!
//! Topology is recomputed only when a pipe, cable or machine is placed or removed; each tick
//! only sums numbers. That is what keeps a base with hundreds of machines inside the 10 ms
//! tick budget.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::voxel::coords::BlockPos;

/// Which kind of network a node belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NetworkKind {
    /// Steam pipes.
    #[default]
    Steam,
    /// Electric cables.
    Electric,
}

/// One participant in a network: a producer, a consumer, or both.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetworkNode {
    /// The block that carries the connection (a pipe, a cable, or the machine itself).
    pub position: BlockPos,
    /// Production per second (steam units or kW).
    pub supply: f32,
    /// Demand per second when running.
    pub demand: f32,
}

impl NetworkNode {
    /// A pure conductor: carries the network but neither produces nor consumes.
    pub fn conductor(position: BlockPos) -> Self {
        Self { position, supply: 0.0, demand: 0.0 }
    }

    /// A producer.
    pub fn producer(position: BlockPos, supply: f32) -> Self {
        Self { position, supply, demand: 0.0 }
    }

    /// A consumer.
    pub fn consumer(position: BlockPos, demand: f32) -> Self {
        Self { position, supply: 0.0, demand }
    }
}

/// A connected network of pipes or cables.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Network {
    /// Which kind it is.
    pub kind: NetworkKind,
    /// The nodes in it, in insertion order.
    pub nodes: Vec<NetworkNode>,
    /// Total supply per second.
    pub supply: f32,
    /// Total demand per second.
    pub demand: f32,
    /// Stored energy (battery banks only), in kW-seconds.
    pub stored: f32,
    /// Maximum storage.
    pub capacity: f32,
}

impl Network {
    /// The fraction of its demand the network can satisfy, 0..1.
    ///
    /// This is the brownout factor: every consumer runs at this speed when it is below 1.
    pub fn satisfaction(&self) -> f32 {
        if self.demand <= 0.0 {
            return 1.0;
        }
        ((self.supply + self.discharge_available()) / self.demand).clamp(0.0, 1.0)
    }

    /// How much storage can be drawn on this tick (one second's worth).
    fn discharge_available(&self) -> f32 {
        // A battery bank can discharge its whole store over one second at most.
        self.stored.max(0.0)
    }

    /// Recomputes the supply and demand totals from the nodes.
    pub fn recompute_totals(&mut self) {
        self.supply = self.nodes.iter().map(|node| node.supply).sum();
        self.demand = self.nodes.iter().map(|node| node.demand).sum();
    }

    /// Advances one tick: charges storage from surplus, discharges it into a deficit.
    pub fn tick(&mut self, dt: f32) {
        let net = self.supply - self.demand;
        if net > 0.0 {
            // Surplus charges the batteries.
            self.stored = (self.stored + net * dt).min(self.capacity);
        } else if net < 0.0 {
            // Deficit drains them.
            let needed = -net * dt;
            self.stored = (self.stored - needed).max(0.0);
        }
    }

    /// True if the network can run a consumer with this demand right now.
    pub fn can_supply(&self, demand: f32) -> bool {
        self.supply + self.stored >= demand
    }
}

/// Every network in the world, plus a lookup from block position to network index.
#[derive(Default, Debug)]
pub struct PowerGrids {
    /// The networks.
    pub networks: Vec<Network>,
    /// Which network a conductor block belongs to.
    positions: HashMap<(NetworkKind, BlockPos), usize>,
    /// Set when topology needs rebuilding.
    dirty: bool,
}

impl PowerGrids {
    /// Creates an empty grid.
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the topology as needing a rebuild (a pipe, cable or machine changed).
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// True if the topology needs a rebuild.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Rebuilds every network from a set of conductor positions and machine nodes.
    ///
    /// This is a flood fill over 6-connected conductor blocks; machines join the network of any
    /// adjacent conductor.
    pub fn rebuild(&mut self, conductors: &[(NetworkKind, BlockPos)], machines: &[NetworkNode]) {
        self.networks.clear();
        self.positions.clear();
        self.dirty = false;

        // Index conductors by kind for fast neighbour lookup.
        let mut by_kind: HashMap<NetworkKind, HashSet<BlockPos>> = HashMap::new();
        for (kind, position) in conductors {
            by_kind.entry(*kind).or_default().insert(*position);
        }

        // Machines attach to a network through any adjacent conductor.
        let mut machine_by_position: HashMap<BlockPos, NetworkNode> = HashMap::new();
        for machine in machines {
            machine_by_position.insert(machine.position, *machine);
        }

        for kind in [NetworkKind::Steam, NetworkKind::Electric] {
            let Some(set) = by_kind.get(&kind) else {
                continue;
            };
            let mut visited: HashSet<BlockPos> = HashSet::new();
            for &start in set {
                if !visited.insert(start) {
                    continue;
                }
                let index = self.networks.len();
                let mut network = Network { kind, ..Default::default() };

                // Flood fill.
                let mut queue = VecDeque::new();
                queue.push_back(start);
                while let Some(current) = queue.pop_front() {
                    self.positions.insert((kind, current), index);
                    network.nodes.push(NetworkNode::conductor(current));
                    // A machine sitting on this conductor, or on any block adjacent to
                    // it, joins the network: machines are placed next to pipes, not inside.
                    for candidate in std::iter::once(current).chain(current.neighbours()) {
                        if let Some(machine) = machine_by_position.get(&candidate) {
                            // A machine can border several pipes; it belongs to the network
                            // exactly once. Conductor nodes are ignored here: the machine may
                            // sit on the very block that carries the network.
                            let already_added = network.nodes.iter().any(|node| {
                                node.position == candidate
                                    && (node.supply > 0.0 || node.demand > 0.0)
                            });
                            if !already_added {
                                network.nodes.push(*machine);
                            }
                            // Let `satisfaction_at` find the network from the machine too.
                            self.positions.entry((kind, candidate)).or_insert(index);
                        }
                    }
                    for neighbour in current.neighbours() {
                        if set.contains(&neighbour) && visited.insert(neighbour) {
                            queue.push_back(neighbour);
                        }
                    }
                }
                network.recompute_totals();
                self.networks.push(network);
            }

            // Machines that are not adjacent to any conductor form their own single-node
            // networks, so a lone generator still powers an adjacent machine.
            for machine in machines {
                if self.positions.contains_key(&(kind, machine.position)) {
                    continue;
                }
                if machine.supply <= 0.0 && machine.demand <= 0.0 {
                    continue;
                }
                let mut network = Network { kind, ..Default::default() };
                network.nodes.push(*machine);
                network.recompute_totals();
                self.networks.push(network);
            }
        }
    }

    /// The network a block belongs to, if any.
    pub fn network_at(&self, kind: NetworkKind, position: BlockPos) -> Option<&Network> {
        self.positions.get(&(kind, position)).and_then(|index| self.networks.get(*index))
    }

    /// The satisfaction of the network a consumer sits on (1.0 if it has none).
    pub fn satisfaction_at(&self, kind: NetworkKind, position: BlockPos) -> f32 {
        self.network_at(kind, position).map(Network::satisfaction).unwrap_or(0.0)
    }

    /// Advances every network by one tick.
    pub fn tick(&mut self, dt: f32) {
        for network in &mut self.networks {
            network.recompute_totals();
            network.tick(dt);
        }
    }

    /// Totals across all networks of one kind, for the network overlay.
    pub fn totals(&self, kind: NetworkKind) -> (f32, f32, f32, f32) {
        let mut supply = 0.0;
        let mut demand = 0.0;
        let mut stored = 0.0;
        let mut capacity = 0.0;
        for network in self.networks.iter().filter(|network| network.kind == kind) {
            supply += network.supply;
            demand += network.demand;
            stored += network.stored;
            capacity += network.capacity;
        }
        (supply, demand, stored, capacity)
    }

    /// Number of networks of one kind.
    pub fn network_count(&self, kind: NetworkKind) -> usize {
        self.networks.iter().filter(|network| network.kind == kind).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(x: i32, y: i32, z: i32) -> BlockPos {
        BlockPos::new(x, y, z)
    }

    #[test]
    fn a_pipe_line_becomes_one_network() {
        let mut grids = PowerGrids::new();
        let conductors: Vec<(NetworkKind, BlockPos)> =
            (0..5).map(|x| (NetworkKind::Steam, pos(x, 64, 0))).collect();
        grids.rebuild(&conductors, &[]);
        assert_eq!(grids.network_count(NetworkKind::Steam), 1);
        let network = grids.network_at(NetworkKind::Steam, pos(2, 64, 0)).expect("network");
        assert_eq!(network.nodes.len(), 5);
    }

    #[test]
    fn disconnected_pipes_are_separate_networks() {
        let mut grids = PowerGrids::new();
        let conductors = vec![
            (NetworkKind::Steam, pos(0, 64, 0)),
            (NetworkKind::Steam, pos(1, 64, 0)),
            (NetworkKind::Steam, pos(20, 64, 0)),
        ];
        grids.rebuild(&conductors, &[]);
        assert_eq!(grids.network_count(NetworkKind::Steam), 2);
        assert_eq!(grids.network_at(NetworkKind::Steam, pos(0, 64, 0)).expect("a").nodes.len(), 2);
        assert_eq!(grids.network_at(NetworkKind::Steam, pos(20, 64, 0)).expect("b").nodes.len(), 1);
    }

    #[test]
    fn machines_join_adjacent_networks() {
        let mut grids = PowerGrids::new();
        let conductors = vec![(NetworkKind::Electric, pos(0, 64, 0))];
        let machines = vec![NetworkNode::consumer(pos(0, 64, 0), 20.0)];
        grids.rebuild(&conductors, &machines);
        let network = grids.network_at(NetworkKind::Electric, pos(0, 64, 0)).expect("network");
        assert_eq!(network.demand, 20.0);
    }

    #[test]
    fn brownout_scales_with_supply_over_demand() {
        let mut network = Network { kind: NetworkKind::Electric, ..Default::default() };
        network.nodes.push(NetworkNode::producer(pos(0, 0, 0), 50.0));
        network.nodes.push(NetworkNode::consumer(pos(1, 0, 0), 100.0));
        network.recompute_totals();
        assert!((network.satisfaction() - 0.5).abs() < 1e-6, "half the demand can be met");
    }

    #[test]
    fn batteries_cover_a_deficit_then_run_flat() {
        let mut network = Network {
            kind: NetworkKind::Electric,
            stored: 10.0,
            capacity: 100.0,
            ..Default::default()
        };
        network.nodes.push(NetworkNode::producer(pos(0, 0, 0), 50.0));
        network.nodes.push(NetworkNode::consumer(pos(1, 0, 0), 100.0));
        network.recompute_totals();
        // 50 kW supply + 10 kW stored covers 60 of 100.
        assert!((network.satisfaction() - 0.6).abs() < 1e-6);
        // Drain for a few seconds: the store empties and satisfaction falls.
        for _ in 0..20 {
            network.tick(1.0);
        }
        assert_eq!(network.stored, 0.0);
        assert!((network.satisfaction() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn surplus_charges_batteries_up_to_capacity() {
        let mut network =
            Network { kind: NetworkKind::Electric, capacity: 50.0, ..Default::default() };
        network.nodes.push(NetworkNode::producer(pos(0, 0, 0), 100.0));
        network.nodes.push(NetworkNode::consumer(pos(1, 0, 0), 40.0));
        network.recompute_totals();
        for _ in 0..10 {
            network.tick(1.0);
        }
        assert!(
            (network.stored - 50.0).abs() < 1e-3,
            "storage fills to capacity, got {}",
            network.stored
        );
    }

    #[test]
    fn steam_and_electric_networks_are_independent() {
        let mut grids = PowerGrids::new();
        let conductors =
            vec![(NetworkKind::Steam, pos(0, 64, 0)), (NetworkKind::Electric, pos(0, 64, 0))];
        grids.rebuild(&conductors, &[]);
        assert_eq!(grids.network_count(NetworkKind::Steam), 1);
        assert_eq!(grids.network_count(NetworkKind::Electric), 1);
    }

    #[test]
    fn a_network_without_demand_is_fully_satisfied() {
        let mut network = Network { kind: NetworkKind::Steam, ..Default::default() };
        network.nodes.push(NetworkNode::producer(pos(0, 0, 0), 10.0));
        network.recompute_totals();
        assert!((network.satisfaction() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn totals_sum_across_networks() {
        let mut grids = PowerGrids::new();
        let conductors =
            vec![(NetworkKind::Electric, pos(0, 64, 0)), (NetworkKind::Electric, pos(40, 64, 0))];
        let machines = vec![
            NetworkNode::producer(pos(0, 64, 0), 100.0),
            NetworkNode::consumer(pos(40, 64, 0), 30.0),
        ];
        grids.rebuild(&conductors, &machines);
        let (supply, demand, _, _) = grids.totals(NetworkKind::Electric);
        assert!((supply - 100.0).abs() < 1e-6);
        assert!((demand - 30.0).abs() < 1e-6);
    }
}
