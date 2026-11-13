//! Inventories: stacks, slots and transfer rules.
//!
//! Every inventory is a flat array of [`Slot`]s. The player has a 9-slot hotbar and a 36-slot
//! backpack (45 slots total, hotbar first), and machines and crates have their own. All the
//! rules for moving items live here so the server and the client UI agree about what a drag,
//! a shift-click or a split does.

use serde::{Deserialize, Serialize};

use crate::content::registry::{ItemId, Registry};

/// Number of hotbar slots.
pub const HOTBAR_SLOTS: usize = 9;
/// Number of backpack slots.
pub const BACKPACK_SLOTS: usize = 36;
/// Total player slots (hotbar first, then backpack).
pub const PLAYER_SLOTS: usize = HOTBAR_SLOTS + BACKPACK_SLOTS;

/// One slot in an inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    /// The item in the slot, if any.
    pub item: Option<ItemId>,
    /// How many of it.
    pub count: u32,
}

impl Slot {
    /// An empty slot.
    pub const EMPTY: Slot = Slot { item: None, count: 0 };

    /// A slot holding `count` of an item.
    pub fn new(item: ItemId, count: u32) -> Self {
        if count == 0 { Self::EMPTY } else { Self { item: Some(item), count } }
    }

    /// True if the slot is empty.
    pub fn is_empty(&self) -> bool {
        self.item.is_none() || self.count == 0
    }

    /// The number of free units this slot can still take of `item`, given its stack size.
    pub fn room_for(&self, item: ItemId, stack_size: u32) -> u32 {
        match self.item {
            None => stack_size,
            Some(current) if current == item => stack_size.saturating_sub(self.count),
            _ => 0,
        }
    }
}

/// A fixed-size array of slots.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    slots: Vec<Slot>,
}

impl Inventory {
    /// An empty inventory with `size` slots.
    pub fn new(size: usize) -> Self {
        Self { slots: vec![Slot::EMPTY; size] }
    }

    /// The player's inventory: hotbar + backpack.
    pub fn player() -> Self {
        Self::new(PLAYER_SLOTS)
    }

    /// Number of slots.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// True if the inventory has no slots at all.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Read a slot.
    pub fn slot(&self, index: usize) -> Option<&Slot> {
        self.slots.get(index)
    }

    /// All slots.
    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    /// Mutably read all slots (the UI needs this for drag and drop).
    pub fn slots_mut(&mut self) -> &mut [Slot] {
        &mut self.slots
    }

    /// Replaces all slots (the client applying a server snapshot).
    pub fn set_slots(&mut self, slots: &[Slot]) {
        self.slots.clear();
        self.slots.extend_from_slice(slots);
    }

    /// True if every slot is empty.
    pub fn is_fully_empty(&self) -> bool {
        self.slots.iter().all(Slot::is_empty)
    }

    /// How many of an item the inventory holds.
    pub fn count_of(&self, item: ItemId) -> u32 {
        self.slots.iter().filter(|slot| slot.item == Some(item)).map(|slot| slot.count).sum()
    }

    /// The first slot holding `item`.
    pub fn find(&self, item: ItemId) -> Option<usize> {
        self.slots.iter().position(|slot| slot.item == Some(item) && slot.count > 0)
    }

    /// Removes up to `count` of an item, returning how many were actually removed.
    pub fn remove(&mut self, item: ItemId, count: u32) -> u32 {
        let mut remaining = count;
        for slot in &mut self.slots {
            if remaining == 0 {
                break;
            }
            if slot.item == Some(item) && slot.count > 0 {
                let taken = slot.count.min(remaining);
                slot.count -= taken;
                remaining -= taken;
                if slot.count == 0 {
                    *slot = Slot::EMPTY;
                }
            }
        }
        count - remaining
    }

