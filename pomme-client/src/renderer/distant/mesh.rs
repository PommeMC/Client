//! Distant terrain sections as geometry: each run a box, faces another run
//! covers left out.

use std::collections::HashMap;

use super::column::{Run, RunKey};
use super::models::{FACE_DISCARD, FACE_DISCARD_WHEN_MERGED, FACE_EMPTY, FACE_TRANSLUCENT};
use super::store::{SECTION_COLUMNS, Section, SectionKey};

/// One corner of a quad; four in a row make a quad, drawn with a shared
/// `0, 1, 2, 2, 3, 0` index pattern.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct DistantVertex {
    /// Section-local block `x` in the low 16 bits, `z` in the high.
    pub xz: u32,
    /// Blocks above the dimension's `min_y`.
    pub y: u16,
    /// Sky light in the high nibble, block light in the low.
    pub light: u8,
    /// Which way the face points ([`Face`]) in the low 3 bits, so the
    /// shaders can tell whose column it is; then which way the corner moves
    /// out along `x` (bits 3-4) and `z` (5-6): 1 for positive, 3 negative;
    /// bit 7 for a seam ([`mesh_section`]).
    pub face: u8,
    /// The tint over the model's texture, or with no model the run's own
    /// colour.
    pub color: [u8; 4],
    /// [`RunLook::model`].
    pub model: u16,
    /// The face's [`super::models`] flags, discarding resolved for the
    /// quad's size.
    pub flags: u8,
    pub _pad: u8,
}

/// How a run draws.
#[derive(Clone, Copy)]
pub(crate) struct RunLook {
    /// Its baked model ([`super::models`]); 0 for none, drawn in its colours
    /// alone.
    pub model: u16,
    /// Blended over what's behind it: water.
    pub translucent: bool,
    /// Hides the faces of runs it touches.
    pub occludes: bool,
    /// The model's face flags, by Voxy face index.
    pub faces: [u8; 6],
    /// The top's and bottom's colour, then the sides'.
    pub top: [u8; 4],
    pub side: [u8; 4],
}

impl RunLook {
    /// What `face` draws with; `None` where the model draws nothing.
    fn paint(&self, face: Face) -> Option<Paint> {
        let flags = self.faces[face.voxy_index()];
        (flags & FACE_EMPTY == 0).then_some(Paint {
            model: self.model,
            flags,
            color: match face {
                Face::Up | Face::Down => self.top,
                _ => self.side,
            },
            translucent: self.translucent,
        })
    }
}

/// What a face draws with: faces alike in it (and in light) merge.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Paint {
    model: u16,
    flags: u8,
    color: [u8; 4],
    translucent: bool,
}

/// A face's direction, as `distant.vert` reads it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
enum Face {
    Up,
    Down,
    North,
    South,
    West,
    East,
}

impl Face {
    /// Its index in a model's faces
    /// ([`crate::world::block::model::Direction::VALUES`]).
    fn voxy_index(self) -> usize {
        match self {
            Self::Down => 0,
            Self::Up => 1,
            Self::North => 2,
            Self::South => 3,
            Self::West => 4,
            Self::East => 5,
        }
    }
}

/// The sides, with the offset of the column each faces.
const SIDES: [(Face, i32, i32); 4] = [
    (Face::North, 0, -1),
    (Face::South, 0, 1),
    (Face::West, -1, 0),
    (Face::East, 1, 0),
];

/// A section's geometry and the height it spans, in blocks above `min_y`.
pub(crate) struct SectionMesh {
    pub key: SectionKey,
    /// Opaque faces, then see-through ones from `translucent_start`.
    pub vertices: Vec<DistantVertex>,
    pub translucent_start: u32,
    pub min_y: u16,
    pub max_y: u16,
}

impl SectionMesh {
    /// A section with nothing seen in it.
    pub fn empty(key: SectionKey) -> Self {
        Self {
            key,
            vertices: Vec::new(),
            translucent_start: 0,
            min_y: 0,
            max_y: 0,
        }
    }
}

/// Top or bottom faces alike in height, paint and light, as a bit per
/// column: bit `x` of row `z`.
type FlatFaces = HashMap<(Face, u32, Paint, u8), [u64; SECTION_COLUMNS as usize]>;

