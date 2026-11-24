//! The client half of vanilla `EnderDragon`: its synced phase, the wing
//! flap clock, `DragonFlightHistory`, which poses the neck and tail, and the
//! `EnderDragonPart`s the player hits it through.

use glam::DVec3;

use crate::entity::dimensions::EntityDimensions;
use crate::entity::wrap_degrees;
use crate::physics::aabb::Aabb;
use crate::physics::movement::{DEG_TO_RAD as DEG, mth_cos, mth_sin};

/// `EnderDragonPhase` ids.
const LANDING: i32 = 3;
const TAKEOFF: i32 = 4;
const SITTING_FLAMING: i32 = 5;
const SITTING_ATTACKING: i32 = 7;
const HOVERING: i32 = 10;

/// A `DragonFlightHistory.Sample`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FlightSample {
    pub y: f64,
    pub y_rot: f32,
}

/// Vanilla `DragonFlightHistory`: the last 64 ticks of height and yaw.
#[derive(Clone)]
pub struct FlightHistory {
    samples: [FlightSample; 64],
    head: i32,
}

impl Default for FlightHistory {
    fn default() -> Self {
        Self {
            samples: [FlightSample::default(); 64],
            head: -1,
        }
    }
}

impl FlightHistory {
    pub fn record(&mut self, y: f64, y_rot: f32) {
        let sample = FlightSample { y, y_rot };
        if self.head < 0 {
            self.samples = [sample; 64];
        }
        self.head = (self.head + 1) % 64;
        self.samples[self.head as usize] = sample;
    }

    pub fn get(&self, delay: i32) -> FlightSample {
        self.samples[((self.head - delay) & 63) as usize]
    }

    /// `get(delay, partialTicks)`: eased from the sample a tick older.
    pub fn get_lerped(&self, delay: i32, partial: f32) -> FlightSample {
        let sample = self.get(delay);
        let old = self.get(delay + 1);
        FlightSample {
            y: old.y + f64::from(partial) * (sample.y - old.y),
            y_rot: crate::entity::lerp_angle(old.y_rot, sample.y_rot, partial),
        }
    }
}

/// `subEntities`' sizes: head, neck, body, three tail segments and two
/// wings.
const PART_SIZES: [EntityDimensions; 8] = [
    EntityDimensions::scalable(1.0, 1.0),
    EntityDimensions::scalable(3.0, 3.0),
    EntityDimensions::scalable(5.0, 3.0),
    EntityDimensions::scalable(2.0, 2.0),
    EntityDimensions::scalable(2.0, 2.0),
    EntityDimensions::scalable(2.0, 2.0),
    EntityDimensions::scalable(4.0, 2.0),
    EntityDimensions::scalable(4.0, 2.0),
];

/// What a tick of [`EnderDragonSim::tick`] asks the client to play.
#[derive(Clone, Copy, Default)]
pub struct DragonSounds {
    /// `onFlap`: `ENDER_DRAGON_FLAP`.
    pub flap: bool,
    /// `aiStep`'s `ENDER_DRAGON_GROWL`.
    pub growl: bool,
}

#[derive(Clone)]
pub struct EnderDragonSim {
    /// `DATA_PHASE`.
    pub phase: i32,
    pub flap_time: f32,
    pub prev_flap_time: f32,
    pub history: FlightHistory,
    /// Squared distance to the top of the exit portal's podium, as of the
    /// last tick.
    pub distance_to_egg: f64,
    /// Each part's position, in [`PART_SIZES`] order; a new part starts at
    /// the origin.
    pub parts: [DVec3; 8],
    growl_time: i32,
}

impl Default for EnderDragonSim {
    fn default() -> Self {
        Self {
            phase: HOVERING,
            flap_time: 0.0,
            prev_flap_time: 0.0,
            history: FlightHistory::default(),
            distance_to_egg: 0.0,
            parts: [DVec3::ZERO; 8],
            growl_time: 100,
        }
    }
}

impl EnderDragonSim {
    /// `DragonPhaseInstance.isSitting`: the three sitting phases and hover.
    pub fn is_sitting(&self) -> bool {
        matches!(self.phase, SITTING_FLAMING..=SITTING_ATTACKING | HOVERING)
    }

    pub fn is_landing_or_taking_off(&self) -> bool {
        matches!(self.phase, LANDING | TAKEOFF)
    }

