//! The light thread. Vanilla's client runs `LevelLightEngine` on the render
//! thread; here it runs on its own, as vanilla's server engine does
//! (`ThreadedLevelLightEngine`), fed the same calls in the same order and
//! publishing the same sections. A departure for frame time: a burst of
//! chunk loads or unloads lights off the game thread instead of inside one
//! frame. Block edits still light within their frame when the thread is
//! idle, which it is unless such a burst is in progress: the game thread
//! waits a moment for them. Vanilla's per-tick metering of queued light
//! tasks is dropped: each run applies every task queued (see
//! [`LevelLightEngine::run`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use azalea_block::BlockState;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use parking_lot::RwLock;

use super::world::LightWorld;
use super::{LevelLightEngine, LightDirty, LightPos, LightTask};
use crate::world::block::BlockStateTable;
use crate::world::chunk::{Chunk, ChunkLightData, ChunkStore};

/// How long the game thread waits for a block edit's light before it draws
/// the frame without it; the light then lands in the frame it is ready for.
const EDIT_WAIT: Duration = Duration::from_millis(2);

/// The thread only ends early by panicking; the game goes down with it.
const THREAD_GONE: &str = "the light thread panicked";

enum Message {
    Task(LightTask),
    ChunkLoaded {
        pos: (i32, i32),
        chunk: Arc<RwLock<Chunk>>,
    },
    ChunkUnloaded((i32, i32)),
    BlockChanged {
        pos: LightPos,
        old: BlockState,
        new: BlockState,
        empty_flip: Option<bool>,
    },
    BlockDirty(LightPos),
}

/// One run's outcome: the columns whose light changed, and what to remesh.
struct Report {
    /// Messages handled so far, so the game thread knows an edit's light is
    /// in.
    handled: u64,
    columns: Vec<((i32, i32), Arc<ChunkLightData>)>,
    dirty: LightDirty,
}

/// The thread's columns, and which changed since the last report.
#[derive(Default)]
pub(crate) struct LightMap {
    columns: HashMap<(i32, i32), Arc<ChunkLightData>>,
    touched: HashSet<(i32, i32)>,
}

impl LightMap {
    pub fn get_mut(&mut self, column: (i32, i32)) -> Option<&mut Arc<ChunkLightData>> {
        let data = self.columns.get_mut(&column)?;
        self.touched.insert(column);
        Some(data)
    }

    pub fn insert(&mut self, column: (i32, i32), data: ChunkLightData) {
        self.columns.insert(column, Arc::new(data));
        self.touched.insert(column);
    }

    pub fn remove(&mut self, column: (i32, i32)) {
        self.columns.remove(&column);
        self.touched.remove(&column);
    }

    fn take_touched(&mut self) -> Vec<((i32, i32), Arc<ChunkLightData>)> {
        self.touched
            .drain()
            .filter_map(|column| Some((column, Arc::clone(self.columns.get(&column)?))))
            .collect()
    }
}

/// The game thread's handle: what it tells the engine, and what it learnt
/// back.
pub(crate) struct LightWorker {
    tx: Sender<Message>,
    rx: Receiver<Report>,
    sent: u64,
    /// The message count an edit's light is waited for up to.
    awaited: Option<u64>,
    /// Columns whose light is on (vanilla `lightOnInColumn`).
    light_on: HashSet<(i32, i32)>,
    /// Each column's last unload, by message count, until a report shows the
    /// thread past it: what an earlier report says of the column is about the
    /// chunk that went, not one loaded there since.
    unloaded: HashMap<(i32, i32), u64>,
    has_sky: bool,
    section_count: i32,
}

impl LightWorker {
    pub fn spawn(
        height: u32,
        min_y: i32,
        has_sky: bool,
        block_state_table: BlockStateTable,
    ) -> Self {
        let (tx, messages) = crossbeam_channel::unbounded();
        let (reports, rx) = crossbeam_channel::unbounded();
        let engine = LevelLightEngine::new(height, min_y, has_sky);
        std::thread::Builder::new()
            .name("chunk-light".into())
            .spawn(move || {
                run(
                    engine,
                    LightWorld::new(block_state_table, min_y),
                    &messages,
                    &reports,
                )
            })
            .expect("failed to spawn the light thread");
        Self {
            tx,
            rx,
            sent: 0,
            awaited: None,
            light_on: HashSet::new(),
            unloaded: HashMap::new(),
            has_sky,
            section_count: (height / 16) as i32,
        }
    }

