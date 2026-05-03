use glam::{DVec3, Mat4, Vec3};

use crate::window::input::InputState;

const UP: Vec3 = Vec3::Y;
pub const DEFAULT_FOV_DEGREES: f32 = 70.0;
#[allow(dead_code)]
pub const MIN_FOV_DEGREES: f32 = 30.0;
#[allow(dead_code)]
pub const MAX_FOV_DEGREES: f32 = 110.0;
const NEAR: f32 = 0.1;
const FAR: f32 = 1000.0;
const SENSITIVITY: f32 = 0.003;
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;
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

pub struct Camera {
    pub position: Vec3,
    pub position_f64: DVec3,
    pub yaw: f32,
    pub pitch: f32,
    pub mode: CameraMode,
    pub third_person_dist: f32,
    aspect_ratio: f32,
    pub base_fov_degrees: f32,
    fov_modifier: f32,
    old_fov_modifier: f32,
}

impl Camera {
    pub fn new(aspect_ratio: f32) -> Self {
        Self {
            position: Vec3::new(0.0, 2.0, 5.0),
            position_f64: DVec3::new(0.0, 2.0, 5.0),
            yaw: 0.0,
            pitch: 0.0,
            mode: CameraMode::FirstPerson,
            third_person_dist: THIRD_PERSON_DISTANCE,
            aspect_ratio,
            base_fov_degrees: DEFAULT_FOV_DEGREES,
            fov_modifier: 1.0,
            old_fov_modifier: 1.0,
        }
    }

    pub fn update_look(&mut self, input: &mut InputState) {
        if input.is_cursor_captured() {
            let (dx, dy) = input.consume_mouse_delta();
            self.yaw -= dx as f32 * SENSITIVITY;
            self.pitch = (self.pitch - dy as f32 * SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }
    }

    pub fn set_aspect_ratio(&mut self, aspect: f32) {
        self.aspect_ratio = aspect;
    }

    pub fn set_position(&mut self, position: Vec3, yaw_degrees: f32, pitch_degrees: f32) {
        self.position = position;
        self.position_f64 = DVec3::new(position.x as f64, position.y as f64, position.z as f64);
        self.yaw = yaw_degrees.to_radians();
        self.pitch = pitch_degrees.to_radians();
    }

    pub fn set_position_f64(&mut self, pos: DVec3) {
        self.position_f64 = pos;
        self.position = pos.as_vec3();
    }

    #[allow(dead_code)]
    pub fn camera_relative_f32(&self, world_pos: DVec3) -> Vec3 {
        (world_pos - self.position_f64).as_vec3()
    }

    pub fn update_fov_modifier(&mut self, target: f32) {
        self.old_fov_modifier = self.fov_modifier;
        self.fov_modifier += (target - self.fov_modifier) * 0.5;
        self.fov_modifier = self.fov_modifier.clamp(0.1, 1.5);
    }

    pub fn fov_radians(&self, partial_tick: f32) -> f32 {
        let modifier =
            self.old_fov_modifier + (self.fov_modifier - self.old_fov_modifier) * partial_tick;
        (self.base_fov_degrees * modifier).to_radians()
    }

    pub fn frustum_planes(&self) -> [[f32; 4]; 6] {
        let m = self.view_projection();
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

    pub fn forward_vec(&self) -> Vec3 {
        Vec3::new(
            -self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            -self.yaw.cos() * self.pitch.cos(),
        )
    }

    pub fn third_person_offset(&self) -> Vec3 {
        let fwd = self.forward_vec();
        match self.mode {
            CameraMode::FirstPerson => Vec3::ZERO,
            CameraMode::ThirdPersonBack => -fwd * self.third_person_dist,
            CameraMode::ThirdPersonFront => fwd * self.third_person_dist,
        }
    }

    pub fn view_projection(&self) -> Mat4 {
        self.view_projection_with_fov(self.fov_radians(1.0))
    }

    pub fn sky_view_projection(&self) -> Mat4 {
        let forward = self.forward_vec();
        let look_dir = if self.mode == CameraMode::ThirdPersonFront {
            -forward
        } else {
            forward
        };
        let view = Mat4::look_to_rh(Vec3::ZERO, look_dir, UP);
        let mut proj = Mat4::perspective_rh(self.fov_radians(1.0), self.aspect_ratio, NEAR, FAR);
        proj.y_axis.y *= -1.0;
        proj * view
    }

    pub fn view_projection_with_fov(&self, fov: f32) -> Mat4 {
        let forward = self.forward_vec();
        let offset = self.third_person_offset();
        let look_dir = if self.mode == CameraMode::ThirdPersonFront {
            -forward
        } else {
            forward
        };
        let view = Mat4::look_to_rh(offset, look_dir, UP);
        let mut proj = Mat4::perspective_rh(fov, self.aspect_ratio, NEAR, FAR);
        proj.y_axis.y *= -1.0;
        proj * view
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    fog_color: [f32; 4],
}

impl CameraUniform {
    pub fn new(camera: &Camera, fog_color: [f32; 3]) -> Self {
        let offset = camera.third_person_offset();
        let pos = camera.position + offset;
        Self {
            view_proj: camera.view_projection().to_cols_array_2d(),
            camera_pos: [pos.x, pos.y, pos.z, 0.0],
            fog_color: [fog_color[0], fog_color[1], fog_color[2], 0.0],
        }
    }
}