    /// Removes `count` from a specific slot.
    ///
    /// Returns how many were removed.
    pub fn take_from_slot(&mut self, index: usize, count: u32) -> u32 {
        let Some(slot) = self.slots.get_mut(index) else {
            return 0;
        };
        let taken = slot.count.min(count);
        slot.count -= taken;
        if slot.count == 0 {
            *slot = Slot::EMPTY;
        }
        taken
    }

    /// Inserts items, filling partial stacks first and then empty slots.
    ///
    /// Returns the number of items that did **not** fit.
    pub fn insert(&mut self, item: ItemId, count: u32, registry: &Registry) -> u32 {
        let stack_size = stack_size(registry, item);
        let mut remaining = count;

        // First pass: top up existing stacks.
        for slot in &mut self.slots {
            if remaining == 0 {
                break;
            }
            if slot.item == Some(item) && slot.count < stack_size {
                let space = stack_size - slot.count;
                let moved = space.min(remaining);
                slot.count += moved;
                remaining -= moved;
            }
        }
        // Second pass: fill empty slots.
        for slot in &mut self.slots {
            if remaining == 0 {
                break;
            }
            if slot.is_empty() {
                let moved = stack_size.min(remaining);
                *slot = Slot::new(item, moved);
                remaining -= moved;
            }
        }
        remaining
    }

    /// True if the whole stack would fit.
    pub fn can_fit(&self, item: ItemId, count: u32, registry: &Registry) -> bool {
        let stack_size = stack_size(registry, item);
        let mut space = 0u32;
        for slot in &self.slots {
            space = space.saturating_add(slot.room_for(item, stack_size));
            if space >= count {
                return true;
            }
        }
        space >= count
    }

    /// Moves up to `count` items from one slot to another, merging stacks when possible.
    ///
    /// Returns how many items moved.
    pub fn move_between(&mut self, from: usize, to: usize, count: u32, registry: &Registry) -> u32 {
        if from == to || from >= self.slots.len() || to >= self.slots.len() {
            return 0;
        }
        let Some(source) = self.slots.get(from).copied() else {
            return 0;
        };
        if source.is_empty() {
            return 0;
        }
        let item = source.item.unwrap_or(ItemId(0));
        let stack_size = stack_size(registry, item);

        let target = self.slots[to];
        let movable = if target.is_empty() {
            source.count.min(count)
        } else if target.item == source.item {
            let room = stack_size.saturating_sub(target.count);
            room.min(source.count).min(count)
        } else {
            0
        };
        if movable == 0 {
            return 0;
        }

        // Apply the move.
        let target_slot = &mut self.slots[to];
        match target_slot.item {
            Some(existing) if existing == item => target_slot.count += movable,
            _ => *target_slot = Slot::new(item, movable),
        }
        let source_slot = &mut self.slots[from];
        source_slot.count -= movable;
        if source_slot.count == 0 {
            *source_slot = Slot::EMPTY;
        }
        movable
    }

    /// Splits a stack in half: the second half goes to the first empty slot.
    ///
    /// Returns the index the new stack landed in.
    pub fn split(&mut self, index: usize) -> Option<usize> {
        let slot = *self.slots.get(index)?;
        if slot.is_empty() || slot.count < 2 {
            return None;
        }
        let target = self.slots.iter().position(Slot::is_empty)?;
        let half = slot.count / 2;
        self.slots[index].count -= half;
        self.slots[target] = Slot::new(slot.item.unwrap_or(ItemId(0)), half);
        Some(target)
    }

