use std::collections::HashMap;

use crate::world::block::model::Direction;

use super::greedy_face::{
    block_coords, block_index, GreedyFaceRecord, MAX_TINT_INDEX, TINTS_PER_BATCH,
};
use super::mesher::{BatchCull, PACKED_WHITE_RGB};

#[derive(Clone, Copy)]
pub(crate) struct RawGreedyFace {
    pub block_index: u16,
    pub direction: Direction,
    pub global_id: u16,
    pub shades: [u8; 4],
    pub packed_tint: u32,
    pub tinted: bool,
    pub mergeable: bool,
    pub cull: BatchCull,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}

#[derive(Clone, Copy)]
struct GridFace {
    global_id: u16,
    packed_tint: u32,
    tinted: bool,
    shades: [u8; 4],
    cull: BatchCull,
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SliceKey {
    direction: Direction,
    slice: u8,
}

fn shade_u5_to_u4(shade: u8) -> u8 {
    ((u16::from(shade.min(31)) * 15 + 15) / 31) as u8
}

/// Greedy quads interpolate corner AO across the whole merge; only merge
/// faces whose corners are uniform so the shader never smooths block AO.
fn face_shades_flat(shades: [u8; 4]) -> bool {
    shades[0] == shades[1] && shades[1] == shades[2] && shades[2] == shades[3]
}

fn quantize_shades(shades: [u8; 4]) -> [u8; 4] {
    shades.map(shade_u5_to_u4)
}

fn mergeable_grid_index(direction: Direction, block_index: u16) -> Option<(u8, u8, u8)> {
    let (x, y, z) = block_coords(block_index);
    Some(match direction {
        Direction::Up | Direction::Down => (y, x, z),
        Direction::North | Direction::South => (z, x, y),
        Direction::West | Direction::East => (x, z, y),
    })
}

fn merged_block_index(direction: Direction, slice: u8, u: u8, v: u8) -> u16 {
    match direction {
        Direction::Up | Direction::Down => block_index(u, slice, v),
        Direction::North | Direction::South => block_index(u, v, slice),
        Direction::West | Direction::East => block_index(slice, v, u),
    }
}

fn can_extend_u(direction: Direction, left: [u8; 4], right: [u8; 4]) -> bool {
    match direction {
        Direction::Up => left[3] == right[0] && left[2] == right[1],
        Direction::Down => left[2] == right[1] && left[3] == right[0],
        Direction::North => left[0] == right[2] && left[1] == right[3],
        Direction::South => left[2] == right[0] && left[3] == right[1],
        Direction::West => left[3] == right[0] && left[2] == right[1],
        Direction::East => left[0] == right[2] && left[1] == right[3],
    }
}

fn can_extend_v(direction: Direction, bottom: [u8; 4], top: [u8; 4]) -> bool {
    match direction {
        Direction::Up => bottom[1] == top[0] && bottom[2] == top[3],
        Direction::Down => bottom[0] == top[1] && bottom[3] == top[2],
        Direction::North => bottom[0] == top[1] && bottom[3] == top[2],
        Direction::South => bottom[1] == top[0] && bottom[2] == top[3],
        Direction::West => bottom[0] == top[1] && bottom[3] == top[2],
        Direction::East => bottom[0] == top[1] && bottom[3] == top[2],
    }
}

fn merged_corner_shades(
    mergeable: &HashMap<(SliceKey, u8, u8), GridFace>,
    key: SliceKey,
    start_u: u8,
    start_v: u8,
    width: u8,
    height: u8,
) -> [u8; 4] {
    let at = |u: u8, v: u8, corner: usize| -> u8 {
        mergeable[&(key, start_u + u, start_v + v)].shades[corner]
    };
    match key.direction {
        Direction::Up => [
            at(0, 0, 0),
            at(0, height - 1, 1),
            at(width - 1, height - 1, 2),
            at(width - 1, 0, 3),
        ],
        Direction::Down => [
            at(0, height - 1, 0),
            at(0, 0, 1),
            at(width - 1, 0, 2),
            at(width - 1, height - 1, 3),
        ],
        Direction::North => [
            at(width - 1, height - 1, 0),
            at(width - 1, 0, 1),
            at(0, 0, 2),
            at(0, height - 1, 3),
        ],
        Direction::South => [
            at(0, height - 1, 0),
            at(0, 0, 1),
            at(width - 1, 0, 2),
            at(width - 1, height - 1, 3),
        ],
        Direction::West => [
            at(0, height - 1, 0),
            at(width - 1, height - 1, 1),
            at(width - 1, 0, 2),
            at(0, 0, 3),
        ],
        Direction::East => [
            at(width - 1, height - 1, 0),
            at(width - 1, 0, 1),
            at(0, 0, 2),
            at(0, height - 1, 3),
        ],
    }
}

pub(crate) struct MergedGreedyFace {
    pub record: GreedyFaceRecord,
    pub packed_tint: u32,
    pub tinted: bool,
    pub cull: BatchCull,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}

pub(crate) fn greedy_merge(raw: Vec<RawGreedyFace>) -> Vec<MergedGreedyFace> {
    let mut mergeable: HashMap<(SliceKey, u8, u8), GridFace> = HashMap::new();
    let mut passthrough = Vec::new();

    for face in raw {
        if face.mergeable && face_shades_flat(face.shades) {
            if let Some((slice, u, v)) = mergeable_grid_index(face.direction, face.block_index) {
                mergeable.insert(
                    (
                        SliceKey {
                            direction: face.direction,
                            slice,
                        },
                        u,
                        v,
                    ),
                    GridFace {
                        global_id: face.global_id,
                        packed_tint: face.packed_tint,
                        tinted: face.tinted,
                        shades: face.shades,
                        cull: face.cull,
                        aabb_min: face.aabb_min,
                        aabb_max: face.aabb_max,
                    },
                );
                continue;
            }
        }
        passthrough.push(MergedGreedyFace {
            record: GreedyFaceRecord::new(
                face.block_index,
                face.direction.index() as u8,
                1,
                1,
                face.global_id,
                quantize_shades(face.shades),
                0,
            ),
            packed_tint: face.packed_tint,
            tinted: face.tinted,
            cull: face.cull,
            aabb_min: face.aabb_min,
            aabb_max: face.aabb_max,
        });
    }

    let mut slices: Vec<(SliceKey, u8, u8)> = mergeable.keys().copied().collect();
    slices.sort_by_key(|(key, u, v)| (key.direction.index(), key.slice, *u, *v));

    let mut visited = HashMap::<(SliceKey, u8, u8), bool>::new();
    let mut merged = Vec::new();

    for (key, start_u, start_v) in slices {
        if *visited.get(&(key, start_u, start_v)).unwrap_or(&false) {
            continue;
        }
        let Some(source) = mergeable.get(&(key, start_u, start_v)).copied() else {
            continue;
        };

        let mut width = 1u8;
        while start_u + width < 16 {
            let Some(next) = mergeable.get(&(key, start_u + width, start_v)).copied() else {
                break;
            };
            if next.global_id != source.global_id
                || next.packed_tint != source.packed_tint
                || next.tinted != source.tinted
            {
                break;
            }
            let left = mergeable[&(key, start_u + width - 1, start_v)];
            if !can_extend_u(key.direction, left.shades, next.shades) {
                break;
            }
            width += 1;
        }

        let mut height = 1u8;
        'height: while start_v + height < 16 {
            for u in 0..width {
                let bottom = mergeable[&(key, start_u + u, start_v + height - 1)];
                let Some(top) = mergeable.get(&(key, start_u + u, start_v + height)).copied() else {
                    break 'height;
                };
                if top.global_id != source.global_id
                    || top.packed_tint != source.packed_tint
                    || top.tinted != source.tinted
                    || !can_extend_v(key.direction, bottom.shades, top.shades)
                {
                    break 'height;
                }
            }
            height += 1;
        }

        for v in 0..height {
            for u in 0..width {
                visited.insert((key, start_u + u, start_v + v), true);
            }
        }

        let mut aabb_min = source.aabb_min;
        let mut aabb_max = source.aabb_max;
        for v in 0..height {
            for u in 0..width {
                let face = mergeable[&(key, start_u + u, start_v + v)];
                for axis in 0..3 {
                    aabb_min[axis] = aabb_min[axis].min(face.aabb_min[axis]);
                    aabb_max[axis] = aabb_max[axis].max(face.aabb_max[axis]);
                }
            }
        }

        let block_index = merged_block_index(key.direction, key.slice, start_u, start_v);
        let shades = quantize_shades(if width == 1 && height == 1 {
            source.shades
        } else {
            merged_corner_shades(&mergeable, key, start_u, start_v, width, height)
        });
        merged.push(MergedGreedyFace {
            record: GreedyFaceRecord::new(
                block_index,
                key.direction.index() as u8,
                width,
                height,
                source.global_id,
                shades,
                0,
            ),
            packed_tint: source.packed_tint,
            tinted: source.tinted,
            cull: source.cull,
            aabb_min,
            aabb_max,
        });
    }

