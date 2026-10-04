mod decoder;
mod openal;
mod sounds;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use self::decoder::{StreamDecoder, decode_all};
use self::openal::{Buffer, Context, Source, SourceState};
use self::sounds::{SoundVariant, SoundsIndex};
use crate::assets::AssetIndex;
use crate::entity::components::Position;
use crate::resource_pack::ResourcePackManager;

const MENU_MUSIC_EVENT: &str = "music.menu";
const UI_CLICK_EVENT: &str = "ui.button.click";

/// Vanilla `SimpleSoundInstance.forUI` plays the click at this fixed volume.
const UI_CLICK_VOLUME: f32 = 0.25;

const SOUND_TICK_SECONDS: f32 = 1.0 / 20.0;
const LOGICAL_SOUND_RETENTION_TICKS: i32 = 20;
const MENU_MUSIC_STARTING_DELAY_TICKS: i32 = 100;
const MENU_MUSIC_MIN_DELAY_TICKS: i32 = 20;
const MENU_MUSIC_MAX_DELAY_TICKS: i32 = 600;

/// Sound categories, matching the protocol `SoundSource` order so a packet's
/// source index maps straight onto a volume slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundCategory {
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
    Ui = 10,
}

/// `SoundSource::BLOCKS` index, for emitting block sounds (e.g. mining)
/// directly.
pub const CATEGORY_BLOCKS: u8 = SoundCategory::Blocks as u8;

/// `SoundSource::PLAYERS` index, for client-side player sounds (e.g. item
/// pickup).
pub const CATEGORY_PLAYERS: u8 = SoundCategory::Players as u8;

/// `SoundSource::AMBIENT` index, for local ambience (e.g. the portal trigger).
pub const CATEGORY_AMBIENT: u8 = SoundCategory::Ambient as u8;

impl SoundCategory {
    pub const COUNT: usize = Self::Ui as usize + 1;

    pub fn from_index(index: u8) -> Self {
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
            10 => Self::Ui,
            _ => Self::Master,
        }
    }
}

/// A sound-event identifier resolved through `sounds.json`.
#[derive(Clone)]
pub struct SoundRef(String);

impl SoundRef {
    pub fn event(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// Both registry-backed and inline protocol holders are sound events.
    pub fn resolve(
        holder: &azalea_registry::Holder<
            azalea_registry::builtin::SoundEvent,
            azalea_core::sound::CustomSound,
        >,
    ) -> Self {
        let id = match holder {
            azalea_registry::Holder::Reference(event) => event.to_str().to_string(),
            azalea_registry::Holder::Direct(custom) => custom.sound_id.to_string(),
        };
        Self::event(id.strip_prefix("minecraft:").unwrap_or(&id))
    }

    fn event_name(&self) -> &str {
        &self.0
    }
}

/// A played sound recorded for the subtitle overlay (vanilla
/// `SoundEventListener.onPlaySound`).
#[derive(Clone, Copy, Debug)]
pub struct EntitySoundTarget {
    pub id: i32,
    pub pos: Position,
}

pub struct QueuedSubtitle {
    /// Subtitle translation key, e.g. `subtitles.block.anvil.land`.
    pub key: String,
    pub pos: Position,
    /// Audible range in blocks (vanilla `max(volume, 1.0) *
    /// attenuationDistance`).
    pub range: f32,
}

#[derive(Clone, Debug)]
struct ResolvedSound {
    sound_id: String,
    path: PathBuf,
    entry_volume: f32,
    entry_pitch: f32,
    stream: bool,
    attenuation_distance: f32,
}

#[derive(Debug)]
struct PlayCommand {
    id: u64,
    sound_id: String,
    path: PathBuf,
    category: SoundCategory,
    volume: f32,
    pitch: f32,
    position: [f32; 3],
    relative: bool,
    looping: bool,
    stream: bool,
    attenuation_distance: Option<f32>,
    entity_id: Option<i32>,
    report_completion: bool,
}

#[derive(Debug)]
enum AudioCommand {
    Play(PlayCommand),
    SetVolumes([f32; SoundCategory::COUNT]),
    SetListener {
        position: [f32; 3],
        forward: [f32; 3],
        up: [f32; 3],
    },
    Stop(u64),
    StopMatching {
        sound_id: Option<String>,
        category: Option<SoundCategory>,
    },
    UpdateEntityPosition {
        entity_id: i32,
        position: [f32; 3],
    },
    StopEntity(i32),
    StopAll,
    ReloadAssets,
    Shutdown,
}

#[derive(Debug)]
enum AudioEvent {
    Finished(u64),
    /// Balances one entity-bound play command, whether the sound ended, was
    /// stopped, or was refused. The engine counts these rather than tracking
    /// an idle flag, which would race a sound dispatched in between.
    EntitySoundEnded(i32),
}

/// Plays menu and in-world sounds using a dedicated OpenAL worker thread.
/// The public facade contains no native handles; OpenAL device/context/source/
/// buffer state is created, used, and dropped only by the worker.
pub struct AudioEngine {
    command_tx: Option<Sender<AudioCommand>>,
    event_rx: Receiver<AudioEvent>,
    worker: Option<JoinHandle<()>>,
    jar_assets_dir: PathBuf,
    asset_index: Option<AssetIndex>,
    sounds: SoundsIndex,
    volumes: [f32; SoundCategory::COUNT],
    subtitles_enabled: bool,
    subtitle_events: std::sync::Mutex<Vec<QueuedSubtitle>>,
    next_id: AtomicU64,
    /// Entity ids to the number of dispatched sounds the worker has not yet
    /// reported ended. Only these entities get position updates. The worker
    /// drives every decrement, so this is never cleared directly.
    entity_sound_targets: HashMap<i32, u32>,
    music_id: Option<u64>,
    menu_music_active: bool,
    next_song_delay_ticks: i32,
    music_tick_accumulator: f32,
}

impl AudioEngine {
    pub fn new(
        jar_assets_dir: &Path,
        asset_index: Option<AssetIndex>,
        packs: &ResourcePackManager,
        volumes: [f32; SoundCategory::COUNT],
    ) -> Self {
        let sounds = SoundsIndex::load(jar_assets_dir, &asset_index, packs);
        let (command_tx, command_rx) = crossbeam_channel::unbounded();
        // The worker reports with `try_send`, and a dropped `Finished` would
        // park the menu-music delay at `i32::MAX` forever.
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let (init_tx, init_rx) = crossbeam_channel::bounded(1);

        let worker = std::thread::Builder::new()
            .name("Pomme Sound engine".to_string())
            .spawn(move || {
                let context = match Context::open_default(false) {
                    Ok(context) => context,
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                        return;
                    }
                };
                let version = context.version().unwrap_or_else(|| "unknown".to_string());
                let limits = (context.static_source_limit, context.streaming_source_limit);
                let _ = init_tx.send(Ok((version, limits)));
                AudioWorker::new(context, volumes, command_rx, event_tx).run();
            });

