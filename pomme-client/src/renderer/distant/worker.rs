//! The distant terrain thread: turns chunks the server forgets into distant
//! sections, keeps them per dimension, on disk too, and meshes the sections
//! the renderer asks for and keeps them up to date.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use parking_lot::RwLock;

use super::build::{BuiltSection, Factory};
use super::colors::DistantColors;
use super::models::{ModelFactory, ModelTile};
use super::section::{SectionKey, WorldSection};
use super::voxel::States;
use super::world::World;
use crate::position::ChunkPos;
use crate::world::chunk::{Chunk, ChunkLightData, ChunkStore};

/// Sections meshed between checks for new jobs.
const MESHES_PER_BATCH: usize = 32;
/// How far past the view radius sections are kept in memory, in blocks: one
/// top-level section, so walking back and forth doesn't drop and reload
/// them.
const KEEP_MARGIN: f64 = 1024.0;
/// How far the player moves, in blocks, before the thread hears of it.
const VIEW_STEP: f64 = 32.0;
/// How often changed sections are written to disk.
const SAVE_EVERY: Duration = Duration::from_secs(60);
/// How long a section a capture changed waits for more before it's meshed
/// again, so a burst of captures meshes it once.
const SETTLE: Duration = Duration::from_millis(250);
/// How long the thread takes in captures before it meshes a batch, so a
/// join's flood of chunks doesn't hold up the meshes the traversal is
/// waiting on. The reference ingests and meshes on separate services; this
/// thread does both, a slice of each in turn.
const CAPTURE_SLICE: Duration = Duration::from_millis(8);
/// The sides a change reaches ([`super::world::Changed::sides`]), as
/// neighbour offsets.
const SIDES: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [-1, 0, 0],
    [1, 0, 0],
    [0, 0, -1],
    [0, 0, 1],
];

/// A chunk to keep, before it goes.
struct Capture {
    dimension: Arc<str>,
    pos: ChunkPos,
    chunk: Arc<RwLock<Chunk>>,
    light: Option<Arc<ChunkLightData>>,
    min_y: i32,
}

/// Everything else the game tells the thread, taken before any capture
/// since the renderer's requests are what's on screen.
enum Job {
    Colors(Arc<DistantColors>),
    View(View),
    /// Sections the renderer wants meshed, and kept meshed as they change.
    Request(Vec<SectionKey>),
    /// Sections the renderer dropped.
    Dropped(Vec<SectionKey>),
    /// Where sections are kept on disk, a folder per dimension; `None`
    /// keeps them in memory only.
    Cache(Option<PathBuf>),
    /// Writes what's unsaved and ends the thread.
    Stop,
}

#[derive(Clone)]
struct View {
    dimension: Arc<str>,
    x: f64,
    z: f64,
    /// In blocks.
    radius: f64,
}

/// Newly meshed sections of one dimension, the models new ones are the
/// first to use, and the biome tints when they changed.
pub(crate) struct DistantUpdate {
    pub dimension: Arc<str>,
    pub sections: Vec<BuiltSection>,
    pub tiles: Vec<ModelTile>,
    pub tints: Option<Vec<u32>>,
}

/// The game's handle on the thread. Dropping it waits for the thread to
/// write what's unsaved.
pub(crate) struct DistantTerrain {
    jobs: Sender<Job>,
    captures: Sender<Capture>,
    updates: Receiver<DistantUpdate>,
    thread: Option<JoinHandle<()>>,
    /// The last view sent, so small moves don't flood the thread.
    view: Option<View>,
    /// This server's or world's folder of distant terrain.
    cache_root: Option<PathBuf>,
    /// The folder last sent, under `cache_root`.
    cache: Option<PathBuf>,
}

impl DistantTerrain {
    pub fn new(colors: Arc<DistantColors>, cache_root: Option<PathBuf>) -> Self {
        let (jobs, job_rx) = crossbeam_channel::unbounded();
        let (captures, capture_rx) = crossbeam_channel::unbounded();
        let (update_tx, updates) = crossbeam_channel::unbounded();
        let thread = std::thread::Builder::new()
            .name("distant-terrain".into())
            .spawn(move || Worker::new(colors, update_tx).run(&job_rx, &capture_rx))
            .expect("failed to spawn the distant terrain thread");
        Self {
            jobs,
            captures,
            updates,
            thread: Some(thread),
            view: None,
            cache_root,
            cache: None,
        }
    }

    /// Keeps sections under the cache root, in a folder for the block
    /// states of table `slot` and the server's `biomes`, since the voxels
    /// store both by id.
    pub fn set_cache_key(
        &mut self,
        slot: usize,
        biomes: impl IntoIterator<Item = impl std::fmt::Display>,
    ) {
        let mut hash = sha1_smol::Sha1::new();
        for biome in biomes {
            hash.update(biome.to_string().as_bytes());
            hash.update(&[0]);
        }
        let digest = hash.digest().to_string();
        let cache = self
            .cache_root
            .as_ref()
            .map(|root| root.join(format!("{slot}-{}", &digest[..16])));
        if cache != self.cache {
            self.cache.clone_from(&cache);
            let _ = self.jobs.send(Job::Cache(cache));
        }
    }

