//! Compact numeric registries resolved from the string ids in the content files.
//!
//! Gameplay code never compares strings: a block is a [`BlockId`] (`u16`), an item an
//! [`ItemId`], a recipe a [`RecipeId`]. The registries keep the string↔number mapping and
//! the lookup tables the simulation needs every tick, all built once at startup.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::model::*;
use crate::voxel::BlockId;

/// Numeric id of an item.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ItemId(pub u16);

impl ItemId {
    /// Index into registry arrays.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Numeric id of a recipe.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RecipeId(pub u16);

impl RecipeId {
    /// Index into registry arrays.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Numeric id of a tech node.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TechId(pub u16);

impl TechId {
    /// Index into registry arrays.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Numeric id of a machine definition.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MachineId(pub u16);

impl MachineId {
    /// Index into registry arrays.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Numeric id of a layer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LayerId(pub u8);

impl LayerId {
    /// Index into registry arrays.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A resolved block: the definition plus its numeric id and resolved drops.
#[derive(Clone, Debug)]
pub struct BlockEntry {
    /// The raw definition.
    pub def: BlockDef,
    /// Resolved drop table.
    pub drops: Vec<(ItemId, u32)>,
    /// Resolved machine id, if the block is a machine.
    pub machine: Option<MachineId>,
}

/// A resolved item.
#[derive(Clone, Debug)]
pub struct ItemEntry {
    /// The raw definition.
    pub def: ItemDef,
    /// The block this item places, if any.
    pub places: Option<BlockId>,
}

/// A resolved recipe.
#[derive(Clone, Debug)]
pub struct RecipeEntry {
    /// The raw definition.
    pub def: RecipeDef,
    /// Resolved machine id (`None` for hand/workbench crafting).
    pub machine: Option<MachineId>,
    /// Resolved inputs.
    pub inputs: Vec<(ItemId, u32)>,
    /// Resolved outputs.
    pub outputs: Vec<(ItemId, u32)>,
    /// Resolved tech node.
    pub tech: Option<TechId>,
    /// Resolved fuel.
    pub fuel: Vec<(ItemId, u32)>,
}

/// A resolved tech node.
#[derive(Clone, Debug)]
pub struct TechEntry {
    /// The raw definition.
    pub def: TechNodeDef,
    /// Resolved parents.
    pub parents: Vec<TechId>,
    /// Resolved delivery cost.
    pub cost: Vec<(ItemId, u32)>,
    /// Machine ids unlocked by this node.
    pub unlock_machines: Vec<MachineId>,
    /// Recipe ids unlocked by this node.
    pub unlock_recipes: Vec<RecipeId>,
}

/// A resolved layer.
#[derive(Clone, Debug)]
pub struct LayerEntry {
    /// The raw definition.
    pub def: LayerDef,
    /// Resolved base block.
    pub base_block: BlockId,
    /// Resolved filler blocks.
    pub filler_blocks: Vec<(BlockId, f32)>,
    /// Resolved ore rules.
    pub ores: Vec<ResolvedOre>,
}

/// An ore rule with its block id resolved.
#[derive(Clone, Debug)]
pub struct ResolvedOre {
    /// The ore block.
    pub block: BlockId,
    /// Spawn chance per chunk.
    pub frequency: f32,
    /// Vein size range.
    pub vein_size: (u32, u32),
    /// Depth band inside the layer.
    pub depth_band: Option<(i32, i32)>,
}

/// The complete, validated content registry.
#[derive(Default, Clone)]
pub struct Registry {
    blocks: Vec<BlockEntry>,
    items: Vec<ItemEntry>,
    recipes: Vec<RecipeEntry>,
    machines: Vec<MachineDef>,
    tech: Vec<TechEntry>,
    layers: Vec<LayerEntry>,
    world: WorldGenDef,

    block_by_name: HashMap<String, BlockId>,
    item_by_name: HashMap<String, ItemId>,
    recipe_by_name: HashMap<String, RecipeId>,
    machine_by_name: HashMap<String, MachineId>,
    tech_by_name: HashMap<String, TechId>,