        let (command_tx, worker) = match worker {
            Ok(handle) => match init_rx.recv() {
                Ok(Ok((version, (static_limit, streaming_limit)))) => {
                    tracing::info!(
                        "OpenAL audio initialized ({version}; {static_limit} static, {streaming_limit} streaming sources)"
                    );
                    (Some(command_tx), Some(handle))
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        "audio disabled: failed to initialize OpenAL ({e}). \
                         Releases ship the library next to the binary; for a dev \
                         build run `just openal` to stage it."
                    );
                    let _ = handle.join();
                    (None, None)
                }
                Err(e) => {
                    tracing::warn!("audio disabled: OpenAL worker exited during startup ({e})");
                    let _ = handle.join();
                    (None, None)
                }
            },
            Err(e) => {
                tracing::warn!("audio disabled: failed to start audio worker ({e})");
                (None, None)
            }
        };

        Self::attached(
            command_tx,
            event_rx,
            worker,
            jar_assets_dir.to_path_buf(),
            asset_index,
            sounds,
            volumes,
        )
    }

    /// Wraps an already-running (or absent) worker.
    fn attached(
        command_tx: Option<Sender<AudioCommand>>,
        event_rx: Receiver<AudioEvent>,
        worker: Option<JoinHandle<()>>,
        jar_assets_dir: PathBuf,
        asset_index: Option<AssetIndex>,
        sounds: SoundsIndex,
        volumes: [f32; SoundCategory::COUNT],
    ) -> Self {
        Self {
            command_tx,
            event_rx,
            worker,
            jar_assets_dir,
            asset_index,
            sounds,
            volumes,
            subtitles_enabled: false,
            subtitle_events: std::sync::Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            entity_sound_targets: HashMap::new(),
            music_id: None,
            menu_music_active: false,
            next_song_delay_ticks: MENU_MUSIC_STARTING_DELAY_TICKS,
            music_tick_accumulator: 0.0,
        }
    }

    /// An engine wired to plain channels instead of a worker thread, so a test
    /// can feed it worker reports and read back the commands it emits.
    #[cfg(test)]
    fn for_test() -> (Self, Sender<AudioEvent>, Receiver<AudioCommand>) {
        let (command_tx, command_rx) = crossbeam_channel::unbounded();
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let engine = Self::attached(
            Some(command_tx),
            event_rx,
            None,
            PathBuf::new(),
            None,
            SoundsIndex::default(),
            [1.0; SoundCategory::COUNT],
        );
        (engine, event_tx, command_rx)
    }

    pub fn set_volumes(&mut self, volumes: [f32; SoundCategory::COUNT]) {
        if volumes == self.volumes {
            return;
        }
        self.volumes = volumes;
        self.send(AudioCommand::SetVolumes(volumes));
    }

    /// Rebuilds the sound-event registry against the current resource-pack
    /// stack and clears native sources/buffers that may reference old assets.
    pub fn reload_assets(&mut self, packs: &ResourcePackManager) {
        self.sounds = SoundsIndex::load(&self.jar_assets_dir, &self.asset_index, packs);
        self.forget_active_sounds();
        self.send(AudioCommand::ReloadAssets);
    }

    /// Forgets the sounds the worker is about to drop without reporting
    /// completion. `play_menu_track` parks the delay at `i32::MAX` and waits
    /// for that report, so the delay has to be re-rolled here or menu music
    /// never starts again. Vanilla's `SoundEngine.stopAll` and `reload` both
    /// clear `soundDeleteTime` outright, so the 20-tick logical retention
    /// does not apply on either path.
    fn forget_active_sounds(&mut self) {
        // `entity_sound_targets` is deliberately untouched; the worker reports
        // the sounds it drops here too, and those reports balance the counts.
        if self.music_id.take().is_some() && self.menu_music_active {
            self.next_song_delay_ticks = random_menu_delay_ticks();
        }
    }

    /// Updates the listener using the camera's full yaw/pitch orientation.
    /// Also the in-game per-frame entry point, so it drains worker reports.
    pub fn set_listener(&mut self, pos: Position, y_rot_deg: f32, x_rot_deg: f32) {
        self.poll_events();
        let (forward, up) = listener_vectors(y_rot_deg, x_rot_deg);
        self.send(AudioCommand::SetListener {
            position: [pos.x as f32, pos.y as f32, pos.z as f32],
            forward,
            up,
        });
    }

    pub fn set_subtitles_enabled(&mut self, enabled: bool) {
        self.subtitles_enabled = enabled;
    }

    pub fn take_subtitle_events(&mut self) -> Vec<QueuedSubtitle> {
        std::mem::take(
            self.subtitle_events
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    pub fn play_ui_click(&self) {
        self.play_ui_sound(UI_CLICK_EVENT, UI_CLICK_VOLUME, 1.0);
    }

    /// Plays a non-positional UI sound using Vanilla's relative, no-attenuation
    /// `SimpleSoundInstance.forUI` semantics.
    pub fn play_ui_sound(&self, event: &str, volume: f32, pitch: f32) {
        let Some(sound) = self.resolve_event(event, None) else {
            return;
        };
        let id = self.allocate_id();
        self.send(AudioCommand::Play(PlayCommand {
            id,
            sound_id: sound.sound_id,
            path: sound.path,
            category: SoundCategory::Ui,
            volume: volume * sound.entry_volume,
            pitch: pitch * sound.entry_pitch,
            position: [0.0, 0.0, 0.0],
            relative: true,
            looping: false,
            stream: sound.stream,
            attenuation_distance: None,
            entity_id: None,
            report_completion: false,
        }));
    }

    /// Plays a positional world sound. OpenAL owns spatialization and distance
    /// attenuation; Pomme passes Vanilla-equivalent source parameters only.
    pub fn play_world_sound(
        &self,
        sound_ref: &SoundRef,
        category: u8,
        pos: Position,
        volume: f32,
        pitch: f32,
        seed: u64,
    ) {
        self.play_positioned_sound(sound_ref, category, pos, volume, pitch, seed, None);
    }

    pub fn play_entity_sound(
        &mut self,
        sound_ref: &SoundRef,
        category: u8,
        target: EntitySoundTarget,
        volume: f32,
        pitch: f32,
        seed: u64,
    ) {
        // Only a dispatched sound gets an end report to balance the count.
        if self.play_positioned_sound(
            sound_ref,
            category,
            target.pos,
            volume,
            pitch,
            seed,
            Some(target.id),
        ) {
            *self.entity_sound_targets.entry(target.id).or_default() += 1;
        }
    }

    /// Returns whether the sound resolved and was handed to the worker.
    #[allow(clippy::too_many_arguments)]
    fn play_positioned_sound(
        &self,
        sound_ref: &SoundRef,
        category: u8,
        pos: Position,
        volume: f32,
        pitch: f32,
        seed: u64,
        entity_id: Option<i32>,
    ) -> bool {
        let Some(sound) = self.resolve_sound(sound_ref, Some(seed)) else {
            return false;
        };
        let instance_volume = volume * sound.entry_volume;

        // Vanilla notifies subtitle listeners before category-volume/distance
        // culling. The subtitle's own range uses the resolved sound's distance.
        if self.subtitles_enabled
            && let Some(key) = self.sounds.subtitle(sound_ref.event_name())
            && let Ok(mut queue) = self.subtitle_events.lock()
        {
            queue.push(QueuedSubtitle {
                key: key.to_string(),
                pos,
                range: instance_volume.max(1.0) * sound.attenuation_distance,
            });
        }

        let id = self.allocate_id();
        self.send(AudioCommand::Play(PlayCommand {
            id,
            sound_id: sound.sound_id,
            path: sound.path,
            category: SoundCategory::from_index(category),
            volume: instance_volume,
            pitch: pitch * sound.entry_pitch,
            position: [pos.x as f32, pos.y as f32, pos.z as f32],
            relative: false,
            looping: false,
            stream: sound.stream,
            attenuation_distance: Some(instance_volume.max(1.0) * sound.attenuation_distance),
            entity_id,
            report_completion: false,
        }));
        self.command_tx.is_some()
    }

    /// Every entity move packet reaches this, so entities with no sound of
    /// their own are filtered out before the worker scans for a match.
    pub fn update_entity_sound_position(&self, entity_id: i32, pos: Position) {
        if !self.entity_sound_targets.contains_key(&entity_id) {
            return;
        }
        self.send(AudioCommand::UpdateEntityPosition {
            entity_id,
            position: [pos.x as f32, pos.y as f32, pos.z as f32],
        });
    }

    pub fn stop_entity_sounds(&mut self, entity_id: i32) {
        if !self.entity_sound_targets.contains_key(&entity_id) {
            return;
        }
        // The count stays until the worker reports each stopped sound, so one
        // dispatched before the worker drains this is still tracked.
        self.send(AudioCommand::StopEntity(entity_id));
    }

    pub fn start_menu_music(&mut self) {
        if !self.menu_music_active {
            self.menu_music_active = true;
            self.next_song_delay_ticks = MENU_MUSIC_STARTING_DELAY_TICKS;
            self.music_tick_accumulator = 0.0;
        }
    }

    pub fn stop_menu_music(&mut self) {
        self.menu_music_active = false;
        self.next_song_delay_ticks = MENU_MUSIC_STARTING_DELAY_TICKS;
        self.music_tick_accumulator = 0.0;
        if let Some(id) = self.music_id.take() {
            self.send(AudioCommand::Stop(id));
        }
    }

    /// Stops all currently active native sources. Static buffer cache remains
    /// valid and is released with the audio context at shutdown.
    pub fn stop_all_sounds(&mut self) {
        self.forget_active_sounds();
        self.send(AudioCommand::StopAll);
    }

    /// Implements Vanilla's `SoundEngine.stop(sound, source)` matching.
    pub fn stop_sounds(&mut self, sound_id: Option<&str>, category: Option<u8>) {
        if sound_id.is_none() && category.is_none() {
            self.stop_all_sounds();
            return;
        }
        self.send(AudioCommand::StopMatching {
            sound_id: sound_id.map(canonical_sound_id),
            category: category.map(SoundCategory::from_index),
        });
    }

    /// Drains everything the worker has reported. Both phases call this every
    /// frame, so it must stay independent of whether menu music is running.
    pub fn poll_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AudioEvent::Finished(id) => {
                    if self.music_id == Some(id) {
                        self.music_id = None;
                        self.next_song_delay_ticks =
                            menu_delay_after_finish(random_menu_delay_ticks());
                    }
                }
                AudioEvent::EntitySoundEnded(entity_id) => {
                    if let Some(count) = self.entity_sound_targets.get_mut(&entity_id) {
                        *count -= 1;
                        if *count == 0 {
                            self.entity_sound_targets.remove(&entity_id);
                        }
                    }
                }
            }
        }
    }

    pub fn update_menu_music(&mut self, dt: f32) {
        self.poll_events();
        if self.command_tx.is_none() || !self.menu_music_active {
            return;
        }

        self.music_tick_accumulator += dt.max(0.0);
        while self.music_tick_accumulator >= SOUND_TICK_SECONDS {
            self.music_tick_accumulator -= SOUND_TICK_SECONDS;
            if self.music_id.is_none() {
                self.next_song_delay_ticks -= 1;
                if self.next_song_delay_ticks <= 0 {
                    self.play_menu_track();
                }
            }
        }
    }

    fn play_menu_track(&mut self) {
        let Some(sound) = self.resolve_event(MENU_MUSIC_EVENT, None) else {
            self.next_song_delay_ticks = MENU_MUSIC_MIN_DELAY_TICKS;
            return;
        };
        let id = self.allocate_id();
        self.send(AudioCommand::Play(PlayCommand {
            id,
            sound_id: sound.sound_id,
            path: sound.path,
            category: SoundCategory::Music,
            volume: sound.entry_volume,
            pitch: sound.entry_pitch,
            position: [0.0, 0.0, 0.0],
            relative: true,
            looping: false,
            stream: sound.stream,
            attenuation_distance: None,
            entity_id: None,
            report_completion: true,
        }));
        self.music_id = Some(id);
        self.next_song_delay_ticks = i32::MAX;
    }

    fn resolve_sound(&self, sound: &SoundRef, seed: Option<u64>) -> Option<ResolvedSound> {
        self.resolve_event(sound.event_name(), seed)
    }

    fn resolve_event(&self, event: &str, seed: Option<u64>) -> Option<ResolvedSound> {
        let variant = self.sounds.choose(event, seed)?;
        Some(self.resolve_variant(event, variant))
    }

    fn resolve_variant(&self, event: &str, variant: SoundVariant) -> ResolvedSound {
        ResolvedSound {
            sound_id: canonical_sound_id(event),
            path: variant.path,
            entry_volume: variant.volume,
            entry_pitch: variant.pitch,
            stream: variant.stream,
            attenuation_distance: variant.attenuation_distance,
        }
    }

    fn allocate_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn send(&self, command: AudioCommand) {
        if let Some(tx) = self.command_tx.as_ref()
            && tx.send(command).is_err()
        {
            tracing::debug!("audio worker is unavailable");
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        if let Some(tx) = self.command_tx.take() {
            let _ = tx.send(AudioCommand::Shutdown);
        }
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::warn!("audio worker panicked during shutdown");
        }
    }
}

