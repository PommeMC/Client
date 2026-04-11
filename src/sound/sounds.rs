use std::fmt::Display;

use azalea_buf::BufReadError;
use azalea_chat::text_component::TextComponent;
use azalea_core::delta::AzBuf;

use crate::{resources::identifier::Identifier, sound::SoundEngine, util::rng::JavaRng};

pub trait Weighted<T> {
    fn get_weight(&self) -> i32;
    fn get_sound(&self, random: JavaRng) -> &T;
    fn preload_if_required(&mut self, sound_engine: &SoundEngine);
}

pub enum SoundSource {
    MASTER,
    MUSIC,
    RECORDS,
    WEATHER,
    BLOCKS,
    HOSTILE,
    NEUTRAL,
    PLAYERS,
    AMBIENT,
    VOICE,
    UI,
}

#[derive(Default)]
pub struct WeighedSoundEvents {
    list: Vec<Sound>,
    subtitle: TextComponent,
}

impl WeighedSoundEvents {
    pub fn new() -> Self {
        WeighedSoundEvents::default()
    }

    pub fn add_sound(&mut self, sound: Sound) {
        self.list.push(sound);
    }

    pub fn get_subtitle(&self) -> &TextComponent {
        return &self.subtitle;
    }
}

impl Weighted<Sound> for WeighedSoundEvents {
    fn get_weight(&self) -> i32 {
        let mut sum = 0;

        for sound in &self.list {
            sum += sound.get_weight();
        }

        sum
    }

    fn get_sound(&self, mut random: JavaRng) -> &Sound {
        let weight = self.get_weight();

        if !self.list.is_empty() && weight != 0 {
            for weighted in &self.list {
                let index = random.next_int(weight);

                if index < 0 {
                    return &weighted.get_sound(random);
                }
            }
        }

        &*super::sound_manager::EMPTY_SOUND
    }

    fn preload_if_required(&mut self, engine: &SoundEngine) {
        for weighted in &mut self.list {
            weighted.preload_if_required(engine);
        }
    }
}

impl Display for WeighedSoundEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WeighedSound[{}]", self.get_subtitle())
    }
}

pub enum SoundType {
    File,
    SoundEvent,
}

impl SoundType {
    pub fn from_name(name: &str) -> Option<SoundType> {
        match name {
            "file" => Some(SoundType::File),
            "event" => Some(SoundType::SoundEvent),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SoundType::File => "file",
            SoundType::SoundEvent => "event",
        }
    }
}

impl AzBuf for SoundType {
    fn azalea_read(buf: &mut std::io::Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let name = String::azalea_read(buf)?;
        SoundType::from_name(&name).ok_or(BufReadError::UnexpectedStringEnumVariant { id: name })
    }

    fn azalea_write(&self, buf: &mut impl std::io::Write) -> std::io::Result<()> {
        self.as_str().to_string().azalea_write(buf)
    }
}

pub struct Sound {
    location: Identifier,
    volume: f32,
    pitch: f32,
    weight: i32,
    sound_type: SoundType,
    stream: bool,
    preload: bool, // should all be preloaded?
    attenuation_distance: i32,
}

impl AzBuf for Sound {
    fn azalea_read(buf: &mut std::io::Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let location = Identifier::azalea_read(buf)?;
        let volume = f32::azalea_read(buf)?;
        let pitch = f32::azalea_read(buf)?;
        let weight = i32::azalea_read(buf)?;
        let sound_type = SoundType::azalea_read(buf)?;
        let stream = bool::azalea_read(buf)?;
        let preload = bool::azalea_read(buf)?;
        let attenuation_distance = i32::azalea_read(buf)?;

        Ok(Self {
            location,
            volume,
            pitch,
            weight,
            sound_type,
            stream,
            preload,
            attenuation_distance,
        })
    }

    fn azalea_write(&self, buf: &mut impl std::io::Write) -> std::io::Result<()> {
        self.location.azalea_write(buf)?;
        self.volume.azalea_write(buf)?;
        self.pitch.azalea_write(buf)?;
        self.weight.azalea_write(buf)?;
        self.sound_type.azalea_write(buf)?;
        self.stream.azalea_write(buf)?;
        self.preload.azalea_write(buf)?;
        self.attenuation_distance.azalea_write(buf)?;
        Ok(())
    }
}

impl Sound {
    pub fn new(
        location: Identifier,
        volume: f32,
        pitch: f32,
        weight: i32,
        sound_type: SoundType,
        stream: bool,
        preload: bool,
        attenuation_distance: i32,
    ) -> Self {
        Sound {
            location,
            volume,
            pitch,
            weight,
            sound_type,
            stream,
            preload,
            attenuation_distance,
        }
    }

    pub fn get_path(&self) -> Identifier {
        // SOUND_LISTER.id_to_file(&self.location)
        unimplemented!("path not yet implented as there is not yet a full registry impl")
    }

    pub fn get_location(&self) -> &Identifier {
        &self.location
    }

    pub fn get_volume(&self) -> f32 {
        self.volume
    }

    pub fn get_pitch(&self) -> f32 {
        self.pitch
    }

    pub fn should_stream(&self) -> bool {
        self.stream
    }

    pub fn should_preload(&self) -> bool {
        self.preload
    }

    pub fn get_attenuation_distance(&self) -> i32 {
        self.attenuation_distance
    }

    pub fn to_string(&self) -> String {
        format!("Sound[{}]", self.location)
    }
}

impl Weighted<Sound> for Sound {
    fn get_weight(&self) -> i32 {
        self.weight
    }

    fn get_sound(&self, _random: JavaRng) -> &Sound {
        self
    }

    fn preload_if_required(&mut self, engine: &SoundEngine) {
        todo!();
    }
}
