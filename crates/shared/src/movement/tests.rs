//! Movement unit tests.
//!
//! These are the tests that protect multiplayer from desyncs: identical inputs must produce
//! identical positions, collisions must never let the player inside rock, and the predicted
//! client path must match the authoritative server path bit for bit.

use bevy::math::Vec3;

use super::*;
use crate::voxel::chunk::Chunk;
use crate::voxel::coords::{BlockPos, ChunkPos, LocalPos};
use crate::voxel::world::VoxelWorld;

const STONE: u16 = 1;

/// A world with a solid floor at y < 10 and air above.
fn floor_world() -> VoxelWorld {
    let mut world = VoxelWorld::new();
    for cy in 0..2 {
        let mut chunk = Chunk::filled(if cy == 0 { STONE } else { 0 });
        world.insert(ChunkPos::new(0, cy, 0), chunk.clone());
        if cy == 0 {
            // Make y = 10..32 air so the surface is exactly y = 10.
            for y in 10..32 {
                for z in 0..32 {
                    for x in 0..32 {
                        chunk.set(LocalPos::new(x, y, z), 0);
                    }
                }
            }
            world.insert(ChunkPos::new(0, 0, 0), chunk);
        }
    }
    world
}

fn step(world: &VoxelWorld, state: &mut PlayerState, input: MoveInput, ticks: usize) {
    let tuning = MoveTuning::default();
    let physics = BlockPhysics::default();
    for _ in 0..ticks {
        step_player(world, &physics, state, input, 1.0 / 30.0, &tuning);
    }
}

#[test]
fn player_falls_and_lands_on_the_floor() {
    let world = floor_world();
    let mut state = PlayerState::at(Vec3::new(16.0, 20.0, 16.0));
    step(&world, &mut state, MoveInput::default(), 120);
    assert!(state.on_ground, "player should be standing after falling");
    assert!(
        (state.position.y - 10.0).abs() < 0.2,
        "expected to rest at y=10, got {}",
        state.position.y
    );
    assert!(state.velocity.y.abs() < 0.01);
}

#[test]
fn player_never_ends_up_inside_rock() {
    let world = floor_world();
    let mut state = PlayerState::at(Vec3::new(16.0, 11.0, 16.0));
    let input = MoveInput { forward: 1.0, sprint: true, ..Default::default() };
    // Walk into the world for a long time; the floor must never be penetrated.
    step(&world, &mut state, input, 600);
    assert!(state.position.y >= 9.99, "player sank into the floor: {}", state.position.y);
}

#[test]
fn identical_inputs_produce_identical_positions() {
    let world = floor_world();
    let mut a = PlayerState::at(Vec3::new(16.0, 12.0, 16.0));
    let mut b = PlayerState::at(Vec3::new(16.0, 12.0, 16.0));
    let input = MoveInput {
        seq: 1,
        forward: 0.7,
        right: -0.3,
        jump: true,
        sprint: true,
        crouch: false,
        yaw: 0.9,
        pitch: 0.1,
    };
    step(&world, &mut a, input, 90);
    step(&world, &mut b, input, 90);
    assert_eq!(a.position, b.position);
    assert_eq!(a.velocity, b.velocity);
}

#[test]
fn diagonal_movement_is_not_faster_than_straight() {
    let world = floor_world();
    let mut straight = PlayerState::at(Vec3::new(16.0, 11.0, 16.0));
    let mut diagonal = PlayerState::at(Vec3::new(16.0, 11.0, 16.0));
    let forward = MoveInput { forward: 1.0, ..Default::default() };
    let both = MoveInput { forward: 1.0, right: 1.0, ..Default::default() };
    step(&world, &mut straight, forward, 30);
    step(&world, &mut diagonal, both, 30);
    let straight_distance = (straight.position - Vec3::new(16.0, 11.0, 16.0)).length();
    let diagonal_distance = (diagonal.position - Vec3::new(16.0, 11.0, 16.0)).length();
    assert!(
        (straight_distance - diagonal_distance).abs() < 0.6,
        "diagonal {diagonal_distance} vs straight {straight_distance}"
    );
}

#[test]
fn sprinting_is_faster_than_walking() {
    let world = floor_world();
    let mut walk = PlayerState::at(Vec3::new(16.0, 11.0, 16.0));
    let mut sprint = PlayerState::at(Vec3::new(16.0, 11.0, 16.0));
    let walk_input = MoveInput { forward: 1.0, ..Default::default() };
    let sprint_input = MoveInput { forward: 1.0, sprint: true, ..Default::default() };
    step(&world, &mut walk, walk_input, 60);
    step(&world, &mut sprint, sprint_input, 60);
    let walked = (walk.position - Vec3::new(16.0, 11.0, 16.0)).length();
    let sprinted = (sprint.position - Vec3::new(16.0, 11.0, 16.0)).length();
    assert!(sprinted > walked * 1.2, "sprint {sprinted} should beat walk {walked}");
}

