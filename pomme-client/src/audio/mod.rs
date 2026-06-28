mod sounds;

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use rodio::source::ChannelVolume;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};

use self::sounds::{SoundsIndex, sound_asset_key};
use crate::assets::{AssetIndex, resolve_asset_path};
use crate::entity::components::Position;

const MENU_MUSIC_EVENT: &str = "music.menu";
const UI_CLICK_EVENT: &str = "ui.button.click";

/// Vanilla `SimpleSoundInstance.forUI` plays the click at this fixed volume.
const UI_CLICK_VOLUME: f32 = 0.25;

/// Vanilla `Music(MUSIC_MENU)` waits a random 20..600 tick gap between tracks
/// (1.0s..30.0s at 20 ticks/second).
const MENU_MUSIC_MIN_GAP: f32 = 1.0;
const MENU_MUSIC_MAX_GAP: f32 = 30.0;

/// Half the distance between the listener's ears, in blocks. Wider values
/// exaggerate left/right panning.
const LISTENER_EAR_OFFSET: f32 = 0.5;

/// Vanilla's linear attenuation distance in blocks for a normal sound.
const SOUND_ATTENUATION_BLOCKS: f32 = 16.0;

/// Sound categories, matching the protocol `SoundSource` order so a packet's
/// source index maps straight onto a volume slot.
#[derive(Clone, Copy)]
enum SoundCategory {
    Master = 0,
    Music = 1,
    Records = 2,
    Weather = 3,
    Blocks = 4,
    Hostile = 5,
    Neutral = 6,
    Players = 7,
    Ambient = 8,
    Voice = 9,
}

/// `SoundSource::BLOCKS` index, for emitting block sounds (e.g. mining)
/// directly.
pub const CATEGORY_BLOCKS: u8 = SoundCategory::Blocks as u8;

/// `SoundSource::PLAYERS` index, for client-side player sounds (e.g. item
/// pickup).
pub const CATEGORY_PLAYERS: u8 = SoundCategory::Players as u8;

impl SoundCategory {
    fn from_index(index: u8) -> Self {
        match index {
            1 => Self::Music,
            2 => Self::Records,
            3 => Self::Weather,
            4 => Self::Blocks,
            5 => Self::Hostile,
            6 => Self::Neutral,
            7 => Self::Players,
            8 => Self::Ambient,
            9 => Self::Voice,
            _ => Self::Master,
        }
    }
}

/// How a server sound resolves to a file: either a `sounds.json` event (which
/// maps to one or more variants) or a direct sound-file path.
#[derive(Clone)]
pub enum SoundRef {
    /// A `sounds.json` event name, e.g. `block.stone.break`.
    Event(String),
    /// A direct sound path, e.g. `minecraft:custom/foo`.
    Direct(String),
}

/// The rodio output device. The `MixerDeviceSink` must be kept alive for the
/// whole program; sounds play by connecting `Player`s to its mixer.
struct Output {
    sink: MixerDeviceSink,
}

/// Plays menu and in-world sounds, resolving `.ogg` files through the same
/// asset pipeline used for textures. Degrades to a silent no-op when no audio
/// output device is available.
pub struct AudioEngine {
    output: Option<Output>,
    jar_assets_dir: PathBuf,
    asset_index: Option<AssetIndex>,
    sounds: SoundsIndex,
    /// Per-category volumes (0.0..=1.0), indexed by `SoundCategory as usize`.
    volumes: [f32; 10],
    listener_left: [f32; 3],
    listener_right: [f32; 3],
    music_sink: Option<Player>,
    /// Per-entry `sounds.json` volume of the track currently in `music_sink`,
    /// reapplied each frame so live volume changes keep its relative loudness.
    music_track_volume: f32,
    menu_music_active: bool,
    gap_remaining: f32,
}

impl AudioEngine {
    pub fn new(jar_assets_dir: &Path, asset_index: Option<AssetIndex>, volumes: [f32; 10]) -> Self {
        let output = match DeviceSinkBuilder::open_default_sink() {
            Ok(mut sink) => {
                // Only dropped at shutdown, so silence rodio's stderr drop warning.
                sink.log_on_drop(false);
                Some(Output { sink })
            }
            Err(e) => {
                tracing::warn!("audio disabled: no output device ({e})");
                None
            }
        };
        let sounds = SoundsIndex::load(jar_assets_dir, &asset_index);
        Self {
            output,
            jar_assets_dir: jar_assets_dir.to_path_buf(),
            asset_index,
            sounds,
            volumes,
            listener_left: [-LISTENER_EAR_OFFSET, 0.0, 0.0],
            listener_right: [LISTENER_EAR_OFFSET, 0.0, 0.0],
            music_sink: None,
            music_track_volume: 1.0,
            menu_music_active: false,
            gap_remaining: 0.0,
        }
    }

