//! Player movement: an AABB against a voxel world.
//!
//! This module is the single most safety-critical piece of shared code. The client predicts
//! with it and the server re-simulates the same inputs with it, so it must be:
//!
//! * **deterministic** — no randomness, no frame-rate dependence (everything is scaled by
//!   an explicit `dt`), and the same order of operations on both sides;
//! * **identical in both processes** — it lives here, not in `client` or `server`;
//! * **cheap** — it runs 30-60 times a second per player and once per replayed input.
//!
//! Collision is resolved one axis at a time (move X, resolve; move Y, resolve; move Z,
//! resolve). That is the standard, robust approach for axis-aligned block worlds: it makes
//! sliding along walls free, avoids tunnelling at normal speeds, and never leaves the player
//! inside a block.

use bevy::math::{Vec2, Vec3};

use crate::voxel::coords::{BlockPos, CHUNK_SIZE};
use crate::voxel::world::VoxelWorld;

/// Half-extents of the player's collision box.
pub const PLAYER_HALF_WIDTH: f32 = 0.3;
/// Full height of the player's collision box.
pub const PLAYER_HEIGHT: f32 = 1.8;
/// Eye height above the feet, used by the camera and the targeting raycast.
pub const PLAYER_EYE_HEIGHT: f32 = 1.62;
/// Height when crouching.
pub const PLAYER_CROUCH_HEIGHT: f32 = 1.2;

/// Tunable movement constants. All of these live in one place so client and server agree.
#[derive(Clone, Copy, Debug)]
pub struct MoveTuning {
    /// Horizontal walking speed in blocks/second.
    pub walk_speed: f32,
    /// Sprint multiplier.
    pub sprint_multiplier: f32,
    /// Crouch multiplier.
    pub crouch_multiplier: f32,
    /// Upward velocity applied when jumping.
    pub jump_velocity: f32,
    /// Downward acceleration in blocks/second².
    pub gravity: f32,
    /// Maximum fall speed.
    pub terminal_velocity: f32,
    /// How fast ground acceleration reaches the target velocity (1/s).
    pub ground_accel: f32,
    /// How fast air acceleration reaches the target velocity (1/s).
    pub air_accel: f32,
    /// Speed of vertical climbing on ladders.
    pub climb_speed: f32,
    /// Maximum height the player can step up automatically (0 = no auto-step).
    pub step_height: f32,
}

impl Default for MoveTuning {
    fn default() -> Self {
        Self {
            walk_speed: 4.6,
            sprint_multiplier: 1.55,
            crouch_multiplier: 0.45,
            jump_velocity: 7.6,
            gravity: 24.0,
            terminal_velocity: 48.0,
            ground_accel: 14.0,
            air_accel: 3.0,
            climb_speed: 3.2,
            step_height: 0.0,
        }
    }
}

/// A player's physical state. This is the state that is predicted and reconciled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerState {
    /// Centre of the player's feet.
    pub position: Vec3,
    /// Velocity in blocks/second.
    pub velocity: Vec3,
    /// True when standing on solid ground.
    pub on_ground: bool,
    /// True when in contact with a ladder.
    pub on_ladder: bool,
    /// True when the head is under water (drives oxygen drain).
    pub submerged: bool,
    /// Look direction, yaw around Y in radians, pitch around X.
    pub yaw: f32,
    /// Look pitch in radians, clamped to +-89 degrees.
    pub pitch: f32,
    /// True while crouching (changes the collision height).
    pub crouching: bool,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            on_ground: false,
            on_ladder: false,
            submerged: false,
            yaw: 0.0,
            pitch: 0.0,
            crouching: false,
        }
    }
}

impl PlayerState {
    /// Creates a state at a feet position.
    pub fn at(position: Vec3) -> Self {
        Self { position, ..default_state() }
    }

    /// The player's collision height for the current crouch state.
    pub fn height(&self) -> f32 {
        if self.crouching { PLAYER_CROUCH_HEIGHT } else { PLAYER_HEIGHT }
    }

    /// World-space eye position, used by the camera and the targeting ray.
    pub fn eye_position(&self) -> Vec3 {
        self.position + Vec3::Y * (self.height() - 0.18)
    }

    /// Unit vector the player is looking along.
    pub fn look_direction(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        Vec3::new(-sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch)
    }

    /// The axis-aligned box this player occupies at its current position.
    pub fn aabb(&self) -> Aabb {
        Aabb::from_feet(self.position, self.height())
    }
}

fn default_state() -> PlayerState {
    PlayerState::default()
}