    /// Whether chunks are being kept: a view with a radius was set.
    pub fn enabled(&self) -> bool {
        self.view.is_some()
    }

    /// Keeps `pos`'s column of `chunks` as distant terrain of `dimension`,
    /// before the chunk goes.
    pub fn capture(&self, dimension: &str, pos: ChunkPos, chunks: &ChunkStore) {
        if !self.enabled() {
            return;
        }
        let Some(chunk) = chunks.get_chunk(&pos) else {
            return;
        };
        let _ = self.captures.send(Capture {
            dimension: dimension.into(),
            pos,
            chunk,
            light: chunks.light_data.get(&(pos.x, pos.z)).cloned(),
            min_y: chunks.min_y(),
        });
    }

    pub fn set_colors(&self, colors: Arc<DistantColors>) {
        let _ = self.jobs.send(Job::Colors(colors));
    }

    /// Sections the renderer wants, and those it dropped.
    pub fn request(&self, wanted: Vec<SectionKey>, dropped: Vec<SectionKey>) {
        if !wanted.is_empty() {
            let _ = self.jobs.send(Job::Request(wanted));
        }
        if !dropped.is_empty() {
            let _ = self.jobs.send(Job::Dropped(dropped));
        }
    }

    /// Where the player is and how far to keep, in chunks; 0 stops keeping
    /// new chunks.
    pub fn set_view(&mut self, dimension: &str, x: f64, z: f64, chunks: u32) {
        if chunks == 0 {
            self.view = None;
            return;
        }
        let view = View {
            dimension: dimension.into(),
            x,
            z,
            radius: f64::from(chunks * 16),
        };
        let unchanged = self.view.as_ref().is_some_and(|last| {
            last.dimension == view.dimension
                && last.radius == view.radius
                && (last.x - x).hypot(last.z - z) < VIEW_STEP
        });
        if !unchanged {
            self.view = Some(view.clone());
            let _ = self.jobs.send(Job::View(view));
        }
    }

    pub fn drain(&self) -> impl Iterator<Item = DistantUpdate> + '_ {
        self.updates.try_iter()
    }
}

impl Drop for DistantTerrain {
    fn drop(&mut self) {
        let _ = self.jobs.send(Job::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// `name` as a single folder name: anything but letters, digits, `-`, `_`
/// and `.` becomes `_`.
pub(crate) fn folder_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Where `dimension`'s sections go under `cache`: a folder of their own, so
/// the column files of earlier builds don't mix in.
fn dimension_dir(cache: Option<&Path>, dimension: &str) -> Option<PathBuf> {
    Some(cache?.join(folder_name(dimension)).join("sections-v2"))
}

/// `dimension`'s world, made with its folder under `cache`.
fn world<'a>(
    worlds: &'a mut HashMap<Arc<str>, World>,
    cache: Option<&Path>,
    dimension: &Arc<str>,
) -> &'a mut World {
    worlds
        .entry(Arc::clone(dimension))
        .or_insert_with(|| World::new(dimension_dir(cache, dimension)))
}

struct Worker {
    colors: Arc<DistantColors>,
    models: ModelFactory,
    states: States,
    factory: Factory,
    updates: Sender<DistantUpdate>,
    /// Per dimension, its sections.
    worlds: HashMap<Arc<str>, World>,
    cache: Option<PathBuf>,
    last_save: Instant,
    view: Option<View>,
    /// Sections to mesh, each from when.
    dirty: HashMap<SectionKey, Instant>,
    /// Sections the renderer holds, kept up to date as captures change them.
    meshed: HashSet<SectionKey>,
    /// Whether the renderer has the current colours' biome tints.
    tints_sent: bool,
}

impl Worker {
    fn new(colors: Arc<DistantColors>, updates: Sender<DistantUpdate>) -> Self {
        Self {
            states: States::new(colors.table()),
            colors,
            models: ModelFactory::new(),
            factory: Factory::new(),
            updates,
            worlds: HashMap::new(),
            cache: None,
            last_save: Instant::now(),
            view: None,
            dirty: HashMap::new(),
            meshed: HashSet::new(),
            tints_sent: false,
        }
    }

    fn run(mut self, jobs: &Receiver<Job>, captures: &Receiver<Capture>) {
        'run: loop {
            let wait = match self.dirty.values().min() {
                Some(ready) => ready.saturating_duration_since(Instant::now()),
                None => SAVE_EVERY.saturating_sub(self.last_save.elapsed()),
            };
            // Sleeps until there's something to do: a job, a capture, a
            // section ready to mesh or a save due.
            crossbeam_channel::select! {
                recv(jobs) -> job => match job {
                    Ok(Job::Stop) | Err(_) => break 'run,
                    Ok(job) => self.handle(job),
                },
                recv(captures) -> capture => match capture {
                    Ok(capture) => self.capture(capture),
                    Err(_) => break 'run,
                },
                default(wait) => {}
            }
            for job in jobs.try_iter() {
                if matches!(job, Job::Stop) {
                    break 'run;
                }
                self.handle(job);
            }
            let slice = Instant::now();
            for capture in captures.try_iter() {
                self.capture(capture);
                if slice.elapsed() >= CAPTURE_SLICE {
                    break;
                }
            }
            self.mesh_nearest();
            if self.last_save.elapsed() >= SAVE_EVERY {
                self.save();
            }
        }
        self.save();
    }