struct AudioWorker {
    context: Context,
    volumes: [f32; SoundCategory::COUNT],
    commands: Receiver<AudioCommand>,
    events: Sender<AudioEvent>,
    static_buffers: HashMap<PathBuf, Rc<Buffer>>,
    active: HashMap<u64, ActiveSound>,
}

struct ActiveSound {
    source: Source,
    sound_id: String,
    category: SoundCategory,
    base_gain: f32,
    entity_id: Option<i32>,
    report_completion: bool,
    kind: ActiveKind,
}

enum ActiveKind {
    Static { _buffer: Rc<Buffer> },
    Stream(StreamPlayback),
}

impl Drop for ActiveSound {
    fn drop(&mut self) {
        if let ActiveKind::Stream(stream) = &mut self.kind
            && let Err(e) = self.source.clear_queue(&mut stream.queued)
        {
            tracing::warn!("failed to clear OpenAL stream queue: {e}");
        }
    }
}

struct StreamPlayback {
    decoder: StreamDecoder,
    queued: VecDeque<Buffer>,
    finished_decoding: bool,
}

impl AudioWorker {
    fn new(
        context: Context,
        volumes: [f32; SoundCategory::COUNT],
        commands: Receiver<AudioCommand>,
        events: Sender<AudioEvent>,
    ) -> Self {
        Self {
            context,
            volumes,
            commands,
            events,
            static_buffers: HashMap::new(),
            active: HashMap::new(),
        }
    }