    merged.extend(passthrough);
    merged
}

struct TintBatchBuilder {
    table: Vec<u32>,
    map: HashMap<u32, u16>,
}

impl TintBatchBuilder {
    fn new() -> Self {
        Self {
            table: vec![0],
            map: HashMap::new(),
        }
    }

    fn is_full(&self) -> bool {
        self.table.len() >= TINTS_PER_BATCH
    }

    fn intern(&mut self, packed_tint: u32, tinted: bool) -> Option<u16> {
        if !tinted || packed_tint == PACKED_WHITE_RGB {
            return Some(0);
        }
        if let Some(&index) = self.map.get(&packed_tint) {
            return Some(index);
        }
        if self.table.len() > MAX_TINT_INDEX as usize {
            return None;
        }
        let index = self.table.len() as u16;
        self.table.push(packed_tint);
        self.map.insert(packed_tint, index);
        Some(index)
    }
}

pub(crate) struct SolidBatchDescriptor {
    pub face_offset: u32,
    pub face_count: u32,
    pub tint_table_offset: u32,
    pub cull: BatchCull,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}

pub(crate) struct SolidTerrainPack {
    pub faces: Vec<GreedyFaceRecord>,
    pub tint_table: Vec<u32>,
    pub batches: Vec<SolidBatchDescriptor>,
}

