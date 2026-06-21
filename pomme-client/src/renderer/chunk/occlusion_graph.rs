//! Vanilla-style section visibility for cave culling. Each section computes a
//! 6x6 face-to-face connectivity set at mesh time (vanilla's `VisGraph` flood
//! fill); the per-frame occlusion walk (`SectionOcclusionGraph`) consumes it.

use std::collections::{HashMap, VecDeque};

use azalea_core::position::ChunkPos;
use glam::DVec3;

/// The six block faces, ordinals matching vanilla `Direction`
/// (DOWN, UP, NORTH, SOUTH, WEST, EAST).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Face {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

impl Face {
    pub const ALL: [Face; 6] = [
        Face::Down,
        Face::Up,
        Face::North,
        Face::South,
        Face::West,
        Face::East,
    ];

    /// Unit step toward this face in section-local space. The Y component is
    /// the step in section index (vertical sections).
    pub fn offset(self) -> (i32, i32, i32) {
        match self {
            Face::Down => (0, -1, 0),
            Face::Up => (0, 1, 0),
            Face::North => (0, 0, -1),
            Face::South => (0, 0, 1),
            Face::West => (-1, 0, 0),
            Face::East => (1, 0, 0),
        }
    }

    pub fn opposite(self) -> Face {
        match self {
            Face::Down => Face::Up,
            Face::Up => Face::Down,
            Face::North => Face::South,
            Face::South => Face::North,
            Face::West => Face::East,
            Face::East => Face::West,
        }
    }

