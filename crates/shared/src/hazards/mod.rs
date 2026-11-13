//! Player vitals and hazards: health, stamina, oxygen, body heat and radiation dose.
//!
//! The brief's numbers, in one place:
//!
//! | Hazard | Effect | Counter |
//! |---|---|---|
//! | Darkness | no visibility | lamps |
//! | Flooding | oxygen drains under water | pumps, sealing |
//! | Gas | oxygen drains, ignites on open flame | respirator, vent fan |
//! | Heat | body heat climbs, health drains above the limit | cooling suit |
//! | Cave-ins | unsupported spans collapse | supports |
//! | Radiation | dose accumulates; 100 grain, 300 no regen, 600 damage | hazmat suit, lead |
//!
//! Everything here is pure maths over a `Vitals` struct, so the server can apply it every tick
//! and the client can show the same bars.

use serde::{Deserialize, Serialize};

use crate::content::registry::Registry;

/// Radiation dose at which the screen starts to grain.
pub const DOSE_GRAIN: f32 = 100.0;
/// Radiation dose at which health regeneration stops.
pub const DOSE_NO_REGEN: f32 = 300.0;
/// Radiation dose at which the player takes damage over time.
pub const DOSE_DAMAGE: f32 = 600.0;
/// Radiation dose at which the player dies.
pub const DOSE_LETHAL: f32 = 1200.0;

/// Body heat above which the player starts losing health.
pub const HEAT_LIMIT: f32 = 45.0;
/// Comfortable body temperature.
pub const HEAT_COMFORT: f32 = 37.0;

/// A player's vitals.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vitals {
    /// Health, 0..100.
    pub health: f32,
    /// Stamina, 0..100.
    pub stamina: f32,
    /// Oxygen, 0..100.
    pub oxygen: f32,
    /// Body temperature in Celsius.
    pub heat: f32,
    /// Accumulated radiation dose.
    pub dose: f32,
}

impl Default for Vitals {
    fn default() -> Self {
        Self { health: 100.0, stamina: 100.0, oxygen: 100.0, heat: HEAT_COMFORT, dose: 0.0 }
    }
}

impl Vitals {
    /// True if the player is dead.
    pub fn is_dead(&self) -> bool {
        self.health <= 0.0 || self.dose >= DOSE_LETHAL
    }

    /// Screen grain amount, 0..1, from the radiation dose.
    pub fn radiation_grain(&self) -> f32 {
        ((self.dose - DOSE_GRAIN) / (DOSE_DAMAGE - DOSE_GRAIN)).clamp(0.0, 1.0)
    }

    /// True if health regeneration is blocked by dose.
    pub fn regen_blocked(&self) -> bool {
        self.dose >= DOSE_NO_REGEN
    }

    /// How the Geiger counter should click, 0..1 (faster near uranium and with a higher dose).
    pub fn geiger_rate(&self, nearby_radiation: f32) -> f32 {
        let ambient = (nearby_radiation / 2.0).clamp(0.0, 1.0);
        let body = (self.dose / DOSE_DAMAGE).clamp(0.0, 1.0);
        (ambient * 0.7 + body * 0.3).clamp(0.0, 1.0)
    }
}

/// What the player is wearing, resolved into multipliers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Protection {
    /// Fraction of radiation absorbed, 0..1.
    pub radiation_resist: f32,
    /// Fraction of heat absorbed, 0..1.
    pub heat_resist: f32,
    /// True if gas does not drain oxygen.
    pub gas_proof: bool,
    /// True if water does not drain oxygen.
    pub water_proof: bool,
}

impl Protection {
    /// Resolves protection from a set of worn items.
    pub fn from_worn(registry: &Registry, worn: &[crate::content::registry::ItemId]) -> Self {
        let mut protection = Protection::default();
        for item in worn {
            if let Some(wearable) = registry.wearable(*item) {
                // Take the best protection of each kind rather than summing, so stacking
                // suits cannot make a player invulnerable.
                protection.radiation_resist =
                    protection.radiation_resist.max(wearable.radiation_resist);
                protection.heat_resist = protection.heat_resist.max(wearable.heat_resist);
                protection.gas_proof |= wearable.gas_proof;
                protection.water_proof |= wearable.water_proof;
            }
        }
        protection
    }
}

