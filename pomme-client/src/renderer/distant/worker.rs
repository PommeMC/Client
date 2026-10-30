//! The distant terrain thread: turns chunks the server forgets into columns,
//! keeps them per dimension, on disk too, and meshes the sections around the
//! player.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use parking_lot::RwLock;

use super::colors::DistantColors;
use super::column::build_chunk;
use super::mesh::{SectionMesh, mesh_section};
use super::store::{SectionKey, Store};
use crate::position::ChunkPos;
use crate::world::chunk::{Chunk, ChunkLightData, ChunkStore};

/// Sections meshed between checks for new jobs.
const MESHES_PER_BATCH: usize = 16;
/// How far past the view radius sections are kept, in blocks: one level-4
/// section, so walking back and forth doesn't drop and rebuild them.
const KEEP_MARGIN: f64 = 1024.0;
/// How far the player moves, in blocks, before the thread hears of it.
const VIEW_STEP: f64 = 32.0;
/// How often changed sections are written to disk.
const SAVE_EVERY: Duration = Duration::from_secs(60);

enum Job {
    Capture {
        dimension: Arc<str>,
        pos: ChunkPos,
        chunk: Arc<RwLock<Chunk>>,
        light: Option<Arc<ChunkLightData>>,
        min_y: i32,
    },
    Colors(Arc<DistantColors>),
    View(View),
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

impl View {
    fn wants(&self, key: SectionKey) -> bool {
        key.wanted(self.x, self.z, self.radius)
    }
}

/// New and dropped section meshes for one dimension.
pub(crate) struct DistantUpdate {
    pub dimension: Arc<str>,
    pub min_y: i32,
    pub meshes: Vec<SectionMesh>,
    pub removed: Vec<SectionKey>,
}

/// The game's handle on the thread. Dropping it waits for the thread to
/// write what's unsaved.
pub(crate) struct DistantTerrain {
    jobs: Sender<Job>,
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
        let (update_tx, updates) = crossbeam_channel::unbounded();
        let thread = std::thread::Builder::new()
            .name("distant-terrain".into())
            .spawn(move || Worker::new(colors, update_tx).run(&job_rx))
            .expect("failed to spawn the distant terrain thread");
        Self {
            jobs,
            updates,
            thread: Some(thread),
            view: None,
            cache_root,
            cache: None,
        }
    }

    /// Keeps sections under the cache root, in a folder for the block
    /// states of table `slot` and the server's `biomes`, since the runs
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
        let _ = self.jobs.send(Job::Capture {
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

    /// Where the player is and how far to keep and draw, in chunks; 0 stops
    /// keeping new chunks.
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

fn dimension_dir(cache: Option<&Path>, dimension: &str) -> Option<PathBuf> {
    Some(cache?.join(folder_name(dimension)))
}

/// `dimension`'s store and `min_y`, made with its folder under `cache`.
fn store<'a>(
    stores: &'a mut HashMap<Arc<str>, (Store, i32)>,
    cache: Option<&Path>,
    dimension: &Arc<str>,
) -> &'a mut (Store, i32) {
    stores
        .entry(Arc::clone(dimension))
        .or_insert_with(|| (Store::new(dimension_dir(cache, dimension)), 0))
}

struct Worker {
    colors: Arc<DistantColors>,
    updates: Sender<DistantUpdate>,
    /// Per dimension, its sections and `min_y`.
    stores: HashMap<Arc<str>, (Store, i32)>,
    cache: Option<PathBuf>,
    last_save: Instant,
    view: Option<View>,
    /// The viewed dimension's sections to mesh.
    dirty: HashSet<SectionKey>,
    /// The viewed dimension's sections with a mesh sent.
    meshed: HashSet<SectionKey>,
}

impl Worker {
    fn new(colors: Arc<DistantColors>, updates: Sender<DistantUpdate>) -> Self {
        Self {
            colors,
            updates,
            stores: HashMap::new(),
            cache: None,
            last_save: Instant::now(),
            view: None,
            dirty: HashSet::new(),
            meshed: HashSet::new(),
        }
    }

