//! Progression cost analysis.
//!
//! Answers the question the brief cares about for pacing: *how much raw ore does an Era
//! actually cost?* The answer is computed by recursively expanding tech-node costs and
//! machine build costs through the recipe graph until only raw, unmined items remain.
//!
//! This is used by `cargo run -p deepcore_shared --bin balance_report` and by the
//! progression tests, so a content change that makes Era 4 absurdly expensive shows up
//! immediately instead of three hours into a play session.

use std::collections::HashMap;

use crate::content::model::ItemCategory;
use crate::content::registry::{ItemId, RecipeId, Registry};

/// How a raw item is obtained, for reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Mined from a layer.
    Ore,
    /// Chopped from trees.
    Timber,
    /// Found on the surface (clay, sand, gravel).
    Surface,
    /// Produced by a recipe whose inputs cycle back to it; the report does not expand it.
    Crafted,
    /// Not producible at all — a content bug the report flags.
    Unobtainable,
}

/// A raw-material requirement: how many of an item, and where it comes from.
#[derive(Clone, Debug)]
pub struct RawRequirement {
    /// The raw item.
    pub item: ItemId,
    /// Display name.
    pub name: String,
    /// How many are needed.
    pub count: u64,
    /// Where it comes from.
    pub origin: Origin,
}

/// Totals for one Era.
#[derive(Clone, Debug, Default)]
pub struct EraCost {
    /// Era number.
    pub era: u8,
    /// Raw materials needed for this Era's tech nodes and the machines they unlock.
    pub raw: Vec<RawRequirement>,
    /// Total number of distinct items that had to be crafted along the way.
    pub crafted_steps: u64,
}

impl EraCost {
    /// Total raw item count, useful as a single "size of the Era" number.
    pub fn total_raw(&self) -> u64 {
        self.raw.iter().map(|r| r.count).sum()
    }
}

/// The full progression report.
#[derive(Clone, Debug, Default)]
pub struct BalanceReport {
    /// One entry per Era, ordered 0..5.
    pub eras: Vec<EraCost>,
    /// Tech nodes whose cost could not be produced from raw materials.
    pub problems: Vec<String>,
}

/// Computes the raw cost of every Era.
pub fn report(registry: &Registry) -> BalanceReport {
    let mut out = BalanceReport::default();
    let max_era = registry.tech_nodes().iter().map(|t| t.def.era).max().unwrap_or(0);

    for era in 0..=max_era {
        let mut needed: HashMap<ItemId, u64> = HashMap::new();
        let mut crafted_steps = 0u64;

        for node in registry.tech_nodes().iter().filter(|n| n.def.era == era) {
            for (item, count) in &node.cost {
                expand(registry, *item, u64::from(*count), &mut needed, &mut crafted_steps);
            }
            for machine in &node.unlock_machines {
                let Some(def) = registry.machine(*machine) else { continue };
                for (name, count) in &def.build_cost {
                    if let Some(item) = registry.item_id(name) {
                        expand(registry, item, u64::from(*count), &mut needed, &mut crafted_steps);
                    }
                }
            }
        }

        let mut raw: Vec<RawRequirement> = needed
            .into_iter()
            .map(|(item, count)| RawRequirement {
                item,
                name: registry.item_name(item).to_owned(),
                count,
                origin: origin_of(registry, item),
            })
            .collect();
        raw.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));

        for requirement in &raw {
            if requirement.origin == Origin::Unobtainable {
                out.problems.push(format!(
                    "era {era}: `{}` is required but cannot be obtained",
                    requirement.name
                ));
            }
        }

        out.eras.push(EraCost { era, raw, crafted_steps });
    }

    out
}

/// Expands an item requirement down to raw materials.
fn expand(
    registry: &Registry,
    item: ItemId,
    count: u64,
    out: &mut HashMap<ItemId, u64>,
    crafted_steps: &mut u64,
) {
    expand_guarded(registry, item, count, out, crafted_steps, &mut Vec::new());
}

