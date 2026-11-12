//! Vanilla-style section visibility for cave culling. Each section computes a
//! 6x6 face-to-face connectivity set at mesh time (vanilla's `VisGraph` flood
//! fill); the per-frame occlusion walk (`SectionOcclusionGraph`) consumes it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use glam::DVec3;

use super::buffer::column_is_near;
use crate::position::ChunkPos;

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
#[derive(Clone, Copy)]
struct Node {
    source: u8,
    cone: u8,
}

/// The walk's nodes and each column's visible sections, in grids over the
/// render distance around the camera. The walk reads and writes a node per
/// step and per ray-march sample, which made a map of them its main cost.
struct WalkGrid {
    /// The grid's lowest column.
    corner: (i32, i32),
    /// Columns along each side.
    side: i32,
    section_count: i32,
    nodes: Vec<Option<Node>>,
    visible: Vec<u32>,
}

impl WalkGrid {
    fn new(cam_col: ChunkPos, render_distance: i32, section_count: i32) -> Self {
        let side = 2 * render_distance + 1;
        let columns = (side * side) as usize;
        Self {
            corner: (cam_col.x - render_distance, cam_col.z - render_distance),
            side,
            section_count,
            nodes: vec![None; columns * section_count as usize],
            visible: vec![0; columns],
        }
    }

    /// The section's slot in `nodes`, or `None` outside the grid.
    fn section(&self, (x, si, z): (i32, i32, i32)) -> Option<usize> {
        let (dx, dz) = (x - self.corner.0, z - self.corner.1);
        let inside = (0..self.side).contains(&dx)
            && (0..self.side).contains(&dz)
            && (0..self.section_count).contains(&si);
        inside.then(|| ((dx * self.side + dz) * self.section_count + si) as usize)
    }

    fn reached(&self, section: (i32, i32, i32)) -> bool {
        self.section(section)
            .is_some_and(|slot| self.nodes[slot].is_some())
    }

    /// Reaches the section at `slot` from the camera, visible from then on.
    fn insert(&mut self, slot: usize, node: Node) {
        self.nodes[slot] = Some(node);
        let sections = self.section_count as usize;
        self.visible[slot / sections] |= 1 << (slot % sections);
    }

    fn into_visible(self) -> HashMap<ChunkPos, u32> {
        let Self { corner, side, .. } = self;
        self.visible
            .into_iter()
            .enumerate()
            .filter(|&(_, mask)| mask != 0)
            .map(|(column, mask)| {
                let (dx, dz) = (column as i32 / side, column as i32 % side);
                (ChunkPos::new(corner.0 + dx, corner.1 + dz), mask)
            })
            .collect()
    }
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
    let mut grid = WalkGrid::new(cam_col, render_distance, section_count);
    let mut queue: VecDeque<(i32, i32, i32)> = VecDeque::new();

    let cam_center = DVec3::new(
        (cam_col.x * 16 + 8) as f64,
        (min_y + cam_si * 16 + 8) as f64,
        (cam_col.z * 16 + 8) as f64,
    );
    let world_min_y = min_y as f64;
    let world_max_y = (min_y + section_count * 16) as f64;

    let start = (cam_col.x, cam_si.clamp(0, section_count - 1), cam_col.z);
    let start_slot = grid.section(start).expect("the camera is in the grid");
    grid.insert(start_slot, Node { source: 0, cone: 0 });
    queue.push_back(start);

    while let Some((x, si, z)) = queue.pop_front() {
        let slot = grid.section((x, si, z)).expect("queued from the grid");
        let Node { source, cone } = grid.nodes[slot].expect("queued once reached");
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
                    &grid,
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
            let next = (x + dx, si + dy, z + dz);
            // Outside the world's height or the render distance.
            let Some(next_slot) = grid.section(next) else {
                continue;
            };
            if let Some(existing) = &mut grid.nodes[next_slot] {
                existing.source |= face.bit();
                continue;
            }
            grid.insert(
                next_slot,
                Node {
                    source: face.bit(),
                    cone: cone | face.bit(),
                },
            );
            queue.push_back(next);
        }
    }
    grid.into_visible()
}

/// Step from the distant section's corner facing the camera toward the camera
/// by one section diagonal at a time; if a step lands in a section the walk
/// hasn't reached (occluded or unloaded), the line of sight is blocked.
#[allow(clippy::too_many_arguments)]
fn ray_occluded(
    grid: &WalkGrid,
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
        if !grid.reached((cx, csi, cz)) {
            return true;
        }
    }
    false
}

/// Extra FOV (radians) for the tier-1 "about to be seen" margin frustum, so
/// small camera turns reveal already-meshed terrain instead of a meshing
/// curtain.
pub const VIS_MARGIN_RADIANS: f32 = 0.6;