    /// Quick-moves a slot to the other half of the player inventory (hotbar <-> backpack),
    /// which is what shift-click does.
    pub fn quick_move(&mut self, index: usize, registry: &Registry) -> u32 {
        if index >= self.slots.len() {
            return 0;
        }
        let slot = self.slots[index];
        if slot.is_empty() {
            return 0;
        }
        let item = slot.item.unwrap_or(ItemId(0));
        let stack_size = stack_size(registry, item);

        // Pick the destination range.
        let (start, end) =
            if index < HOTBAR_SLOTS { (HOTBAR_SLOTS, self.slots.len()) } else { (0, HOTBAR_SLOTS) };

        let mut remaining = slot.count;
        // Merge into matching stacks first.
        for target in start..end {
            if remaining == 0 {
                break;
            }
            let destination = self.slots[target];
            if destination.item == Some(item) && destination.count < stack_size {
                let space = stack_size - destination.count;
                let moved = space.min(remaining);
                self.slots[target].count += moved;
                remaining -= moved;
            }
        }
        // Then into empty slots.
        for target in start..end {
            if remaining == 0 {
                break;
            }
            if self.slots[target].is_empty() {
                let moved = stack_size.min(remaining);
                self.slots[target] = Slot::new(item, moved);
                remaining -= moved;
            }
        }

        let moved = slot.count - remaining;
        self.slots[index].count -= moved;
        if self.slots[index].count == 0 {
            self.slots[index] = Slot::EMPTY;
        }
        moved
    }

    /// Empties the inventory and returns everything it held, for death drops.
    pub fn drain_all(&mut self) -> Vec<Slot> {
        let drained: Vec<Slot> = self.slots.iter().copied().filter(|s| !s.is_empty()).collect();
        self.slots.iter_mut().for_each(|slot| *slot = Slot::EMPTY);
        drained
    }

    /// Puts a list of stacks back, returning whatever did not fit.
    pub fn restore(&mut self, stacks: &[Slot], registry: &Registry) -> Vec<Slot> {
        let mut leftovers = Vec::new();
        for stack in stacks {
            let Some(item) = stack.item else {
                continue;
            };
            let left = self.insert(item, stack.count, registry);
            if left > 0 {
                leftovers.push(Slot::new(item, left));
            }
        }
        leftovers
    }
}