pub(crate) fn pack_solid_terrain(
    merged: Vec<MergedGreedyFace>,
    directional: bool,
) -> SolidTerrainPack {
    let mut merged = merged;
    if directional {
        merged.sort_by_key(|face| face.cull as u8);
    }

    let mut faces = Vec::new();
    let mut tint_table = Vec::new();
    let mut batches = Vec::new();
    let mut builder = TintBatchBuilder::new();
    let mut batch_face_start = 0u32;
    let mut batch_tint_start = 0u32;
    let mut batch_aabb_min = [f32::MAX; 3];
    let mut batch_aabb_max = [f32::MIN; 3];
    let mut batch_cull = BatchCull::Uncullable;

    let flush = |faces: &mut Vec<GreedyFaceRecord>,
                     tint_table: &mut Vec<u32>,
                     batches: &mut Vec<SolidBatchDescriptor>,
                     builder: &mut TintBatchBuilder,
                     batch_face_start: &mut u32,
                     batch_tint_start: &mut u32,
                     batch_aabb_min: &mut [f32; 3],
                     batch_aabb_max: &mut [f32; 3],
                     batch_cull: &mut BatchCull| {
        if *batch_face_start == faces.len() as u32 {
            return;
        }
        tint_table.extend(builder.table.drain(..));
        *builder = TintBatchBuilder::new();
        batches.push(SolidBatchDescriptor {
            face_offset: *batch_face_start * 2,
            face_count: faces.len() as u32 - *batch_face_start,
            tint_table_offset: *batch_tint_start,
            cull: *batch_cull,
            aabb_min: *batch_aabb_min,
            aabb_max: *batch_aabb_max,
        });
        *batch_tint_start = tint_table.len() as u32;
        *batch_face_start = faces.len() as u32;
        *batch_aabb_min = [f32::MAX; 3];
        *batch_aabb_max = [f32::MIN; 3];
        *batch_cull = BatchCull::Uncullable;
    };

    for face in merged {
        let face_cull = if directional {
            face.cull
        } else {
            BatchCull::Uncullable
        };
        if directional && faces.len() as u32 != batch_face_start && face_cull != batch_cull {
            flush(
                &mut faces,
                &mut tint_table,
                &mut batches,
                &mut builder,
                &mut batch_face_start,
                &mut batch_tint_start,
                &mut batch_aabb_min,
                &mut batch_aabb_max,
                &mut batch_cull,
            );
        }

        if builder.is_full() {
            flush(
                &mut faces,
                &mut tint_table,
                &mut batches,
                &mut builder,
                &mut batch_face_start,
                &mut batch_tint_start,
                &mut batch_aabb_min,
                &mut batch_aabb_max,
                &mut batch_cull,
            );
        }

        let tint_index = match builder.intern(face.packed_tint, face.tinted) {
            Some(index) => index,
            None => {
                flush(
                    &mut faces,
                    &mut tint_table,
                    &mut batches,
                    &mut builder,
                    &mut batch_face_start,
                    &mut batch_tint_start,
                    &mut batch_aabb_min,
                    &mut batch_aabb_max,
                    &mut batch_cull,
                );
                builder
                    .intern(face.packed_tint, face.tinted)
                    .expect("fresh tint batch must accept one new tint")
            }
        };

        if faces.len() as u32 == batch_face_start {
            batch_cull = face_cull;
        }

        let (block_index, direction, width, height, global_id, shades, _) = face.record.fields();
        faces.push(GreedyFaceRecord::new(
            block_index,
            direction,
            width,
            height,
            global_id,
            shades,
            tint_index,
        ));

        for axis in 0..3 {
            batch_aabb_min[axis] = batch_aabb_min[axis].min(face.aabb_min[axis]);
            batch_aabb_max[axis] = batch_aabb_max[axis].max(face.aabb_max[axis]);
        }
    }

    flush(
        &mut faces,
        &mut tint_table,
        &mut batches,
        &mut builder,
        &mut batch_face_start,
        &mut batch_tint_start,
        &mut batch_aabb_min,
        &mut batch_aabb_max,
        &mut batch_cull,
    );

    SolidTerrainPack {
        faces,
        tint_table,
        batches,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::chunk::mesher::BatchCull;

    #[test]
    fn merges_coplanar_full_cube_faces_on_x() {
        let mk = |x| RawGreedyFace {
            block_index: block_index(x, 0, 0),
            direction: Direction::Up,
            global_id: 7,
            shades: [31; 4],
            packed_tint: PACKED_WHITE_RGB,
            tinted: false,
            mergeable: true,
            cull: BatchCull::Up,
            aabb_min: [x as f32, 1.0, 0.0],
            aabb_max: [x as f32 + 1.0, 1.0, 1.0],
        };
        let merged = greedy_merge(vec![mk(0), mk(1), mk(2)]);
        assert_eq!(merged.len(), 1);
        let (_, _, width, height, _, shades, _) = merged[0].record.fields();
        assert_eq!((width, height), (3, 1));
        assert_eq!(shades, [15; 4]);
    }

    #[test]
    fn does_not_merge_when_shared_edge_shades_differ() {
        let mk = |x, shades: [u8; 4]| RawGreedyFace {
            block_index: block_index(x, 0, 0),
            direction: Direction::Up,
            global_id: 7,
            shades,
            packed_tint: PACKED_WHITE_RGB,
            tinted: false,
            mergeable: true,
            cull: BatchCull::Up,
            aabb_min: [x as f32, 1.0, 0.0],
            aabb_max: [x as f32 + 1.0, 1.0, 1.0],
        };
        let merged = greedy_merge(vec![mk(0, [15, 15, 15, 15]), mk(1, [14, 15, 15, 14])]);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn does_not_merge_when_corner_shades_are_not_flat() {
        let mk = |x| RawGreedyFace {
            block_index: block_index(x, 0, 0),
            direction: Direction::Up,
            global_id: 7,
            shades: [15, 14, 14, 15],
            packed_tint: PACKED_WHITE_RGB,
            tinted: false,
            mergeable: true,
            cull: BatchCull::Up,
            aabb_min: [x as f32, 1.0, 0.0],
            aabb_max: [x as f32 + 1.0, 1.0, 1.0],
        };
        let merged = greedy_merge(vec![mk(0), mk(1)]);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merges_north_face_row_when_edge_shades_match() {
        let mk = |x, shades: [u8; 4]| RawGreedyFace {
            block_index: block_index(x, 0, 0),
            direction: Direction::North,
            global_id: 7,
            shades,
            packed_tint: PACKED_WHITE_RGB,
            tinted: false,
            mergeable: true,
            cull: BatchCull::North,
            aabb_min: [x as f32, 0.0, 0.0],
            aabb_max: [x as f32 + 1.0, 1.0, 0.0],
        };
        let merged = greedy_merge(vec![mk(0, [10; 4]), mk(1, [10; 4])]);
        assert_eq!(merged.len(), 1);
        let (_, _, width, height, _, _, _) = merged[0].record.fields();
        assert_eq!((width, height), (2, 1));
    }
}
