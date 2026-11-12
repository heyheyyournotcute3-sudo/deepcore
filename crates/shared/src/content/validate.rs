//! Content validation.
//!
//! The brief is explicit: fail fast with a clear message rather than discovering a broken
//! reference three hours into a play session. Every check here produces a list of
//! human-readable problems; loading refuses to continue if the list is non-empty.

use std::collections::{HashMap, HashSet};

use super::model::*;

/// A single validation problem, pointing at the offending content entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// Which content file the problem is in.
    pub file: &'static str,
    /// Which entry inside that file.
    pub entry: String,
    /// What is wrong.
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} [{}]: {}", self.file, self.entry, self.message)
    }
}

/// Validates a whole content set. Returns every problem found, not just the first.
pub fn validate(files: &ContentFiles) -> Vec<Problem> {
    let mut problems = Vec::new();

    let block_names: HashSet<&str> = files.blocks.iter().map(|b| b.id.as_str()).collect();
    let item_names: HashSet<&str> = files.items.iter().map(|i| i.id.as_str()).collect();
    let machine_names: HashSet<&str> = files.machines.iter().map(|m| m.id.as_str()).collect();
    let tech_names: HashSet<&str> = files.tech.iter().map(|t| t.id.as_str()).collect();

    check_duplicates(&mut problems, "blocks.ron", files.blocks.iter().map(|b| &b.id));
    check_duplicates(&mut problems, "items.ron", files.items.iter().map(|i| &i.id));
    check_duplicates(&mut problems, "recipes.ron", files.recipes.iter().map(|r| &r.id));
    check_duplicates(&mut problems, "machines.ron", files.machines.iter().map(|m| &m.id));
    check_duplicates(&mut problems, "tech_tree.ron", files.tech.iter().map(|t| &t.id));
    check_duplicates(&mut problems, "layers.ron", files.layers.iter().map(|l| &l.id));

    // ---- blocks ----
    for block in &files.blocks {
        for (item, _) in &block.drops {
            if !item_names.contains(item.as_str()) {
                problems.push(Problem {
                    file: "blocks.ron",
                    entry: block.id.clone(),
                    message: format!("drop references unknown item `{item}`"),
                });
            }
        }
        if let Some(machine) = &block.machine
            && !machine_names.contains(machine.as_str())
        {
            problems.push(Problem {
                file: "blocks.ron",
                entry: block.id.clone(),
                message: format!("machine block references unknown machine `{machine}`"),
            });
        }
        if block.hardness < 0.0 {
            problems.push(Problem {
                file: "blocks.ron",
                entry: block.id.clone(),
                message: "hardness must not be negative".into(),
            });
        }
        if block.breakable && block.min_tier > 5 {
            problems.push(Problem {
                file: "blocks.ron",
                entry: block.id.clone(),
                message: "min_tier above 5 cannot be reached by any tool".into(),
            });
        }
        if let Some((radius, dose)) = block.radiation
            && (radius <= 0.0 || dose < 0.0)
        {
            problems.push(Problem {
                file: "blocks.ron",
                entry: block.id.clone(),
                message: "radiation radius must be positive and dose non-negative".into(),
            });
        }
    }

    // ---- items ----
    for item in &files.items {
        if let Some(block) = &item.places
            && !block_names.contains(block.as_str())
        {
            problems.push(Problem {
                file: "items.ron",
                entry: item.id.clone(),
                message: format!("places unknown block `{block}`"),
            });
        }
        match &item.category {
            ItemCategory::Tool(tool) => {
                if tool.tier == 0 {
                    problems.push(Problem {
                        file: "items.ron",
                        entry: item.id.clone(),
                        message: "tool tier must be at least 1".into(),
                    });
                }
                if tool.speed <= 0.0 {
                    problems.push(Problem {
                        file: "items.ron",
                        entry: item.id.clone(),
                        message: "tool speed must be positive".into(),
                    });
                }
                if item.stack != 1 {
                    problems.push(Problem {
                        file: "items.ron",
                        entry: item.id.clone(),
                        message: "tools must have stack size 1".into(),
                    });
                }
            }
            ItemCategory::Wearable(w) => {
                for (label, value) in
                    [("radiation_resist", w.radiation_resist), ("heat_resist", w.heat_resist)]
                {
                    if !(0.0..=1.0).contains(&value) {
                        problems.push(Problem {
                            file: "items.ron",
                            entry: item.id.clone(),
                            message: format!("{label} must be within 0..1"),
                        });
                    }
                }
            }
            ItemCategory::Container(c) if c.capacity == 0 => problems.push(Problem {
                file: "items.ron",
                entry: item.id.clone(),
                message: "container capacity must be positive".into(),
            }),
            _ => {}
        }
        if item.stack == 0 {
            problems.push(Problem {
                file: "items.ron",
                entry: item.id.clone(),
                message: "stack size must be at least 1".into(),
            });
        }
    }

    // ---- machines ----
    for machine in &files.machines {
        for (item, count) in &machine.build_cost {
            if !item_names.contains(item.as_str()) {
                problems.push(Problem {
                    file: "machines.ron",
                    entry: machine.id.clone(),
                    message: format!("build_cost references unknown item `{item}`"),
                });
            }
            if *count == 0 {
                problems.push(Problem {
                    file: "machines.ron",
                    entry: machine.id.clone(),
                    message: format!("build_cost for `{item}` must be at least 1"),
                });
            }
        }
        if let Some(tech) = &machine.tech
            && !tech_names.contains(tech.as_str())
        {
            problems.push(Problem {
                file: "machines.ron",
                entry: machine.id.clone(),
                message: format!("references unknown tech node `{tech}`"),
            });
        }
        if machine.size.0 == 0 || machine.size.1 == 0 || machine.size.2 == 0 {
            problems.push(Problem {
                file: "machines.ron",
                entry: machine.id.clone(),
                message: "machine size axes must be at least 1".into(),
            });
        }
        if let Some(cascade) = &machine.cascade
            && cascade.min > cascade.max
        {
            problems.push(Problem {
                file: "machines.ron",
                entry: machine.id.clone(),
                message: "cascade min cannot exceed max".into(),
            });
        }
    }

    // ---- recipes ----
    let mut machines_that_run_recipes: HashSet<&str> = HashSet::new();
    for recipe in &files.recipes {
        if recipe.machine != "hand" && recipe.machine != "workbench" {
            if !machine_names.contains(recipe.machine.as_str()) {
                problems.push(Problem {
                    file: "recipes.ron",
                    entry: recipe.id.clone(),
                    message: format!("no machine `{}` can run this recipe", recipe.machine),
                });
            } else {
                machines_that_run_recipes.insert(recipe.machine.as_str());
            }
        }
        if recipe.inputs.is_empty() {
            problems.push(Problem {
                file: "recipes.ron",
                entry: recipe.id.clone(),
                message: "recipe has no inputs".into(),
            });
        }
        if recipe.outputs.is_empty() {
            problems.push(Problem {
                file: "recipes.ron",
                entry: recipe.id.clone(),
                message: "recipe has no outputs".into(),
            });
        }
        if recipe.time <= 0.0 {
            problems.push(Problem {
                file: "recipes.ron",
                entry: recipe.id.clone(),
                message: "recipe time must be positive".into(),
            });
        }
        for (item, count) in recipe.inputs.iter().chain(recipe.outputs.iter()) {
            if !item_names.contains(item.as_str()) {
                problems.push(Problem {
                    file: "recipes.ron",
                    entry: recipe.id.clone(),
                    message: format!("references unknown item `{item}`"),
                });
            }
            if *count == 0 {
                problems.push(Problem {
                    file: "recipes.ron",
                    entry: recipe.id.clone(),
                    message: format!("count for `{item}` must be at least 1"),
                });
            }
        }
        for (item, _) in &recipe.fuel {
            if !item_names.contains(item.as_str()) {
                problems.push(Problem {
                    file: "recipes.ron",
                    entry: recipe.id.clone(),
                    message: format!("fuel references unknown item `{item}`"),
                });
            }
        }
        if let Some(tech) = &recipe.tech
            && !tech_names.contains(tech.as_str())
        {
            problems.push(Problem {
                file: "recipes.ron",
                entry: recipe.id.clone(),
                message: format!("references unknown tech node `{tech}`"),
            });
        }
    }

    // ---- tech tree ----
    let mut parent_counts: HashMap<&str, usize> = HashMap::new();
    for node in &files.tech {
        for parent in &node.parents {
            match tech_names.contains(parent.as_str()) {
                true => *parent_counts.entry(parent.as_str()).or_default() += 1,
                false => problems.push(Problem {
                    file: "tech_tree.ron",
                    entry: node.id.clone(),
                    message: format!("missing parent node `{parent}`"),
                }),
            }
        }
        for (item, count) in &node.cost {
            if !item_names.contains(item.as_str()) {
                problems.push(Problem {
                    file: "tech_tree.ron",
                    entry: node.id.clone(),
                    message: format!("cost references unknown item `{item}`"),
                });
            }
            if *count == 0 {
                problems.push(Problem {
                    file: "tech_tree.ron",
                    entry: node.id.clone(),
                    message: format!("cost for `{item}` must be at least 1"),
                });
            }
        }
        for unlock in &node.unlocks {
            if !machine_names.contains(unlock.as_str())
                && !files.recipes.iter().any(|r| &r.id == unlock)
            {
                problems.push(Problem {
                    file: "tech_tree.ron",
                    entry: node.id.clone(),
                    message: format!("unlocks unknown machine or recipe `{unlock}`"),
                });
            }
        }
        if node.era > 5 {
            problems.push(Problem {
                file: "tech_tree.ron",
                entry: node.id.clone(),
                message: "era must be within 0..5".into(),
            });
        }
    }
    problems.extend(find_tech_cycles(files));

    // Every machine that is buildable should be reachable from the tech tree.
    let unlocked_machines: HashSet<&str> =
        files.tech.iter().flat_map(|t| t.unlocks.iter().map(String::as_str)).collect();
    for machine in &files.machines {
        if machine.tech.is_some() && !unlocked_machines.contains(machine.id.as_str()) {
            problems.push(Problem {
                file: "machines.ron",
                entry: machine.id.clone(),
                message: "requires tech but no tech node unlocks it".into(),
            });
        }
    }

    // ---- layers ----
    let mut covered: Vec<(i32, i32, String)> = Vec::new();
    for layer in &files.layers {
        if !block_names.contains(layer.base_block.as_str()) {
            problems.push(Problem {
                file: "layers.ron",
                entry: layer.id.clone(),
                message: format!("base_block `{}` does not exist", layer.base_block),
            });
        }
        for (block, share) in &layer.filler_blocks {
            if !block_names.contains(block.as_str()) {
                problems.push(Problem {
                    file: "layers.ron",
                    entry: layer.id.clone(),
                    message: format!("filler block `{block}` does not exist"),
                });
            }
            if !(0.0..=1.0).contains(share) {
                problems.push(Problem {
                    file: "layers.ron",
                    entry: layer.id.clone(),
                    message: format!("filler share for `{block}` must be within 0..1"),
                });
            }
        }
        for ore in &layer.ores {
            if !block_names.contains(ore.block.as_str()) {
                problems.push(Problem {
                    file: "layers.ron",
                    entry: layer.id.clone(),
                    message: format!("ore block `{}` does not exist", ore.block),
                });
            }
            if !(0.0..=1.0).contains(&ore.frequency) {
                problems.push(Problem {
                    file: "layers.ron",
                    entry: layer.id.clone(),
                    message: format!("ore `{}` frequency must be within 0..1", ore.block),
                });
            }
            if ore.vein_size.0 == 0 || ore.vein_size.0 > ore.vein_size.1 {
                problems.push(Problem {
                    file: "layers.ron",
                    entry: layer.id.clone(),
                    message: format!("ore `{}` vein_size must be 1 <= min <= max", ore.block),
                });
            }
        }
        if layer.depth.0 < layer.depth.1 {
            problems.push(Problem {
                file: "layers.ron",
                entry: layer.id.clone(),
                message: "depth band must be written (top, bottom) with top >= bottom".into(),
            });
        }
        covered.push((layer.depth.1, layer.depth.0, layer.id.clone()));
    }

    // Layers must cover the underground without gaps or overlaps.
    covered.sort_by_key(|(bottom, _, _)| *bottom);
    let mut cursor = 0;
    for (bottom, top, id) in &covered {
        if *bottom > cursor {
            problems.push(Problem {
                file: "layers.ron",
                entry: id.clone(),
                message: format!("gap in layer coverage between depth {cursor} and {bottom}"),
            });
        }
        if *bottom < cursor {
            problems.push(Problem {
                file: "layers.ron",
                entry: id.clone(),
                message: format!("layer overlaps the previous layer at depth {bottom}"),
            });
        }
        cursor = (*top).max(cursor);
    }
    if files.world.surface_y - cursor > files.world.bedrock_thickness {
        problems.push(Problem {
            file: "layers.ron",
            entry: "<layers>".into(),
            message: format!("layers stop at depth {cursor} but the world is deeper"),
        });
    }

    // ---- every ore must have a use ----
    let used_items = used_items(files);
    for layer in &files.layers {
        for ore in &layer.ores {
            let Some(block) = files.blocks.iter().find(|b| b.id == ore.block) else {
                continue;
            };
            let mut used = false;
            for (drop, _) in &block.drops {
                if used_items.contains(drop.as_str()) {
                    used = true;
                }
            }
            if block.drops.is_empty() {
                problems.push(Problem {
                    file: "layers.ron",
                    entry: ore.block.clone(),
                    message: "ore block drops nothing, so it is a dead end".into(),
                });
            } else if !used {
                problems.push(Problem {
                    file: "layers.ron",
                    entry: ore.block.clone(),
                    message: "ore has no use in any recipe, machine, tool or tech node".into(),
                });
            }
        }
    }

    problems
}