/// Seams alike in span, paint and light along one chunk border, as a bit
/// per column along it: by face, the border's column, top and bottom, then
/// the rest.
type Seams = HashMap<(Face, u32, u32, u32, Paint, u8), u64>;

/// `look` tells how a run draws. Tops and bottoms alike merge into
/// rectangles; sides are a quad per uncovered stretch.
///
/// See-through runs, water, hide nothing behind them, so the sea floor
/// keeps its top and its sides beside water. They draw no side facing
/// nothing, which at a section's border would be a sheet in the middle of
/// the sea. Instead they get seams, sides along every chunk border, which
/// `distant.frag` keeps only where they face a drawn chunk: where the real
/// water ends, which mesh time can't know, looking on through its edge
/// still passes through water.
pub(crate) fn mesh_section(
    key: SectionKey,
    section: &Section,
    mut look: impl FnMut(RunKey) -> RunLook,
) -> SectionMesh {
    let w = key.column_width() as u32;
    let mut looks = HashMap::new();
    let mut look_of = |run: Run| *looks.entry(run.key).or_insert_with(|| look(run.key));
    // Opaque quads, then see-through ones.
    let mut quads = [Vec::new(), Vec::new()];
    let mut flat = FlatFaces::new();
    let mut seams = Seams::new();
    let (mut min_y, mut max_y) = (u16::MAX, 0);
    for z in 0..SECTION_COLUMNS {
        for x in 0..SECTION_COLUMNS {
            let runs: Vec<Run> = section.column(x, z).collect();
            if runs.is_empty() {
                continue;
            }
            let run_looks: Vec<RunLook> = runs.iter().map(|&run| look_of(run)).collect();
            let neighbours: [Vec<(Run, RunLook)>; 4] = SIDES.map(|(_, dx, dz)| {
                section
                    .column(x + dx, z + dz)
                    .map(|run| (run, look_of(run)))
                    .collect()
            });
            let (x0, z0) = (x as u32 * w, z as u32 * w);
            let (x1, z1) = (x0 + w, z0 + w);
            for (i, (run, look)) in runs.iter().zip(&run_looks).enumerate() {
                let see_through = look.translucent;
                let light = run.sky << 4 | run.block;
                let (bottom, top) = (u32::from(run.bottom), u32::from(run.top) + 1);
                min_y = min_y.min(run.bottom);
                max_y = max_y.max(run.top + 1);
                let mut flat_face = |face: Face, y| {
                    if let Some(paint) = look.paint(face) {
                        flat.entry((face, y, paint, light))
                            .or_insert([0; SECTION_COLUMNS as usize])[z as usize] |= 1 << x;
                    }
                };
                // A face against a run hides unless only the run is see-through.
                let hidden_by = |other: &RunLook| other.occludes || see_through;
                let above_hides =
                    i > 0 && runs[i - 1].bottom == run.top + 1 && hidden_by(&run_looks[i - 1]);
                if !above_hides {
                    flat_face(Face::Up, top);
                }
                let below_hides = runs.get(i + 1).is_some_and(|below| {
                    below.top + 1 == run.bottom && hidden_by(&run_looks[i + 1])
                });
                if run.bottom > 0 && !below_hides {
                    flat_face(Face::Down, bottom);
                }
                if see_through {
                    let (x, z) = (x as u32, z as u32);
                    for (face, on_border, line, along) in [
                        (Face::North, z0 % 16 == 0, z, x),
                        (Face::South, z1 % 16 == 0, z, x),
                        (Face::West, x0 % 16 == 0, x, z),
                        (Face::East, x1 % 16 == 0, x, z),
                    ] {
                        if on_border && let Some(paint) = look.paint(face) {
                            *seams
                                .entry((face, line, top, bottom, paint, light))
                                .or_default() |= 1 << along;
                        }
                    }
                }
                for (&(face, ..), neighbour) in SIDES.iter().zip(&neighbours) {
                    let Some(paint) = look.paint(face) else {
                        continue;
                    };
                    // Water beside an opaque run doesn't hide it.
                    let covering = neighbour
                        .iter()
                        .filter(|(_, other)| see_through || other.occludes)
                        .map(|(run, _)| run);
                    for (low, high, sky) in uncovered(bottom, top, covering) {
                        if see_through && sky.is_none() {
                            continue;
                        }
                        let corners = side_corners(face, [x0, x1], [z0, z1], low, high);
                        // Facing nothing, as at the edge of what's been seen: the
                        // sky over the column, as the run's own can be deep water's.
                        let sky = sky.unwrap_or(runs[0].sky);
                        let light = sky << 4 | run.block;
                        push_quad(&mut quads, corners, light, face, paint, false, w);
                    }
                }
            }
        }
    }
    for ((face, y, paint, light), mut rows) in flat {
        for (x, z, width, depth) in rectangles(&mut rows) {
            let (x0, z0) = (x * w, z * w);
            let (x1, z1) = (x0 + width * w, z0 + depth * w);
            let corners = match face {
                Face::Up => [[x0, y, z0], [x1, y, z0], [x1, y, z1], [x0, y, z1]],
                _ => [[x0, y, z0], [x0, y, z1], [x1, y, z1], [x1, y, z0]],
            };
            push_quad(&mut quads, corners, light, face, paint, false, w);
        }
    }
    for ((face, line, top, bottom, paint, light), mut bits) in seams {
        while bits != 0 {
            let (start, len, mask) = lowest_span(bits);
            bits &= !mask;
            let along = [start * w, (start + len) * w];
            let across = [line * w, (line + 1) * w];
            let (xs, zs) = match face {
                Face::North | Face::South => (along, across),
                _ => (across, along),
            };
            let corners = side_corners(face, xs, zs, bottom, top);
            push_quad(&mut quads, corners, light, face, paint, true, w);
        }
    }
    let [mut vertices, translucent] = quads;
    let translucent_start = vertices.len() as u32;
    vertices.extend(translucent);
    SectionMesh {
        key,
        vertices,
        translucent_start,
        min_y: min_y.min(max_y),
        max_y,
    }
}