    /// Whether the dimension has sky light.
    pub fn has_sky(&self) -> bool {
        self.has_sky
    }

    pub fn light_section_count(&self) -> usize {
        (self.section_count + 2) as usize
    }

    /// Vanilla `LevelLightEngine.lightOnInColumn`: the column's light has been
    /// applied, in both layers the dimension has. Vanilla's renderer requires
    /// it of a section's whole 3x3 neighbourhood before the section may first
    /// compile (`SectionUpdateTracker.hasAllNeighbors`).
    pub fn light_on_in_column(&self, column: (i32, i32)) -> bool {
        self.light_on.contains(&column)
    }

    fn send(&mut self, message: Message) {
        self.sent += 1;
        self.tx.send(message).expect(THREAD_GONE);
    }

    pub fn queue_task(&mut self, task: LightTask) {
        self.send(Message::Task(task));
    }

    /// A chunk column arrived: its blocks for the engine to read, before any
    /// task about it.
    pub fn chunk_loaded(&mut self, pos: (i32, i32), chunk: Arc<RwLock<Chunk>>) {
        self.send(Message::ChunkLoaded { pos, chunk });
    }

    /// Vanilla `queueLightRemoval`, and the sky sources go with the chunk.
    pub fn chunk_unloaded(&mut self, pos: (i32, i32)) {
        self.send(Message::ChunkUnloaded(pos));
        self.unloaded.insert(pos, self.sent);
    }

    /// Vanilla `LevelChunk.setBlockState`'s light hooks, with the block
    /// already written to the chunk.
    pub fn block_changed(
        &mut self,
        pos: LightPos,
        old: BlockState,
        new: BlockState,
        empty_flip: Option<bool>,
    ) {
        self.send_edit(Message::BlockChanged {
            pos,
            old,
            new,
            empty_flip,
        });
    }

    /// Relight hook for positions mutated without old-state tracking (the
    /// player-prediction paths, which apply and roll back edits internally).
    pub fn block_dirty(&mut self, x: i32, y: i32, z: i32) {
        self.send_edit(Message::BlockDirty(LightPos::new(x, y, z)));
    }

    /// An edit's light is waited for, so it lands in the edit's frame.
    fn send_edit(&mut self, message: Message) {
        self.send(message);
        self.awaited = Some(self.sent);
    }

    /// Takes in what the thread lit since the last call: the columns' light
    /// into the store, for the meshers, and the remesh scope into `out`.
    /// Waits briefly for an edit's light, so it lands in the edit's frame.
    pub fn poll(&mut self, store: &mut ChunkStore, out: &mut LightDirty) {
        let mut apply = |this: &mut Self, report: Report| {
            let current = |column: &(i32, i32)| {
                this.unloaded
                    .get(column)
                    .is_none_or(|&seq| report.handled >= seq)
            };
            for (column, data) in report.columns {
                if current(&column) {
                    store.light_data.insert(column, data);
                }
            }
            for (column, on) in report.dirty.enabled.into_iter().filter(|(c, _)| current(c)) {
                if on {
                    this.light_on.insert(column);
                } else {
                    this.light_on.remove(&column);
                }
            }
            out.sections.extend(
                report
                    .dirty
                    .sections
                    .into_iter()
                    .filter(|key| current(&key.column())),
            );
            out.columns
                .extend(report.dirty.columns.into_iter().filter(|c| current(c)));
            this.unloaded.retain(|_, seq| *seq > report.handled);
            if this.awaited.is_some_and(|seq| report.handled >= seq) {
                this.awaited = None;
            }
        };
        while let Some(report) = crate::util::try_recv_live(&self.rx, THREAD_GONE) {
            apply(self, report);
        }
        let deadline = std::time::Instant::now() + EDIT_WAIT;
        while self.awaited.is_some() {
            match self.rx.recv_deadline(deadline) {
                Ok(report) => apply(self, report),
                // Behind a join's chunks the light is frames away, and waiting
                // again would stall every one of them.
                Err(RecvTimeoutError::Timeout) => self.awaited = None,
                Err(RecvTimeoutError::Disconnected) => panic!("{THREAD_GONE}"),
            }
        }
    }
}