    fn save(&mut self) {
        for world in self.worlds.values_mut() {
            world.flush();
        }
        self.last_save = Instant::now();
    }

    /// Writes a chunk into its dimension's sections and marks the meshed
    /// sections it changed, and their neighbours across the sides the
    /// change reached, for meshing again.
    fn capture(&mut self, capture: Capture) {
        let Capture {
            dimension,
            pos,
            chunk,
            light,
            min_y,
        } = capture;
        let chunk = chunk.read();
        let changed = world(&mut self.worlds, self.cache.as_deref(), &dimension).ingest_chunk(
            &self.states,
            pos,
            &chunk,
            light.as_deref(),
            min_y,
        );
        if self
            .view
            .as_ref()
            .is_none_or(|view| view.dimension != dimension)
        {
            return;
        }
        let ready = Instant::now() + SETTLE;
        for change in changed {
            let key = change.key;
            let neighbours = SIDES
                .iter()
                .enumerate()
                .filter(|&(side, _)| change.sides & 1 << side != 0)
                .map(|(_, [dx, dy, dz])| SectionKey {
                    x: key.x + dx,
                    y: key.y + dy,
                    z: key.z + dz,
                    ..key
                });
            for key in std::iter::once(key).chain(neighbours) {
                if self.meshed.contains(&key) {
                    self.dirty.entry(key).or_insert(ready);
                }
            }
        }
    }

    fn handle(&mut self, job: Job) {
        match job {
            Job::Colors(colors) => {
                if !colors.same_models(&self.colors) {
                    // Ids start over; the new tiles overwrite the old.
                    self.models = ModelFactory::new();
                    self.states = States::new(colors.table());
                }
                self.colors = colors;
                self.tints_sent = false;
                let now = Instant::now();
                self.dirty.extend(self.meshed.iter().map(|&key| (key, now)));
            }
            Job::View(view) => {
                if self
                    .view
                    .as_ref()
                    .is_none_or(|last| last.dimension != view.dimension)
                {
                    // The renderer drops the old dimension's meshes itself.
                    self.meshed.clear();
                    self.dirty.clear();
                }
                world(&mut self.worlds, self.cache.as_deref(), &view.dimension).evict_beyond(
                    view.x,
                    view.z,
                    view.radius + KEEP_MARGIN,
                );
                self.view = Some(view);
            }
            Job::Request(keys) => {
                let now = Instant::now();
                for key in keys {
                    self.meshed.insert(key);
                    self.dirty.insert(key, now);
                }
            }
            Job::Dropped(keys) => {
                for key in keys {
                    self.meshed.remove(&key);
                    self.dirty.remove(&key);
                }
            }
            Job::Cache(cache) => {
                for (dimension, world) in &mut self.worlds {
                    world.set_dir(dimension_dir(cache.as_deref(), dimension));
                }
                self.cache = cache;
            }
            Job::Stop => unreachable!("the run loop ends on it"),
        }
    }

    /// Meshes the ready sections nearest the player, a batch at a time.
    fn mesh_nearest(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        let Some(view) = self.view.clone() else {
            self.dirty.clear();
            return;
        };
        let now = Instant::now();
        let mut nearest: Vec<SectionKey> = self
            .dirty
            .iter()
            .filter(|&(_, &ready)| ready <= now)
            .map(|(&key, _)| key)
            .collect();
        if nearest.is_empty() {
            return;
        }
        nearest.sort_by(|a, b| {
            a.distance(view.x, view.z)
                .total_cmp(&b.distance(view.x, view.z))
                .then(b.level.cmp(&a.level))
        });
        nearest.truncate(MESHES_PER_BATCH);
        let world = world(&mut self.worlds, self.cache.as_deref(), &view.dimension);
        let mut sections = Vec::with_capacity(nearest.len());
        let unseen = WorldSection::unseen();
        for key in nearest {
            self.dirty.remove(&key);
            // The section and the six around it, read from disk if need be.
            world.load(key);
            for [dx, dy, dz] in SIDES {
                world.load(SectionKey {
                    x: key.x + dx,
                    y: key.y + dy,
                    z: key.z + dz,
                    ..key
                });
            }
            let section = world.get(key).unwrap_or(&unseen);
            let mut models = self.colors.baking(&mut self.models);
            sections.push(
                self.factory
                    .build(key, section, |k| world.get(k), &mut models),
            );
        }
        let _ = self.updates.send(DistantUpdate {
            dimension: Arc::clone(&view.dimension),
            sections,
            tiles: self.models.take_tiles(),
            tints: (!std::mem::replace(&mut self.tints_sent, true)).then(|| self.colors.tint_lut()),
        });
    }
}
