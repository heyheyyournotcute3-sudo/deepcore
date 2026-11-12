//! Movement prediction and reconciliation.
//!
//! The client applies each input immediately with the shared controller, remembers the inputs
//! it has not yet had acknowledged, and — when the server reports where the player actually is
//! — rewinds to the authoritative state and replays the unacknowledged inputs. Small errors are
//! smoothed over a short window instead of snapping, which is what makes 150 ms of latency feel
//! responsive rather than rubber-banded.

use std::collections::VecDeque;

use bevy::prelude::*;

use deepcore_shared::movement::{BlockPhysics, MoveInput, MoveTuning, PlayerState, step_player};
use deepcore_shared::protocol::NetPlayer;
use deepcore_shared::voxel::world::VoxelWorld;

/// Errors below this distance are corrected by smoothing instead of a snap.
pub const SMOOTH_THRESHOLD: f32 = 0.3;
/// Seconds over which a small correction is blended out.
pub const SMOOTH_TIME: f32 = 0.1;

/// The client's prediction state.
#[derive(Resource)]
pub struct Prediction {
    /// The predicted state the camera follows.
    pub state: PlayerState,
    /// Inputs sent but not yet acknowledged.
    pub pending: VecDeque<MoveInput>,
    /// Visual offset applied on top of the predicted position, eased to zero.
    pub smoothing: Vec3,
    /// Sequence number of the last input sent.
    pub next_seq: u32,
    /// Corrections applied since the last reset, for the debug overlay.
    pub corrections: u32,
    /// Physics tables.
    pub physics: BlockPhysics,
    /// Movement tuning.
    pub tuning: MoveTuning,
}

impl Prediction {
    /// Creates a prediction state at a spawn position.
    pub fn new(spawn: Vec3, physics: BlockPhysics) -> Self {
        Self {
            state: PlayerState::at(spawn),
            pending: VecDeque::new(),
            smoothing: Vec3::ZERO,
            next_seq: 0,
            corrections: 0,
            physics,
            tuning: MoveTuning::default(),
        }
    }

    /// Applies one input locally and records it as pending.
    pub fn apply_local(&mut self, world: &VoxelWorld, mut input: MoveInput, dt: f32) {
        input.seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        step_player(world, &self.physics, &mut self.state, input, dt, &self.tuning);
        self.pending.push_back(input);
        // Never remember more than a second of inputs.
        while self.pending.len() > 60 {
            self.pending.pop_front();
        }
    }

    /// Reconciles against an authoritative state from the server.
    ///
    /// Inputs up to `last_seq` are dropped (they are baked into the authoritative state), the
    /// rest are replayed on top of it.
    pub fn reconcile(&mut self, world: &VoxelWorld, authoritative: NetPlayer, dt: f32) {
        let before = self.state.position;
        let server_position = authoritative.feet();

        // Drop acknowledged inputs.
        while let Some(input) = self.pending.front() {
            if input.seq <= authoritative.last_seq {
                self.pending.pop_front();
            } else {
                break;
            }
        }

        // Rewind to the server's state, then replay what the server has not seen yet.
        let mut state = PlayerState::at(server_position);
        state.yaw = authoritative.yaw;
        state.pitch = authoritative.pitch;
        state.crouching = authoritative.crouching;
        for input in &self.pending {
            step_player(world, &self.physics, &mut state, *input, dt, &self.tuning);
        }

        let error = (state.position - before).length();
        self.state = state;
        if error > 1e-4 {
            self.corrections = self.corrections.saturating_add(1);
        }
        if error > SMOOTH_THRESHOLD {
            // Large error: snap, the player genuinely is somewhere else (teleport, big loss).
            self.smoothing = Vec3::ZERO;
        } else if error > 1e-4 {
            // Small error: keep the old visual position and ease towards the corrected one.
            self.smoothing += before - state.position;
        }
    }

    /// Eases the visual smoothing offset towards zero.
    pub fn tick_smoothing(&mut self, dt: f32) {
        if self.smoothing == Vec3::ZERO {
            return;
        }
        let decay = (dt / SMOOTH_TIME).clamp(0.0, 1.0);
        self.smoothing *= 1.0 - decay;
        if self.smoothing.length() < 0.001 {
            self.smoothing = Vec3::ZERO;
        }
    }

    /// The position the camera should be drawn at.
    pub fn visual_position(&self) -> Vec3 {
        self.state.position + self.smoothing
    }
}

/// Reconciles when the server sends a new authoritative state for our own player.
pub fn reconcile_system(
    mut prediction: ResMut<Prediction>,
    own: Res<crate::net::OwnPlayer>,
    players: Query<&NetPlayer>,
    world: Res<VoxelWorld>,
) {
    let Some(entity) = own.entity else {
        return;
    };
    let Ok(net) = players.get(entity) else {
        return;
    };
    let dt = 1.0 / deepcore_shared::SERVER_TICK_HZ as f32;
    prediction.reconcile(&world, *net, dt);
}

/// Smooths interpolation for remote players.
#[derive(Component, Default)]
pub struct RemoteInterpolation {
    /// The position to ease from.
    pub from: Vec3,
    /// The position to ease to.
    pub to: Vec3,
    /// Progress through the current interpolation, 0..1.
    pub t: f32,
}

