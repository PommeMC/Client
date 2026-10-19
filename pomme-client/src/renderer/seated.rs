//! Seated VR, after Vivecraft's seated defaults (`MCVR.updatePose`, mouse
//! aim): the headset looks around freely while the mouse moves a crosshair
//! up to `KEYHOLE_DEG` either side of where the head faced, turning the
//! player once pushed past that. The crosshair is the player's look, so
//! the raycast, the server's rotation and walking all follow it.

use std::f32::consts::{FRAC_PI_4, PI};

use glam::{Mat4, Quat, Vec3};

use crate::stereo::{EyeView, Fov};

/// Vivecraft `keyholeX`: how far the crosshair strays from the head before
/// the world turns.
const KEYHOLE_DEG: f32 = 15.0;
/// The mouse's horizontal and vertical sweep across the window, in degrees.
const H_RANGE_DEG: f32 = 110.0;
const V_RANGE_DEG: f32 = 180.0;
/// Turn per frame with the crosshair pushed to the window's edge (Vivecraft
/// `20 * xSensitivity`, sensitivity 1).
const TURN_DEG: f32 = 20.0;
/// Vivecraft `0.5 * ySensitivity`.
const PITCH_SPEED: f32 = 0.5;
/// Vivecraft clamps the aim short of straight up and down.
const MAX_PITCH_DEG: f32 = 89.9;

pub(crate) struct Seated {
    pub eyes: [EyeView; 2],
    /// World yaw (vanilla degrees) that the seated space's forward (-Z) maps
    /// to; turning adds to it. Unwrapped, like vanilla's own yaw.
    room_yaw: f32,
    /// The head's yaw in the seated space as of the last turn (Vivecraft
    /// `hmdForwardYaw`): the keyhole's centre.
    aim_centre: f32,
    /// The crosshair's yaw off `aim_centre`, within the keyhole.
    aim_offset: f32,
    /// Vanilla sign: positive looks down.
    aim_pitch: f32,
}

impl Seated {
    pub fn new(yaw: f32, pitch: f32) -> Self {
        // Until the headset's first frame.
        let ahead = EyeView {
            orientation: Quat::IDENTITY,
            position: Vec3::ZERO,
            fov: Fov {
                left: -FRAC_PI_4,
                right: FRAC_PI_4,
                up: FRAC_PI_4,
                down: -FRAC_PI_4,
            },
        };
        Self {
            eyes: [ahead; 2],
            room_yaw: yaw,
            aim_centre: 0.0,
            aim_offset: 0.0,
            aim_pitch: pitch,
        }
    }

    /// The crosshair's direction as vanilla yaw and pitch.
    pub fn aim(&self) -> (f32, f32) {
        (
            self.room_yaw + self.aim_centre + self.aim_offset,
            self.aim_pitch,
        )
    }

    /// Points the crosshair at `yaw`/`pitch` (a server-set rotation) without
    /// moving it in the keyhole.
    pub fn set_aim(&mut self, yaw: f32, pitch: f32) {
        self.room_yaw = yaw - self.aim_centre - self.aim_offset;
        self.aim_pitch = pitch;
    }

    /// Moves the crosshair by a mouse delta given as a fraction of the window
    /// (+x right, +y down).
    pub fn move_aim(&mut self, dx: f32, dy: f32) {
        let offset = self.aim_offset + dx * H_RANGE_DEG;
        if offset.abs() > KEYHOLE_DEG {
            let edge = H_RANGE_DEG * 0.5;
            let push = ((offset.abs() - KEYHOLE_DEG) / (edge - KEYHOLE_DEG)).min(1.0);
            self.turn(offset.signum() * TURN_DEG * push);
            self.aim_offset = offset.signum() * KEYHOLE_DEG;
        } else {
            self.aim_offset = offset;
        }
        self.adjust_pitch(dy * V_RANGE_DEG * PITCH_SPEED);
    }

    /// Turns the player, re-centring the keyhole on where the head faces now.
    pub fn turn(&mut self, yaw_delta: f32) {
        self.room_yaw += yaw_delta;
        let head = self.head_rotation() * Vec3::NEG_Z;
        let head_yaw = (-head.x).atan2(head.z).to_degrees() - self.room_yaw;
        self.aim_centre += wrap_degrees(head_yaw - self.aim_centre);
    }

    pub fn adjust_pitch(&mut self, delta: f32) {
        self.aim_pitch = (self.aim_pitch + delta).clamp(-MAX_PITCH_DEG, MAX_PITCH_DEG);
    }

    /// Seated space to world: the space's -Z faces `room_yaw`.
    fn room_rotation(&self) -> Quat {
        Quat::from_rotation_y(PI - self.room_yaw.to_radians())
    }

    fn head_rotation(&self) -> Quat {
        self.room_rotation() * self.eyes[0].orientation
    }

    /// The head's right and up axes in the world, for camera-facing quads.
    pub fn head_axes(&self) -> (Vec3, Vec3) {
        let rotation = self.head_rotation();
        (rotation * Vec3::X, rotation * Vec3::Y)
    }