#[test]
fn jumping_lifts_the_player_off_the_ground() {
    let world = floor_world();
    let mut state = PlayerState::at(Vec3::new(16.0, 10.0, 16.0));
    step(&world, &mut state, MoveInput::default(), 5);
    let ground = state.position.y;
    step(&world, &mut state, MoveInput { jump: true, ..Default::default() }, 8);
    assert!(state.position.y > ground + 0.5, "jump did not lift the player");
    // And comes back down.
    step(&world, &mut state, MoveInput::default(), 60);
    assert!((state.position.y - ground).abs() < 0.2);
}

#[test]
fn walls_stop_the_player_and_allow_sliding() {
    let mut world = floor_world();
    // Build a wall along x = 18 for all z.
    for z in 0..32 {
        for y in 10..14 {
            world.set_block(BlockPos::new(18, y, z), STONE);
        }
    }
    let mut state = PlayerState::at(Vec3::new(16.0, 10.0, 16.0));
    let input = MoveInput { forward: 0.0, right: 1.0, ..Default::default() };
    step(&world, &mut state, input, 120);
    assert!(state.position.x < 18.0, "player passed through the wall: {}", state.position.x);
    assert!(state.position.x > 17.0, "player should reach the wall, got {}", state.position.x);

    // Now walk diagonally into the wall: the player should slide along z.
    let mut slider = PlayerState::at(Vec3::new(17.0, 10.0, 16.0));
    let slide_input = MoveInput { forward: 1.0, right: 1.0, ..Default::default() };
    step(&world, &mut slider, slide_input, 60);
    assert!(
        (slider.position.z - 16.0).abs() > 1.0,
        "player should slide along the wall, z stayed at {}",
        slider.position.z
    );
}

#[test]
fn crouching_lowers_the_collision_height() {
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 0.0));
    assert_eq!(state.height(), PLAYER_HEIGHT);
    state.crouching = true;
    assert_eq!(state.height(), PLAYER_CROUCH_HEIGHT);
}

#[test]
fn look_direction_matches_yaw_and_pitch() {
    let mut state = PlayerState::at(Vec3::ZERO);
    state.yaw = 0.0;
    state.pitch = 0.0;
    let direction = state.look_direction();
    assert!((direction - Vec3::NEG_Z).length() < 1e-5, "default look should be -Z");

    state.yaw = std::f32::consts::FRAC_PI_2;
    let direction = state.look_direction();
    assert!((direction - Vec3::NEG_X).length() < 1e-5, "yaw 90deg should look along -X");

    let (yaw, pitch) = look_angles(Vec3::new(0.0, 1.0, 0.0));
    assert!((pitch - std::f32::consts::FRAC_PI_2).abs() < 1e-3);
    assert!(yaw.is_finite());
}

#[test]
fn input_sanitising_clamps_cheats() {
    let input =
        MoveInput { forward: 99.0, right: -42.0, pitch: 9.0, yaw: f32::NAN, ..Default::default() };
    let clean = input.sanitized();
    assert!(clean.forward <= 1.0 && clean.forward >= -1.0);
    assert!(clean.right <= 1.0 && clean.right >= -1.0);
    assert!(clean.pitch.abs() < std::f32::consts::FRAC_PI_2);
    assert_eq!(clean.yaw, 0.0);
}

#[test]
fn aabb_intersection_is_exact_at_boundaries() {
    let aabb = Aabb::from_feet(Vec3::new(1.5, 0.0, 1.5), 1.8);
    assert!(aabb.intersects_block(BlockPos::new(1, 0, 1)));
    assert!(aabb.intersects_block(BlockPos::new(1, 1, 1)));
    assert!(!aabb.intersects_block(BlockPos::new(2, 0, 1)), "touching is not overlapping");
    assert!(!aabb.intersects_block(BlockPos::new(1, 2, 1)));
}

#[test]
fn ladders_allow_climbing() {
    let mut world = floor_world();
    const LADDER: u16 = 2;
    for y in 10..22 {
        world.set_block(BlockPos::new(16, y, 16), LADDER);
    }
    let mut state = PlayerState::at(Vec3::new(16.0, 10.0, 16.0));
    let tuning = MoveTuning::default();
    let physics = BlockPhysics { passable: vec![LADDER], ladders: vec![LADDER], water: vec![] };
    for _ in 0..90 {
        step_player(
            &world,
            &physics,
            &mut state,
            MoveInput { jump: true, ..Default::default() },
            1.0 / 30.0,
            &tuning,
        );
    }
    assert!(state.position.y > 14.0, "player should climb the ladder, got y={}", state.position.y);
}

#[test]
fn settle_on_ground_finds_the_surface() {
    let world = floor_world();
    let settled = settle_on_ground(&world, &BlockPhysics::default(), Vec3::new(16.0, 25.0, 16.0));
    assert!((settled.y - 10.0).abs() < 0.15, "settled at {}", settled.y);
}