/// Ends when the game drops its [`LightWorker`].
fn run(
    mut engine: LevelLightEngine,
    mut world: LightWorld,
    messages: &Receiver<Message>,
    reports: &Sender<Report>,
) {
    let mut light_data = LightMap::default();
    let mut handled = 0u64;
    for first in messages {
        let mut out = LightDirty::default();
        for message in std::iter::once(first).chain(messages.try_iter()) {
            handled += 1;
            match message {
                Message::Task(task) => engine.queue_task(task),
                Message::ChunkLoaded { pos, chunk } => {
                    world.insert(pos, chunk);
                    engine.on_chunk_loaded(&world, pos, &mut light_data);
                }
                Message::ChunkUnloaded(pos) => {
                    world.remove(pos);
                    light_data.remove(pos);
                    engine.on_chunk_unloaded(pos);
                    engine.queue_task(LightTask::Remove { pos });
                }
                Message::BlockChanged {
                    pos,
                    old,
                    new,
                    empty_flip,
                } => engine.on_block_changed(&world, pos, old, new, empty_flip),
                Message::BlockDirty(pos) => engine.on_block_dirty(&world, pos),
            }
        }
        engine.run(&world, &mut light_data, &mut out);
        let report = Report {
            handled,
            columns: light_data.take_touched(),
            dirty: out,
        };
        if reports.send(report).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::light::SectionKey;

    /// A worker with no thread behind it: the test sends its reports.
    fn worker() -> (LightWorker, Receiver<Message>, Sender<Report>) {
        let (tx, messages) = crossbeam_channel::unbounded();
        let (reports, rx) = crossbeam_channel::unbounded();
        let worker = LightWorker {
            tx,
            rx,
            sent: 0,
            awaited: None,
            light_on: HashSet::new(),
            unloaded: HashMap::new(),
            has_sky: false,
            section_count: 4,
        };
        (worker, messages, reports)
    }

    fn report(handled: u64, column: (i32, i32)) -> Report {
        let data = ChunkLightData {
            sky_sections: Vec::new(),
            block_sections: Vec::new(),
            min_y: 0,
            has_sky: false,
            sky_top_section: None,
        };
        Report {
            handled,
            columns: vec![(column, Arc::new(data))],
            dirty: LightDirty {
                sections: HashSet::from([SectionKey::new(column.0, 0, column.1)]),
                columns: vec![column],
                enabled: vec![(column, true)],
            },
        }
    }

    #[test]
    fn report_from_before_an_unload_skips_the_reloaded_column() {
        let (mut worker, _messages, reports) = worker();
        let mut store = ChunkStore::new(2);
        let column = (3, 4);
        worker.queue_task(LightTask::Remove { pos: column });
        worker.chunk_unloaded(column);
        // The reload's own messages follow; the thread hasn't seen any yet.
        worker.queue_task(LightTask::Remove { pos: column });

        reports.send(report(1, column)).unwrap();
        let mut out = LightDirty::default();
        worker.poll(&mut store, &mut out);
        assert!(!store.light_data.contains_key(&column));
        assert!(out.sections.is_empty());
        assert!(out.columns.is_empty());
        assert!(!worker.light_on_in_column(column));

        reports.send(report(3, column)).unwrap();
        worker.poll(&mut store, &mut out);
        assert!(store.light_data.contains_key(&column));
        assert_eq!(out.sections.len(), 1);
        assert_eq!(out.columns, [column]);
        assert!(worker.light_on_in_column(column));
        assert!(worker.unloaded.is_empty());
    }

    #[test]
    #[should_panic(expected = "light thread panicked")]
    fn a_dead_thread_takes_the_game_down() {
        let (mut worker, _messages, reports) = worker();
        drop(reports);
        worker.poll(&mut ChunkStore::new(2), &mut LightDirty::default());
    }

    #[test]
    fn an_edit_is_awaited_for_one_frame() {
        let (mut worker, _messages, reports) = worker();
        let mut store = ChunkStore::new(2);
        let mut out = LightDirty::default();
        worker.block_dirty(0, 0, 0);
        worker.block_dirty(1, 0, 0);
        assert_eq!(worker.awaited, Some(2));

        reports.send(report(2, (0, 0))).unwrap();
        worker.poll(&mut store, &mut out);
        assert_eq!(worker.awaited, None, "a report covered it");

        worker.block_dirty(2, 0, 0);
        reports.send(report(2, (0, 0))).unwrap();
        worker.poll(&mut store, &mut out);
        assert_eq!(worker.awaited, None, "the wait ran out");
    }
}
