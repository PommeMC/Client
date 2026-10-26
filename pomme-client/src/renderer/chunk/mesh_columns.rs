//! Which loaded columns need (re)meshing, and the mesh jobs that asks the
//! workers for.

use super::mesher::MeshRequest;
use crate::position::ChunkPos;
use crate::world::chunk::ChunkRing;

/// One column's mesh scheduling state. A slot whose `pos` isn't the column
/// asked about, or whose `generation` is 0, holds nothing for it.
#[derive(Clone, Copy, Default)]
struct ColumnMeta {
    pos: ChunkPos,
    /// The generation this load of `pos` started at; a mesh from any other
    /// load is stale.
    incarnation: u64,
    /// Bumped by every change that needs a remesh.
    generation: u64,
    dirty_sections: u32,
    /// The subset of `dirty_sections` only edits made dirty, which mesh as
    /// recompiles.
    edited_sections: u32,
    /// The LOD the column was last scheduled at.
    lod: Option<u32>,
}

impl ColumnMeta {
    fn holds(&self, pos: ChunkPos) -> bool {
        self.generation != 0 && self.pos == pos
    }
}

/// Every loaded column's mesh scheduling state. A column needs (re)meshing
/// while it has dirty sections its newest generation hasn't been taken for,
/// regardless of visibility, so an edit to a hidden column is never lost.
pub struct MeshColumns {
    meta: ChunkRing<ColumnMeta>,
    /// Source of the globally unique, nonzero generations in `meta`.
    next_generation: u64,
    /// Something changed what the workers should be doing since the last
    /// [`Self::requests`].
    changed: bool,
    /// Camera 8-block bucket the last requests were ordered from.
    camera: (i32, i32),
}

impl Default for MeshColumns {
    fn default() -> Self {
        Self {
            meta: ChunkRing::new(ColumnMeta::default),
            next_generation: 0,
            changed: true,
            camera: (i32::MIN, i32::MIN),
        }
    }
}