    /// Recipes indexed by the machine that runs them (`None` = hand/workbench).
    recipes_by_machine: HashMap<Option<MachineId>, Vec<RecipeId>>,
}

impl Registry {
    /// Number of registered blocks.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
    /// Number of registered items.
    pub fn item_count(&self) -> usize {
        self.items.len()
    }
    /// Number of registered recipes.
    pub fn recipe_count(&self) -> usize {
        self.recipes.len()
    }
    /// Number of registered machines.
    pub fn machine_count(&self) -> usize {
        self.machines.len()
    }
    /// Number of registered tech nodes.
    pub fn tech_count(&self) -> usize {
        self.tech.len()
    }
    /// Number of registered layers.
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// World generation parameters.
    pub fn world(&self) -> &WorldGenDef {
        &self.world
    }

    /// Looks up a block id by name.
    pub fn block_id(&self, name: &str) -> Option<BlockId> {
        self.block_by_name.get(name).copied()
    }
    /// Looks up an item id by name.
    pub fn item_id(&self, name: &str) -> Option<ItemId> {
        self.item_by_name.get(name).copied()
    }
    /// Looks up a recipe id by name.
    pub fn recipe_id(&self, name: &str) -> Option<RecipeId> {
        self.recipe_by_name.get(name).copied()
    }
    /// Looks up a machine id by name.
    pub fn machine_id(&self, name: &str) -> Option<MachineId> {
        self.machine_by_name.get(name).copied()
    }
    /// Looks up a tech node id by name.
    pub fn tech_id(&self, name: &str) -> Option<TechId> {
        self.tech_by_name.get(name).copied()
    }

    /// The block entry for an id, or `None` if the id is unknown.
    pub fn block(&self, id: BlockId) -> Option<&BlockEntry> {
        self.blocks.get(id as usize)
    }
    /// The item entry for an id.
    pub fn item(&self, id: ItemId) -> Option<&ItemEntry> {
        self.items.get(id.index())
    }
    /// The recipe entry for an id.
    pub fn recipe(&self, id: RecipeId) -> Option<&RecipeEntry> {
        self.recipes.get(id.index())
    }
    /// The machine definition for an id.
    pub fn machine(&self, id: MachineId) -> Option<&MachineDef> {
        self.machines.get(id.index())
    }
    /// The tech node for an id.
    pub fn tech(&self, id: TechId) -> Option<&TechEntry> {
        self.tech.get(id.index())
    }
    /// The layer for an id.
    pub fn layer(&self, id: LayerId) -> Option<&LayerEntry> {
        self.layers.get(id.index())
    }

    /// All layers, ordered from the surface down.
    pub fn layers(&self) -> &[LayerEntry] {
        &self.layers
    }
    /// All tech nodes.
    pub fn tech_nodes(&self) -> &[TechEntry] {
        &self.tech
    }
    /// All recipes.
    pub fn recipes(&self) -> &[RecipeEntry] {
        &self.recipes
    }
    /// All items.
    pub fn items(&self) -> &[ItemEntry] {
        &self.items
    }
    /// All blocks.
    pub fn blocks(&self) -> &[BlockEntry] {
        &self.blocks
    }
    /// All machine definitions.
    pub fn machines(&self) -> &[MachineDef] {
        &self.machines
    }

    /// Item name, or `"<unknown>"` if the id is not registered.
    pub fn item_name(&self, id: ItemId) -> &str {
        self.item(id).map(|e| e.def.name.as_str()).unwrap_or("<unknown item>")
    }

    /// Block name, or `"<unknown>"` if the id is not registered.
    pub fn block_name(&self, id: BlockId) -> &str {
        self.block(id).map(|e| e.def.name.as_str()).unwrap_or("<unknown block>")
    }