    fn run(mut self) {
        let mut shutdown = false;
        while !shutdown {
            match self.commands.recv_timeout(Duration::from_millis(10)) {
                Ok(command) => shutdown = self.handle_command(command),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => shutdown = true,
            }
            while !shutdown {
                match self.commands.try_recv() {
                    Ok(command) => shutdown = self.handle_command(command),
                    Err(_) => break,
                }
            }
            if !shutdown {
                self.tick();
            }
        }
        // The one removal that skips `report_entity_sound_ended`; nothing
        // reads the counts after shutdown.
        self.active.clear();
        self.static_buffers.clear();
    }

    fn handle_command(&mut self, command: AudioCommand) -> bool {
        match command {
            AudioCommand::Play(play) => {
                let id = play.id;
                let report_completion = play.report_completion;
                let entity_id = play.entity_id;
                // A refusal still reports, or the engine keeps counting an
                // entity sound that never got a source.
                if !self.play(play) {
                    if report_completion {
                        let _ = self.events.try_send(AudioEvent::Finished(id));
                    }
                    self.report_entity_sound_ended(entity_id);
                }
            }
            AudioCommand::SetVolumes(volumes) => {
                self.volumes = volumes;
                self.refresh_gains();
            }
            AudioCommand::SetListener {
                position,
                forward,
                up,
            } => {
                if let Err(e) = self.context.set_listener(position, forward, up) {
                    tracing::warn!("failed to update OpenAL listener: {e}");
                }
            }
            AudioCommand::Stop(id) => {
                self.stop_sound(id);
            }
            AudioCommand::StopMatching { sound_id, category } => {
                let ids: Vec<u64> = self
                    .active
                    .iter()
                    .filter_map(|(&id, sound)| {
                        sound_matches_stop(
                            &sound.sound_id,
                            sound.category,
                            sound_id.as_deref(),
                            category,
                        )
                        .then_some(id)
                    })
                    .collect();
                for id in ids {
                    self.stop_sound(id);
                }
            }
            AudioCommand::UpdateEntityPosition {
                entity_id,
                position,
            } => {
                for sound in self
                    .active
                    .values()
                    .filter(|sound| sound.entity_id == Some(entity_id))
                {
                    if let Err(e) = sound.source.set_position(position) {
                        tracing::warn!("failed to update entity-bound sound position: {e}");
                    }
                }
            }
            AudioCommand::StopEntity(entity_id) => {
                let ids: Vec<u64> = self
                    .active
                    .iter()
                    .filter_map(|(&id, sound)| (sound.entity_id == Some(entity_id)).then_some(id))
                    .collect();
                for id in ids {
                    self.stop_sound(id);
                }
            }
            AudioCommand::StopAll => {
                self.clear_active();
            }
            AudioCommand::ReloadAssets => {
                self.clear_active();
                self.static_buffers.clear();
            }
            AudioCommand::Shutdown => return true,
        }
        false
    }

