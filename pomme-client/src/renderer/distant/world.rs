//! One dimension's Voxy world (`WorldEngine`, `WorldUpdater`): its sections
//! at every level, those near the player in memory and every one seen on
//! disk, a file per section.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::section::{self, LEVELS, SectionKey, VoxelizedSection, WorldSection};
use super::voxel::States;
use crate::position::ChunkPos;
use crate::world::chunk::{Chunk, ChunkLightData};

/// A section an update changed, and which of its sides the change touched
/// (`WorldUpdater`'s neighbour mask: down, up, -x, +x, -z, +z), so the
/// sections across them can be meshed again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Changed {
    pub key: SectionKey,
    pub sides: u8,
}

#[derive(Default)]
pub(crate) struct World {
    sections: HashMap<SectionKey, WorldSection>,
    dir: Option<PathBuf>,
    /// Sections changed since they were last written.
    unsaved: HashSet<SectionKey>,
    /// Sections `dir` has no file for.
    absent: HashSet<SectionKey>,
}

impl World {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            ..Self::default()
        }
    }

    /// Writes what's unsaved, then keeps further sections in `dir`.
    pub fn set_dir(&mut self, dir: Option<PathBuf>) {
        if self.dir != dir {
            self.flush();
            self.dir = dir;
            self.absent.clear();
        }
    }

    fn path(&self, key: SectionKey) -> Option<PathBuf> {
        let SectionKey { level, x, y, z } = key;
        Some(self.dir.as_ref()?.join(format!("{level}.{x}.{y}.{z}.bin")))
    }

    /// Reads `key` from disk unless it's in memory already. Returns whether
    /// it's in memory now.
    pub fn load(&mut self, key: SectionKey) -> bool {
        if self.sections.contains_key(&key) {
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
                self.sections.insert(key, section);
                true
            }
            None => {
                if read.is_some() {
                    tracing::warn!("Ignoring unreadable distant terrain {}", path.display());
                }
                self.absent.insert(key);
                false
            }
        }
    }

    /// `key`'s section, loaded, or unseen air where there's none
    /// (`ActiveSectionTracker`).
    fn acquire(&mut self, key: SectionKey) -> &mut WorldSection {
        self.load(key);
        self.sections
            .entry(key)
            .or_insert_with(WorldSection::unseen)
    }

    /// `WorldUpdater.insertUpdate`: writes a vanilla section's voxels into
    /// every level, stopping at the first level nothing changed in, and
    /// bubbles a section going empty or non-empty up to its parent.
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

    /// `VoxelIngestService.processJob` for a column: every vanilla section
    /// converted, mipped and written in. Returns what changed.
    pub fn ingest_chunk(
        &mut self,
        states: &States,
        pos: ChunkPos,
        chunk: &Chunk,
        light: Option<&ChunkLightData>,
        min_y: i32,
    ) -> Vec<Changed> {
        let mut changed = Vec::new();
        for (i, section) in chunk.sections.iter().enumerate() {
            let section_y = i as i32 + min_y.div_euclid(16);
            let mut update = VoxelizedSection::new(pos.x, section_y, pos.z);
            if section.block_count == 0 && light.is_none() {
                update.zero();
            } else {
                update.convert(states, section, section::light_of(light, section_y));
                update.mip(states);
            }
            changed.extend(self.insert_update(&update));
        }
        changed
    }

    /// Writes `key`'s section, or removes its file once nothing is left in
    /// it: a missing section reads as unseen.
    fn save(&self, key: SectionKey) {
        let (Some(path), Some(section)) = (self.path(key), self.sections.get(&key)) else {
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

    /// Writes every section changed since it was last written.
    pub fn flush(&mut self) {
        for key in std::mem::take(&mut self.unsaved) {
            self.save(key);
        }
    }

    /// Drops the sections farther than `radius` blocks from `x`/`z`, saving
    /// those unsaved, and returns their keys.
    pub fn evict_beyond(&mut self, x: f64, z: f64, radius: f64) -> Vec<SectionKey> {
        let far: Vec<SectionKey> = self
            .sections
            .keys()
            .copied()
            .filter(|key| key.distance(x, z) > radius)
            .collect();
        for key in &far {
            if self.unsaved.remove(key) {
                self.save(*key);
            }
            self.sections.remove(key);
        }
        far
    }
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

    /// A vanilla section at `x`/`y`/`z` of `block` everywhere, mipped.
    fn stone(x: i32, y: i32, z: i32, block: u32) -> VoxelizedSection {
        let mut section = VoxelizedSection::new(x, y, z);
        section.fill_for_test(
            voxel::compose(0, block, 1),
            if block == voxel::AIR { 0 } else { 4096 },
        );
        section
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
        let section = &world.sections[&level4];
        assert!(!voxel::is_air(section.data[section::index(1, 0, 0)]));
        assert_eq!(section.non_empty_children, 1);
        let level0 = SectionKey { level: 0, ..level4 };
        assert_eq!(world.sections[&level0].non_empty_count, 4096);
        // The same again changes nothing past level 0.
        assert!(world.insert_update(&stone(1, 0, 0, 7)).is_empty());
        // Clearing it empties every level.
        let cleared = world.insert_update(&stone(1, 0, 0, voxel::AIR));
        assert_eq!(cleared.len(), LEVELS as usize);
        assert_eq!(world.sections[&level4].non_empty_children, 0);
    }

    #[test]
    fn evicted_sections_come_back_from_disk() {
        let dir = std::env::temp_dir().join(format!("pomme-voxy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut world = World::new(Some(dir.clone()));
        world.insert_update(&stone(100, 0, 0, 7));
        world.evict_beyond(8.0, 8.0, 500.0);
        assert!(world.sections.is_empty());

        let mut world = World::new(Some(dir.clone()));
        world.insert_update(&stone(101, 0, 0, 8));
        let level4 = &world.sections[&SectionKey {
            level: 4,
            x: 3,
            y: 0,
            z: 0,
        }];
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
}
