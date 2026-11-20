use std::rc::Rc;

use glam::Vec3;

use super::types::{ParticleType, TypedParticleOptions};

pub(crate) struct DustColorTransitionOptions {
    particle_type: Rc<ParticleType<Self>>,
    from_color: u32,
    to_color: u32,
    scale: f32,
}

impl DustColorTransitionOptions {
    pub(crate) fn new(
        particle_type: Rc<ParticleType<Self>>,
        from_color: u32,
        to_color: u32,
        scale: f32,
    ) -> Self {
        Self {
            particle_type,
            from_color,
            to_color,
            scale: scale.clamp(0.01, 4.0),
        }
    }

    pub(crate) fn sculk_to_redstone(particle_type: Rc<ParticleType<Self>>) -> Self {
        Self::new(particle_type, 3790560, 16711680, 1.0)
    }

    pub(crate) fn get_from_color(&self) -> Vec3 {
        Self::rgb(self.from_color)
    }

    pub(crate) fn get_to_color(&self) -> Vec3 {
        Self::rgb(self.to_color)
    }

    pub(crate) fn get_scale(&self) -> f32 {
        self.scale
    }

    fn rgb(color: u32) -> Vec3 {
        Vec3::new(
            ((color >> 16) & 0xFF) as f32 / 255.0,
            ((color >> 8) & 0xFF) as f32 / 255.0,
            (color & 0xFF) as f32 / 255.0,
        )
    }
}

impl TypedParticleOptions for DustColorTransitionOptions {
    fn particle_type(&self) -> &ParticleType<Self> {
        &self.particle_type
    }
}