fn check_duplicates<'a>(
    problems: &mut Vec<Problem>,
    file: &'static str,
    ids: impl Iterator<Item = &'a String>,
) {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id.as_str()) {
            problems.push(Problem { file, entry: id.clone(), message: "duplicate id".into() });
        }
    }
}

/// Detects cycles in the tech tree via depth-first search.
fn find_tech_cycles(files: &ContentFiles) -> Vec<Problem> {
    let index: HashMap<&str, usize> =
        files.tech.iter().enumerate().map(|(i, t)| (t.id.as_str(), i)).collect();
    let mut state = vec![0u8; files.tech.len()]; // 0 = unvisited, 1 = in stack, 2 = done
    let mut problems = Vec::new();

    fn visit(
        node: usize,
        files: &ContentFiles,
        index: &HashMap<&str, usize>,
        state: &mut Vec<u8>,
        problems: &mut Vec<Problem>,
    ) {
        if state[node] == 1 {
            problems.push(Problem {
                file: "tech_tree.ron",
                entry: files.tech[node].id.clone(),
                message: "tech tree contains a cycle".into(),
            });
            return;
        }
        if state[node] == 2 {
            return;
        }
        state[node] = 1;
        for parent in &files.tech[node].parents {
            if let Some(&parent_index) = index.get(parent.as_str()) {
                visit(parent_index, files, index, state, problems);
            }
        }
        state[node] = 2;
    }

    for node in 0..files.tech.len() {
        visit(node, files, &index, &mut state, &mut problems);
    }
    problems
}

