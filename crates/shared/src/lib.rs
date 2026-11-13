//! Deepcore shared game rules.
//!
//! This crate is the single place where the *rules* of Deepcore live: voxel storage,
//! world generation, movement and collision, inventory, crafting, power networks, hazards
//! and the network protocol. It contains no rendering, no sockets and no file I/O beyond
//! loading the content data, so the exact same code runs on the authoritative server, on
//! the client (for prediction and UI) and in headless tests.
//!
//! Rule of thumb: **`shared` holds every rule, `server` decides, `client` shows.**

use bevy::prelude::*;

pub mod atlas;
pub mod balance;
pub mod content;
pub mod crafting;
pub mod inventory;
pub mod mesh;
pub mod mining;
pub mod movement;
pub mod placement;
pub mod protocol;
pub mod tech;
pub mod voxel;
pub mod worldgen;

/// The fixed simulation rate of the server, and the rate the client predicts at.
pub const SERVER_TICK_HZ: f64 = 30.0;

/// Registers the shared resources every Deepcore app needs, server or client.
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // Both sides simulate on the same fixed timestep so that shared movement code
        // produces identical results for identical inputs.
        app.insert_resource(Time::<Fixed>::from_hz(SERVER_TICK_HZ));
        app.add_plugins(content::ContentPlugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_plugin_sets_30hz_fixed_step() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, SharedPlugin));
        let time = app.world().resource::<Time<Fixed>>();
        assert!((time.timestep().as_secs_f64() - 1.0 / 30.0).abs() < 1e-9);
    }
}
