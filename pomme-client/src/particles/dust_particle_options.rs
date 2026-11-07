use std::rc::Rc;

use glam::Vec3;

use super::types::{ParticleType, TypedParticleOptions};

pub(crate) struct DustParticleOptions {
    particle_type: Rc<ParticleType<Self>>,
    color: u32,
    scale: f32,
}

impl DustParticleOptions {
    pub(crate) fn new(particle_type: Rc<ParticleType<Self>>, color: u32, scale: f32) -> Self {
        Self {
            particle_type,
            color,
            scale: scale.clamp(0.01, 4.0),
        }
    }

    pub(crate) fn redstone(particle_type: Rc<ParticleType<Self>>) -> Self {
        Self::new(particle_type, 0xFF0000, 1.0)
    }

    pub(crate) fn get_color(&self) -> Vec3 {
        Vec3::new(
            ((self.color >> 16) & 0xFF) as f32 / 255.0,
            ((self.color >> 8) & 0xFF) as f32 / 255.0,
            (self.color & 0xFF) as f32 / 255.0,
        )
    }

    pub(crate) fn get_scale(&self) -> f32 {
        self.scale
    }
}

impl TypedParticleOptions for DustParticleOptions {
    fn particle_type(&self) -> &ParticleType<Self> {
        &self.particle_type
    }
}