/// Frustum tier for a column: 0 in view, 1 in the dilated margin, 2 behind the
/// camera. (Nearby columns are forced to 0 by the caller.)
fn column_frustum_tier(
    pos: ChunkPos,
    eye: DVec3,
    planes: &[[f32; 4]; 6],
    planes_wide: &[[f32; 4]; 6],
    min_y: f32,
    max_y: f32,
) -> u8 {
    // Camera-relative full-height column box, matching how the GPU cull
    // subtracts the eye before its plane test (cull.comp); f64 first for
    // precision at extreme coordinates.
    let dx = (pos.x as f64 * 16.0 - eye.x) as f32;
    let dz = (pos.z as f64 * 16.0 - eye.z) as f32;
    let mn = [dx, (min_y as f64 - eye.y) as f32, dz];
    let mx = [dx + 16.0, (max_y as f64 - eye.y) as f32, dz + 16.0];
    if aabb_in_frustum(&mn, &mx, planes) {
        0
    } else if aabb_in_frustum(&mn, &mx, planes_wide) {
        1
    } else {
        2
    }
}

/// Full mask for an `n`-section column (bits `0..n` set).
pub fn section_mask(n: i32) -> u32 {
    if n >= 32 { u32::MAX } else { (1u32 << n) - 1 }
}

/// Conservative AABB-vs-frustum test (the dominant-corner max-dot used by
/// `cull.comp`): true unless the box is fully behind some plane.
fn aabb_in_frustum(mn: &[f32; 3], mx: &[f32; 3], planes: &[[f32; 4]; 6]) -> bool {
    for p in planes {
        let d = p[0] * if p[0] >= 0.0 { mx[0] } else { mn[0] }
            + p[1] * if p[1] >= 0.0 { mx[1] } else { mn[1] }
            + p[2] * if p[2] >= 0.0 { mx[2] } else { mn[2] }
            + p[3];
        if d < 0.0 {
            return false;
        }
    }
    true
}

/// What the walk thread is told about the world: a section's visibility set
/// from its mesh, a column loading or unloading, the meshes or the level gone.
pub enum Update {
    Section {
        pos: ChunkPos,
        si: i32,
        vis: VisibilitySet,
    },
    Load(ChunkPos),
    Unload {
        pos: ChunkPos,
        sections: i32,
    },
    /// The meshes went: every section's set goes, the columns stay loaded.
    ClearSections,
    /// The level went, columns and all.
    Clear,
}

/// One walk from the camera, applied against the frustum it was asked with.
pub struct Walk {
    /// Matched against the result's, so a walk asked before the level was
    /// cleared is dropped.
    pub generation: u64,
    pub cam_col: ChunkPos,
    pub eye: DVec3,
    pub planes: [[f32; 4]; 6],
    pub planes_wide: [[f32; 4]; 6],
    pub min_y: i32,
    pub section_count: i32,
    /// Whether occluded sections are left out of what the renderer draws.
    pub occlusion: bool,
}

/// A walk's outcome, per loaded column.
pub struct Visibility {
    pub generation: u64,
    /// What the renderer draws: bit `si` set for a section not occluded, or
    /// every section with occlusion off.
    pub draw: Arc<HashMap<ChunkPos, u32>>,
    /// The in-frustum columns with a visible section, and their masks.
    pub in_view: HashMap<ChunkPos, u32>,
}

impl Walk {
    fn run(
        &self,
        section_vis: &HashMap<(ChunkPos, i32), VisibilitySet>,
        columns: &HashSet<ChunkPos>,
    ) -> Visibility {
        let cam_si = ((self.eye.y - self.min_y as f64) / 16.0).floor() as i32;
        // Bound the walk by the loaded radius (a server can stream terrain
        // past the client render distance).
        let rd = columns
            .iter()
            .map(|p| {
                (p.x - self.cam_col.x)
                    .abs()
                    .max((p.z - self.cam_col.z).abs())
            })
            .max()
            .unwrap_or(0);
        let bfs = compute_visible_mask(
            section_vis,
            self.cam_col,
            cam_si,
            self.eye,
            self.min_y,
            self.section_count,
            rd,
        );
        let min_y = self.min_y as f32;
        let max_y = min_y + (self.section_count * 16) as f32;
        let full = section_mask(self.section_count);
        let mut in_view = HashMap::new();
        let mut masks = HashMap::with_capacity(columns.len());
        for &pos in columns {
            let near = column_is_near(pos, self.eye);
            // Near columns always draw fully; otherwise a column draws only the
            // sections the graph proved occlusion-visible (none => fully hidden).
            let mask = if near {
                full
            } else {
                bfs.get(&pos).copied().unwrap_or(0)
            };
            if mask != 0
                && (near
                    || column_frustum_tier(
                        pos,
                        self.eye,
                        &self.planes,
                        &self.planes_wide,
                        min_y,
                        max_y,
                    ) == 0)
            {
                in_view.insert(pos, mask);
            }
            masks.insert(pos, mask);
        }
        let draw = if self.occlusion {
            Arc::new(masks)
        } else {
            Arc::new(columns.iter().map(|&pos| (pos, full)).collect())
        };
        Visibility {
            generation: self.generation,
            draw,
            in_view,
        }
    }
}