    /// Master-scaled gain for a category. The Master category itself is not
    /// scaled twice.
    fn category_gain(&self, category: SoundCategory) -> f32 {
        let master = self.volumes[SoundCategory::Master as usize];
        match category {
            SoundCategory::Master => master,
            other => master * self.volumes[other as usize],
        }
    }

    fn current_music_volume(&self) -> f32 {
        self.category_gain(SoundCategory::Music) * self.music_track_volume
    }

    /// Sets all per-category volumes (0.0..=1.0), applied live to any playing
    /// menu track. No-op when the volumes are unchanged, so callers can invoke
    /// it every frame cheaply.
    pub fn set_volumes(&mut self, volumes: [f32; 10]) {
        if volumes == self.volumes {
            return;
        }
        self.volumes = volumes;
        if let Some(sink) = self.music_sink.as_ref() {
            sink.set_volume(self.current_music_volume());
        }
    }

    /// Updates the listener (camera) position and facing for positional audio.
    pub fn set_listener(&mut self, pos: Position, y_rot_deg: f32) {
        let y_rot = y_rot_deg.to_radians();
        let rx = -y_rot.cos() * LISTENER_EAR_OFFSET;
        let rz = -y_rot.sin() * LISTENER_EAR_OFFSET;

        self.listener_left = [pos.x as f32 - rx, pos.y as f32, pos.z as f32 - rz];
        self.listener_right = [pos.x as f32 + rx, pos.y as f32, pos.z as f32 + rz];
    }

    /// Plays the vanilla button click: MASTER category at the fixed `forUI`
    /// volume.
    pub fn play_ui_click(&self) {
        if let Some((sink, entry_volume)) = self.make_sink(UI_CLICK_EVENT) {
            sink.set_volume(
                self.category_gain(SoundCategory::Master) * UI_CLICK_VOLUME * entry_volume,
            );
            sink.detach();
        }
    }

    /// Plays a positional world sound at `pos`, mixed for its category and
    /// spatialized relative to the current listener.
    pub fn play_world_sound(
        &self,
        sound: &SoundRef,
        category: u8,
        pos: Position,
        volume: f32,
        pitch: f32,
        seed: u64,
    ) {
        let Some(output) = self.output.as_ref() else {
            return;
        };
        let Some((source, entry_volume)) = self.decode_sound(sound, seed) else {
            return;
        };
        let emitter: [f32; 3] = pos.as_vec3().into();
        let center = [
            (self.listener_left[0] + self.listener_right[0]) * 0.5,
            (self.listener_left[1] + self.listener_right[1]) * 0.5,
            (self.listener_left[2] + self.listener_right[2]) * 0.5,
        ];

        // Vanilla linear distance attenuation, not rodio's 1/d^2.
        let instance_volume = volume * entry_volume;
        let atten_dist = instance_volume.max(1.0) * SOUND_ATTENUATION_BLOCKS;
        let dist_gain = linear_attenuation(dist(center, emitter), atten_dist);
        if dist_gain <= 0.0 {
            return;
        }

        let (left_pan, right_pan) = stereo_pan(self.listener_left, self.listener_right, emitter);
        let base =
            self.category_gain(SoundCategory::from_index(category)) * instance_volume * dist_gain;

        let sink = Player::connect_new(output.sink.mixer());
        sink.set_speed(pitch.max(0.01));
        sink.append(ChannelVolume::new(
            source,
            vec![base * left_pan, base * right_pan],
        ));
        sink.detach();
    }

    /// Begins menu music. Idempotent, so it is safe to call every frame.
    pub fn start_menu_music(&mut self) {
        if !self.menu_music_active {
            self.menu_music_active = true;
            self.gap_remaining = 0.0;
        }
    }

    pub fn stop_menu_music(&mut self) {
        self.menu_music_active = false;
        self.gap_remaining = 0.0;
        if let Some(sink) = self.music_sink.take() {
            sink.stop();
        }
    }

