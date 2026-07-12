use glam::camera::rh::{proj, view};
use glam::{DVec3, FloatExt, Mat4, Vec3};

use crate::app::input::InputState;
use crate::entity::components::{LookDirection, Position};

const UP: Vec3 = Vec3::Y;
pub const DEFAULT_FOV_DEGREES: f32 = 70.0;
#[allow(dead_code)]
pub const MIN_FOV_DEGREES: f32 = 30.0;
#[allow(dead_code)]
pub const MAX_FOV_DEGREES: f32 = 110.0;
const NEAR: f32 = 0.1;
pub(crate) const FAR: f32 = 1000.0;
const MOUSE_SENSITIVITY: f32 = 0.15;
/// Controller look speed in degrees per second, scaled by frame delta.
const CONTROLLER_SENSITIVITY: f32 = 150.0;
pub const THIRD_PERSON_DISTANCE: f32 = 4.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    FirstPerson,
    ThirdPersonBack,
    ThirdPersonFront,
}

impl CameraMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::FirstPerson => Self::ThirdPersonBack,
            Self::ThirdPersonBack => Self::ThirdPersonFront,
            Self::ThirdPersonFront => Self::FirstPerson,
        }
    }
}

/// Cloud graphics setting, mirroring vanilla's `CloudStatus` plus an off state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CloudMode {
    Off,
    Fast,
    #[default]
    Fancy,
}

impl CloudMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Fast,
            Self::Fast => Self::Fancy,
            Self::Fancy => Self::Off,
        }
    }

    /// Short label for the video-options row (the menu prefixes "Clouds: ").
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Fast => "Fast",
            Self::Fancy => "Fancy",
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Fast => 1,
            Self::Fancy => 2,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Off,
            1 => Self::Fast,
            _ => Self::Fancy,
        }
    }
}

pub struct Camera {
    pub position: Position,
    pub look_dir: LookDirection,
    pub mode: CameraMode,
    pub third_person_dist: f32,
    /// When set, render straight down from this many blocks above the pivot,
    /// ignoring `mode`/`look_dir`. Used by the chunk-load benchmark.
    top_down: Option<f32>,
    aspect_ratio: f32,
    pub base_fov_degrees: f32,
    fov_modifier: f32,
    old_fov_modifier: f32,
    /// Unsmoothed multiplier for the death/fluid FOV effect (vanilla
    /// `modifyFovBasedOnDeathOrFluid`). 1.0 = no effect.
    fluid_fov_factor: f32,
    /// Render-frame partial tick used to interpolate `fov_modifier` per frame.
    render_partial_tick: f32,
    bob_walk_dist: f32,
    bob_amount: f32,
    bob_enabled: bool,
}

impl Camera {
    pub fn new(aspect_ratio: f32) -> Self {
        Self {
            position: Position::default(),
            look_dir: LookDirection::default(),
            mode: CameraMode::FirstPerson,
            third_person_dist: THIRD_PERSON_DISTANCE,
            top_down: None,
            aspect_ratio,
            base_fov_degrees: DEFAULT_FOV_DEGREES,
            fov_modifier: 1.0,
            old_fov_modifier: 1.0,
            fluid_fov_factor: 1.0,
            render_partial_tick: 1.0,
            bob_walk_dist: 0.0,
            bob_amount: 0.0,
            bob_enabled: false,
        }
    }

    pub fn set_view_bob(&mut self, walk_dist: f32, bob: f32, enabled: bool) {
        self.bob_walk_dist = walk_dist;
        self.bob_amount = bob;
        self.bob_enabled = enabled;
    }

    /// The view-space bob transform, also applied to the first-person arm/held
    /// item.
    pub fn view_bob_matrix(&self) -> Mat4 {
        self.bob_matrix()
    }