/// One tick of player input, as sent from client to server.
///
/// Movement is expressed in the player's local frame so the server can validate it without
/// knowing the client's camera: `forward` is along the look direction on the XZ plane and
/// `right` is its perpendicular.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoveInput {
    /// Monotonic sequence number, used to acknowledge processed inputs.
    pub seq: u32,
    /// Forward/backward axis, -1..1.
    pub forward: f32,
    /// Right/left axis, -1..1.
    pub right: f32,
    /// Jump pressed this tick.
    pub jump: bool,
    /// Sprint held.
    pub sprint: bool,
    /// Crouch held.
    pub crouch: bool,
    /// Yaw in radians.
    pub yaw: f32,
    /// Pitch in radians.
    pub pitch: f32,
}

impl MoveInput {
    /// Clamps the analog axes into their legal range, as the server does before applying an
    /// untrusted input. This is the only sanitising the server needs for movement, because
    /// everything else is derived from it.
    pub fn sanitized(mut self) -> Self {
        self.forward = self.forward.clamp(-1.0, 1.0);
        self.right = self.right.clamp(-1.0, 1.0);
        if !self.yaw.is_finite() {
            self.yaw = 0.0;
        }
        if !self.pitch.is_finite() {
            self.pitch = 0.0;
        }
        self.pitch = self
            .pitch
            .clamp(-std::f32::consts::FRAC_PI_2 + 0.01, std::f32::consts::FRAC_PI_2 - 0.01);
        // Normalise the combined axis so diagonal movement is not faster.
        let length_squared = self.forward * self.forward + self.right * self.right;
        if length_squared > 1.0 {
            let length = length_squared.sqrt();
            self.forward /= length;
            self.right /= length;
        }
        self
    }
}

/// An axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    /// Minimum corner.
    pub min: Vec3,
    /// Maximum corner.
    pub max: Vec3,
}

impl Aabb {
    /// Builds the player-sized box whose bottom-centre is `feet`.
    pub fn from_feet(feet: Vec3, height: f32) -> Self {
        Self {
            min: Vec3::new(feet.x - PLAYER_HALF_WIDTH, feet.y, feet.z - PLAYER_HALF_WIDTH),
            max: Vec3::new(feet.x + PLAYER_HALF_WIDTH, feet.y + height, feet.z + PLAYER_HALF_WIDTH),
        }
    }

    /// True if this box overlaps the unit cube at `pos`.
    pub fn intersects_block(&self, pos: BlockPos) -> bool {
        let (bx, by, bz) = (pos.x as f32, pos.y as f32, pos.z as f32);
        self.min.x < bx + 1.0
            && self.max.x > bx
            && self.min.y < by + 1.0
            && self.max.y > by
            && self.min.z < bz + 1.0
            && self.max.z > bz
    }

    /// The block range this box touches, inclusive.
    pub fn block_bounds(&self) -> (BlockPos, BlockPos) {
        (
            BlockPos::new(
                self.min.x.floor() as i32,
                self.min.y.floor() as i32,
                self.min.z.floor() as i32,
            ),
            BlockPos::new(
                (self.max.x - f32::EPSILON).floor() as i32,
                (self.max.y - f32::EPSILON).floor() as i32,
                (self.max.z - f32::EPSILON).floor() as i32,
            ),
        )
    }
}

/// Decides whether a block stops the player, and what kind of block it is.
///
/// Air and passable blocks do not stop movement; ladders and water are passable but still
/// matter to movement, so the test reports them separately.
pub trait SolidTest {
    /// True if the block at this position blocks movement.
    fn is_solid(&self, pos: BlockPos) -> bool;
    /// True if the block is a climbable ladder.
    fn is_ladder(&self, _pos: BlockPos) -> bool {
        false
    }
    /// True if the block is water.
    fn is_water(&self, _pos: BlockPos) -> bool {
        false
    }
}

/// Block classification used by movement: which blocks you can walk through, climb, and swim in.
///
/// Built once from the content registry and passed by reference into every movement step, so
/// the client and the server classify blocks identically.
#[derive(Clone, Debug, Default)]
pub struct BlockPhysics {
    /// Blocks the player can walk through.
    pub passable: Vec<crate::voxel::BlockId>,
    /// Blocks that can be climbed.
    pub ladders: Vec<crate::voxel::BlockId>,
    /// Blocks that count as water.
    pub water: Vec<crate::voxel::BlockId>,
}

impl BlockPhysics {
    /// Derives the classification from the content registry.
    pub fn from_registry(registry: &crate::content::registry::Registry) -> Self {
        let mut physics = Self::default();
        for block in registry.blocks() {
            let id = registry.block_id(&block.def.id).unwrap_or(crate::voxel::AIR);
            if id == crate::voxel::AIR {
                continue;
            }
            if block.def.kind == crate::content::model::BlockKind::Passable {
                physics.passable.push(id);
            }
            if block.def.id == "ladder" {
                physics.ladders.push(id);
            }
            if block.def.id == "water" {
                physics.water.push(id);
            }
        }
        physics
    }
}