/// The stack size of an item, defaulting to 64 for unknown ids.
pub fn stack_size(registry: &Registry, item: ItemId) -> u32 {
    registry.item(item).map(|entry| entry.def.stack).unwrap_or(64).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::load_registry;

    fn registry() -> Registry {
        load_registry().expect("content")
    }

    fn item(registry: &Registry, name: &str) -> ItemId {
        registry.item_id(name).unwrap_or_else(|| panic!("{name} exists"))
    }

    #[test]
    fn insert_fills_partial_stacks_then_empty_slots() {
        let registry = registry();
        let coal = item(&registry, "coal");
        let mut inventory = Inventory::new(3);
        inventory.slots_mut()[0] = Slot::new(coal, 60);

        // 10 more: 4 top up the first stack, 6 open a new one.
        assert_eq!(inventory.insert(coal, 10, &registry), 0);
        assert_eq!(inventory.slot(0).map(|s| s.count), Some(64));
        assert_eq!(inventory.slot(1).map(|s| s.count), Some(6));
        assert_eq!(inventory.count_of(coal), 70);
    }

    #[test]
    fn insert_reports_overflow() {
        let registry = registry();
        let coal = item(&registry, "coal");
        let mut inventory = Inventory::new(1);
        // A one-slot inventory holds at most one stack of 64.
        assert_eq!(inventory.insert(coal, 100, &registry), 36);
        assert_eq!(inventory.count_of(coal), 64);
    }

    #[test]
    fn tools_do_not_stack() {
        let registry = registry();
        let pickaxe = item(&registry, "steel_pickaxe");
        let mut inventory = Inventory::new(4);
        // Stack size is 1, so two pickaxes occupy two slots and nothing is lost.
        assert_eq!(inventory.insert(pickaxe, 2, &registry), 0);
        assert_eq!(inventory.count_of(pickaxe), 2);
        assert_eq!(inventory.slot(0).map(|s| s.count), Some(1));
        assert_eq!(inventory.slot(1).map(|s| s.count), Some(1));
        // A one-slot inventory can hold only one.
        let mut single = Inventory::new(1);
        assert_eq!(single.insert(pickaxe, 2, &registry), 1);
    }

    #[test]
    fn move_between_merges_and_swaps_correctly() {
        let registry = registry();
        let coal = item(&registry, "coal");
        let iron = item(&registry, "iron_ingot");
        let mut inventory = Inventory::new(3);
        inventory.slots_mut()[0] = Slot::new(coal, 32);
        inventory.slots_mut()[1] = Slot::new(coal, 48);

        // Merging respects the stack limit: 16 fit into slot 1.
        assert_eq!(inventory.move_between(0, 1, 32, &registry), 16);
        assert_eq!(inventory.slot(0).map(|s| s.count), Some(16));
        assert_eq!(inventory.slot(1).map(|s| s.count), Some(64));

        // Different items do not merge.
        inventory.slots_mut()[2] = Slot::new(iron, 5);
        assert_eq!(inventory.move_between(0, 2, 16, &registry), 0);
    }

    #[test]
    fn split_halves_a_stack_into_an_empty_slot() {
        let registry = registry();
        let coal = item(&registry, "coal");
        let mut inventory = Inventory::new(3);
        inventory.slots_mut()[0] = Slot::new(coal, 32);
        let target = inventory.split(0).expect("split works");
        assert_eq!(target, 1);
        assert_eq!(inventory.slot(0).map(|s| s.count), Some(16));
        assert_eq!(inventory.slot(1).map(|s| s.count), Some(16));
        assert_eq!(inventory.count_of(coal), 32);
    }

    #[test]
    fn quick_move_moves_hotbar_to_backpack_and_back() {
        let registry = registry();
        let coal = item(&registry, "coal");
        let mut inventory = Inventory::player();
        inventory.slots_mut()[0] = Slot::new(coal, 30);
        assert_eq!(inventory.quick_move(0, &registry), 30);
        assert!(inventory.slot(0).expect("slot").is_empty());
        assert_eq!(inventory.count_of(coal), 30);
        // It landed in the backpack range.
        let landed = inventory.find(coal).expect("item moved");
        assert!(landed >= HOTBAR_SLOTS);
        // And back again.
        assert_eq!(inventory.quick_move(landed, &registry), 30);
        assert_eq!(inventory.find(coal), Some(0));
    }

    #[test]
    fn remove_spans_multiple_stacks() {
        let registry = registry();
        let coal = item(&registry, "coal");
        let mut inventory = Inventory::new(3);
        inventory.slots_mut()[0] = Slot::new(coal, 10);
        inventory.slots_mut()[2] = Slot::new(coal, 10);
        assert_eq!(inventory.remove(coal, 15), 15);
        assert_eq!(inventory.count_of(coal), 5);
        assert!(inventory.slot(0).expect("slot").is_empty());
    }

    #[test]
    fn can_fit_accounts_for_partial_stacks() {
        let registry = registry();
        let coal = item(&registry, "coal");
        let mut inventory = Inventory::new(2);
        inventory.slots_mut()[0] = Slot::new(coal, 60);
        assert!(inventory.can_fit(coal, 68, &registry));
        assert!(!inventory.can_fit(coal, 69, &registry));
    }

    #[test]
    fn drain_and_restore_round_trips() {
        let registry = registry();
        let coal = item(&registry, "coal");
        let iron = item(&registry, "iron_ingot");
        let mut inventory = Inventory::player();
        inventory.insert(coal, 100, &registry);
        inventory.insert(iron, 20, &registry);

        let dropped = inventory.drain_all();
        assert!(inventory.is_fully_empty());
        let leftovers = inventory.restore(&dropped, &registry);
        assert!(leftovers.is_empty());
        assert_eq!(inventory.count_of(coal), 100);
        assert_eq!(inventory.count_of(iron), 20);
    }

    #[test]
    fn inventory_round_trips_through_postcard() {
        let registry = registry();
        let coal = item(&registry, "coal");
        let mut inventory = Inventory::player();
        inventory.insert(coal, 42, &registry);
        let bytes = postcard::to_allocvec(&inventory).expect("encode");
        let back: Inventory = postcard::from_bytes(&bytes).expect("decode");
        assert_eq!(inventory, back);
    }
}