/// The environment a player is in this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Environment {
    /// Ambient temperature in Celsius.
    pub ambient_temp: f32,
    /// True if the head is under water.
    pub submerged: bool,
    /// True if the air is methane.
    pub in_gas: bool,
    /// Radiation dose per second from nearby blocks and waste.
    pub ambient_radiation: f32,
    /// True if the player is lit by a lamp or daylight.
    pub lit: bool,
    /// True if the player is moving (stamina drain).
    pub moving: bool,
    /// True if the player is sprinting.
    pub sprinting: bool,
}

impl Environment {
    /// A comfortable surface environment.
    pub fn surface() -> Self {
        Self { ambient_temp: 18.0, lit: true, ..Default::default() }
    }
}

/// Advances a player's vitals by `dt` seconds.
pub fn tick_vitals(
    vitals: &mut Vitals,
    environment: &Environment,
    protection: &Protection,
    dt: f32,
) {
    // ---- oxygen
    if environment.in_gas && !protection.gas_proof {
        vitals.oxygen = (vitals.oxygen - 25.0 * dt).max(0.0);
    } else if environment.submerged && !protection.water_proof {
        vitals.oxygen = (vitals.oxygen - 12.0 * dt).max(0.0);
    } else {
        vitals.oxygen = (vitals.oxygen + 30.0 * dt).min(100.0);
    }

    // ---- stamina
    if environment.sprinting {
        vitals.stamina = (vitals.stamina - 18.0 * dt).max(0.0);
    } else if environment.moving {
        vitals.stamina = (vitals.stamina - 4.0 * dt).max(0.0);
    } else {
        vitals.stamina = (vitals.stamina + 12.0 * dt).min(100.0);
    }

    // ---- heat: drifts towards the ambient temperature, resisted by gear
    let effective_ambient =
        environment.ambient_temp - environment.ambient_temp * protection.heat_resist;
    let drift = (effective_ambient - vitals.heat) * 0.05 * dt;
    vitals.heat += drift;

    // ---- radiation
    let dose = environment.ambient_radiation * (1.0 - protection.radiation_resist);
    vitals.dose = (vitals.dose + dose * dt).max(0.0);

    // ---- damage
    let mut damage = 0.0;
    if vitals.oxygen <= 0.0 {
        damage += 8.0 * dt;
    }
    if vitals.heat > HEAT_LIMIT {
        damage += (vitals.heat - HEAT_LIMIT) * 0.6 * dt;
    }
    if vitals.dose >= DOSE_DAMAGE {
        damage += (vitals.dose - DOSE_DAMAGE) * 0.02 * dt + 1.0 * dt;
    }
    if damage > 0.0 {
        vitals.health = (vitals.health - damage).max(0.0);
    } else if !vitals.regen_blocked() && vitals.oxygen > 20.0 {
        // Slow regeneration when safe.
        vitals.health = (vitals.health + 1.5 * dt).min(100.0);
    }
}

/// Clears radiation dose (the Decon Shower).
pub fn decontaminate(vitals: &mut Vitals, amount: f32) {
    vitals.dose = (vitals.dose - amount).max(0.0);
}

/// The radiation dose a block position emits at a given distance.
pub fn block_radiation(registry: &Registry, block: crate::voxel::BlockId) -> Option<(f32, f32)> {
    registry.block(block).and_then(|entry| entry.def.radiation)
}

/// Total radiation dose per second from the blocks around a position.
///
/// Only blocks that are actually exposed contribute, so uranium buried behind rock is safe to
/// walk past; the Geiger counter is what tells you it is there.
pub fn ambient_radiation(
    registry: &Registry,
    world: &crate::voxel::VoxelWorld,
    position: bevy::math::Vec3,
    radius: i32,
) -> f32 {
    let mut total = 0.0;
    let origin = crate::voxel::BlockPos::new(
        position.x.floor() as i32,
        position.y.floor() as i32,
        position.z.floor() as i32,
    );
    for dy in -radius..=radius {
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let pos = origin.offset(dx, dy, dz);
                if !pos.in_world() {
                    continue;
                }
                let block = world.block(pos);
                let Some((block_radius, dose)) = block_radiation(registry, block) else {
                    continue;
                };
                // Exposed: at least one neighbour is air.
                let exposed = pos
                    .neighbours()
                    .iter()
                    .any(|neighbour| world.block(*neighbour) == crate::voxel::AIR);
                if !exposed {
                    continue;
                }
                let distance = pos.center_f32().distance(position);
                if distance > block_radius {
                    continue;
                }
                // Falloff with distance: full dose at the block, none at the radius.
                let falloff = 1.0 - (distance / block_radius).clamp(0.0, 1.0);
                total += dose * falloff;
            }
        }
    }
    total
}