impl MeshColumns {
    fn next_generation(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    fn get(&self, pos: ChunkPos) -> Option<&ColumnMeta> {
        Some(self.meta.get(pos)).filter(|meta| meta.holds(pos))
    }

    /// Which load of `pos` the columns are tracking, if any.
    pub fn incarnation(&self, pos: ChunkPos) -> Option<u64> {
        self.get(pos).map(|meta| meta.incarnation)
    }

    /// Forgets every column, for a new level. Generations keep rising, so
    /// none is ever mistaken for one a worker already took.
    pub fn reset(&mut self) {
        *self = Self {
            next_generation: self.next_generation,
            ..Self::default()
        };
    }

    /// Marks `sections` of `pos` for remeshing under a fresh generation, as
    /// edits (recompiles) or as new content.
    pub fn mark(&mut self, pos: ChunkPos, sections: u32, edit: bool) {
        let generation = self.next_generation();
        let meta = self.meta.get_mut(pos);
        if !meta.holds(pos) {
            *meta = ColumnMeta {
                pos,
                incarnation: generation,
                ..ColumnMeta::default()
            };
        }
        meta.generation = generation;
        meta.dirty_sections |= sections;
        if edit {
            meta.edited_sections |= sections;
        } else {
            meta.edited_sections &= !sections;
        }
        self.changed = true;
    }

    /// Marks `sections` of `pos` again if the columns still track it, as
    /// new content.
    pub fn remark(&mut self, pos: ChunkPos, sections: u32) {
        if self.get(pos).is_some() {
            self.mark(pos, sections, false);
        }
    }

    /// Forgets `pos` when it unloads, returning a generation newer than
    /// anything queued for it, to retire that work with.
    pub fn forget(&mut self, pos: ChunkPos) -> Option<u64> {
        self.get(pos)?;
        let retired = self.next_generation();
        *self.meta.get_mut(pos) = ColumnMeta::default();
        Some(retired)
    }

    /// Adopts a finished mesh of `sections`: they stop being dirty unless the
    /// column has been marked again since `generation` was asked for.
    pub fn meshed(&mut self, pos: ChunkPos, generation: u64, sections: u32) {
        let meta = self.meta.get_mut(pos);
        if meta.holds(pos) && meta.generation == generation {
            meta.dirty_sections &= !sections;
            meta.edited_sections &= meta.dirty_sections;
        }
    }

    /// The jobs the workers should have now, or `None` when nothing changed
    /// since the last call and the camera stayed in its 8-block bucket.
    /// Columns whose LOD (`lod`) moved since they were scheduled remesh
    /// whole (`full`); columns whose newest generation a worker already took
    /// (`claimed`) are left to it.
    pub fn requests(
        &mut self,
        loaded: &[ChunkPos],
        camera: glam::DVec3,
        full: u32,
        lod: impl Fn(ChunkPos) -> u32,
        claimed: impl Fn(ChunkPos) -> u64,
    ) -> Option<Vec<MeshRequest>> {
        for &pos in loaded {
            if self
                .get(pos)
                .is_some_and(|meta| meta.lod.is_some_and(|scheduled| scheduled != lod(pos)))
            {
                self.mark(pos, full, false);
            }
        }

        let bucket = (
            (camera.x / 8.0).floor() as i32,
            (camera.z / 8.0).floor() as i32,
        );
        if !self.changed && bucket == self.camera {
            return None;
        }
        self.changed = false;
        self.camera = bucket;

        let mut requests = Vec::new();
        for &pos in loaded {
            let taken = claimed(pos);
            let meta = self.meta.get_mut(pos);
            if !meta.holds(pos) || meta.dirty_sections == 0 || taken >= meta.generation {
                continue;
            }
            let column_lod = lod(pos);
            meta.lod = Some(column_lod);
            // One job per column: a sparse dirty set meshes the clean sections
            // between its first and last dirty one too.
            let dirty = meta.dirty_sections;
            requests.push(MeshRequest {
                pos,
                lod: column_lod,
                incarnation: meta.incarnation,
                content_gen: meta.generation,
                sections: dirty.trailing_zeros() as i32..32 - dirty.leading_zeros() as i32,
                recompile: meta.edited_sections == dirty,
            });
        }
        Some(requests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: u32 = 0xFFFFFF;
    const CAMERA: glam::DVec3 = glam::DVec3::new(8.0, 64.0, 8.0);

    fn requests(columns: &mut MeshColumns, loaded: &[ChunkPos], lod: u32) -> Vec<MeshRequest> {
        columns
            .requests(loaded, CAMERA, FULL, |_| lod, |_| 0)
            .unwrap_or_default()
    }

    #[test]
    fn a_marked_column_asks_once_until_marked_again() {
        let pos = ChunkPos::new(0, 0);
        let mut columns = MeshColumns::default();
        columns.mark(pos, FULL, false);
        let first = requests(&mut columns, &[pos], 0);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].sections, 0..24);
        assert!(!first[0].recompile);
        assert!(
            columns
                .requests(&[pos], CAMERA, FULL, |_| 0, |_| 0)
                .is_none(),
            "nothing changed and the camera stayed put"
        );
        columns.meshed(pos, first[0].content_gen, FULL);
        columns.mark(pos, 1 << 5, true);
        let edit = requests(&mut columns, &[pos], 0);
        assert_eq!(edit[0].sections, 5..6);
        assert!(edit[0].recompile);
    }

    #[test]
    fn moving_away_remeshes_at_the_new_lod() {
        let pos = ChunkPos::new(10, 0);
        let mut columns = MeshColumns::default();
        columns.mark(pos, FULL, false);
        let near = requests(&mut columns, &[pos], 0);
        columns.meshed(pos, near[0].content_gen, FULL);
        assert!(requests(&mut columns, &[pos], 0).is_empty());
        let far = requests(&mut columns, &[pos], 1);
        assert_eq!(far.len(), 1);
        assert_eq!(far[0].lod, 1);
        assert_eq!(far[0].sections, 0..24);
    }

    #[test]
    fn a_taken_generation_is_left_to_its_worker() {
        let pos = ChunkPos::new(0, 0);
        let mut columns = MeshColumns::default();
        columns.mark(pos, FULL, false);
        let generation = requests(&mut columns, &[pos], 0)[0].content_gen;
        columns.mark(pos, 1, false);
        let newer = columns
            .requests(&[pos], CAMERA, FULL, |_| 0, |_| generation + 1)
            .unwrap();
        assert!(newer.is_empty(), "a worker took the newest generation");
    }

    #[test]
    fn a_result_from_before_a_newer_mark_keeps_its_sections_dirty() {
        let pos = ChunkPos::new(0, 0);
        let mut columns = MeshColumns::default();
        columns.mark(pos, FULL, false);
        let old = requests(&mut columns, &[pos], 0)[0].content_gen;
        columns.mark(pos, 1 << 3, true);
        columns.meshed(pos, old, FULL);
        let again = requests(&mut columns, &[pos], 0);
        assert_eq!(again[0].sections, 0..24, "the whole column is still dirty");
        assert!(!again[0].recompile, "it holds new content, not only edits");
    }

    #[test]
    fn a_reloaded_column_is_a_new_incarnation() {
        let pos = ChunkPos::new(0, 0);
        let mut columns = MeshColumns::default();
        columns.mark(pos, FULL, false);
        let first = columns.incarnation(pos).unwrap();
        let retired = columns.forget(pos).unwrap();
        assert_eq!(columns.incarnation(pos), None);
        assert_eq!(columns.forget(pos), None);
        columns.mark(pos, FULL, false);
        let second = columns.incarnation(pos).unwrap();
        assert!(first < retired && retired < second);
    }

    #[test]
    fn generations_keep_rising_across_a_reset() {
        let pos = ChunkPos::new(0, 0);
        let mut columns = MeshColumns::default();
        columns.mark(pos, FULL, false);
        let before = columns.incarnation(pos).unwrap();
        columns.reset();
        assert_eq!(columns.incarnation(pos), None);
        columns.mark(pos, FULL, false);
        assert!(columns.incarnation(pos).unwrap() > before);
    }

    #[test]
    fn remarking_ignores_columns_no_longer_tracked() {
        let pos = ChunkPos::new(0, 0);
        let mut columns = MeshColumns::default();
        columns.remark(pos, FULL);
        assert!(requests(&mut columns, &[pos], 0).is_empty());
    }
}