    /// Advances menu music: syncs the live volume, schedules a random gap after
    /// each finished track, and starts the next track once the gap elapses.
    pub fn update_menu_music(&mut self, dt: f32) {
        if !self.menu_music_active {
            return;
        }
        if let Some(sink) = self.music_sink.as_ref() {
            sink.set_volume(self.current_music_volume());
            if !sink.empty() {
                return;
            }
        }
        // A finished track falls through to here; drop it and start the gap.
        if self.music_sink.take().is_some() {
            self.gap_remaining =
                MENU_MUSIC_MIN_GAP + fastrand::f32() * (MENU_MUSIC_MAX_GAP - MENU_MUSIC_MIN_GAP);
            return;
        }
        if self.gap_remaining > 0.0 {
            self.gap_remaining -= dt;
            return;
        }
        self.play_menu_track();
    }

    fn play_menu_track(&mut self) {
        if let Some((sink, track_volume)) = self.make_sink(MENU_MUSIC_EVENT) {
            self.music_track_volume = track_volume;
            sink.set_volume(self.current_music_volume());
            self.music_sink = Some(sink);
        }
    }

    /// Decodes a weighted-random variant of `event` into a queued sink,
    /// returned with the variant's per-entry volume for the caller to
    /// apply.
    fn make_sink(&self, event: &str) -> Option<(Player, f32)> {
        let output = self.output.as_ref()?;
        let (source, volume) = self.decode_event(event, None)?;
        let sink = Player::connect_new(output.sink.mixer());
        sink.append(source);
        Some((sink, volume))
    }

    fn decode_sound(&self, sound: &SoundRef, seed: u64) -> Option<(Decoder<BufReader<File>>, f32)> {
        match sound {
            SoundRef::Event(name) => self.decode_event(name, Some(seed)),
            SoundRef::Direct(path) => Some((self.open_decoder(&sound_asset_key(path))?, 1.0)),
        }
    }

    /// Resolves `event` to a variant (seeded when `seed` is set, else random)
    /// and decodes its `.ogg`, returning the decoder and per-entry volume.
    fn decode_event(
        &self,
        event: &str,
        seed: Option<u64>,
    ) -> Option<(Decoder<BufReader<File>>, f32)> {
        let (name, volume) = self.choose_variant(event, seed)?;
        Some((self.open_decoder(&sound_asset_key(&name))?, volume))
    }

    fn choose_variant(&self, event: &str, seed: Option<u64>) -> Option<(String, f32)> {
        let variants = self.sounds.variants(event)?;
        let total: u32 = variants.iter().map(|v| v.weight).sum();
        if total == 0 {
            return None;
        }
        let mut pick = match seed {
            Some(s) => (s % total as u64) as u32,
            None => fastrand::u32(0..total),
        };
        for v in variants {
            if pick < v.weight {
                return Some((v.name.clone(), v.volume));
            }
            pick -= v.weight;
        }
        let first = &variants[0];
        Some((first.name.clone(), first.volume))
    }

    fn open_decoder(&self, key: &str) -> Option<Decoder<BufReader<File>>> {
        let path = resolve_asset_path(&self.jar_assets_dir, &self.asset_index, key);
        let file = File::open(&path)
            .map_err(|e| tracing::warn!("failed to open sound {}: {e}", path.display()))
            .ok()?;
        Decoder::new(BufReader::new(file))
            .map_err(|e| tracing::warn!("failed to decode sound {}: {e}", path.display()))
            .ok()
    }
}

/// Vanilla linear sound attenuation: 1.0 at the listener, falling to 0.0 at
/// `atten_dist` and beyond (OpenAL `AL_LINEAR_DISTANCE_CLAMPED`, rolloff 1).
fn linear_attenuation(distance: f32, atten_dist: f32) -> f32 {
    1.0 - (distance / atten_dist).clamp(0.0, 1.0)
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Per-channel panning gains from each ear's distance to the emitter, matching
/// rodio's `Spatial` balance so the left/right feel is unchanged.
fn stereo_pan(left_ear: [f32; 3], right_ear: [f32; 3], emitter: [f32; 3]) -> (f32, f32) {
    let left_d = dist(left_ear, emitter);
    let right_d = dist(right_ear, emitter);
    let max_diff = dist(left_ear, right_ear);
    let left = (((left_d - right_d) / max_diff + 1.0) / 4.0 + 0.5).min(1.0);
    let right = (((right_d - left_d) / max_diff + 1.0) / 4.0 + 0.5).min(1.0);
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_attenuation_matches_vanilla() {
        let d = SOUND_ATTENUATION_BLOCKS;
        assert_eq!(linear_attenuation(0.0, d), 1.0);
        assert_eq!(linear_attenuation(8.0, d), 0.5);
        assert_eq!(linear_attenuation(16.0, d), 0.0);
        assert_eq!(linear_attenuation(24.0, d), 0.0);
    }
}
