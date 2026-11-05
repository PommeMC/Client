use std::rc::Rc;

use azalea_block::BlockState;

use super::types::{ParticleType, TypedParticleOptions};

pub(crate) struct BlockParticleOption {
    particle_type: Rc<ParticleType<Self>>,
    state: BlockState,
}

impl BlockParticleOption {
    pub(crate) fn new(particle_type: Rc<ParticleType<Self>>, state: BlockState) -> Self {
        Self {
            particle_type,
            state,
        }
    }

    pub(crate) fn get_state(&self) -> BlockState {
        self.state
    }
}

impl TypedParticleOptions for BlockParticleOption {
    fn particle_type(&self) -> &ParticleType<Self> {
        &self.particle_type
    }
}
