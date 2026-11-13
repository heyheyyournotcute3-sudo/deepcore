//! Server-side player simulation: applying movement, validating intents, mining and placing.
//!
//! Every rule here runs only on the server. The client predicts its own movement with the same
//! shared controller, but the authoritative state is the one produced by these systems.

use bevy::prelude::*;

use bevy_replicon::prelude::*;

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::mining::{
    MineBlocked, MineCheck, ToolProfile, area_blocks, check_mine, face_normal,
};
use deepcore_shared::movement::{BlockPhysics, MoveInput, MoveTuning, PlayerState, step_player};
use deepcore_shared::protocol::*;
use deepcore_shared::voxel::coords::BlockPos;
use deepcore_shared::voxel::world::VoxelWorld;

/// The authoritative state of a connected player.
#[derive(Component)]
pub struct ServerPlayer {
    /// Physics state.
    pub state: PlayerState,
    /// Last movement sequence number applied.
    pub last_seq: u32,
    /// What the player is mining, and how long they have been at it.
    pub mining: Option<MiningJob>,
    /// The tool the server believes the player holds (M3 replaces this with the inventory).
    pub tool: Option<deepcore_shared::content::registry::ItemId>,
    /// Display name, shown to other players.
    pub name: String,
}

/// An in-progress mining job.
#[derive(Clone, Copy, Debug)]
pub struct MiningJob {
    /// The block being mined.
    pub block: BlockPos,
    /// Which face was hit.
    pub face: u8,
    /// Server ticks accumulated so far.
    pub ticks: u32,
    /// Ticks needed, from the shared break-time formula.
    pub required: u32,
}

/// Physics tables, built once from content.
#[derive(Resource)]
pub struct ServerPhysics {
    /// Block classification.
    pub physics: BlockPhysics,
    /// Movement tuning.
    pub tuning: MoveTuning,
}

impl ServerPhysics {
    /// Builds the tables from the content registry.
    pub fn from_registry(registry: &ContentRegistry) -> Self {
        Self { physics: BlockPhysics::from_registry(registry), tuning: MoveTuning::default() }
    }
}

/// Applies queued movement intents in the fixed update, in order, exactly once.
///
/// Inputs are sanitised before use: a client can send anything, and the server must never
/// trust it. Out-of-order or replayed sequence numbers are dropped.
pub fn apply_movement(
    mut players: Query<(&mut ServerPlayer, &mut NetPlayer, &NetOwner)>,
    mut intents: MessageReader<FromClient<MoveIntent>>,
    world: Res<VoxelWorld>,
    physics: Res<ServerPhysics>,
    sessions: Res<crate::net::ClientSessions>,
) {
    // Group intents by the sending client entity so each player gets its own inputs in order.
    let mut by_client: std::collections::HashMap<Entity, Vec<MoveInput>> = Default::default();
    for intent in intents.read() {
        if let Some(entity) = intent.client_id.entity() {
            by_client.entry(entity).or_default().push(intent.input);
        }
    }

    for (mut player, mut net, owner) in &mut players {
        // The player's owner client id is a renet id; resolve it to the client entity.
        let Some(client_entity) = sessions.entities.get(&owner.client).copied() else {
            continue;
        };
        let Some(inputs) = by_client.remove(&client_entity) else {
            continue;
        };
        for input in inputs {
            if input.seq <= player.last_seq {
                // Already applied (a duplicate or a replay): ignore.
                continue;
            }
            let sanitized = input.sanitized();
            step_player(
                &world,
                &physics.physics,
                &mut player.state,
                sanitized,
                1.0 / deepcore_shared::SERVER_TICK_HZ as f32,
                &physics.tuning,
            );
            player.last_seq = input.seq;
        }
        *net = NetPlayer::from_state(&player.state, player.last_seq);
    }
}

/// Announces a player's name to everyone when they join.
pub fn announce_join(
    players: Query<&ServerPlayer, Added<ServerPlayer>>,
    mut chat: MessageWriter<ToClients<ChatLine>>,
) {
    for player in &players {
        chat.write(ToClients {
            targets: SendTargets::All,
            message: ChatLine {
                from: "server".to_owned(),
                text: format!("{} joined the mine", player.name),
            },
        });
    }
}