    /// Machine name, or `"<unknown>"`.
    pub fn machine_name(&self, id: MachineId) -> &str {
        self.machine(id).map(|m| m.name.as_str()).unwrap_or("<unknown machine>")
    }

    /// Recipes that a machine can run.
    pub fn recipes_for_machine(&self, machine: MachineId) -> &[RecipeId] {
        self.recipes_by_machine.get(&Some(machine)).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Recipes craftable by hand or at the workbench.
    pub fn hand_recipes(&self) -> &[RecipeId] {
        self.recipes_by_machine.get(&None).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The layer containing a given absolute Y coordinate, if any.
    pub fn layer_at_y(&self, y: i32) -> Option<LayerId> {
        let surface = self.world.surface_y;
        let depth = surface - y;
        self.layers
            .iter()
            .position(|layer| depth >= layer.def.depth.1 && depth <= layer.def.depth.0)
            .map(|index| LayerId(index as u8))
    }

    /// Depth below the surface of an absolute Y coordinate, in blocks.
    pub fn depth_of(&self, y: i32) -> i32 {
        self.world.surface_y - y
    }

    /// The tool definition of an item, if it is a tool.
    pub fn tool(&self, id: ItemId) -> Option<&ToolDef> {
        match &self.item(id)?.def.category {
            ItemCategory::Tool(tool) => Some(tool),
            _ => None,
        }
    }

    /// The wearable definition of an item, if it is wearable.
    pub fn wearable(&self, id: ItemId) -> Option<&WearableDef> {
        match &self.item(id)?.def.category {
            ItemCategory::Wearable(wearable) => Some(wearable),
            _ => None,
        }
    }

    /// Builds a registry from validated content files.
    ///
    /// The caller must run [`super::validate::validate`] first; this function assumes
    /// every referenced name exists.
    pub(crate) fn build(files: &ContentFiles) -> Self {
        let mut registry = Registry { world: files.world.clone(), ..Default::default() };

        for def in &files.blocks {
            let id: BlockId = registry.blocks.len() as u16;
            registry.block_by_name.insert(def.id.clone(), id);
            registry.blocks.push(BlockEntry { def: def.clone(), drops: Vec::new(), machine: None });
        }

        for def in &files.items {
            let id = ItemId(registry.items.len() as u16);
            registry.item_by_name.insert(def.id.clone(), id);
            registry.items.push(ItemEntry { def: def.clone(), places: None });
        }

        for def in &files.machines {
            let id = MachineId(registry.machines.len() as u16);
            registry.machine_by_name.insert(def.id.clone(), id);
            registry.machines.push(def.clone());
        }

        for def in &files.tech {
            let id = TechId(registry.tech.len() as u16);
            registry.tech_by_name.insert(def.id.clone(), id);
            registry.tech.push(TechEntry {
                def: def.clone(),
                parents: Vec::new(),
                cost: Vec::new(),
                unlock_machines: Vec::new(),
                unlock_recipes: Vec::new(),
            });
        }

        for def in &files.recipes {
            let id = RecipeId(registry.recipes.len() as u16);
            registry.recipe_by_name.insert(def.id.clone(), id);
            registry.recipes.push(RecipeEntry {
                def: def.clone(),
                machine: None,
                inputs: Vec::new(),
                outputs: Vec::new(),
                tech: None,
                fuel: Vec::new(),
            });
        }

        for def in &files.layers {
            let id = LayerId(registry.layers.len() as u8);
            let _ = id;
            registry.layers.push(LayerEntry {
                def: def.clone(),
                base_block: 0,
                filler_blocks: Vec::new(),
                ores: Vec::new(),
            });
        }

        // Second pass: resolve every reference now that all names are known.
        let lookup = Lookup {
            blocks: &registry.block_by_name,
            items: &registry.item_by_name,
            machines: &registry.machine_by_name,
            tech: &registry.tech_by_name,
        };

        for entry in &mut registry.blocks {
            entry.drops = resolve_items(&lookup, &entry.def.drops);
            entry.machine =
                entry.def.machine.as_deref().and_then(|m| lookup.machines.get(m).copied());
        }

        for entry in &mut registry.items {
            entry.places = entry.def.places.as_deref().and_then(|b| lookup.blocks.get(b).copied());
        }

        for entry in &mut registry.recipes {
            entry.machine = match entry.def.machine.as_str() {
                "hand" | "workbench" => None,
                other => lookup.machines.get(other).copied(),
            };
            entry.inputs = resolve_items(&lookup, &entry.def.inputs);
            entry.outputs = resolve_items(&lookup, &entry.def.outputs);
            entry.tech = entry.def.tech.as_deref().and_then(|t| lookup.tech.get(t).copied());
            entry.fuel = resolve_items(&lookup, &entry.def.fuel);
        }

        for entry in &mut registry.tech {
            entry.parents =
                entry.def.parents.iter().filter_map(|p| lookup.tech.get(p).copied()).collect();
            entry.cost = resolve_items(&lookup, &entry.def.cost);
            for unlock in &entry.def.unlocks {
                if let Some(machine) = lookup.machines.get(unlock) {
                    entry.unlock_machines.push(*machine);
                }
                if let Some(recipe) = registry.recipe_by_name.get(unlock) {
                    entry.unlock_recipes.push(*recipe);
                }
            }
        }

        for entry in &mut registry.layers {
            entry.base_block = lookup.blocks.get(&entry.def.base_block).copied().unwrap_or(0);
            entry.filler_blocks = entry
                .def
                .filler_blocks
                .iter()
                .filter_map(|(name, share)| lookup.blocks.get(name).map(|id| (*id, *share)))
                .collect();
            entry.ores = entry
                .def
                .ores
                .iter()
                .filter_map(|ore| {
                    lookup.blocks.get(&ore.block).map(|id| ResolvedOre {
                        block: *id,
                        frequency: ore.frequency,
                        vein_size: ore.vein_size,
                        depth_band: ore.depth_band,
                    })
                })
                .collect();
        }

        // Index recipes by machine for the simulation's hot path.
        for (index, recipe) in registry.recipes.iter().enumerate() {
            registry
                .recipes_by_machine
                .entry(recipe.machine)
                .or_default()
                .push(RecipeId(index as u16));
        }

        registry
    }
}

struct Lookup<'a> {
    blocks: &'a HashMap<String, BlockId>,
    items: &'a HashMap<String, ItemId>,
    machines: &'a HashMap<String, MachineId>,
    tech: &'a HashMap<String, TechId>,
}

fn resolve_items(lookup: &Lookup<'_>, entries: &[(String, u32)]) -> Vec<(ItemId, u32)> {
    entries
        .iter()
        .filter_map(|(name, count)| lookup.items.get(name).map(|id| (*id, *count)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_resolves_names_to_ids() {
        let files = ContentFiles {
            blocks: vec![BlockDef {
                id: "stone".into(),
                name: "Stone".into(),
                hardness: 1.5,
                min_tier: 1,
                drops: vec![("stone_item".into(), 1)],
                color: (0.5, 0.5, 0.5),
                pattern: Pattern::Speckle,
                roughness: 0.9,
                emissive: 0.0,
                radiation: None,
                layer_hint: Some(2),
                machine: None,
                kind: BlockKind::Solid,
                breakable: true,
            }],
            items: vec![ItemDef {
                id: "stone_item".into(),
                name: "Stone".into(),
                stack: 64,
                category: ItemCategory::Block,
                places: Some("stone".into()),
                color: (0.5, 0.5, 0.5),
                description: String::new(),
            }],
            ..Default::default()
        };

        let registry = Registry::build(&files);
        let block = registry.block_id("stone").expect("block registered");
        let item = registry.item_id("stone_item").expect("item registered");
        assert_eq!(registry.block(block).expect("block").drops, vec![(item, 1)]);
        assert_eq!(registry.item(item).expect("item").places, Some(block));
    }
}
