//! What the client keeps of the terrain it has seen: per level of detail,
//! sections of 64x64 columns.

use std::collections::HashMap;

use super::column::{Run, RunKey, merge_columns};
use crate::position::ChunkPos;

/// Columns along each side of a section.
pub(crate) const SECTION_COLUMNS: i32 = 64;
/// Levels of detail: at level `l` a column is `1 << l` blocks wide, so a
/// chunk is `16 >> l` columns across and level 4 is one column per chunk.
pub(crate) const LEVELS: u8 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SectionKey {
    pub level: u8,
    pub x: i32,
    pub z: i32,
}

impl SectionKey {
    /// The section at `level` holding level column `x`/`z`, and that
    /// column's index in it.
    fn holding(level: u8, x: i32, z: i32) -> (Self, usize) {
        let key = Self {
            level,
            x: x.div_euclid(SECTION_COLUMNS),
            z: z.div_euclid(SECTION_COLUMNS),
        };
        let index = z.rem_euclid(SECTION_COLUMNS) * SECTION_COLUMNS + x.rem_euclid(SECTION_COLUMNS);
        (key, index as usize)
    }

    /// Width of one column, in blocks.
    pub fn column_width(self) -> i32 {
        1 << self.level
    }

    /// Width of the section, in blocks.
    pub fn width(self) -> i32 {
        SECTION_COLUMNS << self.level
    }

    /// The block `x`/`z` of the section's corner.
    pub fn origin(self) -> (i32, i32) {
        (self.x * self.width(), self.z * self.width())
    }

    /// The four sections one level finer covering this one.
    pub fn children(self) -> Option<[Self; 4]> {
        let level = self.level.checked_sub(1)?;
        let child = |dx, dz| Self {
            level,
            x: self.x * 2 + dx,
            z: self.z * 2 + dz,
        };
        Some([child(0, 0), child(1, 0), child(0, 1), child(1, 1)])
    }

    fn parent(self) -> Option<Self> {
        (self.level + 1 < LEVELS).then(|| Self {
            level: self.level + 1,
            x: self.x.div_euclid(2),
            z: self.z.div_euclid(2),
        })
    }

    /// Whether, seen from `x`/`z`, this section is too close to draw at its
    /// level and its four children draw instead.
    pub fn splits(self, x: f64, z: f64) -> bool {
        self.level > 0 && self.distance(x, z) < 1.5 * f64::from(self.width())
    }

    /// Whether drawing from `x`/`z` out to `radius` blocks can reach this
    /// section: it's in range and its parent splits (the coarsest level
    /// always draws).
    pub fn wanted(self, x: f64, z: f64, radius: f64) -> bool {
        self.distance(x, z) <= radius && self.parent().is_none_or(|parent| parent.splits(x, z))
    }

    /// The horizontal distance from block position `x`/`z` to the
    /// section's nearest edge; 0 inside it.
    pub fn distance(self, x: f64, z: f64) -> f64 {
        let (ox, oz) = self.origin();
        let width = f64::from(self.width());
        let dx = (f64::from(ox) - x).max(x - f64::from(ox) - width).max(0.0);
        let dz = (f64::from(oz) - z).max(z - f64::from(oz) - width).max(0.0);
        dx.hypot(dz)
    }
}

/// A run packed in a `u64`: palette index (24 bits), top, bottom (12 each),
/// sky and block light (4 each).
fn pack(run: &Run, palette_index: u32) -> u64 {
    u64::from(palette_index)
        | u64::from(run.top) << 24
        | u64::from(run.bottom) << 36
        | u64::from(run.sky) << 48
        | u64::from(run.block) << 52
}

fn unpack(packed: u64, palette: &[RunKey]) -> Run {
    Run {
        key: palette[(packed & 0xFF_FFFF) as usize],
        top: (packed >> 24 & 0xFFF) as u16,
        bottom: (packed >> 36 & 0xFFF) as u16,
        sky: (packed >> 48 & 0xF) as u8,
        block: (packed >> 52 & 0xF) as u8,
    }
}

/// 64x64 columns at one level, their runs packed against a palette of what
/// they're made of.
pub(crate) struct Section {
    palette: Vec<RunKey>,
    lookup: HashMap<RunKey, u32>,
    /// By `z * 64 + x`.
    columns: Vec<Box<[u64]>>,
}

impl Section {
    fn new() -> Self {
        Self {
            palette: Vec::new(),
            lookup: HashMap::new(),
            columns: vec![Box::default(); (SECTION_COLUMNS * SECTION_COLUMNS) as usize],
        }
    }

    fn set_column(&mut self, index: usize, runs: &[Run]) {
        self.columns[index] = runs
            .iter()
            .map(|run| {
                let palette_index = *self.lookup.entry(run.key).or_insert_with(|| {
                    self.palette.push(run.key);
                    (self.palette.len() - 1) as u32
                });
                pack(run, palette_index)
            })
            .collect();
    }

    /// Column `x`/`z`'s runs, top to bottom; none outside the section.
    pub fn column(&self, x: i32, z: i32) -> impl Iterator<Item = Run> + '_ {
        let inside = (0..SECTION_COLUMNS).contains(&x) && (0..SECTION_COLUMNS).contains(&z);
        let runs: &[u64] = if inside {
            &self.columns[(z * SECTION_COLUMNS + x) as usize]
        } else {
            &[]
        };
        runs.iter().map(|&packed| unpack(packed, &self.palette))
    }
}