    /// Each eye's view-projection, relative to the player's eye position (the
    /// seated space's origin). `translate` false keeps the rotation only, for
    /// the sky.
    pub fn view_projections(&self, near: f32, far: f32, translate: bool) -> [Mat4; 2] {
        let room = self.room_rotation();
        self.eyes.map(|eye| {
            let offset = if translate {
                room * eye.position
            } else {
                Vec3::ZERO
            };
            let view = Mat4::from_rotation_translation(room * eye.orientation, offset).inverse();
            off_axis_projection(eye.fov, near, far) * view
        })
    }

    /// One frustum around both eyes, from the head centre, with each side
    /// widened by `extra` radians.
    pub fn culling_view_projection(&self, extra: f32, near: f32, far: f32) -> Mat4 {
        let [a, b] = self.eyes.map(|eye| eye.fov);
        // Short of 90°, where a side's tangent runs off.
        let side = |angle: f32| angle.clamp(-1.5, 1.5);
        let fov = Fov {
            left: side(a.left.min(b.left) - extra),
            right: side(a.right.max(b.right) + extra),
            up: side(a.up.max(b.up) + extra),
            down: side(a.down.min(b.down) - extra),
        };
        let room = self.room_rotation();
        let centre = room * (self.eyes[0].position + self.eyes[1].position) * 0.5;
        let view = Mat4::from_rotation_translation(self.head_rotation(), centre).inverse();
        off_axis_projection(fov, near, far) * view
    }
}

/// The asymmetric frustum of an eye: right-handed, depth 0..1, with Vulkan's
/// +Y-down flip, the conventions `Camera`'s own perspective uses.
pub(crate) fn off_axis_projection(fov: Fov, near: f32, far: f32) -> Mat4 {
    let left = fov.left.tan();
    let right = fov.right.tan();
    let up = fov.up.tan();
    let down = fov.down.tan();
    let depth = far / (near - far);
    let projection = Mat4::from_cols(
        [2.0 / (right - left), 0.0, 0.0, 0.0].into(),
        [0.0, 2.0 / (up - down), 0.0, 0.0].into(),
        [
            (right + left) / (right - left),
            (up + down) / (up - down),
            depth,
            -1.0,
        ]
        .into(),
        [0.0, 0.0, depth * near, 0.0].into(),
    );
    Mat4::from_diagonal([1.0, -1.0, 1.0, 1.0].into()) * projection
}

/// Vanilla `Mth.wrapDegrees`: into [-180, 180).
fn wrap_degrees(degrees: f32) -> f32 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
mod tests {
    use glam::camera::rh::proj;

    use super::*;

    #[test]
    fn symmetric_fov_matches_the_desktop_projection() {
        let (fovy, aspect, near, far) = (1.2f32, 1.6f32, 0.1, 1000.0);
        let half_y = (fovy / 2.0).tan();
        let half_x = (half_y * aspect).atan();
        let fov = Fov {
            left: -half_x,
            right: half_x,
            up: fovy / 2.0,
            down: -fovy / 2.0,
        };
        let mut expected = proj::directx::perspective(fovy, aspect, near, far);
        expected.y_axis.y *= -1.0;
        assert!(off_axis_projection(fov, near, far).abs_diff_eq(expected, 1e-5));
    }

    #[test]
    fn the_crosshair_turns_the_player_past_the_keyhole() {
        let mut seated = Seated::new(90.0, 0.0);
        seated.move_aim(10.0 / H_RANGE_DEG, 0.0);
        assert_eq!(seated.aim(), (100.0, 0.0));
        // 15° more lands 10° past the keyhole: a quarter push.
        seated.move_aim(15.0 / H_RANGE_DEG, 0.0);
        let (yaw, _) = seated.aim();
        assert!((yaw - (90.0 + TURN_DEG * 0.25 + KEYHOLE_DEG)).abs() < 1e-4);
    }

    #[test]
    fn the_seated_space_faces_the_room_yaw() {
        // Vanilla yaw 90 faces -X (west); the head looking ahead sees it.
        let seated = Seated::new(90.0, 0.0);
        let forward = seated.head_rotation() * Vec3::NEG_Z;
        assert!(forward.abs_diff_eq(Vec3::NEG_X, 1e-5));
    }

    #[test]
    fn the_culling_frustum_spans_both_eyes() {
        let mut seated = Seated::new(0.0, 0.0);
        let fov = |left, right| Fov {
            left,
            right,
            up: 0.7,
            down: -0.7,
        };
        seated.eyes[0].fov = fov(-0.9, 0.6);
        seated.eyes[1].fov = fov(-0.6, 0.9);
        let m = seated.culling_view_projection(0.0, 0.1, 100.0);
        // Seated space -Z faces south (+Z); 0.85 rad either side is inside.
        for angle in [-0.85f32, 0.85] {
            let p = Vec3::new(-angle.sin(), 0.0, angle.cos()) * 10.0;
            let clip = m * p.extend(1.0);
            assert!(clip.x.abs() <= clip.w, "{angle}");
        }
    }
}
