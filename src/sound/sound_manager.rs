use crate::{
    resources::identifier::Identifier,
    sound::sounds::{Sound, SoundType, WeighedSoundEvents},
};
use std::sync::LazyLock;

pub static EMPTY_SOUND_LOCATION: LazyLock<Identifier> =
    LazyLock::new(|| Identifier::with_default_namespace("empty").unwrap());

pub static EMPTY_SOUND: LazyLock<Sound> = LazyLock::new(|| {
    Sound::new(
        Identifier::new("empty"),
        1.0,
        1.0,
        1,
        SoundType::File,
        false,
        false,
        16,
    )
});

pub static INTENTIONALLY_EMPTY_SOUND_LOCATION: LazyLock<Identifier> =
    LazyLock::new(|| Identifier::with_default_namespace("intentionally_empty").unwrap());

pub static INTENTIONALLY_EMPTY_SOUND: LazyLock<Sound> = LazyLock::new(|| {
    Sound::new(
        INTENTIONALLY_EMPTY_SOUND_LOCATION.clone(),
        1.0,
        1.0,
        1,
        SoundType::File,
        false,
        false,
        16,
    )
});

pub static INTENTIONALLY_EMPTY_SOUND_EVENT: LazyLock<WeighedSoundEvents> =
    LazyLock::new(|| WeighedSoundEvents::new());