/// Starts a mining job when the client asks, validating reach and line of sight.
pub fn handle_start_mining(
    sessions: Res<crate::net::ClientSessions>,
    mut players: Query<(Entity, &mut ServerPlayer, &NetOwner)>,
    mut intents: MessageReader<FromClient<StartMining>>,
    mut notices: MessageWriter<ToClients<Notice>>,
    world: Res<VoxelWorld>,
    registry: Res<ContentRegistry>,
) {
    for intent in intents.read() {
        let Some(sender_entity) = intent.client_id.entity() else {
            continue;
        };
        let Some(client_id) =
            sessions.entities.iter().find(|(_, e)| **e == sender_entity).map(|(id, _)| *id)
        else {
            continue;
        };
        let Some((_entity, mut player, net)) =
            players.iter_mut().find(|(_, _, owner)| owner.client == client_id)
        else {
            continue;
        };
        let block = unpack_pos(intent.block);
        if !block.in_world() || world.block(block) == deepcore_shared::voxel::AIR {
            continue;
        }
        // Reach check against the player's authoritative position.
        let eye = player.state.eye_position();
        if block.center_f32().distance(eye) > MAX_REACH + 1.5 {
            debug!("client {} tried to mine a block out of reach", intent.client_id);
            continue;
        }
        // Line of sight: cast from the eye towards the block centre; the first solid block hit
        // must be the target, otherwise something is in the way.
        let direction = (block.center_f32() - eye).normalize_or_zero();
        let passable = &physics_passable(&registry);
        let hit = deepcore_shared::mining::raycast(&world, eye, direction, MAX_REACH + 2.0, |b| {
            passable.contains(&b)
        });
        if hit.map(|target| target.block) != Some(block) {
            debug!("client {} tried to mine a block without line of sight", intent.client_id);
            continue;
        }

        let profile = ToolProfile::for_item(&registry, player.tool, true);
        match check_mine(&registry, world.block(block), &profile) {
            MineCheck::Allowed { ticks, .. } => {
                player.mining =
                    Some(MiningJob { block, face: intent.face, ticks: 0, required: ticks });
            }
            MineCheck::Blocked(MineBlocked::TierTooLow { required }) => {
                notices.write(ToClients {
                    targets: SendTargets::Single(intent.client_id),
                    message: Notice { text: format!("Too hard — needs Tier {required}") },
                });
            }
            MineCheck::Blocked(MineBlocked::NoEnergy) => {
                notices.write(ToClients {
                    targets: SendTargets::Single(intent.client_id),
                    message: Notice { text: "Out of power".to_owned() },
                });
            }
            MineCheck::Blocked(MineBlocked::Unbreakable) => {}
        }
        let _ = &net;
    }
}

fn physics_passable(registry: &ContentRegistry) -> Vec<deepcore_shared::voxel::BlockId> {
    let mut passable = Vec::new();
    for block in registry.blocks() {
        if block.def.kind == deepcore_shared::content::model::BlockKind::Passable
            && let Some(id) = registry.block_id(&block.def.id)
        {
            passable.push(id);
        }
    }
    passable
}

/// Cancels a mining job when the client releases the button.
pub fn handle_stop_mining(
    sessions: Res<crate::net::ClientSessions>,
    mut players: Query<(&mut ServerPlayer, &NetOwner)>,
    mut intents: MessageReader<FromClient<StopMining>>,
) {
    for intent in intents.read() {
        let Some(sender) = intent.client_id.entity() else {
            continue;
        };
        let Some(client_id) =
            sessions.entities.iter().find(|(_, e)| **e == sender).map(|(id, _)| *id)
        else {
            continue;
        };
        for (mut player, owner) in &mut players {
            if owner.client == client_id {
                player.mining = None;
            }
        }
    }
}

/// Advances mining jobs and breaks blocks when they finish.
pub fn advance_mining(
    mut commands: Commands,
    machines: Query<(Entity, &crate::sim::machines::BlockEntity)>,
    mut players: Query<(Entity, &mut ServerPlayer, &NetOwner)>,
    mut inventories: Query<&mut crate::sim::inventory::PlayerInventory>,
    mut world: ResMut<VoxelWorld>,
    registry: Res<ContentRegistry>,
    mut changes: MessageWriter<BlockChanged>,
    mut pickups: MessageWriter<crate::sim::inventory::SpawnPickup>,
    mut power: ResMut<crate::sim::power::ServerPower>,
) {
    for (entity, mut player, _owner) in &mut players {
        let Some(job) = player.mining else {
            continue;
        };
        // Cancel if the block vanished or the player wandered away.
        if world.block(job.block) == deepcore_shared::voxel::AIR {
            player.mining = None;
            continue;
        }
        let eye = player.state.eye_position();
        if job.block.center_f32().distance(eye) > MAX_REACH + 2.0 {
            player.mining = None;
            continue;
        }

        let Some(mut job) = player.mining else {
            continue;
        };
        job.ticks += 1;
        if job.ticks < job.required {
            player.mining = Some(job);
            continue;
        }

        // Break the block (or the whole face, for area tools).
        let profile = ToolProfile::for_item(&registry, player.tool, true);
        let normal = face_normal(job.face);
        let targets = area_blocks(job.block, normal, profile.area);
        let mut broke = false;
        for pos in targets {
            let block = world.block(pos);
            if block == deepcore_shared::voxel::AIR {
                continue;
            }
            if !matches!(check_mine(&registry, block, &profile), MineCheck::Allowed { .. }) {
                continue;
            }
            // Breaking a machine block spills its contents rather than destroying them.
            if let Some(entry) = registry.block(block)
                && entry.machine.is_some()
                && let Some((machine_entity, machine)) =
                    machines.iter().find(|(_, machine)| machine.position == pos)
            {
                crate::sim::machines::despawn_machine_with_contents(
                    &mut commands,
                    machine,
                    &registry,
                    &mut pickups,
                );
                commands.entity(machine_entity).despawn();
            }
            // Pipes, cables and machine blocks change the network topology.
            power.needs_rebuild = true;
            if world.set_block(pos, deepcore_shared::voxel::AIR) {
                changes.write(BlockChanged {
                    position: pack_pos(pos),
                    block: deepcore_shared::voxel::AIR,
                });
                // Drops go straight into the miner's inventory; anything that does not fit
                // becomes a pickup in the world so nothing is silently destroyed.
                let mut overflow = Vec::new();
                if let Ok(mut inventory) = inventories.get_mut(entity) {
                    crate::sim::inventory::grant_drops(
                        &registry,
                        &mut inventory.inventory,
                        block,
                        &mut overflow,
                    );
                    inventory.dirty = true;
                }
                for slot in overflow {
                    pickups.write(crate::sim::inventory::SpawnPickup { position: pos, slot });
                }
                broke = true;
            }
        }
        player.mining = None;
        let _ = broke;
    }
}

