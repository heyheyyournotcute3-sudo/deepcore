//! Deepcore dedicated server.
//!
//! Headless: no window, no renderer, no GPU. The server owns the world, the machines, the
//! tech tree and every gameplay decision; clients only send intents.

use bevy::prelude::*;

/// Simulation rate of the server. The brief fixes this at 30 Hz.
pub const TICK_HZ: f64 = 30.0;

fn main() {
    let mut app = App::new();

    app.add_plugins((
        MinimalPlugins,
        bevy::log::LogPlugin { filter: "info,deepcore=debug".to_owned(), ..default() },
        bevy::state::app::StatesPlugin,
        deepcore_shared::SharedPlugin,
    ));

    app.insert_resource(Time::<Fixed>::from_hz(TICK_HZ));

    app.add_systems(Startup, log_banner);
    app.add_systems(FixedUpdate, count_ticks);

    app.run();
}

#[derive(Resource, Default)]
struct TickCounter(u64);

fn log_banner(mut commands: Commands) {
    info!(
        "Deepcore server starting: tick rate {TICK_HZ} Hz, protocol v{}",
        deepcore_shared::protocol::PROTOCOL_VERSION
    );
    commands.insert_resource(TickCounter::default());
}

/// Logs the achieved tick rate once a second, which is the cheapest possible
/// early-warning signal that the simulation is falling behind its budget.
fn count_ticks(mut counter: ResMut<TickCounter>, time: Res<Time<Fixed>>) {
    counter.0 += 1;
    if counter.0.is_multiple_of(TICK_HZ as u64) {
        debug!("tick {} (fixed step {:.2} ms)", counter.0, time.timestep().as_secs_f64() * 1000.0);
    }
}
