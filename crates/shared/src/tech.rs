//! The team-shared tech tree state and research deposits.

use serde::{Deserialize, Serialize};

use crate::content::registry::{ItemId, RecipeId, Registry, TechId};
use crate::inventory::Inventory;

/// The team's research progress. One per world, shared by every player.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TechState {
    /// Nodes that have been researched.
    pub unlocked: Vec<bool>,
    /// Items delivered towards each node, keyed by node index.
    pub deposits: Vec<Vec<(ItemId, u32)>>,
}

impl TechState {
    /// A fresh tree with `count` locked nodes.
    pub fn new(registry: &Registry) -> Self {
        let count = registry.tech_count();
        Self { unlocked: vec![false; count], deposits: vec![Vec::new(); count] }
    }

    /// True if a node is unlocked.
    pub fn is_unlocked(&self, node: TechId) -> bool {
        self.unlocked.get(node.index()).copied().unwrap_or(false)
    }

    /// True if every parent of a node is unlocked (so it can be researched now).
    pub fn is_available(&self, registry: &Registry, node: TechId) -> bool {
        let Some(entry) = registry.tech(node) else {
            return false;
        };
        !self.is_unlocked(node) && entry.parents.iter().all(|parent| self.is_unlocked(*parent))
    }

    /// Marks a node as unlocked. Returns true if it changed.
    pub fn unlock(&mut self, node: TechId) -> bool {
        match self.unlocked.get_mut(node.index()) {
            Some(slot) if !*slot => {
                *slot = true;
                true
            }
            _ => false,
        }
    }

    /// The recipe ids currently unlocked: everything from Era-0 nodes that are always
    /// available, plus anything a researched node unlocks.
    pub fn unlocked_recipes(&self, registry: &Registry) -> Vec<RecipeId> {
        let mut unlocked = Vec::new();
        for (index, node) in registry.tech_nodes().iter().enumerate() {
            let node_id = TechId(index as u16);
            if self.is_unlocked(node_id) {
                unlocked.extend(node.unlock_recipes.iter().copied());
            }
        }
        unlocked
    }

