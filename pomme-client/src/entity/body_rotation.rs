//! Vanilla `BodyRotationControl`: how a `Mob`'s body follows its head on
//! the client (`Mob.tickHeadTurn` runs `clientTick` in place of
//! `LivingEntity`'s turn).

use azalea_registry::builtin::EntityKind;

use super::wrap_degrees;

const HEAD_STABLE_ANGLE: f32 = 15.0;
const DELAY_UNTIL_STARTING_TO_FACE_FORWARD: i32 = 10;
const HOW_LONG_IT_TAKES_TO_FACE_FORWARD: f32 = 10.0;

// TODO: `BodyRotationControl.notCarryingMobPassengers` (no client-side
// vehicle tracking for other entities yet), and the Camel, Creaking,
// HappyGhast, Phantom and Shulker controls once those mobs are modeled.
#[derive(Default)]
pub struct BodyRotationControl {
    head_stable_time: i32,
    last_stable_y_head_rot: f32,
}

impl BodyRotationControl {
    /// `clientTick` for a mob that moved (`dx`, `dz`) this tick, facing
    /// `y_rot`.
    pub fn client_tick(
        &mut self,
        body: &mut f32,
        head: &mut f32,
        y_rot: f32,
        (dx, dz): (f64, f64),
        max_head_y_rot: f32,
    ) {
        if dx * dx + dz * dz > 2.500000277905201E-7 {
            *body = y_rot;
            *head = rotate_if_necessary(*head, *body, max_head_y_rot);
            self.last_stable_y_head_rot = *head;
            self.head_stable_time = 0;
            return;
        }
        if (*head - self.last_stable_y_head_rot).abs() > HEAD_STABLE_ANGLE {
            self.head_stable_time = 0;
            self.last_stable_y_head_rot = *head;
            *body = rotate_if_necessary(*body, *head, max_head_y_rot);
        } else {
            self.head_stable_time += 1;
            if self.head_stable_time > DELAY_UNTIL_STARTING_TO_FACE_FORWARD {
                let since = (self.head_stable_time - DELAY_UNTIL_STARTING_TO_FACE_FORWARD) as f32;
                let fraction = (since / HOW_LONG_IT_TAKES_TO_FACE_FORWARD).clamp(0.0, 1.0);
                *body = rotate_if_necessary(*body, *head, max_head_y_rot * (1.0 - fraction));
            }
        }
    }
}

/// Vanilla `Mob.getMaxHeadYRot` and its overrides (a scared armadillo's 0
/// is left out: its control skips the tick).
pub fn max_head_y_rot(kind: EntityKind) -> f32 {
    match kind {
        EntityKind::Armadillo => 32.0,
        EntityKind::Axolotl | EntityKind::Dolphin => 1.0,
        EntityKind::Frog => 5.0,
        EntityKind::Goat => 15.0,
        EntityKind::Camel | EntityKind::CamelHusk | EntityKind::Breeze => 30.0,
        EntityKind::Ravager => 45.0,
        EntityKind::Sniffer => 50.0,
        EntityKind::Shulker => 180.0,
        _ => 75.0,
    }
}

/// Vanilla `Mth.rotateIfNecessary`: `base` brought within `max_diff` of
/// `target`.
fn rotate_if_necessary(base: f32, target: f32, max_diff: f32) -> f32 {
    target - wrap_degrees(target - base).clamp(-max_diff, max_diff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standing_body_follows_a_head_turned_past_the_limit() {
        let mut control = BodyRotationControl::default();
        let (mut body, mut head) = (0.0, 170.0);
        control.client_tick(&mut body, &mut head, 0.0, (0.0, 0.0), 75.0);
        assert_eq!((body, head), (95.0, 170.0));
    }

    #[test]
    fn moving_body_snaps_to_yaw_and_pulls_the_head_in() {
        let mut control = BodyRotationControl::default();
        let (mut body, mut head) = (0.0, 120.0);
        control.client_tick(&mut body, &mut head, 10.0, (0.1, 0.0), 75.0);
        assert_eq!((body, head), (10.0, 85.0));
    }

    #[test]
    fn body_faces_a_still_head_over_ten_ticks_after_ten() {
        let mut control = BodyRotationControl::default();
        let (mut body, mut head) = (0.0, 60.0);
        control.client_tick(&mut body, &mut head, 0.0, (0.0, 0.0), 75.0);
        for _ in 0..10 {
            control.client_tick(&mut body, &mut head, 0.0, (0.0, 0.0), 75.0);
        }
        assert_eq!(body, 0.0);
        for _ in 0..10 {
            control.client_tick(&mut body, &mut head, 0.0, (0.0, 0.0), 75.0);
        }
        assert!((body - 60.0).abs() < 1e-4);
    }
}
