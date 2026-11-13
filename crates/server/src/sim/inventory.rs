//! Server-side inventory: per-player inventories, item pickups, death crates and mining drops.

use bevy::prelude::*;
use bevy_replicon::prelude::*;

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::inventory::{Inventory, Slot};
use deepcore_shared::protocol::*;
use deepcore_shared::voxel::coords::BlockPos;
use deepcore_shared::voxel::world::VoxelWorld;

use crate::net::ClientSessions;

/// A player's inventory, owned by the server and mirrored to its client on change.
#[derive(Component, Clone, Debug)]
pub struct PlayerInventory {
    /// The 45 slots (hotbar first).
    pub inventory: Inventory,
    /// Hotbar selection, 0..9.
    pub selected: u8,
    /// Set when the inventory changes and the owner must be told.
    pub dirty: bool,
}

impl PlayerInventory {
    /// A new player's starting inventory: one steel pickaxe in the first hotbar slot.
    pub fn starting(registry: &ContentRegistry) -> Self {
        let mut inventory = Inventory::player();
        if let Some(pickaxe) = registry.item_id("steel_pickaxe") {
            inventory.insert(pickaxe, 1, registry);
        }
        Self { inventory, selected: 0, dirty: true }
    }

    /// The item in the selected hotbar slot.
    pub fn held_item(&self) -> Option<deepcore_shared::content::registry::ItemId> {
        self.inventory.slot(self.selected as usize).and_then(|slot| slot.item)
    }

    /// The block the held item places, if any.
    pub fn held_block(
        &self,
        registry: &ContentRegistry,
    ) -> Option<deepcore_shared::voxel::BlockId> {
        self.held_item().and_then(|item| registry.item(item)).and_then(|entry| entry.places)
    }
}

/// Sends an inventory snapshot to its owner whenever it changes.
pub fn sync_inventories(
    players: Query<(&PlayerInventory, &NetOwner)>,
    mut out: MessageWriter<ToClients<InventorySnapshot>>,
    sessions: Res<ClientSessions>,
) {
    for (inventory, owner) in &players {
        if !inventory.dirty {
            continue;
        }
        let Some(entity) = sessions.entities.get(&owner.client).copied() else {
            continue;
        };
        out.write(ToClients {
            targets: SendTargets::Single(ClientId::Client(entity)),
            message: InventorySnapshot {
                slots: inventory.inventory.slots().to_vec(),
                selected: inventory.selected,
            },
        });
    }
}

/// Clears the dirty flags after a sync pass.
pub fn clear_inventory_flags(mut players: Query<&mut PlayerInventory>) {
    for mut inventory in &mut players {
        inventory.dirty = false;
    }
}

/// Grants mined block drops straight into the miner's inventory.
///
/// Called from the mining system; if the inventory is full the drops are left in the world as a
/// pickup so nothing is ever silently destroyed.
pub fn grant_drops(
    registry: &ContentRegistry,
    inventory: &mut Inventory,
    block: deepcore_shared::voxel::BlockId,
    overflow: &mut Vec<Slot>,
) {
    let Some(entry) = registry.block(block) else {
        return;
    };
    for (item, count) in &entry.drops {
        let leftover = inventory.insert(*item, *count, registry);
        if leftover > 0 {
            overflow.push(Slot::new(*item, leftover));
        }
    }
}

/// A crate left in the world, holding a dead player's backpack.
#[derive(Component, Clone, Debug)]
pub struct DeathCrate {
    /// Block position the crate sits on.
    pub position: BlockPos,
    /// What it holds.
    pub contents: Vec<Slot>,
}

/// A request to drop an item stack into the world.
#[derive(Message, Clone, Debug)]
pub struct SpawnPickup {
    /// Where the block was.
    pub position: BlockPos,
    /// What to drop.
    pub slot: Slot,
}

/// Turns spawn requests into world pickups.
pub fn spawn_pickups(mut requests: MessageReader<SpawnPickup>, mut commands: Commands) {
    for request in requests.read() {
        commands.spawn((
            ItemPickup {
                position: request.position.center_f32(),
                slot: request.slot,
                lifetime: 300.0,
            },
            Transform::from_translation(request.position.center_f32()),
        ));
    }
}