/// The corners of the side facing `face` of the box spanning `xs` and `zs`,
/// from `low` to `high`.
fn side_corners(
    face: Face,
    [x0, x1]: [u32; 2],
    [z0, z1]: [u32; 2],
    low: u32,
    high: u32,
) -> [[u32; 3]; 4] {
    match face {
        Face::North => [[x0, low, z0], [x0, high, z0], [x1, high, z0], [x1, low, z0]],
        Face::South => [[x1, low, z1], [x1, high, z1], [x0, high, z1], [x0, low, z1]],
        Face::West => [[x0, low, z1], [x0, high, z1], [x0, high, z0], [x0, low, z0]],
        Face::Up | Face::Down | Face::East => {
            [[x1, low, z0], [x1, high, z0], [x1, high, z1], [x1, low, z1]]
        }
    }
}

/// The lowest stretch of set bits in `bits`: its first bit, length and mask.
fn lowest_span(bits: u64) -> (u32, u32, u64) {
    let start = bits.trailing_zeros();
    let len = (!(bits >> start)).trailing_zeros();
    (start, len, (u64::MAX >> (64 - len)) << start)
}

/// Greedy rectangles covering the set bits of `rows` (bit `x` of row `z`),
/// as `(x, z, width, depth)` in columns; clears `rows`.
fn rectangles(rows: &mut [u64; SECTION_COLUMNS as usize]) -> Vec<(u32, u32, u32, u32)> {
    let mut out = Vec::new();
    for z in 0..rows.len() {
        while rows[z] != 0 {
            let (x, width, mask) = lowest_span(rows[z]);
            let mut end = z + 1;
            while end < rows.len() && rows[end] & mask == mask {
                rows[end] &= !mask;
                end += 1;
            }
            rows[z] &= !mask;
            out.push((x, z as u32, width, (end - z) as u32));
        }
    }
    out
}