    /// Replicates vanilla `GameRenderer.bobView`. Identity when disabled, not
    /// in first person, or at rest.
    fn bob_matrix(&self) -> Mat4 {
        if !self.bob_enabled
            || self.mode != CameraMode::FirstPerson
            || self.bob_amount == 0.0
            || self.top_down.is_some()
        {
            return Mat4::IDENTITY;
        }
        use std::f32::consts::PI;
        // Vanilla's `backwardsInterpolatedWalkDistance` is the negated walk distance.
        let f = -self.bob_walk_dist;
        let bob = self.bob_amount;
        let tx = (f * PI).sin() * bob * 0.5;
        let ty = -((f * PI).cos() * bob).abs();
        let roll_deg = (f * PI).sin() * bob * 3.0;
        let pitch_deg = ((f * PI - 0.2).cos() * bob).abs() * 5.0;
        Mat4::from_translation(Vec3::new(tx, ty, 0.0))
            * Mat4::from_rotation_z(roll_deg.to_radians())
            * Mat4::from_rotation_x(pitch_deg.to_radians())
    }

    pub fn update_look(&mut self, input: &mut InputState, dt: f32) {
        if let Some(look_vec) = input.get_gamepad_right_analog() {
            let step = CONTROLLER_SENSITIVITY * dt;
            let y_rot_deg =
                ((self.look_dir.y_rot_deg() + look_vec.x * step) + 180.0).rem_euclid(360.0) - 180.0;
            let x_rot_deg = self.look_dir.x_rot_deg() - look_vec.y * step; //TODO: Add preference for inverting the Y axis
            self.look_dir = LookDirection::new(y_rot_deg, x_rot_deg);
        }

        if input.is_cursor_captured() {
            let (dx, dy) = input.consume_mouse_delta();
            let y_rot_deg = ((self.look_dir.y_rot_deg() + dx as f32 * MOUSE_SENSITIVITY) + 180.0)
                .rem_euclid(360.0)
                - 180.0;
            let x_rot_deg = self.look_dir.x_rot_deg() + dy as f32 * MOUSE_SENSITIVITY;
            self.look_dir = LookDirection::new(y_rot_deg, x_rot_deg);
        }
    }

    pub fn set_aspect_ratio(&mut self, aspect: f32) {
        self.aspect_ratio = aspect;
    }

    pub fn reset(&mut self, position: Position, look_dir: LookDirection) {
        self.position = position;
        self.look_dir = look_dir;
    }

    pub fn sync_pos(&mut self, position: Position) {
        self.position = position
    }

    /// Render-space anchor: the camera's block position (vanilla
    /// `CameraBlockPos`). World positions are rebased against it in f64
    /// before narrowing to f32, keeping full precision near the camera even
    /// at extreme coordinates (no stripe lands at 2^24). Never narrow the
    /// anchor itself to f32 — its components are integers, exact in i32 and
    /// f64 but not in f32 past 2^24.
    pub fn anchor(&self) -> DVec3 {
        self.position.floor()
    }

    pub fn update_fov_modifier(&mut self, target: f32) {
        self.old_fov_modifier = self.fov_modifier;
        self.fov_modifier += (target - self.fov_modifier) * 0.5;
        self.fov_modifier = self.fov_modifier.clamp(0.1, 1.5);
    }

    pub fn set_fluid_fov_factor(&mut self, factor: f32) {
        self.fluid_fov_factor = factor;
    }

    pub fn set_render_partial_tick(&mut self, partial_tick: f32) {
        self.render_partial_tick = partial_tick;
    }

    pub fn fov_radians(&self, partial_tick: f32) -> f32 {
        let modifier = self.old_fov_modifier.lerp(self.fov_modifier, partial_tick);
        (self.base_fov_degrees * modifier * self.fluid_fov_factor).to_radians()
    }

    pub fn frustum_planes(&self) -> [[f32; 4]; 6] {
        Self::planes_from_view_projection(self.view_projection())
    }

    /// Frustum planes for a FOV widened by `extra_radians` (clamped below
    /// 180°), giving an "about to be seen" margin for occlusion-gated mesh
    /// scheduling.
    pub fn frustum_planes_dilated(&self, extra_radians: f32) -> [[f32; 4]; 6] {
        // Vanilla createProjectionMatrixForCulling never culls narrower than the
        // base FOV, so a narrowing modifier (underwater) can't clip visible edges.
        let cull_fov = self
            .fov_radians(self.render_partial_tick)
            .max(self.base_fov_degrees.to_radians());
        let fov = (cull_fov + extra_radians).min(2.96);
        Self::planes_from_view_projection(self.view_projection_with_fov(fov))
    }