/// A hazard zone left behind by a reactor meltdown.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RadiationZone {
    /// Centre of the zone.
    pub center: [i32; 3],
    /// Radius in blocks.
    pub radius: f32,
    /// Dose per second at the centre.
    pub dose_per_sec: f32,
    /// Seconds until the zone decays.
    pub remaining: f32,
}

impl RadiationZone {
    /// Dose at a position, zero outside the radius.
    pub fn dose_at(&self, position: bevy::math::Vec3) -> f32 {
        if self.remaining <= 0.0 {
            return 0.0;
        }
        let center = bevy::math::Vec3::new(
            self.center[0] as f32,
            self.center[1] as f32,
            self.center[2] as f32,
        );
        let distance = center.distance(position);
        if distance > self.radius {
            return 0.0;
        }
        self.dose_per_sec * (1.0 - distance / self.radius)
    }

    /// Ages the zone.
    pub fn tick(&mut self, dt: f32) {
        self.remaining = (self.remaining - dt).max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::load_registry;

    #[test]
    fn a_healthy_player_in_a_safe_environment_stays_healthy() {
        let mut vitals = Vitals::default();
        for _ in 0..100 {
            tick_vitals(&mut vitals, &Environment::surface(), &Protection::default(), 0.1);
        }
        assert_eq!(vitals.health, 100.0);
        assert_eq!(vitals.oxygen, 100.0);
        assert!(vitals.dose < 1.0);
    }

    #[test]
    fn being_underwater_drains_oxygen_and_then_health() {
        let mut vitals = Vitals::default();
        let environment = Environment { submerged: true, ..Environment::surface() };
        // 10 seconds underwater: oxygen runs out.
        for _ in 0..100 {
            tick_vitals(&mut vitals, &environment, &Protection::default(), 0.1);
        }
        assert!(vitals.oxygen < 1.0, "oxygen should be exhausted, got {}", vitals.oxygen);
        // Keep going: health drains.
        for _ in 0..100 {
            tick_vitals(&mut vitals, &environment, &Protection::default(), 0.1);
        }
        assert!(vitals.health < 100.0, "health should drain without oxygen");
    }

    #[test]
    fn a_water_proof_suit_prevents_drowning() {
        let mut vitals = Vitals::default();
        let environment = Environment { submerged: true, ..Environment::surface() };
        let protection = Protection { water_proof: true, ..Default::default() };
        for _ in 0..200 {
            tick_vitals(&mut vitals, &environment, &protection, 0.1);
        }
        assert_eq!(vitals.oxygen, 100.0);
        assert_eq!(vitals.health, 100.0);
    }

    #[test]
    fn methane_drains_oxygen_unless_the_player_has_a_respirator() {
        let mut without = Vitals::default();
        let mut with = Vitals::default();
        let gas = Environment { in_gas: true, ..Environment::surface() };
        for _ in 0..50 {
            tick_vitals(&mut without, &gas, &Protection::default(), 0.1);
            tick_vitals(
                &mut with,
                &gas,
                &Protection { gas_proof: true, ..Default::default() },
                0.1,
            );
        }
        assert!(without.oxygen < 100.0);
        assert_eq!(with.oxygen, 100.0);
    }

    #[test]
    fn heat_above_the_limit_drains_health() {
        let mut vitals = Vitals::default();
        let hot = Environment { ambient_temp: 120.0, ..Environment::surface() };
        for _ in 0..400 {
            tick_vitals(&mut vitals, &hot, &Protection::default(), 0.1);
        }
        assert!(vitals.heat > HEAT_LIMIT, "body heat should climb, got {}", vitals.heat);
        assert!(vitals.health < 100.0, "heat should cost health");
    }

    #[test]
    fn a_cooling_suit_slows_the_heat_build_up() {
        let mut bare = Vitals::default();
        let mut suited = Vitals::default();
        let hot = Environment { ambient_temp: 120.0, ..Environment::surface() };
        let suit = Protection { heat_resist: 0.7, ..Default::default() };
        for _ in 0..400 {
            tick_vitals(&mut bare, &hot, &Protection::default(), 0.1);
            tick_vitals(&mut suited, &hot, &suit, 0.1);
        }
        assert!(suited.heat < bare.heat, "the cooling suit should keep the player cooler");
    }

    #[test]
    fn radiation_accumulates_and_hazmat_reduces_it() {
        let mut bare = Vitals::default();
        let mut suited = Vitals::default();
        let hot = Environment { ambient_radiation: 2.0, ..Environment::surface() };
        let hazmat = Protection { radiation_resist: 0.8, ..Default::default() };
        for _ in 0..100 {
            tick_vitals(&mut bare, &hot, &Protection::default(), 0.1);
            tick_vitals(&mut suited, &hot, &hazmat, 0.1);
        }
        assert!((bare.dose - 20.0).abs() < 0.5, "2 rad/s for 10 s is 20, got {}", bare.dose);
        assert!(suited.dose < bare.dose * 0.3, "hazmat absorbs most of the dose");
    }

    #[test]
    fn high_dose_blocks_regeneration_and_then_causes_damage() {
        let mut vitals = Vitals { dose: DOSE_NO_REGEN + 1.0, health: 80.0, ..Vitals::default() };
        assert!(vitals.regen_blocked());
        for _ in 0..20 {
            tick_vitals(&mut vitals, &Environment::surface(), &Protection::default(), 0.1);
        }
        assert!(vitals.health <= 80.0, "no regeneration at high dose");

        vitals.dose = DOSE_DAMAGE + 100.0;
        for _ in 0..20 {
            tick_vitals(&mut vitals, &Environment::surface(), &Protection::default(), 0.1);
        }
        assert!(vitals.health < 80.0, "dose above the damage line costs health");
    }

    #[test]
    fn decon_shower_clears_dose() {
        let mut vitals = Vitals { dose: 500.0, ..Vitals::default() };
        decontaminate(&mut vitals, 200.0);
        assert_eq!(vitals.dose, 300.0);
        decontaminate(&mut vitals, 1000.0);
        assert_eq!(vitals.dose, 0.0);
    }

    #[test]
    fn protection_takes_the_best_of_each_kind() {
        let registry = load_registry().expect("content");
        let hazmat = registry.item_id("hazmat_suit").expect("hazmat");
        let cooling = registry.item_id("cooling_suit").expect("cooling suit");
        let protection = Protection::from_worn(&registry, &[hazmat, cooling]);
        assert!((protection.radiation_resist - 0.8).abs() < 1e-6);
        assert!((protection.heat_resist - 0.7).abs() < 1e-6);
        assert!(protection.gas_proof);
    }

    #[test]
    fn uranium_blocks_emit_radiation_and_buried_ones_do_not() {
        let registry = load_registry().expect("content");
        let uranium = registry.block_id("pitchblende").expect("pitchblende");
        let mut world = crate::voxel::VoxelWorld::new();
        let chunk = crate::voxel::ChunkPos::new(0, 0, 0);
        world.insert(chunk, crate::voxel::Chunk::filled(1));
        let pos = crate::voxel::BlockPos::new(5, 5, 5);
        // Buried: no radiation reaches the player.
        let dose = ambient_radiation(&registry, &world, pos.center_f32(), 4);
        assert_eq!(dose, 0.0, "buried ore does not irradiate the tunnel");

        // Exposed: it does.
        world.set_block(pos, uranium);
        world.set_block(crate::voxel::BlockPos::new(4, 5, 5), crate::voxel::AIR);
        let dose = ambient_radiation(&registry, &world, pos.center_f32(), 4);
        assert!(dose > 0.0, "exposed uranium should irradiate the player");
    }

    #[test]
    fn radiation_zones_decay() {
        let mut zone = RadiationZone {
            center: [0, 64, 0],
            radius: 20.0,
            dose_per_sec: 1.0,
            remaining: 1200.0,
        };
        let at_center = bevy::math::Vec3::new(0.5, 64.5, 0.5);
        assert!(zone.dose_at(at_center) > 0.9);
        let outside = bevy::math::Vec3::new(40.0, 64.0, 0.0);
        assert_eq!(zone.dose_at(outside), 0.0);
        // 20 minutes later (1200 s) it is gone.
        for _ in 0..12_050 {
            zone.tick(0.1);
        }
        assert_eq!(zone.remaining, 0.0, "the zone decays after its lifetime");
        assert_eq!(zone.dose_at(at_center), 0.0);
    }

    #[test]
    fn geiger_rate_rises_near_uranium() {
        let quiet = Vitals::default();
        let near = Vitals::default();
        assert!(near.geiger_rate(2.0) > quiet.geiger_rate(0.0));
        let irradiated = Vitals { dose: 600.0, ..Vitals::default() };
        assert!(irradiated.geiger_rate(0.0) > quiet.geiger_rate(0.0));
    }
}
