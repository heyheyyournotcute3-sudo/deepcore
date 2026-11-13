//! Nuclear machinery: centrifuge cascades, reactor state and the core-cell drill.
//!
//! The brief describes the chain as a sequence of structures that take items in, wait, and put
//! items out. Two of them need real simulation on top of that:
//!
//! * **Centrifuge cascades** — a line of 4 to 16 linked centrifuges enriches faster the longer
//!   it is.
//! * **The reactor** — core temperature rises as control rods are withdrawn and falls with
//!   coolant flow; above the warning line it alarms, above the limit it scrams (if a scram panel
//!   is built) or melts down.

use serde::{Deserialize, Serialize};

/// Minimum and maximum centrifuges in a cascade, from the brief.
pub const CASCADE_MIN: u8 = 4;
/// Maximum centrifuges in a cascade.
pub const CASCADE_MAX: u8 = 16;

/// The enrichment speed multiplier of a cascade of `count` centrifuges.
///
/// A cascade below the minimum does nothing; above it, each additional centrifuge adds a
/// linear 25% on top of the first four, so a full 16-machine cascade is four times as fast as
/// the minimum.
pub fn cascade_speed(count: u8) -> f32 {
    if count < CASCADE_MIN {
        return 0.0;
    }
    let extra = u32::from(count.min(CASCADE_MAX) - CASCADE_MIN);
    let base = 1.0;
    base + extra as f32 * 0.25
}

/// True if a cascade of this length can enrich at all.
pub fn cascade_valid(count: u8) -> bool {
    (CASCADE_MIN..=CASCADE_MAX).contains(&count)
}

/// The reactor's operating state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReactorState {
    /// Not running: rods fully inserted.
    Idle,
    /// Running normally.
    Running,
    /// Above the warning line: the alarm sounds.
    Warning,
    /// Scrammed: rods fully inserted automatically.
    Scrammed,
    /// Melted down: the reactor is destroyed and left a radiation zone.
    Meltdown,
}

/// Reactor tuning.
#[derive(Clone, Copy, Debug)]
pub struct ReactorTuning {
    /// Temperature at which the alarm sounds.
    pub warning_temp: f32,
    /// Temperature above which the reactor scrams or melts down.
    pub limit_temp: f32,
    /// Temperature gained per second at full rod withdrawal with no coolant.
    pub heat_rate: f32,
    /// Temperature lost per second at full coolant flow with rods inserted.
    pub cooling_rate: f32,
    /// Power output at full rod withdrawal, in kW.
    pub max_power_kw: f32,
}

impl Default for ReactorTuning {
    fn default() -> Self {
        Self {
            warning_temp: 900.0,
            limit_temp: 1200.0,
            // Heat at full withdrawal with 4 fuel rods is 2x heat_rate; full coolant removes
            // cooling_rate. A reactor at 70% rods with 90% coolant must sit well below the
            // warning line, so cooling outpaces heat at sane operating points.
            heat_rate: 22.0,
            cooling_rate: 45.0,
            max_power_kw: 20.0 * 100.0,
        }
    }
}

/// The reactor core.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reactor {
    /// Core temperature in Celsius.
    pub temperature: f32,
    /// How far the control rods are withdrawn, 0 (in) to 1 (out).
    pub rod_withdrawal: f32,
    /// Coolant flow, 0..1.
    pub coolant_flow: f32,
    /// Fuel rods loaded.
    pub fuel_rods: u32,
    /// Control rods installed.
    pub control_rods: u32,
    /// True if a scram panel is part of the build.
    pub has_scram_panel: bool,
    /// Current state.
    pub state: ReactorState,
    /// True when the state changed this tick (for the alarm).
    pub alarm: bool,
}

impl Default for Reactor {
    fn default() -> Self {
        Self {
            temperature: 20.0,
            rod_withdrawal: 0.0,
            coolant_flow: 0.0,
            fuel_rods: 0,
            control_rods: 0,
            has_scram_panel: false,
            state: ReactorState::Idle,
            alarm: false,
        }
    }
}

impl Reactor {
    /// Creates a reactor from its built parts.
    pub fn new(fuel_rods: u32, control_rods: u32, has_scram_panel: bool) -> Self {
        Self { fuel_rods, control_rods, has_scram_panel, ..Default::default() }
    }

    /// True if the reactor has fuel and enough control rods to be operated.
    pub fn is_built(&self) -> bool {
        self.fuel_rods > 0 && self.control_rods > 0
    }

    /// Power output right now, in kW.
    pub fn power_kw(&self, tuning: &ReactorTuning) -> f32 {
        if !matches!(self.state, ReactorState::Running | ReactorState::Warning) {
            return 0.0;
        }
        let fuel_factor = (self.fuel_rods as f32 / 4.0).clamp(0.0, 1.5);
        tuning.max_power_kw * self.rod_withdrawal * fuel_factor
    }

