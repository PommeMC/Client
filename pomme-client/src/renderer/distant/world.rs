//! One dimension's distant world: its sections at every level, the most
//! recently used in memory and every one seen on disk, a file per section.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use pomme_protocol::section::Section;

use super::section::{self, LEVELS, SectionKey, VoxelizedSection, WorldSection};
use super::voxel::States;
use crate::position::ChunkPos;
use crate::world::chunk::ChunkLightData;
use crate::world::light::DataLayer;

/// Least-recently-used sections held in memory, the reference's cache size
/// on a small heap. Changes are written by [`World::flush`] or on eviction.
const CACHE_SIZE: usize = 1024;
/// Keys known to have no file, so meshing doesn't look for them again;
/// forgotten all at once past this many.
const ABSENT_SIZE: usize = 1 << 16;

/// A section an update changed, and which of its sides the change touched
/// (a bit each: down, up, -x, +x, -z, +z), so the sections across them can
/// be meshed again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Changed {
    pub key: SectionKey,
    pub sides: u8,
}

pub(crate) struct World {
    /// Each with when it was last used.
    sections: HashMap<SectionKey, (WorldSection, u64)>,
    /// `sections` by last use, oldest first.
    used: BTreeMap<u64, SectionKey>,
    clock: u64,
    capacity: usize,
    dir: Option<PathBuf>,
    /// Sections changed since they were last written.
    unsaved: HashSet<SectionKey>,
    /// Sections `dir` has no file for.
    absent: HashSet<SectionKey>,
    /// Never given a folder, so sections leave memory unwritten rather
    /// than wait for one.
    in_memory: bool,
}