/// Items that are consumed or produced by anything: recipes, machine build costs, tech
/// costs, and tool/container definitions. Used to prove no ore is a dead end.
fn used_items(files: &ContentFiles) -> HashSet<&str> {
    let mut used = HashSet::new();
    for recipe in &files.recipes {
        used.extend(recipe.inputs.iter().map(|(i, _)| i.as_str()));
        used.extend(recipe.outputs.iter().map(|(i, _)| i.as_str()));
        used.extend(recipe.fuel.iter().map(|(i, _)| i.as_str()));
    }
    for machine in &files.machines {
        used.extend(machine.build_cost.iter().map(|(i, _)| i.as_str()));
    }
    for node in &files.tech {
        used.extend(node.cost.iter().map(|(i, _)| i.as_str()));
    }
    // An item is also "used" if a block places it or if it is a tool / wearable / container
    // that the player can carry, since those are progression steps in themselves.
    for item in &files.items {
        if matches!(
            item.category,
            ItemCategory::Tool(_)
                | ItemCategory::Wearable(_)
                | ItemCategory::Container(_)
                | ItemCategory::Trophy
        ) {
            used.insert(item.id.as_str());
        }
    }
    used
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str) -> BlockDef {
        BlockDef {
            id: id.into(),
            name: id.into(),
            hardness: 1.0,
            min_tier: 1,
            drops: vec![(format!("{id}_item"), 1)],
            color: (0.5, 0.5, 0.5),
            pattern: Pattern::Flat,
            roughness: 0.8,
            emissive: 0.0,
            radiation: None,
            layer_hint: None,
            machine: None,
            kind: BlockKind::Solid,
            breakable: true,
        }
    }

    fn item(id: &str) -> ItemDef {
        ItemDef {
            id: id.into(),
            name: id.into(),
            stack: 64,
            category: ItemCategory::Material,
            places: None,
            color: (0.5, 0.5, 0.5),
            description: String::new(),
        }
    }

    fn minimal() -> ContentFiles {
        let mut files = ContentFiles {
            blocks: vec![block("stone"), block("ore")],
            items: vec![item("stone_item"), item("ore_item"), item("ingot")],
            recipes: vec![RecipeDef {
                id: "smelt".into(),
                machine: "hand".into(),
                time: 1.0,
                inputs: vec![("ore_item".into(), 1)],
                outputs: vec![("ingot".into(), 1)],
                tech: None,
                power_kw: None,
                fuel: vec![],
            }],
            layers: vec![LayerDef {
                id: "top".into(),
                name: "Top".into(),
                depth: (200, 0),
                base_block: "stone".into(),
                filler_blocks: vec![],
                ores: vec![OreRule {
                    block: "ore".into(),
                    frequency: 0.1,
                    vein_size: (1, 4),
                    depth_band: None,
                }],
                hazards: vec![],
                ambient_temp: 18.0,
                min_tier: 1,
                cave_density: 1.0,
            }],
            ..Default::default()
        };
        files.world.bedrock_thickness = 3;
        files.world.surface_y = 200;
        files
    }

    #[test]
    fn minimal_content_is_valid() {
        let problems = validate(&minimal());
        assert!(problems.is_empty(), "unexpected problems: {problems:?}");
    }

    #[test]
    fn unknown_item_reference_is_reported() {
        let mut files = minimal();
        files.recipes[0].inputs.push(("nope".into(), 1));
        let problems = validate(&files);
        assert!(problems.iter().any(|p| p.message.contains("unknown item `nope`")));
    }

    #[test]
    fn recipe_without_machine_is_reported() {
        let mut files = minimal();
        files.recipes[0].machine = "ghost_furnace".into();
        let problems = validate(&files);
        assert!(problems.iter().any(|p| p.message.contains("no machine `ghost_furnace`")));
    }

    #[test]
    fn missing_tech_parent_is_reported() {
        let mut files = minimal();
        files.tech.push(TechNodeDef {
            id: "steam".into(),
            name: "Steam".into(),
            era: 2,
            parents: vec!["does_not_exist".into()],
            cost: vec![],
            unlocks: vec![],
            description: String::new(),
        });
        let problems = validate(&files);
        assert!(problems.iter().any(|p| p.message.contains("missing parent node")));
    }

    #[test]
    fn tech_cycle_is_reported() {
        let mut files = minimal();
        for (id, parent) in [("a", "b"), ("b", "a")] {
            files.tech.push(TechNodeDef {
                id: id.into(),
                name: id.into(),
                era: 1,
                parents: vec![parent.into()],
                cost: vec![],
                unlocks: vec![],
                description: String::new(),
            });
        }
        let problems = validate(&files);
        assert!(problems.iter().any(|p| p.message.contains("cycle")));
    }

    #[test]
    fn unused_ore_is_reported() {
        let mut files = minimal();
        // `stone` drops `stone_item`, which nothing consumes.
        files.layers[0].ores.push(OreRule {
            block: "stone".into(),
            frequency: 0.1,
            vein_size: (1, 2),
            depth_band: None,
        });
        let problems = validate(&files);
        assert!(problems.iter().any(|p| p.message.contains("no use in any recipe")));
    }

    #[test]
    fn duplicate_ids_are_reported() {
        let mut files = minimal();
        files.items.push(item("ingot"));
        let problems = validate(&files);
        assert!(problems.iter().any(|p| p.message.contains("duplicate id")));
    }

    #[test]
    fn layer_gaps_are_reported() {
        let mut files = minimal();
        files.layers.push(LayerDef {
            id: "deep".into(),
            name: "Deep".into(),
            depth: (400, 250),
            base_block: "stone".into(),
            filler_blocks: vec![],
            ores: vec![],
            hazards: vec![],
            ambient_temp: 20.0,
            min_tier: 2,
            cave_density: 1.0,
        });
        files.world.surface_y = 400;
        let problems = validate(&files);
        assert!(problems.iter().any(|p| p.message.contains("gap in layer coverage")));
    }
}