    /// Advances the core by `dt` seconds.
    ///
    /// Returns true if the state changed (so the caller can play an alarm or spawn a zone).
    pub fn tick(&mut self, tuning: &ReactorTuning, dt: f32) -> bool {
        let previous = self.state;
        self.alarm = false;

        if self.state == ReactorState::Meltdown {
            return false;
        }
        if !self.is_built() {
            self.state = ReactorState::Idle;
            self.cool_towards_ambient(tuning, dt);
            return previous != self.state;
        }

        // Heat from fission minus cooling from the coolant loop.
        let heat_in = tuning.heat_rate * self.rod_withdrawal * (self.fuel_rods as f32).sqrt();
        let heat_out = tuning.cooling_rate * self.coolant_flow;
        self.temperature = (self.temperature + (heat_in - heat_out) * dt).max(20.0);

        // State machine.
        self.state = if self.temperature >= tuning.limit_temp {
            if self.has_scram_panel {
                // The scram panel fires: rods slam in, the core cools.
                self.rod_withdrawal = 0.0;
                ReactorState::Scrammed
            } else {
                ReactorState::Meltdown
            }
        } else if self.temperature >= tuning.warning_temp {
            ReactorState::Warning
        } else if self.rod_withdrawal > 0.0 {
            ReactorState::Running
        } else {
            ReactorState::Idle
        };

        // A scrammed reactor stays scrammed until the player resets it.
        if previous == ReactorState::Scrammed && self.state != ReactorState::Scrammed {
            self.state = ReactorState::Scrammed;
        }

        if self.state == ReactorState::Warning || self.state == ReactorState::Meltdown {
            self.alarm = true;
        }
        previous != self.state
    }

    fn cool_towards_ambient(&mut self, tuning: &ReactorTuning, dt: f32) {
        let ambient = 20.0;
        if self.temperature > ambient {
            self.temperature = (self.temperature - tuning.cooling_rate * dt).max(ambient);
        }
    }

    /// Withdraws or inserts the control rods.
    pub fn set_rod_withdrawal(&mut self, amount: f32) {
        if self.state == ReactorState::Scrammed || self.state == ReactorState::Meltdown {
            return;
        }
        self.rod_withdrawal = amount.clamp(0.0, 1.0);
    }

    /// Sets the coolant flow.
    pub fn set_coolant_flow(&mut self, amount: f32) {
        self.coolant_flow = amount.clamp(0.0, 1.0);
    }

    /// Manually scrams the reactor (the console's emergency button).
    pub fn scram(&mut self) {
        self.rod_withdrawal = 0.0;
        self.state = ReactorState::Scrammed;
    }

    /// Resets a scrammed reactor so it can be started again, once it has cooled.
    pub fn reset_scram(&mut self, tuning: &ReactorTuning) -> bool {
        if self.state == ReactorState::Scrammed && self.temperature < tuning.warning_temp * 0.5 {
            self.state = ReactorState::Idle;
            return true;
        }
        false
    }
}

/// The Nuclear Drill's core cell: how many breaks remain.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CoreCell {
    /// Block breaks remaining.
    pub breaks_remaining: u32,
    /// Total breaks when full.
    pub capacity: u32,
}

impl CoreCell {
    /// A fresh cell.
    pub fn new(capacity: u32) -> Self {
        Self { breaks_remaining: capacity, capacity }
    }

    /// Spends breaks. Returns how many were actually spent.
    pub fn spend(&mut self, breaks: u32) -> u32 {
        let spent = breaks.min(self.breaks_remaining);
        self.breaks_remaining -= spent;
        spent
    }

    /// True when the cell is spent and must be returned to a cooling pool.
    pub fn is_spent(&self) -> bool {
        self.breaks_remaining == 0
    }

    /// Charge fraction, 0..1.
    pub fn fraction(&self) -> f32 {
        if self.capacity == 0 { 0.0 } else { self.breaks_remaining as f32 / self.capacity as f32 }
    }
}

/// A 5x5x5 reactor shell: how many casing blocks and what must be inside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactorLayout {
    /// Casing blocks in the shell.
    pub casing: u32,
    /// Moderator blocks inside.
    pub moderator: u32,
    /// Fuel rods inside.
    pub fuel_rods: u32,
    /// Control rods inside.
    pub control_rods: u32,
}

impl ReactorLayout {
    /// The layout required to build a working reactor.
    pub fn required() -> Self {
        // A 5x5x5 shell is 98 casing blocks (125 minus the 27-block interior).
        Self { casing: 98, moderator: 12, fuel_rods: 4, control_rods: 4 }
    }

