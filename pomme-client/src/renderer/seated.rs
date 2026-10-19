//! Seated VR with mouse aim: the headset looks around freely while the mouse
//! moves a crosshair up to `KEYHOLE_DEG` either side of where the head faced,
//! turning the player once pushed past that. The crosshair is the player's
//! look, so the raycast, the server's rotation and walking all follow it.

use std::collections::VecDeque;
use std::f32::consts::{FRAC_PI_4, PI};

use glam::{Mat4, Quat, Vec3};

use super::panel::GuiPanel;
use crate::stereo::{EyeView, Fov, PanelPlacement};

/// How far the crosshair strays from the head before the world turns.
const KEYHOLE_DEG: f32 = 15.0;
/// The mouse's horizontal and vertical sweep across the window, in degrees.
const H_RANGE_DEG: f32 = 110.0;
const V_RANGE_DEG: f32 = 180.0;
/// Turn per frame with the crosshair pushed to the window's edge.
const TURN_DEG: f32 = 20.0;
const PITCH_SPEED: f32 = 0.5;
const MAX_PITCH_DEG: f32 = 89.9;

/// A panel's width per unit of scale.
const PANEL_WIDTH: f32 = 1.5;
/// A screen opens this far ahead of the head; chat this much higher.
const SCREEN_DISTANCE: f32 = 1.0;
const CHAT_RAISE: f32 = 0.25;
/// The title screens: further out, twice the size.
const MENU_DISTANCE: f32 = 2.5;
const MENU_SCALE: f32 = 2.0;
/// Frames the menu's head average spans.
const HEAD_SAMPLES: usize = 90;

/// How the HUD panel sits, from the VR settings.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct HudPlacement {
    /// Metres from the head.
    pub distance: f32,
    pub scale: f32,
}

impl Default for HudPlacement {
    fn default() -> Self {
        Self {
            distance: 1.25,
            scale: 1.0,
        }
    }
}