/// Recursion guard: recipes can legitimately reference each other (charging a battery pack
/// consumes a battery pack), so we refuse to expand an item that is already being expanded
/// further up the call stack and treat it as a raw input instead.
fn expand_guarded(
    registry: &Registry,
    item: ItemId,
    count: u64,
    out: &mut HashMap<ItemId, u64>,
    crafted_steps: &mut u64,
    stack: &mut Vec<ItemId>,
) {
    if stack.contains(&item) || stack.len() > 16 {
        *out.entry(item).or_default() += count;
        return;
    }
    let recipe = best_recipe(registry, item);
    match recipe {
        Some(recipe) => {
            let entry = match registry.recipe(recipe) {
                Some(entry) => entry,
                None => return,
            };
            let produced = entry
                .outputs
                .iter()
                .find(|(id, _)| *id == item)
                .map(|(_, n)| u64::from(*n))
                .unwrap_or(1)
                .max(1);
            let batches = count.div_ceil(produced);
            *crafted_steps += batches;
            stack.push(item);
            for (input, input_count) in &entry.inputs {
                expand_guarded(
                    registry,
                    *input,
                    u64::from(*input_count) * batches,
                    out,
                    crafted_steps,
                    stack,
                );
            }
            for (fuel, fuel_count) in &entry.fuel {
                expand_guarded(
                    registry,
                    *fuel,
                    u64::from(*fuel_count) * batches,
                    out,
                    crafted_steps,
                    stack,
                );
            }
            stack.pop();
        }
        None => {
            *out.entry(item).or_default() += count;
        }
    }
}

/// The recipe that produces an item with the best output-per-input ratio.
fn best_recipe(registry: &Registry, item: ItemId) -> Option<RecipeId> {
    registry
        .recipes()
        .iter()
        .enumerate()
        .filter(|(_, recipe)| recipe.outputs.iter().any(|(id, _)| *id == item))
        .max_by(|(_, a), (_, b)| yield_ratio(a).total_cmp(&yield_ratio(b)))
        .map(|(index, _)| RecipeId(index as u16))
}

fn yield_ratio(recipe: &crate::content::registry::RecipeEntry) -> f32 {
    let produced: u64 = recipe.outputs.iter().map(|(_, n)| u64::from(*n)).sum();
    let consumed: u64 = recipe.inputs.iter().map(|(_, n)| u64::from(*n)).sum();
    if consumed == 0 { 0.0 } else { produced as f32 / consumed as f32 }
}

/// Where an item comes from, inferred from the content: the first ore block that drops it,
/// or its category.
fn origin_of(registry: &Registry, item: ItemId) -> Origin {
    // A recipe cycle (e.g. "charge battery pack" consumes and produces a battery pack) makes
    // an item look unobtainable if we never follow the productive branch. Expand the best
    // recipe once to decide whether it is craftable at all.
    if best_recipe(registry, item).is_some() {
        return Origin::Crafted;
    }
    for block in registry.blocks() {
        if block.drops.iter().any(|(drop, _)| *drop == item) {
            return match block.def.layer_hint {
                Some(1) => Origin::Surface,
                Some(_) => Origin::Ore,
                None => Origin::Surface,
            };
        }
    }
    match registry.item(item).map(|entry| &entry.def.category) {
        Some(ItemCategory::Block | ItemCategory::MachineKit) => Origin::Ore,
        _ => Origin::Unobtainable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::load_registry;

    #[test]
    fn report_covers_every_era_and_finds_no_unobtainable_requirements() {
        let registry = load_registry().expect("content loads");
        let report = report(&registry);
        assert_eq!(report.eras.len(), 6, "six eras 0..5");
        assert!(report.problems.is_empty(), "unobtainable requirements: {:?}", report.problems);
        // Era 0 is the free starting kit, so it has no research cost; every later Era must
        // require something.
        for era in report.eras.iter().filter(|era| era.era > 0) {
            assert!(!era.raw.is_empty(), "era {} requires nothing at all", era.era);
        }
    }

    #[test]
    fn later_eras_cost_more_raw_material_than_era_zero() {
        let registry = load_registry().expect("content loads");
        let report = report(&registry);
        let era0 = report.eras[0].total_raw();
        let era5 = report.eras[5].total_raw();
        assert!(era5 > era0 * 5, "era 5 ({era5}) should dwarf era 0 ({era0})");
    }
}