    fn stop_sound(&mut self, id: u64) {
        let Some(sound) = self.active.remove(&id) else {
            return;
        };
        if let Err(e) = sound.source.stop() {
            tracing::warn!("failed to stop OpenAL source: {e}");
        }
        if sound.report_completion {
            let _ = self.events.try_send(AudioEvent::Finished(id));
        }
        self.report_entity_sound_ended(sound.entity_id);
    }

    /// Drops every active sound, reporting each so the engine's entity counts
    /// stay balanced.
    fn clear_active(&mut self) {
        let entities: Vec<Option<i32>> = self
            .active
            .drain()
            .map(|(_, sound)| sound.entity_id)
            .collect();
        for entity_id in entities {
            self.report_entity_sound_ended(entity_id);
        }
    }

    fn play(&mut self, play: PlayCommand) -> bool {
        // Frees the sources finished sounds still hold before the pool check.
        self.tick();
        let base_gain = clamped_source_volume(play.volume);
        let gain = category_gain(&self.volumes, play.category) * base_gain;
        if gain == 0.0 && play.category != SoundCategory::Music {
            return false;
        }
        let pitch = clamped_source_pitch(play.pitch);

        let stream_count = self
            .active
            .values()
            .filter(|sound| matches!(sound.kind, ActiveKind::Stream(_)))
            .count();
        let static_count = self.active.len().saturating_sub(stream_count);
        let at_capacity = if play.stream {
            stream_count >= self.context.streaming_source_limit
        } else {
            static_count >= self.context.static_source_limit
        };
        if at_capacity {
            tracing::debug!(
                "OpenAL source pool exhausted; dropping sound {}",
                play.path.display()
            );
            return false;
        }

        let source = match self.context.create_source() {
            Ok(source) => source,
            Err(e) => {
                tracing::warn!("failed to allocate OpenAL source: {e}");
                return false;
            }
        };
        if let Err(e) = source.configure(
            gain,
            pitch,
            play.position,
            play.relative,
            play.looping && !play.stream,
            play.attenuation_distance,
        ) {
            tracing::warn!("failed to configure OpenAL source: {e}");
            return false;
        }

        let (source, kind) = if play.stream {
            match self.start_stream(source, &play.path) {
                Ok((source, stream)) => (source, ActiveKind::Stream(stream)),
                Err(e) => {
                    tracing::warn!("failed to stream sound {}: {e}", play.path.display());
                    return false;
                }
            }
        } else {
            let buffer = match self.static_buffer(&play.path) {
                Ok(buffer) => buffer,
                Err(e) => {
                    tracing::warn!("failed to decode sound {}: {e}", play.path.display());
                    return false;
                }
            };
            if let Err(e) = source.attach_static(&buffer) {
                tracing::warn!("failed to attach OpenAL buffer: {e}");
                return false;
            }
            (source, ActiveKind::Static { _buffer: buffer })
        };

        if let Err(e) = source.play() {
            tracing::warn!("failed to play OpenAL source: {e}");
            return false;
        }
        self.active.insert(
            play.id,
            ActiveSound {
                source,
                sound_id: play.sound_id,
                category: play.category,
                base_gain,
                entity_id: play.entity_id,
                report_completion: play.report_completion,
                kind,
            },
        );
        true
    }

