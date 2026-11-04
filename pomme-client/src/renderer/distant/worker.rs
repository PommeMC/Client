//! The distant terrain thread: turns chunks the server forgets into distant
//! sections, keeps them per dimension, on disk too, and meshes the sections
//! the renderer asks for and keeps them up to date.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use parking_lot::{Mutex, RwLock};

use super::build::{BuiltSection, Factory};
use super::colors::DistantColors;
use super::models::{ModelFactory, ModelTile};
use super::section::{SectionKey, WorldSection};
use super::voxel::States;
use super::world::{World, ingest_order};
use crate::position::ChunkPos;
use crate::world::chunk::{Chunk, ChunkLightData, ChunkStore};

/// Sections meshed between checks for new jobs.
const MESHES_PER_BATCH: usize = 32;
/// How far the player moves, in blocks, before the thread hears of it.
const VIEW_STEP: f64 = 32.0;
/// How often changed sections are written to disk.
const SAVE_EVERY: Duration = Duration::from_secs(60);
/// How often they're written while the thread takes in the last columns on
/// its way out, so an exit that cuts it short loses little.
const SAVE_EVERY_STOPPING: Duration = Duration::from_secs(1);
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

/// The threads of games left, still writing what they kept.
static STOPPING: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// A chunk to keep, before it goes.
struct Capture {
    pos: ChunkPos,
    chunk: Arc<RwLock<Chunk>>,
    light: Option<Arc<ChunkLightData>>,
    min_y: i32,
    /// The cache folder it was taken under ([`DistantTerrain::generation`]).
    generation: u32,
}

/// Captures waiting for the thread, at most one per column: a newer one
/// replaces the one queued, keeping its place.
#[derive(Default)]
struct CaptureQueue {
    order: VecDeque<(Arc<str>, ChunkPos)>,
    latest: HashMap<(Arc<str>, ChunkPos), Capture>,
}

impl CaptureQueue {
    fn push(&mut self, dimension: Arc<str>, capture: Capture) {
        let column = (dimension, capture.pos);
        if self.latest.insert(column.clone(), capture).is_none() {
            self.order.push_back(column);
        }
    }

    fn pop(&mut self) -> Option<(Arc<str>, Capture)> {
        let column = self.order.pop_front()?;
        let capture = self.latest.remove(&column)?;
        Some((column.0, capture))
    }

    fn clear(&mut self) {
        self.order.clear();
        self.latest.clear();
    }
}