/// Ages pickups and removes expired ones.
pub fn age_pickups(
    time: Res<Time>,
    mut commands: Commands,
    mut pickups: Query<(Entity, &mut ItemPickup)>,
) {
    for (entity, mut pickup) in &mut pickups {
        pickup.lifetime -= time.delta_secs();
        if pickup.lifetime <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// Lets a player empty a death crate by walking into it, restoring their backpack.
pub fn collect_death_crates(
    mut commands: Commands,
    mut players: Query<(&mut PlayerInventory, &NetPlayer)>,
    mut crates: Query<(Entity, &mut DeathCrate)>,
    registry: Res<ContentRegistry>,
) {
    for (mut inventory, net) in &mut players {
        let position = net.feet();
        for (entity, mut crate_) in &mut crates {
            let crate_position = crate_.position.center_f32();
            if position.distance(crate_position) > 2.0 {
                continue;
            }
            let leftovers = inventory.inventory.restore(&crate_.contents, &registry);
            crate_.contents = leftovers;
            inventory.dirty = true;
            if crate_.contents.is_empty() {
                commands.entity(entity).despawn();
            }
        }
    }
}

/// Lets players pick up dropped stacks by walking over them.
pub fn collect_pickups(
    mut commands: Commands,
    mut players: Query<(&mut PlayerInventory, &NetPlayer)>,
    mut pickups: Query<(Entity, &mut ItemPickup)>,
    registry: Res<ContentRegistry>,
) {
    for (mut inventory, net) in &mut players {
        let position = net.feet();
        for (entity, mut pickup) in &mut pickups {
            if position.distance(pickup.position) > 1.8 {
                continue;
            }
            let Some(item) = pickup.slot.item else {
                commands.entity(entity).despawn();
                continue;
            };
            let leftover = inventory.inventory.insert(item, pickup.slot.count, &registry);
            if leftover == 0 {
                commands.entity(entity).despawn();
                inventory.dirty = true;
            } else if leftover < pickup.slot.count {
                pickup.slot.count = leftover;
                inventory.dirty = true;
            }
        }
    }
}

/// A dropped item stack waiting to be picked up.
#[derive(Component, Clone, Debug)]
pub struct ItemPickup {
    /// Where it is.
    pub position: Vec3,
    /// What it is.
    pub slot: Slot,
    /// Seconds until it despawns.
    pub lifetime: f32,
}

/// Drops a player's backpack on death and clears it.
///
/// The hotbar is kept, matching the brief: "Death drops the backpack, not the hotbar".
pub fn drop_backpack_on_death(
    mut commands: Commands,
    mut players: Query<(&mut PlayerInventory, &NetPlayer)>,
    world: Res<VoxelWorld>,
) {
    for (mut inventory, net) in &mut players {
        if net.position[1] > 0.0 {
            continue;
        }
        // The player has fallen out of the world (or died): drop slots 9..45.
        let mut contents = Vec::new();
        for index in deepcore_shared::inventory::HOTBAR_SLOTS..inventory.inventory.len() {
            let slot = *inventory.inventory.slot(index).unwrap_or(&Slot::EMPTY);
            if !slot.is_empty() {
                contents.push(slot);
                inventory.inventory.take_from_slot(index, u32::MAX);
            }
        }
        if contents.is_empty() {
            continue;
        }
        let position = BlockPos::new(
            net.position[0].floor() as i32,
            net.position[1].floor() as i32,
            net.position[2].floor() as i32,
        );
        commands.spawn((DeathCrate { position, contents }, Replicated));
        inventory.dirty = true;
        let _ = &world;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepcore_shared::content::load_registry;

    #[test]
    fn starting_inventory_holds_a_steel_pickaxe_in_the_first_slot() {
        let registry = ContentRegistry(load_registry().expect("content"));
        let inventory = PlayerInventory::starting(&registry);
        let pickaxe = registry.item_id("steel_pickaxe").expect("pickaxe");
        assert_eq!(inventory.inventory.count_of(pickaxe), 1);
        assert_eq!(inventory.held_item(), Some(pickaxe));
        assert_eq!(inventory.selected, 0);
    }

    #[test]
    fn mined_drops_go_into_the_inventory() {
        let registry = ContentRegistry(load_registry().expect("content"));
        let mut inventory = Inventory::player();
        let coal_seam = registry.block_id("coal_seam").expect("coal");
        let coal = registry.item_id("coal").expect("coal");
        let mut overflow = Vec::new();
        grant_drops(&registry, &mut inventory, coal_seam, &mut overflow);
        assert!(overflow.is_empty());
        assert_eq!(inventory.count_of(coal), 2);
    }

    #[test]
    fn mined_drops_overflow_when_the_inventory_is_full() {
        let registry = ContentRegistry(load_registry().expect("content"));
        let mut inventory = Inventory::new(1);
        let coal_seam = registry.block_id("coal_seam").expect("coal");
        let coal = registry.item_id("coal").expect("coal");
        // Fill the only slot with something else.
        let stone = registry.item_id("stone_cobble").expect("stone");
        inventory.slots_mut()[0] = Slot::new(stone, 64);
        let mut overflow = Vec::new();
        grant_drops(&registry, &mut inventory, coal_seam, &mut overflow);
        assert_eq!(overflow, vec![Slot::new(coal, 2)]);
    }

    #[test]
    fn held_block_resolves_through_the_registry() {
        let registry = ContentRegistry(load_registry().expect("content"));
        let mut inventory = PlayerInventory::starting(&registry);
        assert_eq!(inventory.held_block(&registry), None, "a pickaxe places nothing");
        let ladder = registry.item_id("ladder").expect("ladder");
        inventory.inventory.slots_mut()[0] = Slot::new(ladder, 4);
        let expected = registry.block_id("ladder").expect("ladder block");
        assert_eq!(inventory.held_block(&registry), Some(expected));
    }
}
