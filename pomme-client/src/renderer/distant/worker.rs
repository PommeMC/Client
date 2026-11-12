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
use super::world::{Changed, World, ingest_order};
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

    /// Puts back a capture taken off, unless a newer one of its column came
    /// since.
    fn requeue(&mut self, dimension: Arc<str>, capture: Capture) {
        if !self
            .latest
            .contains_key(&(Arc::clone(&dimension), capture.pos))
        {
            self.push(dimension, capture);
        }
    }

    fn is_empty(&self) -> bool {
        self.order.is_empty()
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
    /// The renderer's distant state starts over with the option turned on:
    /// it holds no sections, models or tints. Updates carry the reset they
    /// follow, so a batch meshed for the state before is dropped.
    RendererReset(u32),
    /// The option turned off: nothing more to mesh until it's back.
    RendererGone,
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
    /// Writes what's unsaved.
    Flush,
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
    reset: u32,
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
    /// Counts the times the option turned on ([`Job::RendererReset`]).
    resets: u32,
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
                Worker::new(colors, update_tx, queue, in_memory).run(&job_rx, &wake_rx);
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
            resets: 0,
        }
    }

    /// Keeps sections under the cache root, in a folder for the block
    /// states of table `slot` and the server's `biomes`, since the voxels
    /// store both by id. Captures still queued under another folder are
    /// dropped rather than written to it: the table they were read with may
    /// already be the next one's (the colours switch first), and the game
    /// captures its columns again under the new folder as they load.
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
            if self.view.take().is_some() {
                let _ = self.jobs.send(Job::RendererGone);
            }
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
        if turned_on {
            self.resets += 1;
            let _ = self.jobs.send(Job::RendererReset(self.resets));
        }
        if !unchanged {
            self.view = Some(view.clone());
            let _ = self.jobs.send(Job::View(view));
        }
        turned_on
    }

    /// Has the thread, once it reaches this among its jobs, take in every
    /// capture queued and write everything unsaved, for when the app may
    /// be ended without dropping the game (Android in the background).
    /// Returns at once.
    pub fn flush(&self) {
        let _ = self.jobs.send(Job::Flush);
    }

    pub fn drain(&self) -> impl Iterator<Item = DistantUpdate> + '_ {
        self.updates
            .try_iter()
            .filter(|update| update.reset == self.resets)
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
    let threads = std::mem::take(&mut *STOPPING.lock());
    for thread in threads {
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

/// How long the thread may sleep: not at all with captures queued, else
/// until the next section is ready to mesh or the next save is due.
fn sleep_for(queued: bool, next_ready: Option<Instant>, last_save: Instant) -> Duration {
    if queued {
        return Duration::ZERO;
    }
    match next_ready {
        Some(ready) => ready.saturating_duration_since(Instant::now()),
        None => SAVE_EVERY.saturating_sub(last_save.elapsed()),
    }
}

/// Takes queued captures through `take` for up to `slice`; whether `take`
/// asked to stop.
fn take_slice(
    captures: &Mutex<CaptureQueue>,
    slice: Duration,
    mut take: impl FnMut(Arc<str>, Capture) -> bool,
) -> bool {
    let start = Instant::now();
    while let Some((dimension, capture)) = next(captures) {
        if take(dimension, capture) {
            return true;
        }
        if start.elapsed() >= slice {
            break;
        }
    }
    false
}

/// The thread's kept sections: a world per dimension and where they're
/// written.
struct Store {
    worlds: HashMap<Arc<str>, World>,
    cache: Option<PathBuf>,
    /// Whether there's no cache root, so no folder will ever come.
    in_memory: bool,
    /// `cache`'s [`DistantTerrain::generation`].
    generation: u32,
    last_save: Instant,
}

impl Store {
    fn new(in_memory: bool) -> Self {
        Self {
            worlds: HashMap::new(),
            cache: None,
            in_memory,
            generation: 0,
            last_save: Instant::now(),
        }
    }

    /// `dimension`'s world, made with its folder under the cache, or kept
    /// in memory only when no folder will come.
    fn world(&mut self, dimension: &Arc<str>) -> &mut World {
        let (cache, in_memory) = (self.cache.as_deref(), self.in_memory);
        self.worlds.entry(Arc::clone(dimension)).or_insert_with(|| {
            if in_memory {
                World::in_memory()
            } else {
                World::new(dimension_dir(cache, dimension))
            }
        })
    }

    /// Writes a chunk into its dimension's sections; `None` for a capture
    /// from an earlier folder, dropped.
    fn ingest(
        &mut self,
        states: &States,
        dimension: &Arc<str>,
        capture: Capture,
    ) -> Option<Vec<Changed>> {
        if capture.generation != self.generation {
            return None;
        }
        // Copied so the game's thread can write the chunk while it's taken
        // in, which may read sections from disk.
        let sections = capture.chunk.read().sections.clone();
        Some(self.world(dimension).ingest_chunk(
            states,
            capture.pos,
            &sections,
            capture.light.as_deref(),
            capture.min_y,
        ))
    }

    fn save(&mut self) {
        for world in self.worlds.values_mut() {
            world.flush();
        }
        self.last_save = Instant::now();
    }

    /// Keeps sections in `dir` from now on. Returns whether the sections
    /// held were another folder's, written there and let go.
    fn set_cache(&mut self, dir: Option<PathBuf>, generation: u32) -> bool {
        let switched = self.cache.is_some();
        if switched {
            self.save();
            self.worlds.clear();
        } else {
            for (dimension, world) in &mut self.worlds {
                world.set_dir(dimension_dir(dir.as_deref(), dimension));
            }
        }
        self.cache = dir;
        self.generation = generation;
        switched
    }

    /// Writes and lets go of every dimension's sections but `dimension`'s,
    /// keeping those with nowhere to be written yet.
    fn keep_only(&mut self, dimension: &str) {
        self.worlds.retain(|kept, world| {
            if **kept == *dimension {
                return true;
            }
            world.flush();
            !world.is_saved()
        });
    }

    /// Takes in every queued capture, in an order that reads each section
    /// about once however many there are, then writes what's unsaved,
    /// writing as it goes too. A capture under a folder not switched to yet
    /// goes back in the queue.
    fn take_all(&mut self, states: &States, captures: &Mutex<CaptureQueue>) {
        let mut backlog: Vec<_> = std::iter::from_fn(|| next(captures)).collect();
        backlog.sort_by_key(|(_, capture)| ingest_order(capture.pos));
        for (dimension, capture) in backlog {
            if capture.generation > self.generation {
                captures.lock().requeue(dimension, capture);
                continue;
            }
            self.ingest(states, &dimension, capture);
            if self.last_save.elapsed() >= SAVE_EVERY_STOPPING {
                self.save();
            }
        }
        self.save();
    }
}

struct Worker {
    colors: Arc<DistantColors>,
    models: ModelFactory,
    states: States,
    factory: Factory,
    updates: Sender<DistantUpdate>,
    captures: Arc<Mutex<CaptureQueue>>,
    store: Store,
    view: Option<View>,
    /// Sections to mesh, each from when.
    dirty: HashMap<SectionKey, Instant>,
    /// Sections the renderer holds, kept up to date as captures change them.
    meshed: HashSet<SectionKey>,
    /// Whether the renderer has the current colours' biome tints.
    tints_sent: bool,
    /// The renderer's last [`Job::RendererReset`].
    reset: u32,
}

impl Worker {
    fn new(
        colors: Arc<DistantColors>,
        updates: Sender<DistantUpdate>,
        captures: Arc<Mutex<CaptureQueue>>,
        in_memory: bool,
    ) -> Self {
        Self {
            states: States::new(colors.table()),
            colors,
            models: ModelFactory::new(),
            factory: Factory::new(),
            updates,
            captures,
            store: Store::new(in_memory),
            view: None,
            dirty: HashMap::new(),
            meshed: HashSet::new(),
            tints_sent: false,
            reset: 0,
        }
    }

    fn run(mut self, jobs: &Receiver<Job>, wake: &Receiver<()>) {
        let captures = Arc::clone(&self.captures);
        loop {
            let wait = sleep_for(
                !captures.lock().is_empty(),
                self.dirty.values().min().copied(),
                self.store.last_save,
            );
            // Sleeps until there's something to do: a job, a capture, a
            // section ready to mesh or a save due.
            let stop = crossbeam_channel::select! {
                recv(jobs) -> job => self.handle(job.ok()),
                recv(wake) -> _ => false,
                default(wait) => false,
            };
            if stop || self.take_jobs(jobs) {
                break;
            }
            let stop = take_slice(&captures, CAPTURE_SLICE, |dimension, capture| {
                // A capture under a newer folder was queued after the job
                // switching to it.
                let stop = capture.generation != self.store.generation && self.take_jobs(jobs);
                self.capture(&dimension, capture);
                stop
            });
            if stop {
                break;
            }
            self.mesh_nearest();
            if self.store.last_save.elapsed() >= SAVE_EVERY {
                self.store.save();
            }
        }
        // What's unsaved first, then the columns kept on the way out.
        self.store.save();
        self.store.take_all(&self.states, &captures);
    }

    /// Handles the jobs waiting; whether one was the stop.
    fn take_jobs(&mut self, jobs: &Receiver<Job>) -> bool {
        jobs.try_iter().any(|job| self.handle(Some(job)))
    }

    /// Meshes every section the renderer holds again.
    fn remesh_all(&mut self) {
        let now = Instant::now();
        self.dirty.extend(self.meshed.iter().map(|&key| (key, now)));
    }

    /// Writes a chunk into its dimension's sections and marks the meshed
    /// sections it changed, and their neighbours across the sides the
    /// change reached, for meshing again.
    fn capture(&mut self, dimension: &Arc<str>, capture: Capture) {
        let Some(changed) = self.store.ingest(&self.states, dimension, capture) else {
            return;
        };
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
            Some(Job::RendererReset(reset)) => {
                self.reset = reset;
                self.models = ModelFactory::new();
                self.tints_sent = false;
                self.meshed.clear();
                self.dirty.clear();
            }
            Some(Job::RendererGone) => self.view = None,
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
                self.store.keep_only(&view.dimension);
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
                if self.store.set_cache(dir, generation) {
                    // What the renderer holds, meshed again from the new
                    // folder.
                    self.remesh_all();
                }
            }
            Some(Job::Flush) => self.store.take_all(&self.states, &self.captures),
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
        let world = self.store.world(&view.dimension);
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
            reset: self.reset,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::distant::world::tests::{column, temp_dir};
    use crate::world::block::BlockStateTable;

    fn capture(x: i32, generation: u32) -> Capture {
        Capture {
            pos: ChunkPos::new(x, 0),
            chunk: Arc::new(RwLock::new(Chunk::empty(1))),
            light: None,
            min_y: 0,
            generation,
        }
    }

    /// A capture of a lit four-section column, stone at the bottom.
    fn lit(x: i32, generation: u32) -> Capture {
        let (sections, light) = column(Some(15));
        let mut chunk = Chunk::empty(sections.len());
        chunk.sections = sections.into();
        Capture {
            chunk: Arc::new(RwLock::new(chunk)),
            light: light.map(Arc::new),
            ..capture(x, generation)
        }
    }

    fn states() -> States {
        States::new(BlockStateTable::native())
    }

    /// The level-0 section file of column `x`'s bottom section in `dir`.
    fn file(dir: &Path, x: i32) -> PathBuf {
        let SectionKey { level, x, y, z } = SectionKey::of_vanilla(0, x, 0, 0);
        dimension_dir(Some(dir), "overworld")
            .expect("a cache dir")
            .join(format!("{level}.{x}.{y}.{z}.bin"))
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

    #[test]
    fn captures_past_a_slice_drain_without_another_wake() {
        let captures = Mutex::new(CaptureQueue::default());
        for x in 0..3 {
            captures.lock().push("overworld".into(), lit(x * 2, 0));
        }
        let (states, mut store) = (states(), Store::new(true));
        let mut slices = 0;
        // The run loop with no wake left: it sleeps only as long as
        // `sleep_for` says.
        while sleep_for(!captures.lock().is_empty(), None, Instant::now()).is_zero() {
            take_slice(&captures, Duration::ZERO, |dimension, capture| {
                store.ingest(&states, &dimension, capture);
                false
            });
            slices += 1;
        }
        assert_eq!(slices, 3, "one capture per slice");
        let world = store.world(&"overworld".into());
        for x in 0..3 {
            assert!(world.get(SectionKey::of_vanilla(0, x * 2, 0, 0)).is_some());
        }
        assert!(sleep_for(false, None, Instant::now()) > Duration::ZERO);
    }

    #[test]
    fn a_cache_switch_writes_the_old_folder_and_drops_its_captures() {
        let (old, new) = (temp_dir("old"), temp_dir("new"));
        let (states, mut store) = (states(), Store::new(false));
        let overworld: Arc<str> = "overworld".into();
        store.set_cache(Some(old.clone()), 0);
        assert!(store.ingest(&states, &overworld, lit(0, 0)).is_some());

        assert!(store.set_cache(Some(new.clone()), 1), "another folder's");
        assert!(file(&old, 0).exists(), "written to the old folder");
        assert!(store.worlds.is_empty());
        assert!(store.ingest(&states, &overworld, lit(4, 0)).is_none());
        assert!(store.ingest(&states, &overworld, lit(8, 1)).is_some());
        store.save();
        assert!(!file(&new, 4).exists() && !file(&old, 4).exists());
        assert!(file(&new, 8).exists());
        for dir in [old, new] {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn a_flush_takes_in_the_queued_captures_before_writing() {
        let dir = temp_dir("flush");
        let (states, mut store) = (states(), Store::new(false));
        store.set_cache(Some(dir.clone()), 0);
        let captures = Mutex::new(CaptureQueue::default());
        for x in [6, 0, 2] {
            captures.lock().push("overworld".into(), lit(x, 0));
        }
        // Queued after a switch the thread hasn't reached: kept for it.
        captures.lock().push("overworld".into(), lit(10, 1));
        store.take_all(&states, &captures);
        for x in [6, 0, 2] {
            assert!(file(&dir, x).exists());
        }
        assert!(!file(&dir, 10).exists());
        let left: Vec<i32> = std::iter::from_fn(|| next(&captures))
            .map(|(_, capture)| capture.pos.x)
            .collect();
        assert_eq!(left, [10]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn waiting_joins_a_stopping_thread_once_it_finishes() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        STOPPING.lock().push(std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::SeqCst);
        }));
        let start = Instant::now();
        wait_for_stopped(Duration::from_secs(30));
        assert!(done.load(Ordering::SeqCst));
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(STOPPING.lock().is_empty());
    }
}
