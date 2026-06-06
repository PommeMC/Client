mod sounds;

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, SpatialSink};

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

/// The rodio output device. `OutputStream` must be kept alive for the whole
/// program, hence it is stored even though it is never read directly.
struct Output {
    _stream: OutputStream,
    handle: OutputStreamHandle,
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
    music_sink: Option<Sink>,
    /// Per-entry `sounds.json` volume of the track currently in `music_sink`,
    /// reapplied each frame so live volume changes keep its relative loudness.
    music_track_volume: f32,
    menu_music_active: bool,
    gap_remaining: f32,
}

impl AudioEngine {
    pub fn new(jar_assets_dir: &Path, asset_index: Option<AssetIndex>, volumes: [f32; 10]) -> Self {
        let output = match OutputStream::try_default() {
            Ok((stream, handle)) => Some(Output {
                _stream: stream,
                handle,
            }),
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
        let Ok(sink) = SpatialSink::try_new(
            &output.handle,
            pos.as_vec3().into(),
            self.listener_left,
            self.listener_right,
        ) else {
            return;
        };
        let gain = self.category_gain(SoundCategory::from_index(category));
        sink.set_volume(gain * volume * entry_volume);
        sink.set_speed(pitch.max(0.01));
        sink.append(source);
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
    fn make_sink(&self, event: &str) -> Option<(Sink, f32)> {
        let output = self.output.as_ref()?;
        let (source, volume) = self.decode_event(event, None)?;
        let sink = Sink::try_new(&output.handle).ok()?;
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