/// The parts of `[low, high)` none of `neighbour`'s runs cover, highest
/// first, each with the sky light of the air it faces: that over the
/// neighbour's run under it, as Distant Horizons lights a side under an
/// overhang, or `None` with no run under it. A neighbour outside the
/// section has no runs, so the section's border faces always draw: a
/// coarser or missing neighbour can't hide them.
fn uncovered<'a>(
    low: u32,
    high: u32,
    neighbour: impl IntoIterator<Item = &'a Run>,
) -> Vec<(u32, u32, Option<u8>)> {
    let mut open = Vec::new();
    let mut cursor = high;
    // Runs go top to bottom, so walk down.
    for run in neighbour {
        let (run_low, run_high) = (u32::from(run.bottom), u32::from(run.top) + 1);
        if run_low >= cursor {
            continue;
        }
        if run_high < cursor {
            open.push((run_high.max(low), cursor, Some(run.sky)));
        }
        cursor = run_low;
        if cursor <= low {
            return open;
        }
    }
    if cursor > low {
        open.push((low, cursor, None));
    }
    open
}

/// A quad of `face` into `quads`' opaque or see-through half. A model face
/// short of its side discards once the quad spans more than one column,
/// where its gaps would otherwise stretch.
fn push_quad(
    quads: &mut [Vec<DistantVertex>; 2],
    corners: [[u32; 3]; 4],
    light: u8,
    face: Face,
    paint: Paint,
    seam: bool,
    w: u32,
) {
    let extent = |axis: usize| {
        corners.iter().fold((u32::MAX, 0), |(lo, hi), c| {
            (lo.min(c[axis]), hi.max(c[axis]))
        })
    };
    let merged = (0..3).any(|axis| {
        let (min, max) = extent(axis);
        max - min > w
    });
    let mut flags = paint.flags & !FACE_DISCARD_WHEN_MERGED;
    if merged && paint.flags & FACE_DISCARD_WHEN_MERGED != 0 {
        flags |= FACE_DISCARD;
    }
    if paint.translucent {
        flags |= FACE_TRANSLUCENT;
    }
    // Each corner moves out along the quad's own plane, closing pinholes
    // where quads meet (Distant Horizons' `LodQuadBuilder` micro offset).
    let micro_offset = |axis: usize, value: u32| {
        let (min, max) = extent(axis);
        if min == max {
            0
        } else if value == min {
            3
        } else {
            1
        }
    };
    quads[usize::from(paint.translucent)].extend(corners.map(|[x, y, z]| DistantVertex {
        xz: x | z << 16,
        y: y as u16,
        light,
        face: face as u8 | micro_offset(0, x) << 3 | micro_offset(2, z) << 5 | u8::from(seam) << 7,
        color: paint.color,
        model: paint.model,
        flags,
        _pad: 0,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::ChunkPos;
    use crate::renderer::distant::store::Store;

    fn run(top: u16, bottom: u16) -> Run {
        lit(top, bottom, 15)
    }

    fn lit(top: u16, bottom: u16, sky: u8) -> Run {
        Run {
            key: RunKey {
                state: azalea_block::BlockState::AIR,
                biome: 0,
            },
            top,
            bottom,
            sky,
            block: 0,
        }
    }

    #[test]
    fn a_neighbour_hides_the_side_it_covers() {
        // A 0..10 box beside a 0..4 one shows only 4..10.
        assert_eq!(uncovered(0, 10, &[run(3, 0)]), [(4, 10, Some(15))]);
        // Beside a floating 4..6 run: above and below it.
        assert_eq!(
            uncovered(0, 10, &[run(5, 4)]),
            [(6, 10, Some(15)), (0, 4, None)]
        );
        // Beside a taller one: nothing.
        assert!(uncovered(2, 5, &[run(9, 0)]).is_empty());
        // Beside nothing, or a run wholly above or below: the whole side.
        assert_eq!(uncovered(2, 5, &[]), [(2, 5, None)]);
        assert_eq!(uncovered(2, 5, &[run(9, 7), run(0, 0)]), [(2, 5, Some(15))]);
        // Under an overhang, lit by the dim air over the floor beside it.
        assert_eq!(
            uncovered(2, 9, &[run(12, 9), lit(3, 0, 4)]),
            [(4, 9, Some(4))]
        );
    }

    /// The level-0 mesh of chunk 0 with `columns` (by `z * 16 + x`), runs
    /// of biome 1 see-through as water.
    fn mesh(columns: impl Fn(usize) -> Vec<Run>) -> SectionMesh {
        let mut store = Store::default();
        store.capture(ChunkPos::new(0, 0), (0..256).map(columns).collect());
        let key = SectionKey {
            level: 0,
            x: 0,
            z: 0,
        };
        mesh_section(key, store.get(key).unwrap(), |key| {
            let water = key.biome == 1;
            let alpha = if water { 128 } else { 255 };
            RunLook {
                model: 0,
                translucent: water,
                occludes: !water,
                faces: [0; 6],
                top: [0, 0, 0, alpha],
                side: [0, 0, 0, alpha],
            }
        })
    }

    fn quads(columns: impl Fn(usize) -> Vec<Run>) -> usize {
        mesh(columns).vertices.len() / 4
    }

    fn water(top: u16, bottom: u16) -> Run {
        Run {
            key: RunKey {
                biome: 1,
                ..run(top, bottom).key
            },
            ..run(top, bottom)
        }
    }

    #[test]
    fn the_sea_floor_shows_through_water() {
        // A flat chunk of sea: the water's top and the floor's under it, and
        // the floor's sides beside water.
        let sea = mesh(|_| vec![water(10, 6), run(5, 0)]);
        let see_through = |v: &DistantVertex| v.color[3] < 255;
        let (opaque, clear) = sea.vertices.split_at(sea.translucent_start as usize);
        assert!(opaque.iter().all(|v| !see_through(v)));
        assert!(clear.iter().all(see_through));
        // The water's top and a seam from it down to the floor per side of
        // the chunk: its sides at the chunk's border face nothing.
        assert_eq!(clear.len() / 4, 1 + 4);
        let seams = clear.iter().filter(|v| v.face >> 7 == 1);
        assert!(seams.clone().all(|v| v.y == 6 || v.y == 11));
        assert_eq!(seams.count(), 4 * 4);
        // The floor's top and its sides at the border.
        assert_eq!(opaque.len() / 4, 1 + 4 * 16);
        // A floor step under water keeps its side.
        let step = mesh(|i| {
            if i % 16 == 0 {
                vec![water(10, 8), run(7, 0)]
            } else {
                vec![water(10, 6), run(5, 0)]
            }
        });
        assert!(
            step.vertices[..step.translucent_start as usize]
                .iter()
                .any(|v| v.face & 7 == Face::East as u8 && v.y == 8)
        );
    }

    #[test]
    fn sides_facing_nothing_take_the_sky_over_their_column() {
        // A lit run over a dark one at the section's west edge: both west
        // sides face nothing, so neither is drawn dark.
        let mesh = mesh(|i| {
            if i == 0 {
                vec![lit(20, 15, 15), lit(10, 0, 0)]
            } else {
                Vec::new()
            }
        });
        let west: Vec<u8> = mesh
            .vertices
            .iter()
            .filter(|v| v.face & 7 == Face::West as u8)
            .map(|v| v.light >> 4)
            .collect();
        assert_eq!(west, [15; 8]);
    }

    #[test]
    fn a_lone_column_is_a_box_without_its_world_bottom() {
        let lone = |i| if i == 0 { vec![run(5, 0)] } else { Vec::new() };
        assert_eq!(quads(lone), 5);
        let floating = |i| if i == 0 { vec![run(5, 3)] } else { Vec::new() };
        assert_eq!(quads(floating), 6);
    }

    #[test]
    fn equal_neighbours_hide_their_shared_sides() {
        // Two side by side: their tops merge, their shared sides go.
        let pair = |i| if i < 2 { vec![run(5, 0)] } else { Vec::new() };
        assert_eq!(quads(pair), 1 + 2 * 3);
        // A flat chunk: one top, and the chunk's outer sides, which stay
        // because they're in the middle of nothing.
        assert_eq!(quads(|_| vec![run(5, 0)]), 1 + 4 * 16);
    }

    #[test]
    fn flat_faces_merge_into_rectangles() {
        let mut rows = [0u64; SECTION_COLUMNS as usize];
        // An L: a 3x2 block with one more cell under its left edge.
        rows[0] = 0b111;
        rows[1] = 0b111;
        rows[2] = 0b001;
        assert_eq!(rectangles(&mut rows), [(0, 0, 3, 2), (0, 2, 1, 1)]);
        assert!(rows.iter().all(|&row| row == 0));
        rows[5] = u64::MAX;
        assert_eq!(rectangles(&mut rows), [(0, 5, 64, 1)]);
    }
}