    fn run(mut self, jobs: &Receiver<Job>) {
        'run: loop {
            let wait = if self.dirty.is_empty() {
                SAVE_EVERY.saturating_sub(self.last_save.elapsed())
            } else {
                Duration::ZERO
            };
            let first = match jobs.recv_timeout(wait) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            for job in first.into_iter().chain(jobs.try_iter()) {
                if matches!(job, Job::Stop) {
                    break 'run;
                }
                self.handle(job);
            }
            self.mesh_nearest();
            if self.last_save.elapsed() >= SAVE_EVERY {
                self.save();
            }
        }
        self.save();
    }

    fn save(&mut self) {
        for (store, _) in self.stores.values_mut() {
            store.flush();
        }
        self.last_save = Instant::now();
    }

    fn handle(&mut self, job: Job) {
        match job {
            Job::Capture {
                dimension,
                pos,
                chunk,
                light,
                min_y,
            } => {
                let columns = build_chunk(&chunk.read(), light.as_deref(), min_y, &self.colors);
                let (store, store_min_y) =
                    store(&mut self.stores, self.cache.as_deref(), &dimension);
                *store_min_y = min_y;
                let touched = store.capture(pos, columns);
                if let Some(view) = &self.view
                    && view.dimension == dimension
                {
                    self.dirty
                        .extend(touched.into_iter().filter(|key| view.wants(*key)));
                }
            }
            Job::Colors(colors) => {
                self.colors = colors;
                self.dirty.extend(self.meshed.iter().copied());
            }
            Job::View(view) => {
                if self
                    .view
                    .as_ref()
                    .is_some_and(|last| last.dimension != view.dimension)
                {
                    // The renderer drops the old dimension's meshes itself.
                    self.meshed.clear();
                    self.dirty.clear();
                }
                let (store, min_y) =
                    store(&mut self.stores, self.cache.as_deref(), &view.dimension);
                store.evict_beyond(view.x, view.z, view.radius + KEEP_MARGIN);
                let wanted: HashSet<SectionKey> =
                    SectionKey::all_wanted(view.x, view.z, view.radius)
                        .into_iter()
                        .filter(|key| store.load(*key))
                        .collect();
                let removed: Vec<SectionKey> = self.meshed.difference(&wanted).copied().collect();
                for key in &removed {
                    self.meshed.remove(key);
                }
                self.dirty.retain(|key| wanted.contains(key));
                self.dirty.extend(wanted.difference(&self.meshed).copied());
                if !removed.is_empty() {
                    let _ = self.updates.send(DistantUpdate {
                        dimension: Arc::clone(&view.dimension),
                        min_y: *min_y,
                        meshes: Vec::new(),
                        removed,
                    });
                }
                self.view = Some(view);
            }
            Job::Cache(cache) => {
                for (dimension, (store, _)) in &mut self.stores {
                    store.set_dir(dimension_dir(cache.as_deref(), dimension));
                }
                self.cache = cache;
            }
            Job::Stop => unreachable!("the run loop ends on it"),
        }
    }

    /// Meshes the dirty sections nearest the player, a batch at a time.
    fn mesh_nearest(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        let Some(view) = &self.view else {
            self.dirty.clear();
            return;
        };
        let Some((store, min_y)) = self.stores.get(&view.dimension) else {
            self.dirty.clear();
            return;
        };
        let mut nearest: Vec<SectionKey> = self.dirty.iter().copied().collect();
        nearest.sort_by(|a, b| {
            a.distance(view.x, view.z)
                .total_cmp(&b.distance(view.x, view.z))
                .then(b.level.cmp(&a.level))
        });
        nearest.truncate(MESHES_PER_BATCH);
        let meshes = nearest
            .iter()
            .filter_map(|&key| {
                self.dirty.remove(&key);
                self.meshed.insert(key);
                let section = store.get(key)?;
                Some(mesh_section(key, section, |run| self.colors.colors(run)))
            })
            .collect();
        let _ = self.updates.send(DistantUpdate {
            dimension: Arc::clone(&view.dimension),
            min_y: *min_y,
            meshes,
            removed: Vec::new(),
        });
    }
}
