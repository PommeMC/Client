use tracing::{debug, warn};

pub mod sound_instance;
pub mod sound_manager;
pub mod sounds;

use crate::{assets::AssetIndex, sound::sound_instance::SoundInstance};

pub struct SoundEngine {
    pub assets: AssetIndex,
}

impl SoundEngine {
    pub fn new(assets: AssetIndex) -> Self {
        Self { assets }
    }

    pub fn play(&self, sound: &mut dyn SoundInstance) {
        if let Some(resolved) = sound.resolve(self) {
            debug!("Playing sound {}", resolved);
        } else {
            warn!("Unable to resolve sound: {}", sound.display_name());
        }
    }
}
