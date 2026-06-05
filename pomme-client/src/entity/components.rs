use std::ops::{Add, AddAssign, Deref, DerefMut, Sub, SubAssign};

use glam::{DVec3, Vec3, dvec3, vec3};

// -- POSITION --

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position(DVec3);

impl Position {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self(dvec3(x, y, z))
    }

    /// Performs a linear interpolation between `self` and `rhs` based on the
    /// value `s`.
    ///
    /// When `s` is `0.0`, the result will be equal to `self`.  When `s` is
    /// `1.0`, the result will be equal to `rhs`. When `s` is outside of
    /// range `[0, 1]`, the result is linearly extrapolated.
    #[doc(alias = "mix")]
    #[inline]
    #[must_use]
    pub fn lerp(self, rhs: Self, s: f64) -> Self {
        Self(self.0.lerp(rhs.0, s))
    }
}

impl Deref for Position {
    type Target = DVec3;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Position {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Default for Position {
    fn default() -> Self {
        Self(DVec3::ZERO)
    }
}

impl Add<DVec3> for Position {
    type Output = Self;

    fn add(self, rhs: DVec3) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl AddAssign<DVec3> for Position {
    fn add_assign(&mut self, rhs: DVec3) {
        self.0 += rhs;
    }
}

impl Sub<Position> for Position {
    type Output = DVec3;
    fn sub(self, rhs: Position) -> DVec3 {
        self.0 - rhs.0
    }
}

impl SubAssign<DVec3> for Position {
    fn sub_assign(&mut self, rhs: DVec3) {
        self.0 -= rhs
    }
}

impl From<DVec3> for Position {
    fn from(value: DVec3) -> Self {
        Self(value)
    }
}

impl From<Position> for DVec3 {
    fn from(value: Position) -> Self {
        value.0
    }
}

impl From<Position> for azalea_core::position::Vec3 {
    fn from(value: Position) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}

impl From<azalea_core::position::Vec3> for Position {
    fn from(value: azalea_core::position::Vec3) -> Self {
        Self(DVec3::new(value.x, value.y, value.z))
    }
}

// -- VELOCITY --

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Velocity(DVec3);

impl Velocity {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self(dvec3(x, y, z))
    }

    pub fn x_rot(self, radians: f64) -> Self {
        let (sin, cos) = radians.sin_cos();
        Self::new(
            self.x,
            self.y * cos - self.z * sin,
            self.y * sin + self.z * cos,
        )
    }

    pub fn y_rot(self, radians: f64) -> Self {
        let (sin, cos) = radians.sin_cos();
        Self::new(
            self.x * cos + self.z * sin,
            self.y,
            -self.x * sin + self.z * cos,
        )
    }
}

impl Deref for Velocity {
    type Target = DVec3;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Velocity {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Default for Velocity {
    fn default() -> Self {
        Self(DVec3::ZERO)
    }
}

impl From<DVec3> for Velocity {
    fn from(value: DVec3) -> Self {
        Self(value)
    }
}

impl From<Velocity> for DVec3 {
    fn from(value: Velocity) -> Self {
        value.0
    }
}

impl From<Velocity> for azalea_core::position::Vec3 {
    fn from(value: Velocity) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}

impl From<azalea_core::position::Vec3> for Velocity {
    fn from(value: azalea_core::position::Vec3) -> Self {
        Self(DVec3::new(value.x, value.y, value.z))
    }
}

// -- LOOK DIRECTION --

/// y_rot: n*360° + 0º: +Z (South), n*360° + 90°: -X (West)
///
/// x_rot: -90°: UP, 90°: DOWN
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct LookDirection {
    y_rot_deg: f32,
    x_rot_deg: f32,
}

impl LookDirection {
    pub fn new(y_rot_deg: f32, x_rot_deg: f32) -> Self {
        Self {
            y_rot_deg,
            x_rot_deg: x_rot_deg.clamp(-89.999, 89.999),
        }
    }

    pub fn y_rot_deg(&self) -> f32 {
        self.y_rot_deg
    }

    pub fn x_rot_deg(&self) -> f32 {
        self.x_rot_deg
    }

    pub fn y_rot_rad(&self) -> f32 {
        self.y_rot_deg.to_radians()
    }

    pub fn x_rot_rad(&self) -> f32 {
        self.x_rot_deg.to_radians()
    }

    pub fn as_vec(self) -> Vec3 {
        let y_rot_rad = self.y_rot_rad();
        let x_rot_rad = self.x_rot_rad();
        let (sin_y_rot, cos_y_rot) = y_rot_rad.sin_cos();
        let (sin_x_rot, cos_x_rot) = x_rot_rad.sin_cos();
        vec3(-sin_y_rot * cos_x_rot, -sin_x_rot, cos_y_rot * cos_x_rot)
    }
}

impl From<LookDirection> for azalea_entity::LookDirection {
    fn from(value: LookDirection) -> Self {
        Self::new(value.y_rot_deg, value.x_rot_deg)
    }
}
