use crate::{
    resources::identifier::Identifier,
    sound::{
        SoundEngine,
        sound_manager::{
            EMPTY_SOUND, INTENTIONALLY_EMPTY_SOUND, INTENTIONALLY_EMPTY_SOUND_EVENT,
            INTENTIONALLY_EMPTY_SOUND_LOCATION,
        },
        sounds::{Sound, SoundSource, WeighedSoundEvents},
    },
    util::rng::JavaRng,
};

pub trait SoundInstance {
    fn resolve(&mut self, sound_manager: &SoundEngine) -> Option<WeighedSoundEvents>;
    fn sound(&self) -> Option<Sound>;
    fn source(&self) -> SoundSource;
    fn is_looping(&self) -> bool;
    fn is_relative(&self) -> bool;
    fn delay(&self) -> i32;
    fn volume(&self) -> f32;
    fn pitch(&self) -> f32;
    fn x(&self) -> f64;
    fn y(&self) -> f64;
    fn z(&self) -> f64;
    fn attenuation(&self) -> Attenuation;

    fn can_start_silent(&self) -> bool {
        false
    }

    fn can_play_sound(&self) -> bool {
        true
    }

    fn display_name(&self) -> String {
        self.sound()
            .map(|sound| sound.to_string())
            .unwrap_or_else(|| "Sound[unknown]".to_string())
    }

    fn create_unseeded_random(&self) -> JavaRng {
        JavaRng::new_from_random_seed()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attenuation {
    None,
    Linear,
}

trait TickableSoundInstance {
    fn is_stopped(&self) -> bool;
    fn tick(&mut self);
}

struct AbstractSoundInstance {
    sound: Sound,
    source: SoundSource,
    identifier: Identifier,
    volume: f32,
    pitch: f32,
    x: f64,
    y: f64,
    z: f64,
    looping: bool,
    delay: i32,
    attenuation: Attenuation,
    relative: bool,
    random: JavaRng,
}

impl AbstractSoundInstance {}

impl SoundInstance for AbstractSoundInstance {
    fn resolve(&mut self, sound_manager: &SoundManager) -> Option<WeighedSoundEvents> {
        if self.identifier == *INTENTIONALLY_EMPTY_SOUND_LOCATION {
            self.sound = *INTENTIONALLY_EMPTY_SOUND;
            Some(*INTENTIONALLY_EMPTY_SOUND_EVENT)
        } else {
            if let Some(sound_event) = sound_manager.get_sound_event(self.identifier) {
                self.sound = sound_event.get_sound(self.random);

                sound_event
            } else {
                self.sound = *EMPTY_SOUND;

                None
            }
        }
    }

    fn sound(&self) -> Option<Sound> {
        todo!()
    }

    fn source(&self) -> SoundSource {
        todo!()
    }

    fn is_looping(&self) -> bool {
        todo!()
    }

    fn is_relative(&self) -> bool {
        todo!()
    }

    fn delay(&self) -> i32 {
        todo!()
    }

    fn volume(&self) -> f32 {
        todo!()
    }

    fn pitch(&self) -> f32 {
        todo!()
    }

    fn x(&self) -> f64 {
        todo!()
    }

    fn y(&self) -> f64 {
        todo!()
    }

    fn z(&self) -> f64 {
        todo!()
    }

    fn attenuation(&self) -> Attenuation {
        todo!()
    }
}

struct AbstractTickablelSoundInstance {
    stopped: bool,
}

impl TickableSoundInstance for AbstractTickablelSoundInstance {
    fn is_stopped(&self) -> bool {
        self.stopped
    }

    fn tick(&mut self) {}
}

pub struct EntityBoundSoundInstance {}