impl Default for World {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl World {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            sections: HashMap::new(),
            used: BTreeMap::new(),
            clock: 0,
            capacity: CACHE_SIZE,
            dir,
            unsaved: HashSet::new(),
            absent: HashSet::new(),
            in_memory: false,
        }
    }

    pub fn in_memory() -> Self {
        Self {
            in_memory: true,
            ..Self::new(None)
        }
    }

    /// Gives a world kept without a folder its folder: what it holds is
    /// written there at the next flush.
    pub fn set_dir(&mut self, dir: Option<PathBuf>) {
        self.dir = dir;
        self.absent.clear();
        self.evict();
    }

    fn path(&self, key: SectionKey) -> Option<PathBuf> {
        let SectionKey { level, x, y, z } = key;
        Some(self.dir.as_ref()?.join(format!("{level}.{x}.{y}.{z}.bin")))
    }

    /// Reads `key` from disk unless it's in memory already. Returns whether
    /// it's in memory now.
    pub fn load(&mut self, key: SectionKey) -> bool {
        if let Some((_, used)) = self.sections.get_mut(&key) {
            self.used.remove(&*used);
            self.clock += 1;
            *used = self.clock;
            self.used.insert(self.clock, key);
            return true;
        }
        if self.absent.contains(&key) {
            return false;
        }
        let Some(path) = self.path(key) else {
            return false;
        };
        let read = std::fs::read(&path).ok();
        match read.as_deref().and_then(|file| section::decode(key, file)) {
            Some(section) => {
                self.insert(key, section);
                true
            }
            None => {
                if read.is_some() {
                    tracing::warn!("Ignoring unreadable distant terrain {}", path.display());
                }
                if self.absent.len() >= ABSENT_SIZE {
                    self.absent.clear();
                }
                self.absent.insert(key);
                false
            }
        }
    }

    /// `key`'s section if it's in memory.
    pub fn get(&self, key: SectionKey) -> Option<&WorldSection> {
        self.sections.get(&key).map(|(section, _)| section)
    }

    /// Keeps `section` as the most recently used.
    fn insert(&mut self, key: SectionKey, section: WorldSection) {
        self.clock += 1;
        self.sections.insert(key, (section, self.clock));
        self.used.insert(self.clock, key);
        self.evict();
    }

    /// Drops the least recently used sections past the cache's size,
    /// writing those unsaved first. Without a folder yet everything stays
    /// until there is one.
    fn evict(&mut self) {
        if self.dir.is_none() && !self.in_memory {
            return;
        }
        while self.sections.len() > self.capacity {
            let Some((_, key)) = self.used.pop_first() else {
                break;
            };
            if self.unsaved.remove(&key) {
                self.save(key);
            }
            self.sections.remove(&key);
        }
    }

    /// `key`'s section, loaded, or unseen air where there's none.
    fn acquire(&mut self, key: SectionKey) -> &mut WorldSection {
        if !self.load(key) {
            self.insert(key, WorldSection::unseen());
        }
        &mut self
            .sections
            .get_mut(&key)
            .expect("the newest section stays")
            .0
    }

    /// Writes a vanilla section's voxels into every level, stopping at the
    /// first level nothing changed in, and bubbles a section going empty or
    /// non-empty up to its parent.
    pub fn insert_update(&mut self, update: &VoxelizedSection) -> Vec<Changed> {
        let mut changed = Vec::new();
        let mut child: Option<(u8, bool)> = None;
        for level in 0..LEVELS {
            let key = SectionKey::of_vanilla(level, update.x, update.y, update.z);
            let into = self.acquire(key);
            let (written, before) = section::insert_level(update, into, level);
            let (touched, emptiness) =
                section::settle_level(into, level, written, before, update.non_air_count, child);
            let non_empty = into.non_empty_children != 0;
            if touched {
                self.absent.remove(&key);
                self.unsaved.insert(key);
                changed.push(Changed {
                    key,
                    sides: if written {
                        neighbour_sides(update, level)
                    } else {
                        0
                    },
                });
            }
            if !written && emptiness != 2 {
                break;
            }
            child = (emptiness == 2).then_some((key.child_index(), non_empty));
        }
        changed
    }

    /// Takes in a column of `sections` once any of them has light: every
    /// one converted, mipped and written in. A column without light is
    /// taken only when it's all air, as unlit air, so a column that went
    /// before its light arrived never darkens what was kept lit. Returns
    /// what changed.
    pub fn ingest_chunk(
        &mut self,
        states: &States,
        pos: ChunkPos,
        sections: &[Section],
        light: Option<&ChunkLightData>,
        min_y: i32,
    ) -> Vec<Changed> {
        let lit = light.is_some_and(|light| has_light(light, sections.len()));
        if !lit && sections.iter().any(|section| section.block_count != 0) {
            return Vec::new();
        }
        let mut changed = Vec::new();
        for (i, section) in sections.iter().enumerate() {
            let section_y = i as i32 + min_y.div_euclid(16);
            let mut update = VoxelizedSection::new(pos.x, section_y, pos.z);
            if lit {
                update.convert(states, section, section::light_of(light, section_y));
                update.mip(states);
            } else {
                update.zero();
            }
            changed.extend(self.insert_update(&update));
        }
        changed
    }

    /// Writes `key`'s section, or removes its file once nothing is left in
    /// it: a missing section reads as unseen.
    fn save(&self, key: SectionKey) {
        let (Some(path), Some(section)) = (self.path(key), self.get(key)) else {
            return;
        };
        if section.is_unseen() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        let written = std::fs::create_dir_all(path.parent().expect("a section file has a folder"))
            .and_then(|()| crate::util::write_atomic(&path, &section::encode(key, section)));
        if let Err(error) = written {
            tracing::warn!("Failed to save distant terrain {}: {error}", path.display());
        }
    }

    /// Writes every section changed since it was last written; without a
    /// folder they wait for one.
    pub fn flush(&mut self) {
        if self.dir.is_none() {
            return;
        }
        for key in std::mem::take(&mut self.unsaved) {
            self.save(key);
        }
    }

    /// Whether dropping the world would lose nothing.
    pub fn is_saved(&self) -> bool {
        self.in_memory || self.unsaved.is_empty()
    }
}

/// A column's place in a Z-order curve over x and z. Columns taken in in
/// this order fill each section before moving on, so a backlog larger than
/// the cache reads each section from disk about once.
pub(crate) fn ingest_order(pos: ChunkPos) -> u64 {
    let spread = |c: i32| {
        let mut v = u64::from(c as u32 ^ 0x8000_0000);
        v = (v | v << 16) & 0x0000_FFFF_0000_FFFF;
        v = (v | v << 8) & 0x00FF_00FF_00FF_00FF;
        v = (v | v << 4) & 0x0F0F_0F0F_0F0F_0F0F;
        v = (v | v << 2) & 0x3333_3333_3333_3333;
        (v | v << 1) & 0x5555_5555_5555_5555
    };
    spread(pos.x) | spread(pos.z) << 1
}

/// Whether any of a column's `sections` has light data, sky or block: the
/// reference's `LIGHT_AND_DATA`. Layer 0 is the section below the world.
fn has_light(light: &ChunkLightData, sections: usize) -> bool {
    let any =
        |layers: &[Option<DataLayer>]| layers.iter().skip(1).take(sections).any(Option::is_some);
    any(&light.sky_sections) || any(&light.block_sections)
}

