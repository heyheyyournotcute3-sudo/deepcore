//! Content loading: RON files → validated [`Registry`] → a Bevy resource.
//!
//! Content is embedded into the binary with `include_str!`. That keeps the promise that a
//! fresh clone runs with zero downloaded assets and zero working-directory assumptions, and
//! it means the client and the server are guaranteed to be looking at the same data. The
//! files still live in `assets/data/` where humans edit them.

pub mod model;
pub mod registry;
pub mod validate;

use std::fmt;

use bevy::prelude::*;

use crate::content::model::{
    BlockDef, ContentFiles, ItemDef, LayerDef, MachineDef, RecipeDef, TechNodeDef,
};
use crate::content::registry::Registry;
use crate::content::validate::{Problem, validate};

/// Where the content files live, relative to the workspace root.
pub const CONTENT_DIR: &str = "assets/data";

macro_rules! embedded {
    ($file:literal) => {
        (concat!("assets/data/", $file), include_str!(concat!("../../../../assets/data/", $file)))
    };
}

/// The raw text of every content file, embedded at compile time.
pub const CONTENT_SOURCES: &[(&str, &str)] = &[
    embedded!("blocks.ron"),
    embedded!("items.ron"),
    embedded!("recipes.ron"),
    embedded!("machines.ron"),
    embedded!("tech_tree.ron"),
    embedded!("layers.ron"),
    embedded!("world.ron"),
];

/// Errors that can occur while loading content.
#[derive(Debug)]
pub enum ContentError {
    /// A file could not be parsed as RON.
    Parse {
        /// File name.
        file: &'static str,
        /// Parser error, including position.
        message: String,
    },
    /// The content parsed but failed validation.
    Invalid(Vec<Problem>),
}

impl fmt::Display for ContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContentError::Parse { file, message } => {
                write!(f, "content file `{file}` failed to parse: {message}")
            }
            ContentError::Invalid(problems) => {
                writeln!(f, "content validation failed with {} problem(s):", problems.len())?;
                for problem in problems {
                    writeln!(f, "  - {problem}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ContentError {}

/// Parses every embedded content file and validates the result.
pub fn load_content() -> Result<ContentFiles, ContentError> {
    let files = ContentFiles {
        blocks: parse(embedded_file("blocks.ron"))?,
        items: parse(embedded_file("items.ron"))?,
        recipes: parse(embedded_file("recipes.ron"))?,
        machines: parse(embedded_file("machines.ron"))?,
        tech: parse(embedded_file("tech_tree.ron"))?,
        layers: parse(embedded_file("layers.ron"))?,
        world: parse(embedded_file("world.ron"))?,
    };

    let problems = validate(&files);
    if !problems.is_empty() {
        return Err(ContentError::Invalid(problems));
    }
    Ok(files)
}

fn embedded_file(name: &str) -> (&'static str, &'static str) {
    CONTENT_SOURCES
        .iter()
        .find(|(file, _)| file.ends_with(name))
        .copied()
        .unwrap_or_else(|| panic!("content file `{name}` is not embedded"))
}

fn parse<T: serde::de::DeserializeOwned>(
    (file, source): (&'static str, &'static str),
) -> Result<T, ContentError> {
    ron::from_str(source).map_err(|error| ContentError::Parse {
        file,
        message: format!("{error} (line {})", error.span.start.line + 1),
    })
}

/// Loads the content, validates it and builds the registry.
pub fn load_registry() -> Result<Registry, ContentError> {
    let files = load_content()?;
    Ok(Registry::build(&files))
}

/// A stable 64-bit hash of all embedded content, exchanged during the handshake so a
/// client running different data is refused instead of silently desyncing.
pub fn content_hash() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (file, source) in CONTENT_SOURCES {
        file.hash(&mut hasher);
        source.hash(&mut hasher);
    }
    hasher.finish()
}

/// The content registry as a Bevy resource. Present on both server and client.
#[derive(Resource)]
pub struct ContentRegistry(pub Registry);

impl std::fmt::Debug for ContentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContentRegistry")
            .field("blocks", &self.0.block_count())
            .field("items", &self.0.item_count())
            .field("recipes", &self.0.recipe_count())
            .finish()
    }
}

impl std::ops::Deref for ContentRegistry {
    type Target = Registry;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Loads content into the app, panicking with a readable report if it is broken.
///
/// Content errors are fatal by design: shipping a build whose data files disagree with
/// each other is worse than failing at startup with a precise list of problems.
pub struct ContentPlugin;

impl Plugin for ContentPlugin {
    fn build(&self, app: &mut App) {
        match load_registry() {
            Ok(registry) => {
                info!(
                    "content loaded: {} blocks, {} items, {} recipes, {} machines, {} tech nodes, {} layers",
                    registry.block_count(),
                    registry.item_count(),
                    registry.recipe_count(),
                    registry.machine_count(),
                    registry.tech_count(),
                    registry.layer_count()
                );
                app.insert_resource(ContentRegistry(registry));
            }
            Err(error) => panic!("{error}"),
        }
    }
}

/// Convenience accessor used by systems: the registry resource.
pub type Content<'w> = Res<'w, ContentRegistry>;

// Keep the model types reachable through this module for callers that only need the
// on-disk shapes.
pub use crate::content::model::{BlockKind, ItemCategory, Pattern, ToolDef, ToolEnergy};

#[allow(dead_code)]
fn _assert_model_types_used(
    _: &BlockDef,
    _: &ItemDef,
    _: &RecipeDef,
    _: &MachineDef,
    _: &TechNodeDef,
    _: &LayerDef,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_content_parses_and_validates() {
        match load_content() {
            Ok(_) => {}
            Err(error) => panic!("{error}"),
        }
    }

    #[test]
    fn registry_builds_from_embedded_content() {
        let registry = load_registry().expect("content loads");
        assert!(registry.block_count() > 10, "expected a populated block registry");
        assert!(registry.item_count() > 10);
        assert!(registry.layer_count() >= 6, "the brief requires six layers");
        assert_eq!(registry.world().surface_y, 480);
    }

    #[test]
    fn content_hash_is_stable_within_a_build() {
        assert_eq!(content_hash(), content_hash());
        assert_ne!(content_hash(), 0);
    }
}