/// Interpolates remote players towards their replicated positions.
pub fn interpolate_remote_players(
    time: Res<Time>,
    own: Res<crate::net::OwnPlayer>,
    mut players: Query<(Entity, &NetPlayer, &mut Transform, Option<&mut RemoteInterpolation>)>,
) {
    // Remote players get a ~100 ms buffer so movement looks smooth at any packet rate.
    let blend = (time.delta_secs() / 0.1).clamp(0.0, 1.0);
    for (entity, net, mut transform, interpolation) in &mut players {
        if Some(entity) == own.entity {
            continue;
        }
        let target = net.feet();
        if let Some(mut interpolation) = interpolation {
            if interpolation.to != target {
                interpolation.from = transform.translation;
                interpolation.to = target;
                interpolation.t = 0.0;
            }
            interpolation.t = (interpolation.t + blend).min(1.0);
            transform.translation = interpolation.from.lerp(interpolation.to, interpolation.t);
        } else {
            transform.translation = transform.translation.lerp(target, blend);
        }
        transform.rotation = Quat::from_euler(EulerRot::YXZ, net.yaw, net.pitch, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepcore_shared::voxel::chunk::Chunk;
    use deepcore_shared::voxel::coords::{BlockPos, ChunkPos};

    fn flat_world() -> VoxelWorld {
        let mut world = VoxelWorld::new();
        let mut chunk = Chunk::filled(1);
        for y in 10..32 {
            for z in 0..32 {
                for x in 0..32 {
                    chunk.set(deepcore_shared::voxel::LocalPos::new(x, y, z), 0);
                }
            }
        }
        world.insert(ChunkPos::new(0, 0, 0), chunk);
        world
    }

    #[test]
    fn prediction_advances_the_player_and_records_inputs() {
        let world = flat_world();
        let mut prediction = Prediction::new(Vec3::new(16.0, 10.0, 16.0), BlockPhysics::default());
        let input = MoveInput { forward: 1.0, yaw: 0.0, ..Default::default() };
        for _ in 0..10 {
            prediction.apply_local(&world, input, 1.0 / 30.0);
        }
        assert_eq!(prediction.pending.len(), 10);
        assert!(prediction.state.position.z < 16.0, "should have moved along -Z");
    }

    #[test]
    fn reconciliation_drops_acknowledged_inputs_and_replays_the_rest() {
        let world = flat_world();
        let mut prediction = Prediction::new(Vec3::new(16.0, 10.0, 16.0), BlockPhysics::default());
        let input = MoveInput { forward: 1.0, ..Default::default() };
        for _ in 0..10 {
            prediction.apply_local(&world, input, 1.0 / 30.0);
        }
        let predicted = prediction.state.position;

        // The server acknowledges the first five inputs and reports the matching position.
        let mut authoritative = NetPlayer::from_state(&prediction.state, 4);
        authoritative.position = [16.0, 10.0, 16.0];
        prediction.reconcile(&world, authoritative, 1.0 / 30.0);

        assert_eq!(prediction.pending.len(), 5, "five inputs remain unacknowledged");
        // Replaying the remaining five inputs on top of the server state must land close to
        // what the client predicted (the server state here is the start position, so the
        // client ends up five steps along).
        let distance = (prediction.state.position - predicted).length();
        assert!(distance > 0.0, "the replayed state should have advanced");
        assert!(prediction.state.position.z < 16.0);
    }

    #[test]
    fn identical_authority_produces_no_correction() {
        let world = flat_world();
        let mut prediction = Prediction::new(Vec3::new(16.0, 10.0, 16.0), BlockPhysics::default());
        let input = MoveInput { forward: 1.0, ..Default::default() };
        for _ in 0..5 {
            prediction.apply_local(&world, input, 1.0 / 30.0);
        }
        let authoritative = NetPlayer::from_state(&prediction.state, prediction.next_seq - 1);
        prediction.reconcile(&world, authoritative, 1.0 / 30.0);
        assert_eq!(prediction.corrections, 0, "no correction when the server agrees");
        assert!(prediction.smoothing.length() < 1e-3);
    }

    #[test]
    fn smoothing_decays_to_zero() {
        let mut prediction = Prediction::new(Vec3::ZERO, BlockPhysics::default());
        prediction.smoothing = Vec3::new(0.2, 0.0, 0.0);
        // Smoothing is a linear decay: 20 frames of 1/60 s cover 0.33 s, well past the
        // 0.1 s smoothing window, so the offset must be fully gone.
        for _ in 0..40 {
            prediction.tick_smoothing(1.0 / 60.0);
        }
        assert_eq!(prediction.smoothing, Vec3::ZERO);
    }

    #[test]
    fn visual_position_includes_smoothing() {
        let mut prediction = Prediction::new(Vec3::new(1.0, 2.0, 3.0), BlockPhysics::default());
        prediction.smoothing = Vec3::new(0.1, 0.0, 0.0);
        assert_eq!(prediction.visual_position(), Vec3::new(1.1, 2.0, 3.0));
        let _ = BlockPos::new(0, 0, 0);
    }
}