    fn static_buffer(&mut self, path: &Path) -> Result<Rc<Buffer>, String> {
        if let Some(buffer) = self.static_buffers.get(path) {
            return Ok(Rc::clone(buffer));
        }
        let decoded = decode_all(path)?;
        let buffer = Rc::new(
            self.context
                .create_buffer(decoded.format, &decoded.samples)?,
        );
        self.static_buffers
            .insert(path.to_path_buf(), Rc::clone(&buffer));
        Ok(buffer)
    }

    fn start_stream(
        &self,
        source: Source,
        path: &Path,
    ) -> Result<(Source, StreamPlayback), String> {
        let mut decoder = StreamDecoder::open(path)?;
        let mut queued = VecDeque::new();
        let mut finished_decoding = false;
        for _ in 0..4 {
            match queue_stream_chunk(&self.context, &source, &mut decoder, &mut queued) {
                Ok(true) => {}
                Ok(false) => {
                    finished_decoding = true;
                    break;
                }
                Err(error) => {
                    if let Err(cleanup_error) = source.clear_queue(&mut queued) {
                        // Releasing the source detaches any buffers OpenAL refused to unqueue.
                        // Only then may the Rust Buffer owners drop and delete those native ids.
                        drop(source);
                        drop(queued);
                        return Err(format!(
                            "{error}; additionally failed to clear partial stream queue: {cleanup_error}"
                        ));
                    }
                    return Err(error);
                }
            }
        }
        if queued.is_empty() {
            return Err("stream contained no PCM samples".to_string());
        }
        Ok((
            source,
            StreamPlayback {
                decoder,
                queued,
                finished_decoding,
            },
        ))
    }

    fn refresh_gains(&mut self) {
        for sound in self.active.values() {
            let gain = category_gain(&self.volumes, sound.category) * sound.base_gain;
            if let Err(e) = sound.source.set_gain(gain) {
                tracing::warn!("failed to update OpenAL source gain: {e}");
            }
        }
    }

    fn tick(&mut self) {
        let ids: Vec<u64> = self.active.keys().copied().collect();
        let mut finished = Vec::new();
        for id in ids {
            let Some(sound) = self.active.get_mut(&id) else {
                continue;
            };
            let done = match &mut sound.kind {
                ActiveKind::Static { .. } => match sound.source.state() {
                    Ok(SourceState::Playing | SourceState::Paused) => false,
                    Ok(_) => true,
                    Err(e) => {
                        tracing::warn!("failed to query OpenAL source state: {e}");
                        true
                    }
                },
                ActiveKind::Stream(stream) => {
                    if let Err(e) = tick_stream(&self.context, &sound.source, stream) {
                        tracing::warn!("stream playback failed: {e}");
                        true
                    } else {
                        stream.finished_decoding && stream.queued.is_empty()
                    }
                }
            };
            if done {
                finished.push(id);
            }
        }
        for id in finished {
            let Some(sound) = self.active.remove(&id) else {
                continue;
            };
            if sound.report_completion {
                let _ = self.events.try_send(AudioEvent::Finished(id));
            }
            self.report_entity_sound_ended(sound.entity_id);
        }
    }

