//! Distant terrain sections as geometry: each run a box, faces another run
//! covers left out.

use std::collections::HashMap;

use super::column::{Run, RunKey};
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
    /// out along `x` (bits 3-4) and `z` (5-6): 1 for positive, 3 negative.
    pub face: u8,
    pub color: [u8; 4],
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
    /// Vanilla's directional shade (`Direction` brightness under the
    /// overworld's cardinal lighting).
    fn shade(self) -> f32 {
        match self {
            Self::Up => 1.0,
            Self::Down => 0.5,
            Self::North | Self::South => 0.8,
            Self::West | Self::East => 0.6,
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
    pub vertices: Vec<DistantVertex>,
    pub min_y: u16,
    pub max_y: u16,
}

impl SectionMesh {
    /// A section with nothing seen in it.
    pub fn empty(key: SectionKey) -> Self {
        Self {
            key,
            vertices: Vec::new(),
            min_y: 0,
            max_y: 0,
        }
    }
}

/// Top or bottom faces alike in height, colour and light, as a bit per
/// column: bit `x` of row `z`.
type FlatFaces = HashMap<(Face, u32, [u8; 3], u8), [u64; SECTION_COLUMNS as usize]>;

/// `colors` gives a run's top and side colour. Tops and bottoms alike merge
/// into rectangles; sides are a quad per uncovered stretch.
pub(crate) fn mesh_section(
    key: SectionKey,
    section: &Section,
    colors: impl Fn(RunKey) -> ([u8; 3], [u8; 3]),
) -> SectionMesh {
    let w = key.column_width() as u32;
    let mut vertices = Vec::new();
    let mut flat = FlatFaces::new();
    let (mut min_y, mut max_y) = (u16::MAX, 0);
    for z in 0..SECTION_COLUMNS {
        for x in 0..SECTION_COLUMNS {
            let runs: Vec<Run> = section.column(x, z).collect();
            if runs.is_empty() {
                continue;
            }
            let neighbours: [Vec<Run>; 4] =
                SIDES.map(|(_, dx, dz)| section.column(x + dx, z + dz).collect());
            let (x0, z0) = (x as u32 * w, z as u32 * w);
            let (x1, z1) = (x0 + w, z0 + w);
            for (i, run) in runs.iter().enumerate() {
                let (top_rgb, side_rgb) = colors(run.key);
                let light = run.sky << 4 | run.block;
                let (bottom, top) = (u32::from(run.bottom), u32::from(run.top) + 1);
                min_y = min_y.min(run.bottom);
                max_y = max_y.max(run.top + 1);
                let mut flat_face = |face: Face, y| {
                    flat.entry((face, y, top_rgb, light))
                        .or_insert([0; SECTION_COLUMNS as usize])[z as usize] |= 1 << x;
                };
                let above_touches = i > 0 && runs[i - 1].bottom == run.top + 1;
                if !above_touches {
                    flat_face(Face::Up, top);
                }
                let below_touches = runs
                    .get(i + 1)
                    .is_some_and(|below| below.top + 1 == run.bottom);
                if run.bottom > 0 && !below_touches {
                    flat_face(Face::Down, bottom);
                }
                for (&(face, ..), neighbour) in SIDES.iter().zip(&neighbours) {
                    for (low, high, sky) in uncovered(bottom, top, neighbour) {
                        let corners = match face {
                            Face::North => {
                                [[x0, low, z0], [x0, high, z0], [x1, high, z0], [x1, low, z0]]
                            }
                            Face::South => {
                                [[x1, low, z1], [x1, high, z1], [x0, high, z1], [x0, low, z1]]
                            }
                            Face::West => {
                                [[x0, low, z1], [x0, high, z1], [x0, high, z0], [x0, low, z0]]
                            }
                            Face::Up | Face::Down | Face::East => {
                                [[x1, low, z0], [x1, high, z0], [x1, high, z1], [x1, low, z1]]
                            }
                        };
                        let light = sky.map_or(light, |sky| sky << 4 | run.block);
                        push_quad(&mut vertices, corners, light, face, side_rgb);
                    }
                }
            }
        }
    }
    for ((face, y, rgb, light), mut rows) in flat {
        for (x, z, width, depth) in rectangles(&mut rows) {
            let (x0, z0) = (x * w, z * w);
            let (x1, z1) = (x0 + width * w, z0 + depth * w);
            let corners = match face {
                Face::Up => [[x0, y, z0], [x1, y, z0], [x1, y, z1], [x0, y, z1]],
                _ => [[x0, y, z0], [x0, y, z1], [x1, y, z1], [x1, y, z0]],
            };
            push_quad(&mut vertices, corners, light, face, rgb);
        }
    }
    SectionMesh {
        key,
        vertices,
        min_y: min_y.min(max_y),
        max_y,
    }
}

/// Greedy rectangles covering the set bits of `rows` (bit `x` of row `z`),
/// as `(x, z, width, depth)` in columns; clears `rows`.
fn rectangles(rows: &mut [u64; SECTION_COLUMNS as usize]) -> Vec<(u32, u32, u32, u32)> {
    let mut out = Vec::new();
    for z in 0..rows.len() {
        while rows[z] != 0 {
            let x = rows[z].trailing_zeros();
            let width = (!(rows[z] >> x)).trailing_zeros();
            let mask = (u64::MAX >> (64 - width)) << x;
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
fn uncovered(low: u32, high: u32, neighbour: &[Run]) -> Vec<(u32, u32, Option<u8>)> {
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

/// A quad of `face`, its colour shaded for the direction it points.
fn push_quad(
    vertices: &mut Vec<DistantVertex>,
    corners: [[u32; 3]; 4],
    light: u8,
    face: Face,
    rgb: [u8; 3],
) {
    let [r, g, b] = rgb.map(|c| (f32::from(c) * face.shade()).round() as u8);
    // Each corner moves out along the quad's own plane, closing pinholes
    // where quads meet (Distant Horizons' `LodQuadBuilder` micro offset).
    let micro_offset = |axis: usize, value: u32| {
        let (min, max) = corners.iter().fold((u32::MAX, 0), |(lo, hi), c| {
            (lo.min(c[axis]), hi.max(c[axis]))
        });
        if min == max {
            0
        } else if value == min {
            3
        } else {
            1
        }
    };
    vertices.extend(corners.map(|[x, y, z]| DistantVertex {
        xz: x | z << 16,
        y: y as u16,
        light,
        face: face as u8 | micro_offset(0, x) << 3 | micro_offset(2, z) << 5,
        color: [r, g, b, 255],
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

    /// Quads in the level-0 mesh of chunk 0 with `columns` (by `z * 16 + x`).
    fn quads(columns: impl Fn(usize) -> Vec<Run>) -> usize {
        let mut store = Store::default();
        store.capture(ChunkPos::new(0, 0), (0..256).map(columns).collect());
        let key = SectionKey {
            level: 0,
            x: 0,
            z: 0,
        };
        let mesh = mesh_section(key, store.get(key).unwrap(), |_| ([0; 3], [0; 3]));
        mesh.vertices.len() / 4
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