    /// Recipes that need no research at all.
    pub fn base_recipes(registry: &Registry) -> Vec<RecipeId> {
        registry
            .recipes()
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.tech.is_none())
            .map(|(index, _)| RecipeId(index as u16))
            .collect()
    }

    /// All recipes the team can currently craft, including the unresearched-free ones.
    pub fn available_recipes(&self, registry: &Registry) -> Vec<RecipeId> {
        let mut recipes = Self::base_recipes(registry);
        recipes.extend(self.unlocked_recipes(registry));
        recipes.sort_by_key(|recipe| recipe.0);
        recipes.dedup();
        recipes
    }

    /// Items delivered so far towards a node.
    pub fn delivered(&self, node: TechId, item: ItemId) -> u32 {
        self.deposits
            .get(node.index())
            .map(|deposits| {
                deposits.iter().filter(|(id, _)| *id == item).map(|(_, count)| *count).sum()
            })
            .unwrap_or(0)
    }

    /// Delivers items towards a node, consuming them from the inventory.
    ///
    /// Returns how many were accepted (0 if the node is already unlocked or not available).
    pub fn deposit(&mut self, registry: &Registry, node: TechId, inventory: &mut Inventory) -> u32 {
        if !self.is_available(registry, node) {
            return 0;
        }
        let Some(entry) = registry.tech(node) else {
            return 0;
        };
        let costs = entry.cost.clone();
        let mut accepted = 0;
        for (item, needed) in costs {
            let already = self.delivered(node, item);
            let remaining = needed.saturating_sub(already);
            if remaining == 0 {
                continue;
            }
            let given = inventory.remove(item, remaining);
            if given == 0 {
                continue;
            }
            accepted += given;
            if let Some(deposits) = self.deposits.get_mut(node.index()) {
                match deposits.iter_mut().find(|(id, _)| *id == item) {
                    Some((_, count)) => *count += given,
                    None => deposits.push((item, given)),
                }
            }
        }
        accepted
    }

    /// True if every cost of a node has been delivered.
    pub fn is_fully_deposited(&self, registry: &Registry, node: TechId) -> bool {
        let Some(entry) = registry.tech(node) else {
            return false;
        };
        entry.cost.iter().all(|(item, needed)| self.delivered(node, *item) >= *needed)
    }

    /// Tries to complete research on a node: if every cost is delivered, unlocks it.
    ///
    /// Returns true if the node became unlocked.
    pub fn try_complete(&mut self, registry: &Registry, node: TechId) -> bool {
        if !self.is_available(registry, node) || !self.is_fully_deposited(registry, node) {
            return false;
        }
        self.unlock(node)
    }

    /// Number of nodes unlocked.
    pub fn unlocked_count(&self) -> usize {
        self.unlocked.iter().filter(|unlocked| **unlocked).count()
    }

    /// The highest Era the team has reached (the highest Era of any unlocked node).
    pub fn era(&self, registry: &Registry) -> u8 {
        registry
            .tech_nodes()
            .iter()
            .enumerate()
            .filter(|(index, _)| self.unlocked.get(*index).copied().unwrap_or(false))
            .map(|(_, node)| node.def.era)
            .max()
            .unwrap_or(0)
    }

    /// The next researchable nodes, for the objective list.
    pub fn available_nodes(&self, registry: &Registry) -> Vec<TechId> {
        registry
            .tech_nodes()
            .iter()
            .enumerate()
            .filter(|(index, _)| self.is_available(registry, TechId(*index as u16)))
            .map(|(index, _)| TechId(index as u16))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::load_registry;

    fn registry() -> Registry {
        load_registry().expect("content")
    }

    #[test]
    fn fresh_tree_has_only_era_zero_available() {
        let registry = registry();
        let state = TechState::new(&registry);
        let available = state.available_nodes(&registry);
        assert!(!available.is_empty(), "the root node should be available");
        for node in available {
            let entry = registry.tech(node).expect("node");
            assert!(entry.parents.is_empty(), "only parentless nodes are available at the start");
        }
    }

    #[test]
    fn unlocking_a_node_opens_its_children() {
        let registry = registry();
        let mut state = TechState::new(&registry);
        let root = registry.tech_id("hand_tools").expect("root");
        assert!(state.unlock(root));
        let smelting = registry.tech_id("smelting").expect("smelting");
        assert!(state.is_available(&registry, smelting));
    }

    #[test]
    fn deposit_consumes_items_and_completes_research() {
        let registry = registry();
        let mut state = TechState::new(&registry);
        let root = registry.tech_id("hand_tools").expect("root");
        state.unlock(root);
        let smelting = registry.tech_id("smelting").expect("smelting");
        let entry = registry.tech(smelting).expect("node");
        let costs = entry.cost.clone();

        let mut inventory = Inventory::player();
        for (item, count) in &costs {
            inventory.insert(*item, *count, &registry);
        }
        let accepted = state.deposit(&registry, smelting, &mut inventory);
        assert!(accepted > 0);
        assert!(state.is_fully_deposited(&registry, smelting));
        assert!(state.try_complete(&registry, smelting));
        assert!(state.is_unlocked(smelting));
        // The inventory was emptied of the deposited items.
        for (item, _) in &costs {
            assert_eq!(inventory.count_of(*item), 0);
        }
    }

    #[test]
    fn cannot_deposit_into_a_locked_parent_chain() {
        let registry = registry();
        let mut state = TechState::new(&registry);
        let steam = registry.tech_id("steam_power").expect("steam");
        let mut inventory = Inventory::player();
        assert_eq!(state.deposit(&registry, steam, &mut inventory), 0);
    }

    #[test]
    fn recipes_unlock_with_their_nodes() {
        let registry = registry();
        let mut state = TechState::new(&registry);
        let before = state.available_recipes(&registry);
        // Pick a recipe that is gated behind research and prove it is locked until then.
        let gated = registry
            .recipes()
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.tech.is_some())
            .map(|(index, _)| RecipeId(index as u16))
            .expect("content has research-gated recipes");
        assert!(!before.contains(&gated), "a gated recipe must start locked");

        let root = registry.tech_id("hand_tools").expect("root");
        state.unlock(root);
        let smelting = registry.tech_id("smelting").expect("smelting");
        state.unlock(smelting);
        let after = state.available_recipes(&registry);
        assert!(after.len() > before.len(), "research should unlock more recipes");
    }

    #[test]
    fn era_tracks_progress() {
        let registry = registry();
        let mut state = TechState::new(&registry);
        assert_eq!(state.era(&registry), 0);
        let root = registry.tech_id("hand_tools").expect("root");
        state.unlock(root);
        assert_eq!(state.era(&registry), 0);
        let smelting = registry.tech_id("smelting").expect("smelting");
        state.unlock(smelting);
        assert_eq!(state.era(&registry), 1);
    }
}