    /// True if the build meets the requirement.
    pub fn is_complete(&self) -> bool {
        let required = Self::required();
        self.casing >= required.casing
            && self.moderator >= required.moderator
            && self.fuel_rods >= required.fuel_rods
            && self.control_rods >= required.control_rods
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cascade_below_the_minimum_does_nothing() {
        assert_eq!(cascade_speed(0), 0.0);
        assert_eq!(cascade_speed(3), 0.0);
        assert!(!cascade_valid(3));
        assert!(cascade_valid(4));
    }

    #[test]
    fn a_longer_cascade_enriches_faster() {
        let four = cascade_speed(4);
        let sixteen = cascade_speed(16);
        assert!((four - 1.0).abs() < 1e-6);
        assert!(sixteen > four * 3.0, "16 centrifuges should be much faster than 4");
        assert!(!cascade_valid(17), "cascades are capped at 16");
    }

    #[test]
    fn reactor_withdrawing_rods_heats_the_core() {
        let tuning = ReactorTuning::default();
        let mut reactor = Reactor::new(4, 4, true);
        reactor.set_rod_withdrawal(1.0);
        for _ in 0..100 {
            reactor.tick(&tuning, 0.1);
        }
        assert!(reactor.temperature > 20.0, "the core should heat up");
        assert!(reactor.power_kw(&tuning) > 0.0);
    }

    #[test]
    fn coolant_holds_the_temperature_down() {
        let tuning = ReactorTuning::default();
        let mut hot = Reactor::new(4, 4, true);
        let mut cooled = Reactor::new(4, 4, true);
        hot.set_rod_withdrawal(1.0);
        cooled.set_rod_withdrawal(1.0);
        cooled.set_coolant_flow(1.0);
        for _ in 0..200 {
            hot.tick(&tuning, 0.1);
            cooled.tick(&tuning, 0.1);
        }
        assert!(cooled.temperature < hot.temperature, "coolant must keep the core cooler");
    }

    #[test]
    fn the_alarm_sounds_above_the_warning_line() {
        let tuning = ReactorTuning::default();
        let mut reactor = Reactor::new(4, 4, true);
        reactor.set_rod_withdrawal(1.0);
        let mut warned = false;
        for _ in 0..2000 {
            reactor.tick(&tuning, 0.1);
            if reactor.state == ReactorState::Warning {
                warned = true;
                assert!(reactor.alarm, "the warning state raises the alarm");
                break;
            }
        }
        assert!(warned, "the reactor should reach the warning line without coolant");
    }

    #[test]
    fn a_scram_panel_prevents_a_meltdown() {
        let tuning = ReactorTuning::default();
        let mut reactor = Reactor::new(4, 4, true);
        reactor.set_rod_withdrawal(1.0);
        for _ in 0..20_000 {
            reactor.tick(&tuning, 0.1);
        }
        assert_eq!(
            reactor.state,
            ReactorState::Scrammed,
            "the panel scrams instead of melting down"
        );
        assert_ne!(reactor.state, ReactorState::Meltdown);
    }

    #[test]
    fn without_a_scram_panel_the_reactor_melts_down() {
        let tuning = ReactorTuning::default();
        let mut reactor = Reactor::new(4, 4, false);
        reactor.set_rod_withdrawal(1.0);
        for _ in 0..20_000 {
            reactor.tick(&tuning, 0.1);
        }
        assert_eq!(reactor.state, ReactorState::Meltdown);
    }

    #[test]
    fn a_scrammed_reactor_can_be_reset_once_cool() {
        let tuning = ReactorTuning::default();
        let mut reactor = Reactor::new(4, 4, true);
        reactor.scram();
        assert_eq!(reactor.state, ReactorState::Scrammed);
        reactor.temperature = 100.0;
        assert!(reactor.reset_scram(&tuning));
        assert_eq!(reactor.state, ReactorState::Idle);
    }

    #[test]
    fn an_unbuilt_reactor_does_nothing() {
        let tuning = ReactorTuning::default();
        let mut reactor = Reactor::default();
        assert!(!reactor.is_built());
        reactor.set_rod_withdrawal(1.0);
        for _ in 0..100 {
            reactor.tick(&tuning, 0.1);
        }
        assert_eq!(reactor.state, ReactorState::Idle);
        assert_eq!(reactor.power_kw(&tuning), 0.0);
    }

    #[test]
    fn a_reactor_out_powers_the_coal_grid() {
        let tuning = ReactorTuning::default();
        let mut reactor = Reactor::new(4, 4, true);
        reactor.set_rod_withdrawal(1.0);
        reactor.state = ReactorState::Running;
        let reactor_kw = reactor.power_kw(&tuning);
        // The brief asks for about 20x a coal grid; a single generator is 60 kW.
        assert!(
            reactor_kw > 60.0 * 20.0,
            "reactor should be about 20x a generator, got {reactor_kw}"
        );
    }

    #[test]
    fn a_core_cell_runs_out_after_its_breaks() {
        let mut cell = CoreCell::new(2000);
        assert_eq!(cell.spend(1999), 1999);
        assert!(!cell.is_spent());
        assert_eq!(cell.spend(9), 1);
        assert!(cell.is_spent());
        assert_eq!(cell.fraction(), 0.0);
    }

    #[test]
    fn the_reactor_layout_requirement_is_the_5x5x5_shell() {
        let required = ReactorLayout::required();
        assert_eq!(required.casing, 98, "a 5x5x5 shell is 98 blocks");
        assert!(!ReactorLayout { casing: 50, ..required }.is_complete());
        assert!(
            ReactorLayout { casing: 98, moderator: 12, fuel_rods: 4, control_rods: 4 }
                .is_complete()
        );
    }
}