enum Message {
    Update(Update),
    Walk(Box<Walk>),
}

/// The thread that owns the section visibility sets and walks them. Updates
/// and walks arrive in order, so a walk sees every update sent before it,
/// and the game thread never copies the sets. Vanilla's
/// `SectionOcclusionGraph` runs its walk on a thread the same way.
pub struct Walker {
    tx: Sender<Message>,
    rx: Receiver<Visibility>,
}

/// The thread only ends early by panicking; the game goes down with it.
const THREAD_GONE: &str = "the chunk occlusion thread panicked";

impl Walker {
    pub fn spawn() -> Self {
        let (tx, messages) = crossbeam_channel::unbounded();
        let (results, rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("chunk-occlusion".into())
            .spawn(move || run(&messages, &results))
            .expect("failed to spawn the chunk occlusion thread");
        Self { tx, rx }
    }

    pub fn update(&self, update: Update) {
        self.tx.send(Message::Update(update)).expect(THREAD_GONE);
    }

    pub fn walk(&self, walk: Walk) {
        self.tx
            .send(Message::Walk(Box::new(walk)))
            .expect(THREAD_GONE);
    }

    pub fn try_result(&self) -> Option<Visibility> {
        crate::util::try_recv_live(&self.rx, THREAD_GONE)
    }
}

/// The walk thread's view of the world.
#[derive(Default)]
struct WalkState {
    section_vis: HashMap<(ChunkPos, i32), VisibilitySet>,
    columns: HashSet<ChunkPos>,
}

impl WalkState {
    fn apply(&mut self, update: Update) {
        match update {
            Update::Section { pos, si, vis } => {
                self.section_vis.insert((pos, si), vis);
            }
            Update::Load(pos) => {
                self.columns.insert(pos);
            }
            Update::Unload { pos, sections } => {
                self.columns.remove(&pos);
                for si in 0..sections {
                    self.section_vis.remove(&(pos, si));
                }
            }
            Update::ClearSections => self.section_vis.clear(),
            Update::Clear => {
                self.section_vis.clear();
                self.columns.clear();
            }
        }
    }

    fn walk(&self, walk: &Walk) -> Visibility {
        walk.run(&self.section_vis, &self.columns)
    }
}

/// Ends when the game drops its [`Walker`].
fn run(messages: &Receiver<Message>, results: &Sender<Visibility>) {
    let mut state = WalkState::default();
    for message in messages {
        match message {
            Message::Update(update) => state.apply(update),
            Message::Walk(walk) => {
                if results.send(state.walk(&walk)).is_err() {
                    break;
                }
            }
        }
    }
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

    fn far_walk() -> Walk {
        // Every plane passes everything, so only occlusion decides.
        let open = [[0.0, 0.0, 0.0, 1.0]; 6];
        Walk {
            generation: 0,
            cam_col: ChunkPos::new(0, 0),
            eye: DVec3::new(8.0, 72.0, 8.0),
            planes: open,
            planes_wide: open,
            min_y: 0,
            section_count: 8,
            occlusion: true,
        }
    }

    #[test]
    #[should_panic(expected = "occlusion thread panicked")]
    fn a_dead_thread_takes_the_game_down() {
        let (tx, _messages) = crossbeam_channel::unbounded();
        let (results, rx) = crossbeam_channel::unbounded::<Visibility>();
        drop(results);
        Walker { tx, rx }.try_result();
    }

    #[test]
    fn clearing_sections_keeps_loaded_columns() {
        let pos = ChunkPos::new(6, 0);
        let mut state = WalkState::default();
        state.apply(Update::Load(ChunkPos::new(0, 0)));
        state.apply(Update::Load(pos));
        state.apply(Update::Section {
            pos,
            si: 4,
            vis: VisibilitySet::none(),
        });
        state.apply(Update::ClearSections);
        let vis = state.walk(&far_walk());
        assert_eq!(vis.draw.get(&pos), Some(&section_mask(8)));

        state.apply(Update::Clear);
        assert!(state.walk(&far_walk()).draw.is_empty());
    }
}