/// The next capture, the queue's lock let go before it's taken in.
fn next(captures: &Mutex<CaptureQueue>) -> Option<(Arc<str>, Capture)> {
    captures.lock().pop()
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
    Cache {
        dir: Option<PathBuf>,
        generation: u32,
    },
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

/// The game's handle on the thread. Dropping it lets the thread finish
/// writing in the background ([`wait_for_stopped`]).
pub(crate) struct DistantTerrain {
    jobs: Sender<Job>,
    captures: Arc<Mutex<CaptureQueue>>,
    /// Wakes the thread for a capture.
    wake: Sender<()>,
    updates: Receiver<DistantUpdate>,
    thread: Option<JoinHandle<()>>,
    /// The last view sent, so small moves don't flood the thread.
    view: Option<View>,
    /// This server's or world's folder of distant terrain.
    cache_root: Option<PathBuf>,
    /// The folder last sent, under `cache_root`.
    cache: Option<PathBuf>,
    /// Counts the folders switched from, so captures taken under one never
    /// land in the next.
    generation: u32,
}

impl DistantTerrain {
    pub fn new(colors: Arc<DistantColors>, cache_root: Option<PathBuf>) -> Self {
        let (jobs, job_rx) = crossbeam_channel::unbounded();
        let (wake, wake_rx) = crossbeam_channel::bounded(1);
        let (update_tx, updates) = crossbeam_channel::unbounded();
        let captures = Arc::new(Mutex::new(CaptureQueue::default()));
        let queue = Arc::clone(&captures);
        // Games left may still be writing to the same folders.
        let earlier = std::mem::take(&mut *STOPPING.lock());
        let in_memory = cache_root.is_none();
        let thread = std::thread::Builder::new()
            .name("distant-terrain".into())
            .spawn(move || {
                for thread in earlier {
                    let _ = thread.join();
                }
                Worker::new(colors, update_tx, in_memory).run(&job_rx, &wake_rx, &queue);
            })
            .expect("failed to spawn the distant terrain thread");
        Self {
            jobs,
            captures,
            wake,
            updates,
            thread: Some(thread),
            view: None,
            cache_root,
            cache: None,
            generation: 0,
        }
    }

    /// Keeps sections under the cache root, in a folder for the block
    /// states of table `slot` and the server's `biomes`, since the voxels
    /// store both by id. Captures still queued under another folder are
    /// dropped.
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
        if cache == self.cache {
            return;
        }
        if self.cache.is_some() {
            self.generation += 1;
            self.captures.lock().clear();
        }
        self.cache.clone_from(&cache);
        let _ = self.jobs.send(Job::Cache {
            dir: cache,
            generation: self.generation,
        });
    }

    /// Whether chunks are being kept: a view with a radius was set.
    pub fn enabled(&self) -> bool {
        self.view.is_some()
    }

    /// Keeps `pos`'s column of `chunks` as distant terrain of `dimension`:
    /// once lit, and again before it goes.
    pub fn capture(&self, dimension: &str, pos: ChunkPos, chunks: &ChunkStore) {
        if !self.enabled() {
            return;
        }
        let Some(chunk) = chunks.get_chunk(&pos) else {
            return;
        };
        self.captures.lock().push(
            dimension.into(),
            Capture {
                pos,
                chunk,
                light: chunks.light_data.get(&(pos.x, pos.z)).cloned(),
                min_y: chunks.min_y(),
                generation: self.generation,
            },
        );
        let _ = self.wake.try_send(());
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
    /// new chunks. Returns whether this turned distant terrain on, so the
    /// caller can hand over the columns lit while it was off.
    pub fn set_view(&mut self, dimension: &str, x: f64, z: f64, chunks: u32) -> bool {
        if chunks == 0 {
            self.view = None;
            return false;
        }
        let turned_on = self.view.is_none();
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
        turned_on
    }

    pub fn drain(&self) -> impl Iterator<Item = DistantUpdate> + '_ {
        self.updates.try_iter()
    }
}

/// The thread goes on taking in the columns kept on the way out and writing
/// them, off the game's thread: the next game's thread waits for it before
/// reading, and the app waits for it before exiting ([`wait_for_stopped`]).
impl Drop for DistantTerrain {
    fn drop(&mut self) {
        let _ = self.jobs.send(Job::Stop);
        let _ = self.wake.try_send(());
        if let Some(thread) = self.thread.take() {
            STOPPING.lock().push(thread);
        }
    }
}