pub(crate) struct Seated {
    eyes: [EyeView; 2],
    /// World yaw (vanilla degrees) that the seated space's forward (-Z) maps
    /// to; turning adds to it. Unwrapped, like vanilla's own yaw.
    room_yaw: f32,
    /// The head's yaw in the seated space as of the last turn: the keyhole's
    /// centre.
    aim_centre: f32,
    /// The crosshair's yaw off `aim_centre`, within the keyhole.
    aim_offset: f32,
    /// Vanilla sign: positive looks down.
    aim_pitch: f32,
    /// Where an open screen's panel was put when it opened.
    screen_anchor: Option<(Vec3, Quat)>,
    last_panel: GuiPanel,
    head_samples: HeadSamples,
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
            screen_anchor: None,
            last_panel: GuiPanel::Menu,
            head_samples: HeadSamples::default(),
        }
    }

    pub fn set_eyes(&mut self, eyes: [EyeView; 2]) {
        self.eyes = eyes;
        self.head_samples
            .add(self.head_position(), self.head_room_yaw());
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
        self.aim_centre += wrap_degrees(self.head_room_yaw() - self.aim_centre);
    }

    /// The runtime made the head's pose the seated origin: turn the world
    /// with it, so neither the view nor the aim jumps.
    pub fn recenter(&mut self) {
        let head_yaw = self.head_room_yaw();
        self.room_yaw += head_yaw;
        self.aim_centre -= head_yaw;
        self.screen_anchor = None;
        self.head_samples = HeadSamples::default();
    }

    /// The head's yaw in the seated space, in vanilla's sign (right is
    /// positive).
    fn head_room_yaw(&self) -> f32 {
        let forward = self.eyes[0].orientation * Vec3::NEG_Z;
        forward.x.atan2(-forward.z).to_degrees()
    }

    fn head_position(&self) -> Vec3 {
        (self.eyes[0].position + self.eyes[1].position) * 0.5
    }

    /// Where the panel floats for `kind`: the HUD rides the crosshair, a
    /// screen stays where it opened, and the title screens trail the head's
    /// average heading. `aspect` is the panel's height over width.
    pub fn panel(&mut self, kind: GuiPanel, hud: HudPlacement, aspect: f32) -> PanelPlacement {
        let head = self.head_position();
        let (position, orientation, scale) = match kind {
            GuiPanel::Hud => {
                let aim = self.aim_room_direction();
                (head + aim * hud.distance, facing(aim), hud.scale)
            }
            GuiPanel::Screen | GuiPanel::Chat => {
                let reopened = matches!(self.last_panel, GuiPanel::Hud | GuiPanel::Menu)
                    || (kind == GuiPanel::Chat && self.last_panel != GuiPanel::Chat);
                if reopened || self.screen_anchor.is_none() {
                    let raise = if kind == GuiPanel::Chat {
                        CHAT_RAISE
                    } else {
                        0.0
                    };
                    let offset = Vec3::new(0.0, raise, -SCREEN_DISTANCE);
                    let position = head + self.eyes[0].orientation * offset;
                    self.screen_anchor = Some((position, facing(position - head)));
                }
                let (position, orientation) = self.screen_anchor.unwrap();
                (position, orientation, 1.0)
            }
            GuiPanel::Menu => {
                let (position, yaw) = self.head_samples.average();
                let ahead = yaw_direction(yaw);
                (position + ahead * MENU_DISTANCE, facing(ahead), MENU_SCALE)
            }
        };
        self.last_panel = kind;
        let width = PANEL_WIDTH * scale;
        PanelPlacement {
            orientation,
            position,
            width,
            height: width * aspect,
        }
    }

    /// The crosshair's direction in the seated space.
    fn aim_room_direction(&self) -> Vec3 {
        let pitch = self.aim_pitch.to_radians();
        yaw_direction(self.aim_centre + self.aim_offset) * pitch.cos() - Vec3::Y * pitch.sin()
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
        let centre = self.room_rotation() * self.head_position();
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

/// Level, `yaw` degrees right of the seated space's -Z.
fn yaw_direction(yaw: f32) -> Vec3 {
    let (sin, cos) = yaw.to_radians().sin_cos();
    Vec3::new(sin, 0.0, -cos)
}

/// The roll-free rotation turning -Z to `direction`, so a panel's front
/// (+Z) faces back along it.
fn facing(direction: Vec3) -> Quat {
    let d = direction.normalize();
    Quat::from_rotation_y((-d.x).atan2(-d.z)) * Quat::from_rotation_x(d.y.asin())
}

/// The head's recent position, and a heading that only follows once the
/// head has turned 20° from it, settling within 1°, so the title screens
/// don't drift with every glance.
#[derive(Default)]
struct HeadSamples {
    positions: VecDeque<Vec3>,
    yaws: VecDeque<f32>,
    /// The head's yaw, unwrapped.
    yaw_total: f32,
    yaw_last: f32,
    following: bool,
}

impl HeadSamples {
    fn add(&mut self, position: Vec3, yaw: f32) {
        if self.positions.len() == HEAD_SAMPLES {
            self.positions.pop_front();
        }
        if self.yaws.len() == HEAD_SAMPLES {
            self.yaws.pop_front();
        }
        self.positions.push_back(position);
        self.yaw_total += wrap_degrees(yaw - self.yaw_last);
        self.yaw_last = yaw;
        let (_, average) = self.average();
        let off = (self.yaw_total - average).abs();
        if off > 20.0 {
            self.following = true;
        } else if off < 1.0 {
            self.following = false;
        }
        if self.following || self.yaws.is_empty() {
            self.yaws.push_back(self.yaw_total);
        }
    }

    fn average(&self) -> (Vec3, f32) {
        let mean = |n: usize| n.max(1) as f32;
        (
            self.positions.iter().sum::<Vec3>() / mean(self.positions.len()),
            self.yaws.iter().sum::<f32>() / mean(self.yaws.len()),
        )
    }
}

/// Vanilla `Mth.wrapDegrees`: into [-180, 180).
fn wrap_degrees(degrees: f32) -> f32 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
mod tests {
    use glam::camera::rh::proj;

    use super::*;
    use crate::renderer::panel::PANEL_ASPECT;

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
    fn the_hud_rides_the_crosshair() {
        let mut seated = Seated::new(0.0, 0.0);
        seated.move_aim(10.0 / H_RANGE_DEG, 0.0);
        let panel = seated.panel(GuiPanel::Hud, HudPlacement::default(), PANEL_ASPECT);
        let ahead = yaw_direction(10.0);
        assert!(panel.position.abs_diff_eq(ahead * 1.25, 1e-5));
        assert!((panel.orientation * Vec3::NEG_Z).abs_diff_eq(ahead, 1e-5));
        assert_eq!((panel.width, panel.height), (1.5, 1.5 * PANEL_ASPECT));
    }

    #[test]
    fn a_screen_stays_where_it_opened() {
        let mut seated = Seated::new(0.0, 0.0);
        seated.panel(GuiPanel::Hud, HudPlacement::default(), PANEL_ASPECT);
        let opened = seated.panel(GuiPanel::Screen, HudPlacement::default(), PANEL_ASPECT);
        assert!(opened.position.abs_diff_eq(Vec3::NEG_Z, 1e-5));
        let mut eyes = seated.eyes;
        for eye in &mut eyes {
            eye.orientation = Quat::from_rotation_y(1.0);
        }
        seated.set_eyes(eyes);
        assert_eq!(
            seated.panel(GuiPanel::Screen, HudPlacement::default(), PANEL_ASPECT),
            opened
        );
    }

    #[test]
    fn the_title_screen_waits_for_a_real_turn() {
        let mut seated = Seated::new(0.0, 0.0);
        let mut turn_head = |yaw: f32| {
            let mut eyes = seated.eyes;
            for eye in &mut eyes {
                eye.orientation = Quat::from_rotation_y(-yaw.to_radians());
            }
            seated.set_eyes(eyes);
            seated.panel(GuiPanel::Menu, HudPlacement::default(), PANEL_ASPECT)
        };
        let start = turn_head(0.0);
        assert!(
            start
                .position
                .abs_diff_eq(Vec3::NEG_Z * MENU_DISTANCE, 1e-5)
        );
        assert_eq!(start.width, PANEL_WIDTH * MENU_SCALE);
        // A glance under 20° leaves it.
        assert_eq!(turn_head(15.0).position, start.position);
        // A real turn draws it round.
        assert!(turn_head(40.0).position.x > start.position.x);
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