    fn planes_from_view_projection(m: Mat4) -> [[f32; 4]; 6] {
        let mt = m.transpose();
        let r0 = mt.x_axis;
        let r1 = mt.y_axis;
        let r2 = mt.z_axis;
        let r3 = mt.w_axis;

        let raw = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 + r2, r3 - r2];

        let mut planes = [[0.0f32; 4]; 6];
        for (i, v) in raw.iter().enumerate() {
            let len = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
            if len > 0.0 {
                planes[i] = [v.x / len, v.y / len, v.z / len, v.w / len];
            }
        }
        planes
    }

    pub fn third_person_offset(&self) -> Vec3 {
        if let Some(height) = self.top_down {
            return Vec3::new(0.0, height, 0.0);
        }
        let fwd = self.look_dir.as_vec();
        match self.mode {
            CameraMode::FirstPerson => Vec3::ZERO,
            CameraMode::ThirdPersonBack => -fwd * self.third_person_dist,
            CameraMode::ThirdPersonFront => fwd * self.third_person_dist,
        }
    }

    /// Screen-space right/up axes for camera-facing particle billboards.
    /// Equivalent to rotating quad corners by vanilla's roll-free camera
    /// quaternion; derived from yaw/pitch analytically so there's no
    /// degeneracy looking straight up or down.
    pub fn billboard_axes(&self) -> (Vec3, Vec3) {
        if self.top_down.is_some() {
            // Looking straight down with north up.
            return (Vec3::X, Vec3::NEG_Z);
        }
        let (sin_yaw, cos_yaw) = self.look_dir.y_rot_rad().sin_cos();
        let (sin_pitch, cos_pitch) = self.look_dir.x_rot_rad().sin_cos();
        let right = Vec3::new(-cos_yaw, 0.0, -sin_yaw);
        let up = Vec3::new(-sin_yaw * sin_pitch, cos_pitch, cos_yaw * sin_pitch);
        if self.mode == CameraMode::ThirdPersonFront {
            // The camera faces the opposite way: right flips, up stays.
            (-right, up)
        } else {
            (right, up)
        }
    }

    /// Forward and up vectors for the view matrix, accounting for the top-down
    /// override and front-facing third person.
    fn view_basis(&self) -> (Vec3, Vec3) {
        if self.top_down.is_some() {
            // Looking straight down Y; Y can't be the up hint, so use -Z (north up).
            return (Vec3::NEG_Y, Vec3::NEG_Z);
        }
        let look_dir = self.look_dir.as_vec();
        let forward = if self.mode == CameraMode::ThirdPersonFront {
            -look_dir
        } else {
            look_dir
        };
        (forward, UP)
    }

    pub fn top_down(&self) -> Option<f32> {
        self.top_down
    }

    /// Frame a straight-down view roughly fitting `radius_blocks` vertically
    /// under the current FOV, sitting just inside the far plane. The factor
    /// pulls in a bit closer than an exact fit so the load area fills more
    /// of the screen.
    pub fn frame_top_down(&mut self, radius_blocks: f32) {
        let half_fov = self.fov_radians(1.0) / 2.0;
        let height = (radius_blocks / half_fov.tan() * 0.8).min(FAR - 64.0);
        self.top_down = Some(height);
    }

    pub fn clear_top_down(&mut self) {
        self.top_down = None;
    }

    pub fn view_projection(&self) -> Mat4 {
        self.view_projection_with_fov(self.fov_radians(self.render_partial_tick))
    }

    pub fn sky_view_projection(&self) -> Mat4 {
        let (forward, up) = self.view_basis();
        let view = view::look_to_mat4(Vec3::ZERO, forward, up);
        let mut proj = proj::directx::perspective(
            self.fov_radians(self.render_partial_tick),
            self.aspect_ratio,
            NEAR,
            FAR,
        );
        proj.y_axis.y *= -1.0; // Vulkan NDC has +Y down
        proj * view
    }

    /// Vanilla `Camera.getViewRotationProjectionMatrix`: rotation-only view
    /// times the GL-convention projection — no view bob, no third-person
    /// translation, no Vulkan Y flip. Used by the locator bar's waypoint
    /// pitch test.
    pub fn view_rotation_projection(&self) -> Mat4 {
        let (forward, up) = self.view_basis();
        let view = view::look_to_mat4(Vec3::ZERO, forward, up);
        let proj = proj::opengl::perspective(
            self.fov_radians(self.render_partial_tick),
            self.aspect_ratio,
            NEAR,
            FAR,
        );
        proj * view
    }

    /// Camera yaw/pitch in degrees as vanilla `Camera.setRotation` sees them:
    /// the mirrored third-person view turns around (yaw + 180, pitch negated).
    pub fn effective_look_deg(&self) -> (f32, f32) {
        let yaw = self.look_dir.y_rot_deg();
        let pitch = self.look_dir.x_rot_deg();
        if self.mode == CameraMode::ThirdPersonFront {
            (yaw + 180.0, -pitch)
        } else {
            (yaw, pitch)
        }
    }

    pub fn fov_degrees(&self) -> f32 {
        self.fov_radians(self.render_partial_tick).to_degrees()
    }

    pub fn view_projection_with_fov(&self, fov: f32) -> Mat4 {
        let offset = self.third_person_offset();
        let (forward, up) = self.view_basis();
        let view = self.bob_matrix() * view::look_to_mat4(offset, forward, up);
        let mut proj = proj::directx::perspective(fov, self.aspect_ratio, NEAR, FAR);
        proj.y_axis.y *= -1.0; // Vulkan NDC has +Y down
        proj * view
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    /// xyz: eye position relative to the render anchor (small, full f32
    /// precision; see `Camera::anchor`), w: fog start. All world-space data
    /// is uploaded anchor-relative, so shaders never see large floats.
    camera_pos: [f32; 4],
    fog_color: [f32; 4],
    /// xyz: the anchor as integers (vanilla `CameraBlockPos`). Declared only
    /// in chunk.vert, which subtracts it from the absolute integer section
    /// origins; the other shaders keep the shorter block prefix.
    camera_block: [i32; 4],
}

