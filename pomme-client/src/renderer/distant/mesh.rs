//! Distant terrain sections as geometry: each run a box, faces another run
//! covers left out.

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
    /// Which way the face points ([`Face`]), so the shaders can tell
    /// whose column it is.
    pub face: u8,
    pub color: [u8; 4],
}

/// A face's direction, as `distant.vert` reads it.
#[derive(Clone, Copy)]
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

/// `colors` gives a run's top and side colour.
pub(crate) fn mesh_section(
    key: SectionKey,
    section: &Section,
    colors: impl Fn(RunKey) -> ([u8; 3], [u8; 3]),
) -> SectionMesh {
    let w = key.column_width() as u32;
    let mut vertices = Vec::new();
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
                let mut quad =
                    |corners, face, rgb| push_quad(&mut vertices, corners, light, face, rgb);
                let above_touches = i > 0 && runs[i - 1].bottom == run.top + 1;
                if !above_touches {
                    quad(
                        [[x0, top, z0], [x1, top, z0], [x1, top, z1], [x0, top, z1]],
                        Face::Up,
                        top_rgb,
                    );
                }
                let below_touches = runs
                    .get(i + 1)
                    .is_some_and(|below| below.top + 1 == run.bottom);
                if run.bottom > 0 && !below_touches {
                    quad(
                        [
                            [x0, bottom, z0],
                            [x0, bottom, z1],
                            [x1, bottom, z1],
                            [x1, bottom, z0],
                        ],
                        Face::Down,
                        top_rgb,
                    );
                }
                for (&(face, ..), neighbour) in SIDES.iter().zip(&neighbours) {
                    for (low, high) in uncovered(bottom, top, neighbour) {
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
                        quad(corners, face, side_rgb);
                    }
                }
            }
        }
    }
    SectionMesh {
        key,
        vertices,
        min_y: min_y.min(max_y),
        max_y,
    }
}

/// The parts of `[low, high)` none of `neighbour`'s runs cover, highest
/// first. A neighbour outside the section has no runs, so the section's
/// border faces always draw: a coarser or missing neighbour can't hide them.
fn uncovered(low: u32, high: u32, neighbour: &[Run]) -> Vec<(u32, u32)> {
    let mut open = Vec::new();
    let mut cursor = high;
    // Runs go top to bottom, so walk down.
    for run in neighbour {
        let (run_low, run_high) = (u32::from(run.bottom), u32::from(run.top) + 1);
        if run_low >= cursor {
            continue;
        }
        if run_high <= low {
            break;
        }
        if run_high < cursor {
            open.push((run_high, cursor));
        }
        cursor = run_low.max(low);
        if cursor <= low {
            return open;
        }
    }
    if cursor > low {
        open.push((low, cursor));
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
    vertices.extend(corners.map(|[x, y, z]| DistantVertex {
        xz: x | z << 16,
        y: y as u16,
        light,
        face: face as u8,
        color: [r, g, b, 255],
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::ChunkPos;
    use crate::renderer::distant::store::Store;

    fn run(top: u16, bottom: u16) -> Run {
        Run {
            key: RunKey {
                state: azalea_block::BlockState::AIR,
                biome: 0,
            },
            top,
            bottom,
            sky: 15,
            block: 0,
        }
    }

    #[test]
    fn a_neighbour_hides_the_side_it_covers() {
        // A 0..10 box beside a 0..4 one shows only 4..10.
        assert_eq!(uncovered(0, 10, &[run(3, 0)]), [(4, 10)]);
        // Beside a floating 4..6 run: above and below it.
        assert_eq!(uncovered(0, 10, &[run(5, 4)]), [(6, 10), (0, 4)]);
        // Beside a taller one: nothing.
        assert!(uncovered(2, 5, &[run(9, 0)]).is_empty());
        // Beside nothing, or a run wholly above or below: the whole side.
        assert_eq!(uncovered(2, 5, &[]), [(2, 5)]);
        assert_eq!(uncovered(2, 5, &[run(9, 7), run(0, 0)]), [(2, 5)]);
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
        let pair = |i| if i < 2 { vec![run(5, 0)] } else { Vec::new() };
        assert_eq!(quads(pair), 2 + 2 * 3);
        // A flat chunk: one top per column, and only the chunk's outer
        // sides, which stay because they're in the middle of nothing.
        assert_eq!(quads(|_| vec![run(5, 0)]), 256 + 4 * 16);
    }
}
