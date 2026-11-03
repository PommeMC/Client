//! Owned extraction values, not GPU pipelines or simulation storage.

use glam::{DVec3, Quat, Vec3};

/// Engine grouping, unrelated to atlas selection or a provider's Rust type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticleRenderType {
    SingleQuads,
    ItemPickup,
    ElderGuardians,
    NoRender,
}

/// Rust spelling of Vanilla `SingleQuadParticle.Layer` (traits cannot contain
/// nested enum declarations). Opaque includes cutout pixels. The existing
/// combined GPU atlas is a backend adaptation, not six physical atlas promises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticleLayer {
    Opaque,
    Translucent,
    BlockOpaque,
    BlockTranslucent,
    ItemOpaque,
    ItemTranslucent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticleFacing {
    Camera,
    CameraY,
}

/// Smooth lightmap coordinates, each 0..=240. Not scalar brightness or RGB
/// multiplication. The backend converts these to its existing packed light
/// lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParticleLight {
    pub block: u8,
    pub sky: u8,
}

/// Narrow extraction input. Quad groups point-test current world positions;
/// culling affects extraction only, never ticking or lifetime.
pub trait ParticleFrustum {
    fn contains(&self, position: DVec3) -> bool;
}

pub struct ParticleView<'a> {
    pub position: DVec3,
    pub rotation: Quat,
    pub partial_tick: f32,
    pub frustum: &'a dyn ParticleFrustum,
}

/// One owned quad entry, not the group's accumulator or a mandatory
/// per-particle result. SingleQuadParticle extraction appends zero or more
/// entries to a Vec. Interpolate/subtract the view position in f64 before
/// casting to f32. Size is half-width; rotation includes facing and
/// interpolated roll.
pub struct QuadParticleRenderState {
    pub position: Vec3,
    pub rotation: Quat,
    pub size: f32,
    /// u0, u1, v0, v1; may describe a cropped or reversed sprite region.
    pub uv: [f32; 4],
    pub color: [f32; 4],
    pub light: ParticleLight,
    pub layer: ParticleLayer,
}
