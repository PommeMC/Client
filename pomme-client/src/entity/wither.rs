//! The client half of vanilla `WitherBoss`: its synced targets and
//! invulnerable ticks, and the side heads turning toward their targets.

use glam::DVec3;

use super::wrap_degrees;
use crate::physics::movement::{mth_cos, mth_sin};
use crate::world::waypoints::mth_atan2;

/// `WitherBoss.aiStep`'s radians-to-degrees factor.
const DEGREES_PER_RADIAN: f64 = 57.2957763671875;

// TODO: `WitherBoss.aiStep`'s SMOKE and ENTITY_EFFECT particles at the heads
// (no client-spawned smoke or entity effect particle yet), and
// `getScale`, which pomme reads as 1.
#[derive(Clone, Default)]
pub struct WitherSim {
    /// `DATA_TARGET_A`..`C`: entity ids, 0 for none.
    pub targets: [i32; 3],
    /// `DATA_ID_INV`.
    pub invulnerable_ticks: i32,
    pub x_rot_heads: [f32; 2],
    pub y_rot_heads: [f32; 2],
}

impl WitherSim {
    /// The entity id side head `i` (0 or 1) aims at, if any.
    pub fn head_target(&self, i: usize) -> Option<i32> {
        Some(self.targets[i + 1]).filter(|&id| id > 0)
    }

    /// `WitherBoss.getHeadX`/`Y`/`Z` for side head `i` (0 or 1).
    fn side_head_position(position: DVec3, body_y_rot_deg: f32, i: usize) -> DVec3 {
        let angle = (body_y_rot_deg + 180.0 * i as f32).to_radians();
        position
            + DVec3::new(
                f64::from(mth_cos(angle)) * 1.3,
                2.2,
                f64::from(mth_sin(angle)) * 1.3,
            )
    }

    /// `WitherBoss.aiStep`'s head turn: each side head eases toward its
    /// target's eye, or its yaw back to the body's.
    pub fn tick_heads(
        &mut self,
        position: DVec3,
        body_y_rot_deg: f32,
        target_eyes: [Option<DVec3>; 2],
    ) {
        for (i, eye) in target_eyes.into_iter().enumerate() {
            let Some(eye) = eye else {
                self.y_rot_heads[i] = rot_lerp(self.y_rot_heads[i], body_y_rot_deg, 10.0);
                continue;
            };
            let d = eye - Self::side_head_position(position, body_y_rot_deg, i);
            let horizontal = (d.x * d.x + d.z * d.z).sqrt();
            let y_rot = (mth_atan2(d.z, d.x) * DEGREES_PER_RADIAN) as f32 - 90.0;
            let x_rot = -(mth_atan2(d.y, horizontal) * DEGREES_PER_RADIAN) as f32;
            self.x_rot_heads[i] = rot_lerp(self.x_rot_heads[i], x_rot, 40.0);
            self.y_rot_heads[i] = rot_lerp(self.y_rot_heads[i], y_rot, 10.0);
        }
    }
}

/// `WitherBoss.isPowered`: at half health or below.
pub fn is_powered(health: f32, max_health: f32) -> bool {
    health <= max_health / 2.0
}

/// `WitherBoss.rotlerp`.
fn rot_lerp(from: f32, to: f32, max: f32) -> f32 {
    from + wrap_degrees(to - from).clamp(-max, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_side_head_eases_toward_its_target() {
        let mut wither = WitherSim::default();
        // Due south of the right head (body yaw 0 puts it at +x 1.3), level
        // with it: yaw 0, pitch 0.
        let eye = DVec3::new(1.3, 2.2, 10.0);
        wither.tick_heads(DVec3::ZERO, 0.0, [Some(eye), None]);
        // `Mth.atan2` is a table approximation.
        assert!(wither.x_rot_heads[0].abs() < 1e-4);
        assert!(wither.y_rot_heads[0].abs() < 1e-4);
        // A target due east turns it at most 10 degrees a tick.
        wither.tick_heads(DVec3::ZERO, 0.0, [Some(DVec3::new(20.0, 2.2, 0.0)), None]);
        assert!((wither.y_rot_heads[0] + 10.0).abs() < 1e-4);
    }

    #[test]
    fn an_idle_side_head_returns_to_the_body() {
        let mut wither = WitherSim {
            y_rot_heads: [0.0, 45.0],
            ..Default::default()
        };
        wither.tick_heads(DVec3::ZERO, 30.0, [None, None]);
        assert_eq!(wither.y_rot_heads, [10.0, 35.0]);
    }
}