/// Which sides of the level-`level` section a vanilla section's update
/// reaches: those it borders on.
fn neighbour_sides(update: &VoxelizedSection, level: u8) -> u8 {
    let shift = level + 1;
    let crosses = |c: i32, step: i32| u8::from((c ^ (c + step)) >> shift != 0);
    crosses(update.y, -1)
        | crosses(update.y, 1) << 1
        | crosses(update.x, -1) << 2
        | crosses(update.x, 1) << 3
        | crosses(update.z, -1) << 4
        | crosses(update.z, 1) << 5
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::distant::voxel;
    use crate::world::block::BlockStateTable;

    /// A vanilla section at `x`/`y`/`z` of `block` everywhere, mipped.
    fn stone(x: i32, y: i32, z: i32, block: u32) -> VoxelizedSection {
        let mut section = VoxelizedSection::new(x, y, z);
        section.fill_for_test(
            voxel::compose(0, block, 1),
            if block == voxel::AIR { 0 } else { 4096 },
        );
        section
    }

    /// A fresh folder under the temp dir for test `name`.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pomme-distant-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn an_update_reaches_every_level_and_stops_once_nothing_changes() {
        let mut world = World::default();
        let changed = world.insert_update(&stone(1, 0, 0, 7));
        assert_eq!(changed.len(), LEVELS as usize);
        // Level 0 holds vanilla sections 0..1 per axis, so from (1, 0, 0)
        // the update reaches the -y, +x and -z sides.
        assert_eq!(changed[0].sides, 1 | 1 << 3 | 1 << 4);
        let level4 = SectionKey {
            level: 4,
            x: 0,
            y: 0,
            z: 0,
        };
        let section = world.get(level4).unwrap();
        assert!(!voxel::is_air(section.data[section::index(1, 0, 0)]));
        assert_eq!(section.non_empty_children, 1);
        let level0 = SectionKey { level: 0, ..level4 };
        assert_eq!(world.get(level0).unwrap().non_empty_count, 4096);
        // The same again changes nothing past level 0.
        assert!(world.insert_update(&stone(1, 0, 0, 7)).is_empty());
        // Clearing it empties every level.
        let cleared = world.insert_update(&stone(1, 0, 0, voxel::AIR));
        assert_eq!(cleared.len(), LEVELS as usize);
        assert_eq!(world.get(level4).unwrap().non_empty_children, 0);
    }

    #[test]
    fn neighbour_sides_hold_for_negative_sections() {
        // Level 0 holds vanilla sections -2..-1 per axis: from -1 an update
        // reaches the +x side, from -2 the -x side.
        let sides = |x: i32, level: u8| neighbour_sides(&VoxelizedSection::new(x, 0, 0), level);
        let x_sides = |sides: u8| sides >> 2 & 0b11;
        assert_eq!(x_sides(sides(-1, 0)), 0b10);
        assert_eq!(x_sides(sides(-2, 0)), 0b01);
        // Level 1 holds -4..-1: -3 and -2 reach neither side.
        assert_eq!(x_sides(sides(-3, 1)), 0);
        assert_eq!(x_sides(sides(-2, 1)), 0);
        assert_eq!(x_sides(sides(-4, 1)), 0b01);
        // Section y 0 still borders -y (on y -1) and not +y.
        assert_eq!(sides(-1, 0) & 0b11, 0b01);
    }

    #[test]
    fn evicted_sections_come_back_from_disk() {
        let dir = temp_dir("evict");
        let mut world = World::new(Some(dir.clone()));
        world.capacity = 1;
        world.insert_update(&stone(100, 0, 0, 7));
        assert_eq!(world.sections.len(), 1, "the rest written and dropped");
        world.flush();

        let mut world = World::new(Some(dir.clone()));
        world.insert_update(&stone(101, 0, 0, 8));
        let level4 = world
            .get(SectionKey {
                level: 4,
                x: 3,
                y: 0,
                z: 0,
            })
            .unwrap();
        assert_eq!(voxel::block_id(level4.data[section::index(4, 0, 0)]), 7);
        assert_eq!(voxel::block_id(level4.data[section::index(5, 0, 0)]), 8);
        assert!(!world.load(SectionKey {
            level: 0,
            x: -9,
            y: 0,
            z: 0
        }));
        // A section cleared back to unseen loses its file.
        let level0 = SectionKey::of_vanilla(0, 103, 0, 0);
        world.insert_update(&stone(103, 0, 0, 9));
        world.flush();
        assert!(world.path(level0).unwrap().exists());
        let mut cleared = stone(103, 0, 0, voxel::AIR);
        cleared.fill_for_test(voxel::compose(0x0F, voxel::AIR, 0), 0);
        world.insert_update(&cleared);
        world.flush();
        assert!(!world.path(level0).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn memory_holds_the_most_recently_used_and_absent_keys_stay_bounded() {
        let dir = temp_dir("lru");
        let mut world = World::new(Some(dir.clone()));
        world.capacity = 8;
        for x in 0..10 {
            world.insert_update(&stone(x * 64, 0, 0, 7));
            assert!(world.sections.len() <= 8);
            assert_eq!(world.used.len(), world.sections.len());
        }
        // The last update's five levels are the newest.
        for level in 0..LEVELS {
            assert!(
                world
                    .get(SectionKey::of_vanilla(level, 9 * 64, 0, 0))
                    .is_some()
            );
        }
        // The first, dropped, was written on the way out.
        let first = SectionKey::of_vanilla(0, 0, 0, 0);
        assert!(world.get(first).is_none());
        assert!(world.path(first).unwrap().exists());
        assert!(world.load(first));

        let missing = |x| SectionKey {
            level: 0,
            x,
            y: 50,
            z: 0,
        };
        world.absent.extend((0..ABSENT_SIZE as i32).map(missing));
        assert!(!world.load(missing(-1)));
        assert_eq!(world.absent.len(), 1, "forgotten, then the new one kept");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_world_without_a_folder_coming_stays_bounded() {
        let mut world = World {
            capacity: 8,
            ..World::in_memory()
        };
        for x in 0..10 {
            world.insert_update(&stone(x * 64, 0, 0, 7));
        }
        assert_eq!(world.sections.len(), 8);
        assert!(world.is_saved(), "nothing to wait for");
    }

    #[test]
    fn z_order_keeps_neighbouring_columns_together() {
        let order = |x, z| ingest_order(ChunkPos::new(x, z));
        assert!(order(-1, -1) < order(0, -1));
        assert!(order(0, -1) < order(-1, 0));
        // The four columns of a 2x2 block come one after another.
        let mut block = [order(0, 0), order(1, 0), order(0, 1), order(1, 1)];
        block.sort_unstable();
        assert_eq!(block[3] - block[0], 3);
    }

    #[test]
    fn sections_kept_before_a_folder_are_written_once_there_is_one() {
        let mut world = World {
            capacity: 1,
            ..World::new(None)
        };
        world.insert_update(&stone(5, 0, 0, 7));
        world.flush();
        assert_eq!(world.sections.len(), LEVELS as usize, "nothing dropped");
        assert!(!world.is_saved());

        let dir = temp_dir("late-dir");
        world.set_dir(Some(dir.clone()));
        assert_eq!(world.sections.len(), 1, "the rest written as they left");
        world.flush();
        assert!(world.is_saved());
        let mut reread = World::new(Some(dir.clone()));
        for level in 0..LEVELS {
            assert!(reread.load(SectionKey::of_vanilla(level, 5, 0, 0)));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A column of four sections, the lowest stone, lit `light` if any.
    fn column(light: Option<u8>) -> (Vec<Section>, Option<ChunkLightData>) {
        let table = BlockStateTable::native();
        let id = u32::from(table.find_state("stone", &[]).id());
        let mut sections = vec![Section::default(); 4];
        for i in 0..4096u32 {
            sections[0]
                .states
                .get_and_set(i & 15, i >> 8, i >> 4 & 15, id, table.state_count());
        }
        sections[0].block_count = 4096;
        let light = light.map(|level| ChunkLightData {
            sky_sections: vec![Some(DataLayer::filled(level)); 6],
            block_sections: vec![None; 6],
            min_y: 0,
            has_sky: true,
            sky_top_section: Some(6),
        });
        (sections, light)
    }

    #[test]
    fn a_column_without_light_keeps_what_was_kept_lit() {
        let states = States::new(BlockStateTable::native());
        let mut world = World::default();
        let pos = ChunkPos::new(0, 0);
        let (sections, light) = column(Some(9));
        assert!(
            !world
                .ingest_chunk(&states, pos, &sections, light.as_ref(), 0)
                .is_empty()
        );
        let key = SectionKey::of_vanilla(0, 0, 1, 0);
        let lit = world.get(key).unwrap().data.clone();
        assert_eq!(voxel::light(lit[section::index(0, 16, 0)]), 9);

        // The same column gone before its light: nothing taken.
        let (sections, _) = column(None);
        assert!(
            world
                .ingest_chunk(&states, pos, &sections, None, 0)
                .is_empty()
        );
        let no_layers = ChunkLightData {
            sky_sections: vec![None; 6],
            sky_top_section: None,
            ..light.unwrap()
        };
        assert!(
            world
                .ingest_chunk(&states, pos, &sections, Some(&no_layers), 0)
                .is_empty()
        );
        assert_eq!(world.get(key).unwrap().data, lit);

        // An all-air column without light is taken, as unlit air.
        let air = vec![Section::default(); 4];
        assert!(!world.ingest_chunk(&states, pos, &air, None, 0).is_empty());
        let section = world.get(key).unwrap();
        assert!(voxel::is_air(section.data[section::index(0, 0, 0)]));
        assert_eq!(voxel::light(section.data[section::index(0, 16, 0)]), 0);
    }
}