    /// Every path that removes a sound from `active`, or refuses to add one,
    /// must call this exactly once or the engine's count never reaches zero.
    fn report_entity_sound_ended(&self, entity_id: Option<i32>) {
        if let Some(entity_id) = entity_id {
            let _ = self
                .events
                .try_send(AudioEvent::EntitySoundEnded(entity_id));
        }
    }
}

fn queue_stream_chunk(
    context: &Context,
    source: &Source,
    decoder: &mut StreamDecoder,
    queued: &mut VecDeque<Buffer>,
) -> Result<bool, String> {
    let format = decoder.format();
    let one_second_samples = usize::try_from(format.sample_rate)
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::from(format.channels));
    let samples = decoder.read_samples(one_second_samples)?;
    if samples.is_empty() {
        return Ok(false);
    }
    let buffer = context.create_buffer(format, &samples)?;
    source.queue_buffer(&buffer)?;
    queued.push_back(buffer);
    Ok(true)
}

fn tick_stream(
    context: &Context,
    source: &Source,
    stream: &mut StreamPlayback,
) -> Result<(), String> {
    source.remove_processed(&mut stream.queued)?;
    while !stream.finished_decoding && stream.queued.len() < 4 {
        if !queue_stream_chunk(context, source, &mut stream.decoder, &mut stream.queued)? {
            stream.finished_decoding = true;
        }
    }
    if !stream.queued.is_empty() && matches!(source.state()?, SourceState::Stopped) {
        source.play()?;
    }
    Ok(())
}

fn listener_vectors(y_rot_deg: f32, x_rot_deg: f32) -> ([f32; 3], [f32; 3]) {
    let yaw = y_rot_deg.to_radians();
    let pitch = x_rot_deg.to_radians();
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let (sin_pitch, cos_pitch) = pitch.sin_cos();
    let forward = [-sin_yaw * cos_pitch, -sin_pitch, cos_yaw * cos_pitch];
    // Vanilla's up vector is calculateViewVector(pitch - 90°, yaw).
    let up = [-sin_yaw * sin_pitch, cos_pitch, cos_yaw * sin_pitch];
    (forward, up)
}

fn sound_matches_stop(
    sound_id: &str,
    category: SoundCategory,
    expected_sound_id: Option<&str>,
    expected_category: Option<SoundCategory>,
) -> bool {
    expected_sound_id.is_none_or(|expected| sound_id == expected)
        && expected_category.is_none_or(|expected| category == expected)
}

fn random_menu_delay_ticks() -> i32 {
    fastrand::i32(MENU_MUSIC_MIN_DELAY_TICKS..=MENU_MUSIC_MAX_DELAY_TICKS)
}

fn menu_delay_after_finish(random_delay: i32) -> i32 {
    LOGICAL_SOUND_RETENTION_TICKS.saturating_add(random_delay)
}

fn canonical_sound_id(sound_id: &str) -> String {
    if sound_id.contains(':') {
        sound_id.to_string()
    } else {
        format!("minecraft:{sound_id}")
    }
}

fn clamped_source_volume(volume: f32) -> f32 {
    volume.clamp(0.0, 1.0)
}

fn clamped_source_pitch(pitch: f32) -> f32 {
    pitch.clamp(0.5, 2.0)
}