/// Solid test backed by a [`VoxelWorld`] and a set of passable block ids.
pub struct WorldSolid<'a> {
    world: &'a VoxelWorld,
    /// Block ids the player can walk through (air is always passable).
    pub passable: &'a [crate::voxel::BlockId],
    /// Block ids that can be climbed.
    pub ladders: &'a [crate::voxel::BlockId],
    /// Block ids that count as water.
    pub water: &'a [crate::voxel::BlockId],
}

impl<'a> WorldSolid<'a> {
    /// Creates a solid test for a world with no ladders or water.
    pub fn new(world: &'a VoxelWorld, passable: &'a [crate::voxel::BlockId]) -> Self {
        Self { world, passable, ladders: &[], water: &[] }
    }

    /// Creates a solid test with ladder and water blocks classified.
    pub fn with_environment(
        world: &'a VoxelWorld,
        passable: &'a [crate::voxel::BlockId],
        ladders: &'a [crate::voxel::BlockId],
        water: &'a [crate::voxel::BlockId],
    ) -> Self {
        Self { world, passable, ladders, water }
    }
}

impl SolidTest for WorldSolid<'_> {
    fn is_solid(&self, pos: BlockPos) -> bool {
        if !pos.in_world() {
            return false;
        }
        let block = self.world.block(pos);
        block != crate::voxel::AIR && !self.passable.contains(&block)
    }

    fn is_ladder(&self, pos: BlockPos) -> bool {
        let block = self.world.block(pos);
        block != crate::voxel::AIR && self.ladders.contains(&block)
    }

    fn is_water(&self, pos: BlockPos) -> bool {
        let block = self.world.block(pos);
        block != crate::voxel::AIR && self.water.contains(&block)
    }
}