    /// `isFlapping`: the wing crossed the downstroke this tick.
    fn is_flapping(&self) -> bool {
        let flap = mth_cos(self.flap_time * std::f32::consts::TAU);
        let old_flap = mth_cos(self.prev_flap_time * std::f32::consts::TAU);
        old_flap <= -0.3 && flap >= -0.3
    }

    /// Each part's id, numbered after the dragon's own as the client does,
    /// position and pick box.
    pub fn part_boxes(&self, dragon_id: i32) -> impl Iterator<Item = (i32, DVec3, Aabb)> + '_ {
        (dragon_id + 1..)
            .zip(self.parts.iter().zip(PART_SIZES))
            .map(|(id, (&position, size))| (id, position, size.make_bounding_box(position)))
    }

    /// The client side of `EnderDragon.aiStep`, for a dragon at `position`
    /// facing `y_rot` and moving `velocity`. A `silent` one neither sounds
    /// nor counts down its growl.
    // TODO: `checkCrystals` (no end crystals yet), the phases'
    // `doClientTick` particles, and `dragonDeathTime`'s dying.
    pub fn tick(
        &mut self,
        position: DVec3,
        y_rot: f32,
        velocity: DVec3,
        dead: bool,
        no_ai: bool,
        silent: bool,
    ) -> DragonSounds {
        let mut sounds = DragonSounds {
            flap: !silent && self.is_flapping(),
            growl: false,
        };
        if !silent && !self.is_sitting() {
            self.growl_time -= 1;
            if self.growl_time < 0 {
                sounds.growl = true;
                self.growl_time = 200 + fastrand::i32(..200);
            }
        }
        self.prev_flap_time = self.flap_time;
        if dead {
            return sounds;
        }
        let horizontal = (velocity.x * velocity.x + velocity.z * velocity.z).sqrt() as f32;
        let flap_speed = 0.2 / (horizontal * 10.0 + 1.0);
        self.flap_time += if self.is_sitting() {
            0.1
        } else {
            flap_speed * 2f32.powf(velocity.y as f32)
        };
        if no_ai {
            self.flap_time = 0.5;
            return sounds;
        }
        let y_rot = wrap_degrees(y_rot);
        self.history.record(position.y, y_rot);
        self.tick_parts(position, y_rot);
        sounds
    }

    /// `aiStep`'s `tickPart`s. `yRotA` only turns on the server, so the head
    /// and neck face the body's way.
    fn tick_parts(&mut self, position: DVec3, y_rot: f32) {
        let p5 = self.history.get(5);
        let tilt = (p5.y - self.history.get(10).y) as f32 * 10.0 * DEG;
        let (cc_tilt, ss_tilt) = (mth_cos(tilt), mth_sin(tilt));
        let (ss1, cc1) = (mth_sin(y_rot * DEG), mth_cos(y_rot * DEG));
        // `getHeadYOffset`.
        let head_y = if self.is_sitting() {
            -1.0
        } else {
            (p5.y - self.history.get(0).y) as f32
        };
        let reach = |length: f32| {
            DVec3::new(
                f64::from(ss1 * length * cc_tilt),
                f64::from(head_y + ss_tilt * length),
                f64::from(-cc1 * length * cc_tilt),
            )
        };
        let tail = |i: i32| {
            let p0 = self.history.get(12 + i * 2);
            let rot = y_rot * DEG + wrap_degrees(p0.y_rot - p5.y_rot) * DEG;
            let (ss, cc) = (mth_sin(rot), mth_cos(rot));
            let dd = (i + 1) as f32 * 2.0;
            DVec3::new(
                f64::from(-(ss1 * 1.5 + ss * dd) * cc_tilt),
                p0.y - p5.y - f64::from((dd + 1.5) * ss_tilt) + 1.5,
                f64::from((cc1 * 1.5 + cc * dd) * cc_tilt),
            )
        };
        let wing = |side: f32| {
            DVec3::new(
                f64::from(cc1 * 4.5 * side),
                2.0,
                f64::from(ss1 * 4.5 * side),
            )
        };
        let offsets = [
            reach(6.5),
            reach(5.5),
            DVec3::new(f64::from(ss1 * 0.5), 0.0, f64::from(-cc1 * 0.5)),
            tail(0),
            tail(1),
            tail(2),
            wing(1.0),
            wing(-1.0),
        ];
        self.parts = offsets.map(|offset| position + offset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: DVec3 = DVec3::new(0.0, 64.0, 0.0);

    fn flying() -> EnderDragonSim {
        EnderDragonSim {
            phase: 0,
            ..Default::default()
        }
    }

    #[test]
    fn the_first_record_fills_the_history() {
        let mut history = FlightHistory::default();
        history.record(64.0, 90.0);
        assert_eq!(history.get(0), history.get(63));
        history.record(65.0, 100.0);
        assert_eq!(history.get(0).y, 65.0);
        assert_eq!(history.get(1).y, 64.0);
        let mid = history.get_lerped(0, 0.5);
        assert_eq!((mid.y, mid.y_rot), (64.5, 95.0));
    }

    #[test]
    fn a_no_ai_dragon_holds_its_wings_mid_flap() {
        let mut dragon = EnderDragonSim::default();
        dragon.tick(AT, 0.0, DVec3::ZERO, false, true, false);
        assert_eq!(dragon.flap_time, 0.5);
        assert_eq!(dragon.history.get(0), FlightSample::default());
        assert_eq!(dragon.parts, [DVec3::ZERO; 8]);
    }

    #[test]
    fn hovering_flaps_a_tenth_a_tick_and_never_growls() {
        let mut dragon = EnderDragonSim::default();
        for _ in 0..300 {
            assert!(!dragon.tick(AT, 0.0, DVec3::ZERO, false, false, false).growl);
        }
        assert!((dragon.flap_time - 30.0).abs() < 1e-3);
    }

    #[test]
    fn a_wingbeat_crosses_the_downstroke_once() {
        let mut dragon = flying();
        let flaps = (0..20)
            .filter(|_| dragon.tick(AT, 0.0, DVec3::ZERO, false, false, false).flap)
            .count();
        // 0.2 a tick: four wingbeats in twenty ticks.
        assert_eq!(flaps, 4);
    }

    #[test]
    fn flight_growls_after_a_hundred_ticks() {
        let mut dragon = flying();
        let mut growl = || dragon.tick(AT, 0.0, DVec3::ZERO, false, false, false).growl;
        assert!((0..100).all(|_| !growl()));
        assert!(growl());
    }

    #[test]
    fn a_silent_dragon_neither_flaps_nor_growls() {
        let mut dragon = flying();
        for _ in 0..300 {
            let sounds = dragon.tick(AT, 0.0, DVec3::ZERO, false, false, true);
            assert!(!sounds.flap && !sounds.growl);
        }
    }

    #[test]
    fn speed_slows_the_wings_and_climbing_quickens_them() {
        let flap_step = |velocity| {
            let mut dragon = flying();
            dragon.tick(AT, 0.0, velocity, false, false, false);
            dragon.flap_time
        };
        assert!((flap_step(DVec3::new(0.3, 0.0, 0.4)) - 0.2 / 6.0).abs() < 1e-6);
        assert!((flap_step(DVec3::new(0.0, 1.0, 0.0)) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn a_dying_dragon_stops_flapping_recording_and_moving_its_parts() {
        let mut dragon = flying();
        dragon.tick(AT, 0.0, DVec3::ZERO, false, false, false);
        let parts = dragon.parts;
        let flap_time = dragon.flap_time;
        dragon.tick(AT + DVec3::X, 90.0, DVec3::ZERO, true, false, false);
        assert_eq!(dragon.flap_time, flap_time);
        assert_eq!(dragon.history.get(0), dragon.history.get(1));
        assert_eq!(dragon.parts, parts);
    }

    /// A climbing, turning flight, against the game's own `aiStep` maths.
    #[test]
    fn parts_follow_the_flight_history() {
        let mut dragon = flying();
        for t in 0..20 {
            let position = DVec3::new(10.0, 60.0 + f64::from(t) * 0.5, -5.0);
            dragon.tick(
                position,
                t as f32 * 7.0 - 30.0,
                DVec3::ZERO,
                false,
                false,
                false,
            );
        }
        let expected = [
            [15.740116596221924, 69.74695587158203, -3.6751245260238647],
            [14.85702133178711, 69.32434749603271, -3.8789515495300293],
            [10.487191259860992, 69.5, -4.887551456689835],
            [7.208954572677612, 66.02086985111237, -4.2402501702308655],
            [6.34529972076416, 64.17565250396729, -2.5284647941589355],
            [6.29162335395813, 62.330435276031494, -0.41817617416381836],
            [8.987963080406189, 71.5, -0.6152787208557129],
            [11.012036919593811, 71.5, -9.384721279144287],
        ];
        for (part, expected) in dragon.parts.iter().zip(expected) {
            assert!(part.distance(DVec3::from_array(expected)) < 1e-9, "{part}");
        }
    }
}