/// Places a block when the client asks, validating reach, emptiness and overlap with players.
pub fn handle_place_block(
    mut machine_requests: MessageWriter<crate::sim::placement::PlaceMachineRequest>,
    sessions: Res<crate::net::ClientSessions>,
    mut players: Query<(&mut ServerPlayer, &NetOwner)>,
    inventories: Query<(&crate::sim::inventory::PlayerInventory, &NetOwner)>,
    registry: Res<ContentRegistry>,
    mut intents: MessageReader<FromClient<PlaceBlock>>,
    mut world: ResMut<VoxelWorld>,
    mut changes: MessageWriter<BlockChanged>,
) {
    for intent in intents.read() {
        let Some(sender) = intent.client_id.entity() else {
            continue;
        };
        let Some(client_id) =
            sessions.entities.iter().find(|(_, e)| **e == sender).map(|(id, _)| *id)
        else {
            continue;
        };
        let Some((player, _)) = players.iter_mut().find(|(_, owner)| owner.client == client_id)
        else {
            continue;
        };
        let pos = unpack_pos(intent.position);
        if !pos.in_world() || world.block(pos) != deepcore_shared::voxel::AIR {
            continue;
        }
        let eye = player.state.eye_position();
        if pos.center_f32().distance(eye) > MAX_REACH + 1.5 {
            continue;
        }
        // Do not place a block inside a player.
        let player_box = deepcore_shared::movement::Aabb::from_feet(
            player.state.position,
            player.state.height(),
        );
        if player_box.intersects_block(pos) {
            continue;
        }
        // Only the placing player's own inventory matters: derive the block from the item the
        // server knows they hold, never from what the client claims.
        let Some((inventory, _)) = inventories.iter().find(|(_, owner)| owner.client == client_id)
        else {
            continue;
        };
        let Some(block) = inventory.held_block(&registry) else {
            continue;
        };

        // Machine kits go through the footprint validator, which also checks research.
        if let Some(entry) = registry.block(block)
            && let Some(machine_id) = entry.machine
        {
            machine_requests.write(crate::sim::placement::PlaceMachineRequest {
                client: client_id,
                machine: machine_id.0,
                origin: pos,
                facing: 0,
            });
            continue;
        }

        if world.set_block(pos, block) {
            changes.write(BlockChanged { position: pack_pos(pos), block });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_physics_matches_client_physics() {
        let registry = ContentRegistry(deepcore_shared::content::load_registry().expect("content"));
        let physics = ServerPhysics::from_registry(&registry);
        let client_physics = BlockPhysics::from_registry(&registry);
        assert_eq!(physics.physics.passable, client_physics.passable);
        assert_eq!(physics.physics.ladders, client_physics.ladders);
        assert_eq!(physics.physics.water, client_physics.water);
    }

    #[test]
    fn passable_blocks_are_identified() {
        let registry = ContentRegistry(deepcore_shared::content::load_registry().expect("content"));
        let passable = physics_passable(&registry);
        let water = registry.block_id("water").expect("water");
        let ladder = registry.block_id("ladder").expect("ladder");
        // Water and ladders are passable: they do not stop movement but do change it.
        assert!(passable.contains(&water));
        assert!(passable.contains(&ladder));
        // Solid rock is not.
        let stone = registry.block_id("stone").expect("stone");
        assert!(!passable.contains(&stone));
    }
}