// Vanilla FogType.WATER defaults (EnvironmentAttributes): color 0xFF050533,
// start -8, end 96. Vanilla scales end by getWaterVision() (min 0.25); we use
// vision = 1.0. TODO: water-vision ramp (fog brightens as the eyes adjust) and
// lava fog.
pub const WATER_FOG_COLOR: [f32; 3] = [5.0 / 255.0, 5.0 / 255.0, 51.0 / 255.0];
const WATER_FOG_START: f32 = -8.0;
const WATER_FOG_END: f32 = 96.0;

impl CameraUniform {
    pub fn new(
        camera: &Camera,
        sky_color: [f32; 3],
        render_distance_chunks: u32,
        eyes_in_water: bool,
    ) -> Self {
        let anchor = camera.anchor();
        let offset = camera.third_person_offset();
        let pos = (*camera.position - anchor).as_vec3() + offset;
        // Vanilla render-distance fog band: the last clamp(blocks / 10, 4, 64) blocks.
        // The top-down benchmark view sits hundreds of blocks up, so push fog past the
        // far plane to keep the whole loaded area visible.
        let (fog_start, fog_end, fog_rgb) = if camera.top_down().is_some() {
            (FAR, FAR, sky_color)
        } else if eyes_in_water {
            (WATER_FOG_START, WATER_FOG_END, WATER_FOG_COLOR)
        } else {
            let blocks = (render_distance_chunks * 16) as f32;
            let span = (blocks / 10.0).clamp(4.0, 64.0);
            // Fade distant terrain to the sky color so it melts into the flat sky disc
            // with no horizon edge (at night both go dark).
            (blocks - span, blocks, sky_color)
        };
        Self {
            view_proj: camera.view_projection().to_cols_array_2d(),
            camera_pos: [pos.x, pos.y, pos.z, fog_start],
            fog_color: [fog_rgb[0], fog_rgb[1], fog_rgb[2], fog_end],
            camera_block: anchor.as_ivec3().extend(0).to_array(),
        }
    }

    pub fn with_view_proj(view_proj: Mat4) -> Self {
        Self {
            view_proj: view_proj.to_cols_array_2d(),
            camera_pos: [0.0; 4],
            fog_color: [0.0; 4],
            camera_block: [0; 4],
        }
    }
}