/// One dimension's sections at every level.
#[derive(Default)]
pub(crate) struct Store {
    sections: HashMap<SectionKey, Section>,
}

impl Store {
    /// Writes a chunk's columns, `level0` by `z * 16 + x`, into every level,
    /// merging them down for the coarser ones. Returns the sections it
    /// touched.
    pub fn capture(&mut self, pos: ChunkPos, level0: Vec<Vec<Run>>) -> Vec<SectionKey> {
        let mut touched = Vec::with_capacity(LEVELS as usize);
        let mut columns = level0;
        for level in 0..LEVELS {
            let across = 16 >> level;
            if level > 0 {
                let finer = across * 2;
                columns = (0..across * across)
                    .map(|i| {
                        let (x, z) = (i % across * 2, i / across * 2);
                        let at =
                            |dx: usize, dz: usize| columns[(z + dz) * finer + x + dx].as_slice();
                        merge_columns(&[at(0, 0), at(1, 0), at(0, 1), at(1, 1)])
                    })
                    .collect();
            }
            for (i, runs) in columns.iter().enumerate() {
                let x = pos.x * across as i32 + (i % across) as i32;
                let z = pos.z * across as i32 + (i / across) as i32;
                let (key, index) = SectionKey::holding(level, x, z);
                self.sections
                    .entry(key)
                    .or_insert_with(Section::new)
                    .set_column(index, runs);
                if !touched.contains(&key) {
                    touched.push(key);
                }
            }
        }
        touched
    }

    pub fn get(&self, key: SectionKey) -> Option<&Section> {
        self.sections.get(&key)
    }

    pub fn keys(&self) -> impl Iterator<Item = SectionKey> + '_ {
        self.sections.keys().copied()
    }

    /// Drops the sections farther than `radius` blocks from `x`/`z`,
    /// returning their keys.
    pub fn evict_beyond(&mut self, x: f64, z: f64, radius: f64) -> Vec<SectionKey> {
        let far: Vec<SectionKey> = self
            .sections
            .keys()
            .copied()
            .filter(|key| key.distance(x, z) > radius)
            .collect();
        for key in &far {
            self.sections.remove(key);
        }
        far
    }
}

#[cfg(test)]
mod tests {
    use azalea_block::BlockState;

    use super::*;

    fn run(state: u32, top: u16, bottom: u16) -> Run {
        Run {
            key: RunKey {
                state: BlockState::try_from(state).unwrap(),
                biome: 7,
            },
            top,
            bottom,
            sky: 15,
            block: 3,
        }
    }

    #[test]
    fn runs_survive_packing() {
        let mut section = Section::new();
        let runs = [run(5, 4095, 100), run(9, 99, 0)];
        section.set_column(65, &runs);
        assert_eq!(section.column(1, 1).collect::<Vec<_>>(), runs);
        assert_eq!(section.column(64, 0).count(), 0);
    }

    #[test]
    fn a_capture_writes_every_level_down_to_a_column_per_chunk() {
        let mut store = Store::default();
        let pos = ChunkPos::new(-1, 70);
        let level0 = (0..256).map(|i| vec![run(1, 10 + i % 2, 0)]).collect();
        let touched = store.capture(pos, level0);
        assert_eq!(touched.len(), LEVELS as usize);

        // Level 4: chunk x -1 is column -1, in section -1 at column 63.
        let level4 = SectionKey {
            level: 4,
            x: -1,
            z: 1,
        };
        assert!(touched.contains(&level4));
        let section = store.get(level4).unwrap();
        // Half the columns reach 11, half 10: the merged column takes the
        // taller run's top, as two of four vote stone there.
        assert_eq!(section.column(63, 6).collect::<Vec<_>>(), [run(1, 11, 0)]);
    }

    #[test]
    fn nearer_sections_split_into_finer_levels() {
        let key = |level, x| SectionKey { level, x, z: 0 };
        // A level-4 section (1024 wide) splits within 1536 blocks.
        assert!(key(4, 0).splits(500.0, 8.0));
        assert!(!key(4, 3).splits(500.0, 8.0));
        // Level 0 never splits.
        assert!(!key(0, 0).splits(8.0, 8.0));
        // A level-3 child of a far level-4 section isn't wanted; one under
        // the player is.
        assert!(!key(3, 6).wanted(8.0, 8.0, 4096.0));
        assert!(key(3, 0).wanted(8.0, 8.0, 4096.0));
        assert!(!key(4, 9).wanted(8.0, 8.0, 4096.0), "past the radius");
    }

    #[test]
    fn eviction_drops_sections_beyond_the_radius() {
        let mut store = Store::default();
        store.capture(ChunkPos::new(0, 0), vec![Vec::new(); 256]);
        store.capture(ChunkPos::new(100, 0), vec![Vec::new(); 256]);
        // Chunk 100 is 1600 blocks out; even its level-4 section starts at
        // 1024.
        let gone = store.evict_beyond(8.0, 8.0, 500.0);
        assert_eq!(gone.len(), LEVELS as usize);
        assert!(gone.iter().all(|key| key.x > 0));
        assert_eq!(store.keys().count(), LEVELS as usize);
    }
}
