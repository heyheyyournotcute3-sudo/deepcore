//! `balance_report` — prints the raw-material cost of each Era.
//!
//! Run with `cargo run -p deepcore_shared --bin balance_report`. It expands every tech node
//! and machine through the recipe graph and prints what the players will actually have to
//! mine, which is the fastest way to check pacing after a content change.

use deepcore_shared::balance::{BalanceReport, Origin, report};
use deepcore_shared::content::load_registry;

fn main() {
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("content error: {error}");
            std::process::exit(1);
        }
    };

    println!("Deepcore balance report");
    println!(
        "content: {} blocks, {} items, {} recipes, {} machines, {} tech nodes, {} layers\n",
        registry.block_count(),
        registry.item_count(),
        registry.recipe_count(),
        registry.machine_count(),
        registry.tech_count(),
        registry.layer_count()
    );

    let report = report(&registry);
    print_report(&report);

    if !report.problems.is_empty() {
        println!("\nPROBLEMS:");
        for problem in &report.problems {
            println!("  - {problem}");
        }
        std::process::exit(1);
    }
}

fn print_report(report: &BalanceReport) {
    println!("{:<5} {:>10} {:>12}  Largest requirements", "Era", "Raw items", "Craft steps");
    println!("{}", "-".repeat(100));
    for era in &report.eras {
        let top: Vec<String> =
            era.raw.iter().take(6).map(|r| format!("{} x{}", r.name, r.count)).collect();
        println!(
            "{:<5} {:>10} {:>12}  {}",
            era.era,
            era.total_raw(),
            era.crafted_steps,
            top.join(", ")
        );
    }

    println!("\nRaw materials per Era:");
    for era in &report.eras {
        println!("\n  Era {}:", era.era);
        for requirement in &era.raw {
            let origin = match requirement.origin {
                Origin::Ore => "ore",
                Origin::Surface => "surface",
                Origin::Timber => "timber",
                Origin::Crafted => "crafted",
                Origin::Unobtainable => "UNOBTAINABLE",
            };
            println!("    {:<28} {:>8}  ({origin})", requirement.name, requirement.count);
        }
    }
}