/// True if the player's box at this position overlaps any solid block.
pub fn collides(solid: &impl SolidTest, feet: Vec3, height: f32) -> bool {
    let aabb = Aabb::from_feet(feet, height);
    let (min, max) = aabb.block_bounds();
    for y in min.y..=max.y {
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                if solid.is_solid(BlockPos::new(x, y, z))
                    && aabb.intersects_block(BlockPos::new(x, y, z))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Advances a player by one tick.
///
/// This is the whole movement rule set: gravity, jumping, sprinting, crouching, ladders and
/// AABB collision, in that order. Both the client (prediction) and the server (authority)
/// call exactly this function with exactly the same inputs and timestep.
pub fn step_player(
    world: &VoxelWorld,
    physics: &BlockPhysics,
    state: &mut PlayerState,
    input: MoveInput,
    dt: f32,
    tuning: &MoveTuning,
) {
    let solid =
        WorldSolid::with_environment(world, &physics.passable, &physics.ladders, &physics.water);
    step_player_with(&solid, state, input, dt, tuning);
}

/// [`step_player`] against an arbitrary solid test, so tests can use fake worlds.
pub fn step_player_with(
    solid: &impl SolidTest,
    state: &mut PlayerState,
    input: MoveInput,
    dt: f32,
    tuning: &MoveTuning,
) {
    let input = input.sanitized();
    state.yaw = input.yaw;
    state.pitch = input.pitch;
    state.crouching = input.crouch;

    // ---- environment probes
    let feet_block = BlockPos::new(
        state.position.x.floor() as i32,
        state.position.y.floor() as i32,
        state.position.z.floor() as i32,
    );
    state.on_ladder = is_ladder(solid, feet_block);
    state.submerged = is_water(solid, feet_block);
    state.on_ground = collides(solid, state.position - Vec3::Y * 0.02, state.height());

    // ---- horizontal target velocity in world space
    let (sin_yaw, cos_yaw) = state.yaw.sin_cos();
    let forward = Vec3::new(-sin_yaw, 0.0, -cos_yaw);
    let right = Vec3::new(cos_yaw, 0.0, -sin_yaw);
    let mut target = forward * input.forward + right * input.right;
    let mut speed = tuning.walk_speed;
    if input.sprint && input.forward > 0.0 {
        speed *= tuning.sprint_multiplier;
    }
    if state.crouching {
        speed *= tuning.crouch_multiplier;
    }
    if state.submerged {
        speed *= 0.7;
    }
    target *= speed;

    let accel =
        if state.on_ground || state.on_ladder { tuning.ground_accel } else { tuning.air_accel };
    let blend = (accel * dt).clamp(0.0, 1.0);
    state.velocity.x += (target.x - state.velocity.x) * blend;
    state.velocity.z += (target.z - state.velocity.z) * blend;

    // ---- vertical: ladders, jumping, gravity
    if state.on_ladder {
        let climb = if input.jump {
            tuning.climb_speed
        } else if input.forward > 0.0 {
            tuning.climb_speed * 0.75
        } else {
            -tuning.climb_speed * 0.4
        };
        state.velocity.y = climb;
    } else {
        if input.jump && state.on_ground {
            state.velocity.y = tuning.jump_velocity;
        }
        state.velocity.y -= tuning.gravity * dt;
        if state.velocity.y < -tuning.terminal_velocity {
            state.velocity.y = -tuning.terminal_velocity;
        }
    }

    // ---- integrate, one axis at a time
    let height = state.height();
    move_axis(solid, state, Vec3::X, state.velocity.x * dt, height);
    move_axis(solid, state, Vec3::Z, state.velocity.z * dt, height);
    let vertical = state.velocity.y * dt;
    if !move_axis(solid, state, Vec3::Y, vertical, height) {
        // Blocked vertically: stop moving that way, and landing means we are grounded.
        if vertical < 0.0 {
            state.on_ground = true;
        }
        state.velocity.y = 0.0;
    }

    // ---- keep the player inside the world
    let max_x = (crate::voxel::WORLD_SIZE_X - 1) as f32;
    let max_z = (crate::voxel::WORLD_SIZE_Z - 1) as f32;
    state.position.x = state.position.x.clamp(0.0, max_x);
    state.position.z = state.position.z.clamp(0.0, max_z);
    if state.position.y < 0.0 {
        state.position.y = 0.0;
        state.velocity.y = 0.0;
    }
}

/// Moves the player along one axis and resolves any collision on that axis.
///
/// Returns `true` if the move was unobstructed.
fn move_axis(
    solid: &impl SolidTest,
    state: &mut PlayerState,
    axis: Vec3,
    amount: f32,
    height: f32,
) -> bool {
    if amount == 0.0 {
        return true;
    }
    let before = state.position;
    state.position += axis * amount;

    if !collides(solid, state.position, height) {
        return true;
    }

    // Step the position back to just touching the blocking surface. Sub-stepping makes this
    // exact regardless of speed, which matters for fast falls.
    let steps = amount.abs().max(1.0).ceil() as i32 * 4;
    let increment = amount / steps as f32;
    state.position = before;
    for _ in 0..steps.abs() {
        let candidate = state.position + axis * increment;
        if collides(solid, candidate, height) {
            break;
        }
        state.position = candidate;
    }
    false
}

/// True if the player's feet or chest are in a ladder block.
fn is_ladder(solid: &impl SolidTest, feet: BlockPos) -> bool {
    solid.is_ladder(feet) || solid.is_ladder(feet.offset(0, 1, 0))
}

/// True if the player's head is under water.
fn is_water(solid: &impl SolidTest, feet: BlockPos) -> bool {
    solid.is_water(feet) || solid.is_water(feet.offset(0, 1, 0))
}

/// Snap a player to the ground under them, used on spawn.
pub fn settle_on_ground(world: &VoxelWorld, physics: &BlockPhysics, feet: Vec3) -> Vec3 {
    let solid =
        WorldSolid::with_environment(world, &physics.passable, &physics.ladders, &physics.water);
    let mut y = feet.y;

    // Drop until the player is standing on something: the box is free at `y` and blocked a
    // hair below it. If there is nothing underneath at all, rest on the world floor.
    while y > 0.0 {
        let free_here = !collides(&solid, Vec3::new(feet.x, y, feet.z), PLAYER_HEIGHT);
        let blocked_below = collides(&solid, Vec3::new(feet.x, y - 0.05, feet.z), PLAYER_HEIGHT);
        if free_here && blocked_below {
            return Vec3::new(feet.x, y, feet.z);
        }
        y -= 0.05;
    }
    Vec3::new(feet.x, 0.0, feet.z)
}

/// Converts a yaw/pitch pair to the unit look vector (shared with the client camera).
pub fn look_vector(yaw: f32, pitch: f32) -> Vec3 {
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let (sin_pitch, cos_pitch) = pitch.sin_cos();
    Vec3::new(-sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch)
}

/// Returns the yaw/pitch that points along a direction.
pub fn look_angles(direction: Vec3) -> (f32, f32) {
    let pitch = direction.y.clamp(-1.0, 1.0).asin();
    let yaw = (-direction.x).atan2(-direction.z);
    (yaw, pitch)
}

/// Horizontal velocity length, for stamina and footstep effects.
pub fn horizontal_speed(velocity: Vec3) -> f32 {
    Vec2::new(velocity.x, velocity.z).length()
}

/// True if a chunk coordinate is the one containing this world position.
pub fn chunk_of(x: f32, y: f32, z: f32) -> crate::voxel::ChunkPos {
    crate::voxel::ChunkPos::new(
        (x.floor() as i32).div_euclid(CHUNK_SIZE),
        (y.floor() as i32).div_euclid(CHUNK_SIZE),
        (z.floor() as i32).div_euclid(CHUNK_SIZE),
    )
}

#[cfg(test)]
mod tests;