fn category_gain(volumes: &[f32; SoundCategory::COUNT], category: SoundCategory) -> f32 {
    let master = volumes[SoundCategory::Master as usize];
    match category {
        SoundCategory::Master => master,
        other => master * volumes[other as usize],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_vectors_match_vanilla_axes() {
        let (forward, up) = listener_vectors(90.0, 30.0);
        let expected_forward = [-30.0_f32.to_radians().cos(), -0.5, 0.0];
        let expected_up = [-0.5, 30.0_f32.to_radians().cos(), 0.0];
        for (actual, expected) in forward.into_iter().zip(expected_forward) {
            assert!((actual - expected).abs() < 1.0e-6, "{actual} != {expected}");
        }
        for (actual, expected) in up.into_iter().zip(expected_up) {
            assert!((actual - expected).abs() < 1.0e-6, "{actual} != {expected}");
        }
    }

    #[test]
    fn category_gain_does_not_double_scale_master() {
        let mut volumes = [1.0; SoundCategory::COUNT];
        volumes[SoundCategory::Master as usize] = 0.5;
        volumes[SoundCategory::Music as usize] = 0.25;
        assert_eq!(category_gain(&volumes, SoundCategory::Master), 0.5);
        assert_eq!(category_gain(&volumes, SoundCategory::Music), 0.125);
    }

    #[test]
    fn source_parameter_clamps_match_vanilla() {
        assert_eq!(clamped_source_volume(-1.0), 0.0);
        assert_eq!(clamped_source_volume(0.25), 0.25);
        assert_eq!(clamped_source_volume(2.0), 1.0);
        assert_eq!(clamped_source_pitch(0.1), 0.5);
        assert_eq!(clamped_source_pitch(1.25), 1.25);
        assert_eq!(clamped_source_pitch(3.0), 2.0);
    }

    #[test]
    fn sound_ids_use_minecraft_default_namespace() {
        assert_eq!(
            canonical_sound_id("block.stone.break"),
            "minecraft:block.stone.break"
        );
        assert_eq!(canonical_sound_id("mod:custom.sound"), "mod:custom.sound");
    }

    #[test]
    fn stop_filter_matches_vanilla_name_and_source_combinations() {
        let id = "minecraft:block.stone.break";
        let category = SoundCategory::Blocks;
        assert!(sound_matches_stop(id, category, None, None));
        assert!(sound_matches_stop(id, category, Some(id), None));
        assert!(sound_matches_stop(id, category, None, Some(category)));
        assert!(sound_matches_stop(id, category, Some(id), Some(category)));
        assert!(!sound_matches_stop(
            id,
            category,
            Some("minecraft:block.grass.break"),
            None,
        ));
        assert!(!sound_matches_stop(
            id,
            category,
            None,
            Some(SoundCategory::Players),
        ));
    }

    #[test]
    fn menu_music_finish_includes_vanilla_logical_retention_ticks() {
        assert_eq!(menu_delay_after_finish(400), 420);
        assert_eq!(menu_delay_after_finish(20), 40);
    }

    #[test]
    fn ui_matches_vanilla_sound_source_ordinal() {
        assert_eq!(SoundCategory::Ui as usize, 10);
        assert!(matches!(SoundCategory::from_index(10), SoundCategory::Ui));
    }

    /// Drains the worker's view of the command channel, reporting each
    /// entity-bound play back the way `AudioWorker` does.
    fn end_dispatched_sounds(events: &Sender<AudioEvent>, commands: &Receiver<AudioCommand>) {
        while let Ok(command) = commands.try_recv() {
            if let AudioCommand::Play(PlayCommand {
                entity_id: Some(entity_id),
                ..
            }) = command
            {
                events
                    .send(AudioEvent::EntitySoundEnded(entity_id))
                    .unwrap();
            }
        }
    }

    #[test]
    fn entity_sound_end_report_stops_position_forwarding() {
        let (mut engine, events, commands) = AudioEngine::for_test();
        engine.entity_sound_targets.insert(7, 1);

        engine.update_entity_sound_position(7, Position::new(1.0, 2.0, 3.0));
        assert!(matches!(
            commands.try_recv(),
            Ok(AudioCommand::UpdateEntityPosition { entity_id: 7, .. })
        ));

        events.send(AudioEvent::EntitySoundEnded(7)).unwrap();
        engine.poll_events();
        assert!(!engine.entity_sound_targets.contains_key(&7));

        engine.update_entity_sound_position(7, Position::new(4.0, 5.0, 6.0));
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn untracked_entity_never_reaches_the_worker() {
        let (engine, _events, commands) = AudioEngine::for_test();
        engine.update_entity_sound_position(7, Position::new(1.0, 2.0, 3.0));
        assert!(commands.try_recv().is_err());
    }

    /// A second sound for the same entity must survive the first one's end
    /// report, which an idle flag rather than a count would drop.
    #[test]
    fn overlapping_entity_sounds_keep_tracking_until_the_last_ends() {
        let (mut engine, events, _commands) = AudioEngine::for_test();
        engine.entity_sound_targets.insert(7, 2);

        events.send(AudioEvent::EntitySoundEnded(7)).unwrap();
        engine.poll_events();
        assert_eq!(engine.entity_sound_targets.get(&7), Some(&1));

        events.send(AudioEvent::EntitySoundEnded(7)).unwrap();
        engine.poll_events();
        assert!(!engine.entity_sound_targets.contains_key(&7));
    }

    /// Stop-all drops sounds the worker never reported finished, so the counts
    /// must reach zero from its reports, not from the engine clearing them.
    #[test]
    fn stop_all_leaves_no_entity_counted() {
        let (mut engine, events, commands) = AudioEngine::for_test();
        engine.sounds = SoundsIndex::for_test_event("entity.test");

        for _ in 0..3 {
            engine.play_entity_sound(
                &SoundRef::event("entity.test"),
                CATEGORY_PLAYERS,
                EntitySoundTarget {
                    id: 7,
                    pos: Position::new(0.0, 0.0, 0.0),
                },
                1.0,
                1.0,
                0,
            );
        }
        assert_eq!(engine.entity_sound_targets.get(&7), Some(&3));

        engine.stop_all_sounds();
        end_dispatched_sounds(&events, &commands);
        engine.poll_events();
        assert!(engine.entity_sound_targets.is_empty());
    }

    /// Menu music parks the delay at `i32::MAX` until the worker reports the
    /// track finished, so that report has to survive whatever else is queued.
    #[test]
    fn music_finish_report_survives_a_burst_of_entity_reports() {
        let (mut engine, events, _commands) = AudioEngine::for_test();
        engine.music_id = Some(42);
        engine.next_song_delay_ticks = i32::MAX;

        for entity_id in 0..64 {
            events
                .send(AudioEvent::EntitySoundEnded(entity_id))
                .unwrap();
        }
        events.send(AudioEvent::Finished(42)).unwrap();
        engine.poll_events();

        assert_eq!(engine.music_id, None);
        assert!(
            engine.next_song_delay_ticks <= menu_delay_after_finish(MENU_MUSIC_MAX_DELAY_TICKS)
        );
    }
}