/// Waits up to `timeout` for the threads of games left to finish writing,
/// for the app to call once its window is gone. Each file is written whole,
/// so a thread cut off loses only what it hadn't written yet.
pub(crate) fn wait_for_stopped(timeout: Duration) {
    let deadline = Instant::now() + timeout;
    for thread in std::mem::take(&mut *STOPPING.lock()) {
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !thread.is_finished() {
            tracing::warn!("Exiting before distant terrain finished saving");
            return;
        }
        let _ = thread.join();
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

/// Where `dimension`'s sections go under `cache`, versioned with the file
/// format.
fn dimension_dir(cache: Option<&Path>, dimension: &str) -> Option<PathBuf> {
    Some(cache?.join(folder_name(dimension)).join("sections-v2"))
}

/// `dimension`'s world, made with its folder under `cache`, or kept in
/// memory only when no folder will come.
fn world<'a>(
    worlds: &'a mut HashMap<Arc<str>, World>,
    cache: Option<&Path>,
    in_memory: bool,
    dimension: &Arc<str>,
) -> &'a mut World {
    worlds.entry(Arc::clone(dimension)).or_insert_with(|| {
        if in_memory {
            World::in_memory()
        } else {
            World::new(dimension_dir(cache, dimension))
        }
    })
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
    /// Whether there's no cache root, so no folder will ever come.
    in_memory: bool,
    /// `cache`'s [`DistantTerrain::generation`].
    generation: u32,
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
    fn new(colors: Arc<DistantColors>, updates: Sender<DistantUpdate>, in_memory: bool) -> Self {
        Self {
            states: States::new(colors.table()),
            colors,
            models: ModelFactory::new(),
            factory: Factory::new(),
            updates,
            worlds: HashMap::new(),
            cache: None,
            in_memory,
            generation: 0,
            last_save: Instant::now(),
            view: None,
            dirty: HashMap::new(),
            meshed: HashSet::new(),
            tints_sent: false,
        }
    }

    fn run(mut self, jobs: &Receiver<Job>, wake: &Receiver<()>, captures: &Mutex<CaptureQueue>) {
        'run: loop {
            let wait = match self.dirty.values().min() {
                Some(ready) => ready.saturating_duration_since(Instant::now()),
                None => SAVE_EVERY.saturating_sub(self.last_save.elapsed()),
            };
            // Sleeps until there's something to do: a job, a capture, a
            // section ready to mesh or a save due.
            crossbeam_channel::select! {
                recv(jobs) -> job => if self.handle(job.ok()) {
                    break 'run;
                },
                recv(wake) -> _ => {}
                default(wait) => {}
            }
            if self.take_jobs(jobs) {
                break 'run;
            }
            let slice = Instant::now();
            while let Some((dimension, capture)) = next(captures) {
                // A capture under a newer folder was queued after the job
                // switching to it.
                let stop = capture.generation != self.generation && self.take_jobs(jobs);
                self.capture(&dimension, capture);
                if stop {
                    break 'run;
                }
                if slice.elapsed() >= CAPTURE_SLICE {
                    break;
                }
            }
            self.mesh_nearest();
            if self.last_save.elapsed() >= SAVE_EVERY {
                self.save();
            }
        }
        // What's unsaved first, then the columns kept on the way out, which
        // were queued before the stop, in an order that reads each section
        // about once however many there are.
        self.save();
        let mut backlog: Vec<_> = std::iter::from_fn(|| next(captures)).collect();
        backlog.sort_by_key(|(_, capture)| ingest_order(capture.pos));
        for (dimension, capture) in backlog {
            self.capture(&dimension, capture);
            if self.last_save.elapsed() >= SAVE_EVERY_STOPPING {
                self.save();
            }
        }
        self.save();
    }

    /// Handles the jobs waiting; whether one was the stop.
    fn take_jobs(&mut self, jobs: &Receiver<Job>) -> bool {
        jobs.try_iter().any(|job| self.handle(Some(job)))
    }

    fn save(&mut self) {
        for world in self.worlds.values_mut() {
            world.flush();
        }
        self.last_save = Instant::now();
    }

    /// Meshes every section the renderer holds again.
    fn remesh_all(&mut self) {
        let now = Instant::now();
        self.dirty.extend(self.meshed.iter().map(|&key| (key, now)));
    }

    /// Writes a chunk into its dimension's sections and marks the meshed
    /// sections it changed, and their neighbours across the sides the
    /// change reached, for meshing again. A capture from an earlier folder
    /// is dropped.
    fn capture(&mut self, dimension: &Arc<str>, capture: Capture) {
        if capture.generation != self.generation {
            return;
        }
        // Copied so the game's thread can write the chunk while it's taken
        // in, which may read sections from disk.
        let sections = capture.chunk.read().sections.clone();
        let changed = world(
            &mut self.worlds,
            self.cache.as_deref(),
            self.in_memory,
            dimension,
        )
        .ingest_chunk(
            &self.states,
            capture.pos,
            &sections,
            capture.light.as_deref(),
            capture.min_y,
        );
        if self
            .view
            .as_ref()
            .is_none_or(|view| view.dimension != *dimension)
        {
            return;
        }
        let ready = Instant::now() + SETTLE;
        for change in changed {
            let neighbours = SIDES
                .iter()
                .enumerate()
                .filter(|&(side, _)| change.sides & 1 << side != 0)
                .map(|(_, &side)| change.key.offset(side));
            for key in std::iter::once(change.key).chain(neighbours) {
                if self.meshed.contains(&key) {
                    self.dirty.entry(key).or_insert(ready);
                }
            }
        }
    }

    /// Handles `job`, `None` for the game gone; whether to stop.
    fn handle(&mut self, job: Option<Job>) -> bool {
        match job {
            None | Some(Job::Stop) => return true,
            Some(Job::Colors(colors)) => {
                if !colors.same_models(&self.colors) {
                    // Ids start over; the new tiles overwrite the old.
                    self.models = ModelFactory::new();
                    self.states = States::new(colors.table());
                }
                self.colors = colors;
                self.tints_sent = false;
                self.remesh_all();
            }
            Some(Job::View(view)) => {
                if self
                    .view
                    .as_ref()
                    .is_none_or(|last| last.dimension != view.dimension)
                {
                    // The renderer drops the old dimension's meshes itself.
                    self.meshed.clear();
                    self.dirty.clear();
                }
                // Other dimensions' sections are written and let go, but
                // for those with nowhere to be written yet.
                self.worlds.retain(|dimension, world| {
                    if *dimension == view.dimension {
                        return true;
                    }
                    world.flush();
                    !world.is_saved()
                });
                self.view = Some(view);
            }
            Some(Job::Request(keys)) => {
                let now = Instant::now();
                for key in keys {
                    self.meshed.insert(key);
                    self.dirty.insert(key, now);
                }
            }
            Some(Job::Dropped(keys)) => {
                for key in keys {
                    self.meshed.remove(&key);
                    self.dirty.remove(&key);
                }
            }
            Some(Job::Cache { dir, generation }) => {
                if self.cache.is_some() {
                    // Another server's or registry's sections: written to
                    // their folder and forgotten, and what the renderer
                    // holds meshed again from the new one.
                    self.save();
                    self.worlds.clear();
                    self.remesh_all();
                } else {
                    // Sections kept before there was a folder go in it.
                    for (dimension, world) in &mut self.worlds {
                        world.set_dir(dimension_dir(dir.as_deref(), dimension));
                    }
                }
                self.cache = dir;
                self.generation = generation;
            }
        }
        false
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
        let world = world(
            &mut self.worlds,
            self.cache.as_deref(),
            self.in_memory,
            &view.dimension,
        );
        let mut sections = Vec::with_capacity(nearest.len());
        let unseen = WorldSection::unseen();
        for key in nearest {
            self.dirty.remove(&key);
            // The section and the six around it, read from disk if need be.
            world.load(key);
            for side in SIDES {
                world.load(key.offset(side));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(x: i32, generation: u32) -> Capture {
        Capture {
            pos: ChunkPos::new(x, 0),
            chunk: Arc::new(RwLock::new(Chunk::empty(1))),
            light: None,
            min_y: 0,
            generation,
        }
    }

    #[test]
    fn a_newer_capture_of_a_column_replaces_the_queued_one() {
        let mut queue = CaptureQueue::default();
        let overworld: Arc<str> = "overworld".into();
        queue.push(Arc::clone(&overworld), capture(1, 0));
        queue.push(Arc::clone(&overworld), capture(2, 0));
        queue.push(Arc::clone(&overworld), capture(1, 3));
        queue.push("nether".into(), capture(1, 0));
        let popped: Vec<(i32, u32)> = std::iter::from_fn(|| queue.pop())
            .map(|(_, c)| (c.pos.x, c.generation))
            .collect();
        assert_eq!(popped, [(1, 3), (2, 0), (1, 0)]);
    }
}
