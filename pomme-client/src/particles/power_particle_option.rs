use std::rc::Rc;

use super::types::{ParticleType, TypedParticleOptions};

pub(crate) struct PowerParticleOption {
    particle_type: Rc<ParticleType<Self>>,
    power: f32,
}

impl PowerParticleOption {
    pub(crate) fn create(particle_type: Rc<ParticleType<Self>>, power: f32) -> Self {
        Self {
            particle_type,
            power,
        }
    }

    pub(crate) fn get_power(&self) -> f32 {
        self.power
    }
}

impl TypedParticleOptions for PowerParticleOption {
    fn particle_type(&self) -> &ParticleType<Self> {
        &self.particle_type
    }
}
