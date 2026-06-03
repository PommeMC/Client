mod sounds;

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

use self::sounds::{SoundsIndex, sound_asset_key};
use crate::assets::{AssetIndex, resolve_asset_path};

const MENU_MUSIC_EVENT: &str = "music.menu";
const UI_CLICK_EVENT: &str = "ui.button.click";

/// Vanilla `SimpleSoundInstance.forUI` plays the click at this fixed volume.
const UI_CLICK_VOLUME: f32 = 0.25;

/// Vanilla `Music(MUSIC_MENU)` waits a random 20..600 tick gap between tracks
/// (1.0s..30.0s at 20 ticks/second).
const MENU_MUSIC_MIN_GAP: f32 = 1.0;
const MENU_MUSIC_MAX_GAP: f32 = 30.0;

/// The rodio output device. `OutputStream` must be kept alive for the whole
/// program, hence it is stored even though it is never read directly.
struct Output {
    _stream: OutputStream,
    handle: OutputStreamHandle,
}

/// Plays UI and music sounds for the menu, resolving `.ogg` files through the
/// same asset pipeline used for textures. Degrades to a silent no-op when no
/// audio output device is available.
pub struct AudioEngine {
    output: Option<Output>,
    jar_assets_dir: PathBuf,
    asset_index: Option<AssetIndex>,
    sounds: SoundsIndex,
    master: f32,
    music: f32,
    music_sink: Option<Sink>,
    /// Per-entry `sounds.json` volume of the track currently in `music_sink`,
    /// reapplied each frame so live volume changes keep its relative loudness.
    music_track_volume: f32,
    menu_music_active: bool,
    gap_remaining: f32,
}

impl AudioEngine {
    pub fn new(
        jar_assets_dir: &Path,
        asset_index: Option<AssetIndex>,
        master: f32,
        music: f32,
    ) -> Self {
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
            master,
            music,
            music_sink: None,
            music_track_volume: 1.0,
            menu_music_active: false,
            gap_remaining: 0.0,
        }
    }

    /// Combined `master * music * per-track` volume for the active menu track.
    fn current_music_volume(&self) -> f32 {
        self.master * self.music * self.music_track_volume
    }

    /// Sets the master/music volumes (0.0..=1.0), applied live to any playing
    /// track.
    pub fn set_volumes(&mut self, master: f32, music: f32) {
        self.master = master;
        self.music = music;
        if let Some(sink) = self.music_sink.as_ref() {
            sink.set_volume(self.current_music_volume());
        }
    }

    /// Plays the vanilla button click: MASTER category at the fixed `forUI`
    /// volume.
    pub fn play_ui_click(&self) {
        if let Some((sink, entry_volume)) = self.make_sink(UI_CLICK_EVENT) {
            sink.set_volume(self.master * UI_CLICK_VOLUME * entry_volume);
            sink.detach();
        }
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
        let (source, volume) = self.decode_event(event)?;
        let sink = Sink::try_new(&output.handle).ok()?;
        sink.append(source);
        Some((sink, volume))
    }

    /// Picks a weighted-random variant for `event`, decodes its `.ogg`, and
    /// returns the decoder together with the variant's per-entry volume.
    fn decode_event(&self, event: &str) -> Option<(Decoder<BufReader<File>>, f32)> {
        let variants = self.sounds.variants(event)?;
        let total: u32 = variants.iter().map(|v| v.weight).sum();
        if total == 0 {
            return None;
        }
        let mut pick = fastrand::u32(0..total);
        let mut chosen = &variants[0];
        for v in variants {
            if pick < v.weight {
                chosen = v;
                break;
            }
            pick -= v.weight;
        }
        let key = sound_asset_key(&chosen.name);
        let path = resolve_asset_path(&self.jar_assets_dir, &self.asset_index, &key);
        let file = File::open(&path)
            .map_err(|e| tracing::warn!("failed to open sound {}: {e}", path.display()))
            .ok()?;
        let decoder = Decoder::new(BufReader::new(file))
            .map_err(|e| tracing::warn!("failed to decode sound {}: {e}", path.display()))
            .ok()?;
        Some((decoder, chosen.volume))
    }
}