    /// 0 = X, 1 = Y, 2 = Z.
    fn axis(self) -> u8 {
        match self {
            Face::West | Face::East => 0,
            Face::Down | Face::Up => 1,
            Face::North | Face::South => 2,
        }
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// Symmetric 6x6 face-to-face connectivity (36 bits): bit `a*6+b` set means a
/// sightline crosses the section between faces `a` and `b`. A fully-empty
/// section connects all faces; a fully-solid one connects none.
#[derive(Clone, Copy, Default)]
pub struct VisibilitySet(u64);

impl VisibilitySet {
    pub const fn none() -> Self {
        VisibilitySet(0)
    }

    pub const fn all() -> Self {
        VisibilitySet((1u64 << 36) - 1)
    }

    /// Whether a sightline crosses the section between faces `a` and `b`.
    pub fn visible_between(self, a: Face, b: Face) -> bool {
        self.0 >> (a as u32 * 6 + b as u32) & 1 != 0
    }

    /// Mark every pair of faces in `faces` (a 6-bit mask) mutually visible.
    fn add(&mut self, faces: u8) {
        for a in 0..6u8 {
            if faces & (1 << a) == 0 {
                continue;
            }
            for b in 0..6u8 {
                if faces & (1 << b) != 0 {
                    self.0 |= 1u64 << (a * 6 + b);
                }
            }
        }
    }
}

const fn idx(x: usize, y: usize, z: usize) -> usize {
    x + y * 16 + z * 256
}

/// Faces of the section that cell `(x, y, z)` lies on (its 0/15 boundary
/// planes).
fn edge_faces(x: usize, y: usize, z: usize) -> u8 {
    let mut f = 0u8;
    if x == 0 {
        f |= Face::West.bit();
    } else if x == 15 {
        f |= Face::East.bit();
    }
    if y == 0 {
        f |= Face::Down.bit();
    } else if y == 15 {
        f |= Face::Up.bit();
    }
    if z == 0 {
        f |= Face::North.bit();
    } else if z == 15 {
        f |= Face::South.bit();
    }
    f
}

/// Compute a section's `VisibilitySet` from its 16³ opacity grid via vanilla's
/// `VisGraph`: flood each connected non-opaque region seeded from the boundary
/// and connect every section face that region reaches.
pub fn compute_visibility(opaque: impl Fn(usize, usize, usize) -> bool) -> VisibilitySet {
    let mut blocked = [false; 4096];
    let mut opaque_count = 0usize;
    for z in 0..16usize {
        for y in 0..16usize {
            for x in 0..16usize {
                if opaque(x, y, z) {
                    blocked[idx(x, y, z)] = true;
                    opaque_count += 1;
                }
            }
        }
    }
    // VisGraph.resolve() short-circuits: sparse fill => every face connected,
    // fully solid => none.
    if opaque_count < 256 {
        return VisibilitySet::all();
    }
    if opaque_count == 4096 {
        return VisibilitySet::none();
    }

    let mut vis = VisibilitySet::none();
    let mut stack: Vec<usize> = Vec::new();
    for z in 0..16usize {
        for y in 0..16usize {
            for x in 0..16usize {
                if x != 0 && x != 15 && y != 0 && y != 15 && z != 0 && z != 15 {
                    continue;
                }
                let seed = idx(x, y, z);
                if blocked[seed] {
                    continue;
                }
                blocked[seed] = true;
                stack.clear();
                stack.push(seed);
                let mut faces = 0u8;
                while let Some(i) = stack.pop() {
                    let (cx, cy, cz) = (i & 15, i >> 4 & 15, i >> 8 & 15);
                    faces |= edge_faces(cx, cy, cz);
                    for face in Face::ALL {
                        let (dx, dy, dz) = face.offset();
                        let (nx, ny, nz) = (cx as i32 + dx, cy as i32 + dy, cz as i32 + dz);
                        if !(0..16).contains(&nx)
                            || !(0..16).contains(&ny)
                            || !(0..16).contains(&nz)
                        {
                            continue;
                        }
                        let ni = idx(nx as usize, ny as usize, nz as usize);
                        if blocked[ni] {
                            continue;
                        }
                        blocked[ni] = true;
                        stack.push(ni);
                    }
                }
                vis.add(faces);
            }
        }
    }
    vis
}

/// Vanilla advanced-culling thresholds: sections farther than this many
/// sections (any axis) also get the ray-march, which steps by one section
/// diagonal toward the camera and stops within 60 blocks (the near field is
/// covered by the walk).
const ADV_CULL_SECTION_DIST: i32 = 3;
const CEILED_SECTION_DIAGONAL: f64 = 28.0;
const ADV_CULL_MIN_DIST_SQ: f64 = 3600.0;

/// BFS node: the faces this section was entered from (`source`) and the cone of
/// travel directions taken to reach it (`cone`, for backtrack prevention).
struct Node {
    source: u8,
    cone: u8,
}

/// Vanilla's `SectionOcclusionGraph` walk: flood the section grid outward from
/// the camera section, stepping into a neighbor only when the path isn't a
/// backtrack and a sightline crosses the current section from an entered face
/// to the exit face. Returns each visible column's set of visible section
/// indices (bit `si`). Sections without computed visibility default to
/// see-through, so the result only ever under-culls (never hides geometry that
/// should show). Frustum culling is applied separately (on the GPU), so this is
/// occlusion only.
pub fn compute_visible_mask(
    section_vis: &HashMap<(ChunkPos, i32), VisibilitySet>,
    cam_col: ChunkPos,
    cam_si: i32,
    eye: DVec3,
    min_y: i32,
    section_count: i32,
    render_distance: i32,
) -> HashMap<ChunkPos, u32> {
    let mut visible: HashMap<ChunkPos, u32> = HashMap::new();
    let mut nodes: HashMap<(i32, i32, i32), Node> = HashMap::new();
    let mut queue: VecDeque<(i32, i32, i32)> = VecDeque::new();

    let cam_center = DVec3::new(
        (cam_col.x * 16 + 8) as f64,
        (min_y + cam_si * 16 + 8) as f64,
        (cam_col.z * 16 + 8) as f64,
    );
    let world_min_y = min_y as f64;
    let world_max_y = (min_y + section_count * 16) as f64;

    let start = (cam_col.x, cam_si.clamp(0, section_count - 1), cam_col.z);
    nodes.insert(start, Node { source: 0, cone: 0 });
    queue.push_back(start);
    visible.insert(ChunkPos::new(start.0, start.2), 1u32 << start.1);

    while let Some((x, si, z)) = queue.pop_front() {
        let Node { source, cone } = nodes[&(x, si, z)];
        let vis = section_vis
            .get(&(ChunkPos::new(x, z), si))
            .copied()
            .unwrap_or_else(VisibilitySet::all);
        let distant = (x - cam_col.x).abs() > ADV_CULL_SECTION_DIST
            || (si - cam_si).abs() > ADV_CULL_SECTION_DIST
            || (z - cam_col.z).abs() > ADV_CULL_SECTION_DIST;
        for face in Face::ALL {
            // Don't walk back the way we came (limits the search to a cone).
            if cone & face.opposite().bit() != 0 {
                continue;
            }
            // A sightline must cross this section from an entered face to `face`.
            // The start node (no source) may exit any direction.
            if source != 0
                && !Face::ALL
                    .into_iter()
                    .any(|sf| source & sf.bit() != 0 && vis.visible_between(sf.opposite(), face))
            {
                continue;
            }
            // Distant sections also get a coarse ray-march back to the camera: if a
            // step lands in a section the walk never reached, the line of sight is
            // blocked, so don't propagate.
            if distant
                && ray_occluded(
                    &nodes,
                    eye,
                    cam_center,
                    face,
                    (x, si, z),
                    min_y,
                    world_min_y,
                    world_max_y,
                )
            {
                continue;
            }
            let (dx, dy, dz) = face.offset();
            let (nx, nsi, nz) = (x + dx, si + dy, z + dz);
            if nsi < 0 || nsi >= section_count {
                continue;
            }
            if (nx - cam_col.x).abs() > render_distance || (nz - cam_col.z).abs() > render_distance
            {
                continue;
            }
            if let Some(existing) = nodes.get_mut(&(nx, nsi, nz)) {
                existing.source |= face.bit();
                continue;
            }
            nodes.insert(
                (nx, nsi, nz),
                Node {
                    source: face.bit(),
                    cone: cone | face.bit(),
                },
            );
            *visible.entry(ChunkPos::new(nx, nz)).or_insert(0) |= 1u32 << nsi;
            queue.push_back((nx, nsi, nz));
        }
    }
    visible
}

/// Step from the distant section's corner facing the camera toward the camera
/// by one section diagonal at a time; if a step lands in a section the walk
/// hasn't reached (occluded or unloaded), the line of sight is blocked.
#[allow(clippy::too_many_arguments)]
fn ray_occluded(
    nodes: &HashMap<(i32, i32, i32), Node>,
    eye: DVec3,
    cam_center: DVec3,
    face: Face,
    (sx, ssi, sz): (i32, i32, i32),
    min_y: i32,
    world_min_y: f64,
    world_max_y: f64,
) -> bool {
    let ox = (sx * 16) as f64;
    let oy = (min_y + ssi * 16) as f64;
    let oz = (sz * 16) as f64;
    // Per-axis corner the ray starts from (vanilla's advanced-cull pick).
    let corner = |axis: u8, c: f64, o: f64| {
        let max = if face.axis() == axis { c > o } else { c < o };
        o + if max { 16.0 } else { 0.0 }
    };
    let mut check = DVec3::new(
        corner(0, cam_center.x, ox),
        corner(1, cam_center.y, oy),
        corner(2, cam_center.z, oz),
    );
    let step = (eye - check).normalize() * CEILED_SECTION_DIAGONAL;
    while check.distance_squared(eye) > ADV_CULL_MIN_DIST_SQ {
        check += step;
        if check.y > world_max_y || check.y < world_min_y {
            return false;
        }
        let cx = (check.x / 16.0).floor() as i32;
        let csi = ((check.y - min_y as f64) / 16.0).floor() as i32;
        let cz = (check.z / 16.0).floor() as i32;
        if !nodes.contains_key(&(cx, csi, cz)) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_section_connects_all_faces() {
        let v = compute_visibility(|_, _, _| false);
        for a in Face::ALL {
            for b in Face::ALL {
                assert!(v.visible_between(a, b));
            }
        }
    }

    #[test]
    fn solid_section_connects_nothing() {
        let v = compute_visibility(|_, _, _| true);
        for a in Face::ALL {
            for b in Face::ALL {
                assert!(!v.visible_between(a, b));
            }
        }
    }

    #[test]
    fn wall_splits_west_from_east() {
        // Full YZ plane at x=8 (256 cells, so the flood path runs).
        let v = compute_visibility(|x, _, _| x == 8);
        assert!(!v.visible_between(Face::West, Face::East));
        assert!(v.visible_between(Face::West, Face::Up));
        assert!(v.visible_between(Face::East, Face::Up));
    }

    #[test]
    fn open_grid_reaches_every_section() {
        // No visibility data => every section is see-through; the walk must reach
        // every section within render distance (monotonic paths cover the box).
        // rd > the advanced-culling distance so the ray-march runs too — in open
        // space it must never false-cull.
        let section_vis = HashMap::new();
        let sc = 8;
        let rd = 6;
        let eye = DVec3::new(8.0, 72.0, 8.0);
        let mask = compute_visible_mask(&section_vis, ChunkPos::new(0, 0), 4, eye, 0, sc, rd);
        let full = (1u32 << sc) - 1;
        for x in -rd..=rd {
            for z in -rd..=rd {
                assert_eq!(
                    mask.get(&ChunkPos::new(x, z)).copied().unwrap_or(0),
                    full,
                    "column ({x}, {z}) not fully reached"
                );
            }
        }
    }

    #[test]
    fn solid_wall_hides_sections_behind_it() {
        // A solid section directly north of the camera should block the sections
        // beyond it (same row) from being reached.
        let mut section_vis = HashMap::new();
        section_vis.insert((ChunkPos::new(0, -1), 4), VisibilitySet::none());
        let eye = DVec3::new(8.0, 72.0, 8.0);
        let mask = compute_visible_mask(&section_vis, ChunkPos::new(0, 0), 4, eye, 0, 8, 4);
        // The wall section itself is reached (visible face)...
        assert!(mask.get(&ChunkPos::new(0, -1)).copied().unwrap_or(0) & (1 << 4) != 0);
        // ...but the section directly behind it on the same row/height is not.
        assert!(mask.get(&ChunkPos::new(0, -2)).copied().unwrap_or(0) & (1 << 4) == 0);
    }
}
