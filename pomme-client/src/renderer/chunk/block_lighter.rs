//! Vanilla `BlockModelLighter`: each terrain quad's per-vertex light
//! coordinates and shade, smooth (`prepareQuadAmbientOcclusion`) or flat
//! (`prepareQuadFlat`), with `LightCoordsUtil`'s blending.

use crate::physics::block_shape::is_collision_shape_full_block;
use crate::world::block::model::{Direction, quad_bounds};
use crate::world::block::{BlockStateData, BlockStateTable};
use crate::world::chunk::ChunkStore;

/// What the lighter reads from the world around the block it lights.
pub(crate) trait LightSource {
    fn state_data(&self, at: [i32; 3]) -> &BlockStateData;
    /// The light engine's sky and block levels at a position.
    fn levels(&self, at: [i32; 3]) -> LightLevels;
}

/// `at` moved by `by`.
pub(super) fn offset(at: [i32; 3], by: [i32; 3]) -> [i32; 3] {
    [at[0] + by[0], at[1] + by[1], at[2] + by[2]]
}

/// The live world, for light read outside the mesher's snapshot.
pub(crate) struct WorldLight<'a> {
    pub chunks: &'a ChunkStore,
    pub table: BlockStateTable,
}

impl LightSource for WorldLight<'_> {
    fn state_data(&self, [x, y, z]: [i32; 3]) -> &BlockStateData {
        self.table
            .block_state_data(self.chunks.get_block_state(x, y, z))
    }

    fn levels(&self, [x, y, z]: [i32; 3]) -> LightLevels {
        LightLevels::new(
            self.chunks.get_sky_light(x, y, z),
            self.chunks.get_block_light(x, y, z),
        )
    }
}

/// A block's sky and block light levels, packed `sky << 4 | block`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LightLevels(pub u8);

impl LightLevels {
    pub const FULL_BRIGHT: Self = Self::new(15, 15);

    pub const fn new(sky: u8, block: u8) -> Self {
        Self(sky << 4 | block)
    }

    pub fn block(self) -> u8 {
        self.0 & 0xF
    }

    pub fn sky(self) -> u8 {
        self.0 >> 4
    }

    /// Vanilla `LightCoordsUtil.max`: each channel's brighter level.
    pub fn max(self, other: Self) -> Self {
        Self::new(self.sky().max(other.sky()), self.block().max(other.block()))
    }

    /// Vanilla `lightCoordsWithEmission`: both channels raised to a quad's
    /// own `light_emission`.
    pub fn with_emission(self, emission: u8) -> Self {
        if emission == 0 {
            return self;
        }
        Self::new(self.sky().max(emission), self.block().max(emission))
    }

    /// The shaders' light lane (`light_lane_color`): vanilla's UV2, block
    /// and sky light coordinates in 0..240, as `block + sky * 256`.
    pub fn lane(self) -> f32 {
        f32::from(self.block()) * 16.0 + f32::from(self.sky()) * 16.0 * 256.0
    }
}

/// A terrain vertex's light, as vanilla's BLOCK vertex carries it: the colour
/// factor (ambient occlusion × face shade) and the smooth block and sky light
/// coordinates, each 0..240.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct VertexLight {
    pub shade: f32,
    pub block: u8,
    pub sky: u8,
}

impl VertexLight {
    /// A flat-lit vertex: `levels` at face shade `shade`.
    pub fn flat(shade: f32, levels: LightLevels) -> Self {
        Self {
            shade,
            block: levels.block() * 16,
            sky: levels.sky() * 16,
        }
    }

    /// What the quad's diagonal balances.
    pub fn brightness(self) -> f32 {
        self.shade * f32::from(self.block.max(self.sky))
    }
}

/// The quad being lit, in block-local coordinates.
pub(super) struct Quad<'a> {
    pub positions: &'a [[f32; 3]; 4],
    /// Vanilla `BakedQuad.direction()`.
    pub direction: Direction,
    /// The face whose cardinal shade applies; `None` for `shade: false`.
    pub shade_face: Option<Direction>,
    /// The element's `light_emission`.
    pub emission: u8,
}

/// The light at a position, by the state there.
pub(crate) fn light_coords(world: &impl LightSource, at: [i32; 3]) -> LightLevels {
    light_coords_of(world.state_data(at), world, at)
}

/// Vanilla `LightCoordsUtil.getLightCoords`: full bright when `data` renders
/// emissive, else the light engine's levels at the position with block light
/// raised to `data`'s emission. The flat path passes the lit block's own state
/// with its neighbour's position.
pub(super) fn light_coords_of(
    data: &BlockStateData,
    world: &impl LightSource,
    at: [i32; 3],
) -> LightLevels {
    if emissive_rendering(data) {
        return LightLevels::FULL_BRIGHT;
    }
    let levels = world.levels(at);
    let emission = data.light_props().emission;
    if levels.block() < emission {
        LightLevels::new(levels.sky(), emission)
    } else {
        levels
    }
}

/// Vanilla `Properties.emissiveRendering` predicates (`Blocks`): magma blocks,
/// and sculk sensors while active.
pub(super) fn emissive_rendering(data: &BlockStateData) -> bool {
    match data.id() {
        "magma_block" => true,
        "sculk_sensor" | "calibrated_sculk_sensor" => {
            data.properties().get("sculk_sensor_phase") == Some("active")
        }
        _ => false,
    }
}

/// Vanilla `getShadeBrightness`: 0.2 for a full collision cube, with the
/// blocks that override it.
pub(super) fn shade_brightness(data: &BlockStateData) -> f32 {
    let id = data.id();
    match id {
        "barrier" | "light" | "structure_void" | "glass" | "tinted_glass" => 1.0,
        _ if id.ends_with("stained_glass") || id.ends_with("copper_grate") => 1.0,
        "mud" | "soul_sand" => 0.2,
        "snow" => {
            if data.properties().get("layers") == Some("8") {
                0.2
            } else {
                1.0
            }
        }
        _ if is_collision_shape_full_block(data) => 0.2,
        _ => 1.0,
    }
}

/// Vanilla `isSolidRender`: an occluding state whose shape fills the cell.
pub(super) fn is_solid_render(data: &BlockStateData) -> bool {
    data.light_props().can_occlude && data.outline_boxes().is_none()
}

/// Vanilla `isLightPermeable`.
fn is_light_permeable(data: &BlockStateData) -> bool {
    !is_solid_render(data) || data.light_props().dampening == 0
}

/// Vanilla `SizeInfo`'s indices into the face shape.
#[derive(Clone, Copy)]
enum Size {
    Down,
    Up,
    North,
    South,
    West,
    East,
    FlipDown,
    FlipUp,
    FlipNorth,
    FlipSouth,
    FlipWest,
    FlipEast,
}

/// Vanilla `prepareQuadShape`: whether the face lies on the block's boundary
/// (or the block is a full collision cube), whether it covers less than the
/// whole face, and its bounds for the partial-face weights.
struct Shape {
    cubic: bool,
    partial: bool,
    bounds: [f32; 12],
}

impl Shape {
    fn new(quad: &Quad, data: &BlockStateData) -> Self {
        let ([min_x, min_y, min_z], [max_x, max_y, max_z]) = quad_bounds(quad.positions);
        const MIN: f32 = 1.0e-4;
        const MAX: f32 = 0.9999;
        let partial = match quad.direction {
            Direction::Down | Direction::Up => {
                min_x >= MIN || min_z >= MIN || max_x <= MAX || max_z <= MAX
            }
            Direction::North | Direction::South => {
                min_x >= MIN || min_y >= MIN || max_x <= MAX || max_y <= MAX
            }
            Direction::West | Direction::East => {
                min_y >= MIN || min_z >= MIN || max_y <= MAX || max_z <= MAX
            }
        };
        let full = || is_collision_shape_full_block(data);
        let cubic = match quad.direction {
            Direction::Down => min_y == max_y && (min_y < MIN || full()),
            Direction::Up => min_y == max_y && (max_y > MAX || full()),
            Direction::North => min_z == max_z && (min_z < MIN || full()),
            Direction::South => min_z == max_z && (max_z > MAX || full()),
            Direction::West => min_x == max_x && (min_x < MIN || full()),
            Direction::East => min_x == max_x && (max_x > MAX || full()),
        };
        let mut bounds = [0.0; 12];
        for (size, value) in [
            (Size::West, min_x),
            (Size::East, max_x),
            (Size::Down, min_y),
            (Size::Up, max_y),
            (Size::North, min_z),
            (Size::South, max_z),
            (Size::FlipWest, 1.0 - min_x),
            (Size::FlipEast, 1.0 - max_x),
            (Size::FlipDown, 1.0 - min_y),
            (Size::FlipUp, 1.0 - max_y),
            (Size::FlipNorth, 1.0 - min_z),
            (Size::FlipSouth, 1.0 - max_z),
        ] {
            bounds[size as usize] = value;
        }
        Self {
            cubic,
            partial,
            bounds,
        }
    }

    fn weight(&self, [a, b]: [Size; 2]) -> f32 {
        self.bounds[a as usize] * self.bounds[b as usize]
    }
}

/// Vanilla `AdjacencyInfo`: the face's four edge neighbours and, per vanilla
/// vertex, the bound pairs weighting a partial face.
struct Adjacency {
    corners: [Direction; 4],
    weights: [[[Size; 2]; 4]; 4],
}

fn adjacency(direction: Direction) -> Adjacency {
    use Direction as D;
    use Size::*;
    let (corners, weights) = match direction {
        D::Down => (
            [D::West, D::East, D::North, D::South],
            [
                [
                    [FlipWest, South],
                    [FlipWest, FlipSouth],
                    [West, FlipSouth],
                    [West, South],
                ],
                [
                    [FlipWest, North],
                    [FlipWest, FlipNorth],
                    [West, FlipNorth],
                    [West, North],
                ],
                [
                    [FlipEast, North],
                    [FlipEast, FlipNorth],
                    [East, FlipNorth],
                    [East, North],
                ],
                [
                    [FlipEast, South],
                    [FlipEast, FlipSouth],
                    [East, FlipSouth],
                    [East, South],
                ],
            ],
        ),
        D::Up => (
            [D::East, D::West, D::North, D::South],
            [
                [
                    [East, South],
                    [East, FlipSouth],
                    [FlipEast, FlipSouth],
                    [FlipEast, South],
                ],
                [
                    [East, North],
                    [East, FlipNorth],
                    [FlipEast, FlipNorth],
                    [FlipEast, North],
                ],
                [
                    [West, North],
                    [West, FlipNorth],
                    [FlipWest, FlipNorth],
                    [FlipWest, North],
                ],
                [
                    [West, South],
                    [West, FlipSouth],
                    [FlipWest, FlipSouth],
                    [FlipWest, South],
                ],
            ],
        ),
        D::North => (
            [D::Up, D::Down, D::East, D::West],
            [
                [
                    [Up, FlipWest],
                    [Up, West],
                    [FlipUp, West],
                    [FlipUp, FlipWest],
                ],
                [
                    [Up, FlipEast],
                    [Up, East],
                    [FlipUp, East],
                    [FlipUp, FlipEast],
                ],
                [
                    [Down, FlipEast],
                    [Down, East],
                    [FlipDown, East],
                    [FlipDown, FlipEast],
                ],
                [
                    [Down, FlipWest],
                    [Down, West],
                    [FlipDown, West],
                    [FlipDown, FlipWest],
                ],
            ],
        ),
        D::South => (
            [D::West, D::East, D::Down, D::Up],
            [
                [
                    [Up, FlipWest],
                    [FlipUp, FlipWest],
                    [FlipUp, West],
                    [Up, West],
                ],
                [
                    [Down, FlipWest],
                    [FlipDown, FlipWest],
                    [FlipDown, West],
                    [Down, West],
                ],
                [
                    [Down, FlipEast],
                    [FlipDown, FlipEast],
                    [FlipDown, East],
                    [Down, East],
                ],
                [
                    [Up, FlipEast],
                    [FlipUp, FlipEast],
                    [FlipUp, East],
                    [Up, East],
                ],
            ],
        ),
        D::West => (
            [D::Up, D::Down, D::North, D::South],
            [
                [
                    [Up, South],
                    [Up, FlipSouth],
                    [FlipUp, FlipSouth],
                    [FlipUp, South],
                ],
                [
                    [Up, North],
                    [Up, FlipNorth],
                    [FlipUp, FlipNorth],
                    [FlipUp, North],
                ],
                [
                    [Down, North],
                    [Down, FlipNorth],
                    [FlipDown, FlipNorth],
                    [FlipDown, North],
                ],
                [
                    [Down, South],
                    [Down, FlipSouth],
                    [FlipDown, FlipSouth],
                    [FlipDown, South],
                ],
            ],
        ),
        D::East => (
            [D::Down, D::Up, D::North, D::South],
            [
                [
                    [FlipDown, South],
                    [FlipDown, FlipSouth],
                    [Down, FlipSouth],
                    [Down, South],
                ],
                [
                    [FlipDown, North],
                    [FlipDown, FlipNorth],
                    [Down, FlipNorth],
                    [Down, North],
                ],
                [
                    [FlipUp, North],
                    [FlipUp, FlipNorth],
                    [Up, FlipNorth],
                    [Up, North],
                ],
                [
                    [FlipUp, South],
                    [FlipUp, FlipSouth],
                    [Up, FlipSouth],
                    [Up, South],
                ],
            ],
        ),
    };
    Adjacency { corners, weights }
}

/// Vanilla `AmbientVertexRemap`: the quad vertex each computed vertex goes to.
fn remap(direction: Direction) -> [usize; 4] {
    match direction {
        Direction::Down | Direction::South => [0, 1, 2, 3],
        Direction::Up => [2, 3, 0, 1],
        Direction::North | Direction::West => [3, 0, 1, 2],
        Direction::East => [1, 2, 3, 0],
    }
}

/// Smooth light coordinates, each channel 0..240.
#[derive(Clone, Copy)]
struct Smooth {
    block: f32,
    sky: f32,
}

impl Smooth {
    /// Vanilla `lightCoordsWithEmission` on smooth coordinates: an emitting
    /// quad's channels become whole levels, raised to its emission.
    fn with_emission(self, emission: u8) -> Self {
        if emission == 0 {
            return self;
        }
        let level = |v: f32| ((v as u8 >> 4).max(emission) * 16) as f32;
        Self {
            block: level(self.block),
            sky: level(self.sky),
        }
    }
}

/// Vanilla `LightCoordsUtil.smoothBlend`: around a lit centre, a neighbour
/// that's dark (both channels 0) takes the centre's light, and one without
/// sky light takes the centre's sky; then the four average.
fn smooth_blend(neighbours: [LightLevels; 3], center: LightLevels) -> Smooth {
    let lit = center.sky() > 2 || center.block() > 2;
    let mut block = u32::from(center.block());
    let mut sky = u32::from(center.sky());
    for mut n in neighbours {
        if lit {
            if n.0 == 0 {
                n = center;
            } else if n.sky() == 0 {
                n = LightLevels::new(center.sky(), n.block());
            }
        }
        block += u32::from(n.block());
        sky += u32::from(n.sky());
    }
    // Each level is 16 coordinate units; `>> 2` averages the four.
    Smooth {
        block: ((block * 16) >> 2) as f32,
        sky: ((sky * 16) >> 2) as f32,
    }
}

/// Vanilla `prepareQuadAmbientOcclusion`; `cardinal` is the face shade
/// (`getDirectionalBrightness`).
pub(super) fn ambient_occlusion(
    world: &impl LightSource,
    cardinal: impl Fn(Option<Direction>) -> f32,
    pos: [i32; 3],
    quad: &Quad,
) -> [VertexLight; 4] {
    let shape = Shape::new(quad, world.state_data(pos));
    let dir = quad.direction.offset();
    let next = offset(pos, dir);
    let base = if shape.cubic { next } else { pos };
    let info = adjacency(quad.direction);
    let corner = info.corners.map(|d| d.offset());
    let sample = |at: [i32; 3]| {
        (
            light_coords(world, at),
            shade_brightness(world.state_data(at)),
        )
    };
    let edges = corner.map(|c| sample(offset(base, c)));
    let permeable =
        corner.map(|c| is_light_permeable(world.state_data(offset(offset(base, c), dir))));
    // Vanilla falls back to edge 0 for every unreachable diagonal.
    let diagonal = |a: usize, b: usize| {
        if permeable[b] || permeable[a] {
            sample(offset(offset(base, corner[a]), corner[b]))
        } else {
            edges[0]
        }
    };
    let c02 = diagonal(0, 2);
    let c03 = diagonal(0, 3);
    let c12 = diagonal(1, 2);
    let c13 = diagonal(1, 3);

    let center = if shape.cubic || !is_solid_render(world.state_data(next)) {
        next
    } else {
        pos
    };
    let light_center = light_coords(world, center);
    let shade_center = shade_brightness(world.state_data(base));

    // Vanilla's vertices 0..3: edges (3, 0), (2, 0), (2, 1), (3, 1) and the
    // diagonal between each pair.
    let corners = [
        (edges[3], edges[0], c03),
        (edges[2], edges[0], c02),
        (edges[2], edges[1], c12),
        (edges[3], edges[1], c13),
    ];
    let shades = corners.map(|(a, b, c)| (a.1 + b.1 + c.1 + shade_center) * 0.25);
    let lights = corners
        .map(|(a, b, c)| smooth_blend([a.0, b.0, c.0], light_center).with_emission(quad.emission));
    let face_shade = cardinal(quad.shade_face);
    let mut out = [VertexLight::flat(0.0, LightLevels(0)); 4];
    for (vertex, &target) in remap(quad.direction).iter().enumerate() {
        let (shade, light) = if shape.partial {
            let weights = info.weights[vertex].map(|pair| shape.weight(pair));
            let weighted = |value: &dyn Fn(usize) -> f32| -> f32 {
                (0..4).map(|i| value(i) * weights[i]).sum()
            };
            (
                weighted(&|i| shades[i]).clamp(0.0, 1.0),
                Smooth {
                    block: weighted(&|i| lights[i].block).trunc(),
                    sky: weighted(&|i| lights[i].sky).trunc(),
                },
            )
        } else {
            (shades[vertex], lights[vertex])
        };
        out[target] = VertexLight {
            shade: shade * face_shade,
            block: light.block as u8,
            sky: light.sky as u8,
        };
    }
    out
}

/// Vanilla `prepareQuadFlat`: `culled` is a culled quad's light (see
/// [`light_coords_of`] at its neighbour); an unculled quad reads its neighbour
/// when it lies on the block's boundary, else the block itself.
pub(super) fn flat(
    world: &impl LightSource,
    cardinal: impl Fn(Option<Direction>) -> f32,
    pos: [i32; 3],
    quad: &Quad,
    culled: Option<LightLevels>,
) -> [VertexLight; 4] {
    let state = world.state_data(pos);
    let levels = culled.unwrap_or_else(|| {
        let at = if Shape::new(quad, state).cubic {
            offset(pos, quad.direction.offset())
        } else {
            pos
        };
        light_coords_of(state, world, at)
    });
    [VertexLight::flat(
        cardinal(quad.shade_face),
        levels.with_emission(quad.emission),
    ); 4]
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::world::block::BlockStateTable;

    /// A block name and its properties.
    type Block = (&'static str, Vec<(&'static str, &'static str)>);

    /// A world of named blocks, air elsewhere, and their light levels.
    struct World {
        table: BlockStateTable,
        blocks: HashMap<[i32; 3], Block>,
        light: HashMap<[i32; 3], LightLevels>,
        default_light: LightLevels,
    }

    impl World {
        fn new(default_light: LightLevels) -> Self {
            Self {
                table: BlockStateTable::native(),
                blocks: HashMap::new(),
                light: HashMap::new(),
                default_light,
            }
        }

        fn set(&mut self, at: [i32; 3], name: &'static str) {
            self.blocks.insert(at, (name, Vec::new()));
            self.light.insert(at, LightLevels(0));
        }
    }

    impl LightSource for World {
        fn state_data(&self, at: [i32; 3]) -> &BlockStateData {
            let (name, props) = self.blocks.get(&at).cloned().unwrap_or(("air", Vec::new()));
            self.table
                .block_state_data(self.table.find_state(name, &props))
        }

        fn levels(&self, at: [i32; 3]) -> LightLevels {
            self.light.get(&at).copied().unwrap_or(self.default_light)
        }
    }

    fn up_face() -> [[f32; 3]; 4] {
        crate::world::block::model::face_positions(Direction::Up, [0.0; 3], [1.0; 3])
    }

    fn quad(positions: &[[f32; 3]; 4], direction: Direction) -> Quad<'_> {
        Quad {
            positions,
            direction,
            shade_face: Some(direction),
            emission: 0,
        }
    }

    #[test]
    fn a_wall_beside_a_lit_floor_takes_the_floors_light() {
        // A stone floor at y 0 in torchlight, a stone wall along x 1 whose
        // inside is dark: `smoothBlend` swaps the wall's zeros for the centre.
        let mut world = World::new(LightLevels::new(0, 14));
        world.set([0, 0, 0], "stone");
        for z in -1..=1 {
            world.set([1, 1, z], "stone");
        }
        let positions = up_face();
        let lights =
            ambient_occlusion(&world, |_| 1.0, [0, 0, 0], &quad(&positions, Direction::Up));
        for light in lights {
            assert_eq!(light.block, 224, "{lights:?}");
        }
    }

    #[test]
    fn smooth_blend_keeps_a_dark_centre_dark() {
        let blended = smooth_blend([LightLevels(0); 3], LightLevels::new(0, 2));
        assert_eq!(blended.block, 8.0);
    }

    #[test]
    fn light_giving_and_emissive_blocks_light_their_faces_fully() {
        let world = {
            let mut world = World::new(LightLevels(0));
            world.set([0, 0, 0], "glowstone");
            world.set([5, 0, 0], "magma_block");
            world
        };
        let positions = up_face();
        let glowstone = flat(
            &world,
            |_| 1.0,
            [0, 0, 0],
            &quad(&positions, Direction::Up),
            None,
        );
        assert_eq!((glowstone[0].block, glowstone[0].sky), (240, 0));
        let magma = flat(
            &world,
            |_| 1.0,
            [5, 0, 0],
            &quad(&positions, Direction::Up),
            None,
        );
        assert_eq!((magma[0].block, magma[0].sky), (240, 240));
    }

    #[test]
    fn a_full_cube_face_reads_its_neighbour_and_a_slab_top_itself() {
        let mut world = World::new(LightLevels(0));
        world.set([0, 0, 0], "stone");
        world.light.insert([0, 1, 0], LightLevels::new(15, 0));
        world
            .blocks
            .insert([0, 0, 2], ("smooth_stone_slab", vec![("type", "bottom")]));
        world.light.insert([0, 0, 2], LightLevels::new(9, 0));
        let positions = up_face();
        let cube = flat(
            &world,
            |_| 1.0,
            [0, 0, 0],
            &quad(&positions, Direction::Up),
            None,
        );
        assert_eq!(cube[0].sky, 240);
        let slab_top =
            crate::world::block::model::face_positions(Direction::Up, [0.0; 3], [1.0, 0.5, 1.0]);
        let slab = flat(
            &world,
            |_| 1.0,
            [0, 0, 2],
            &quad(&slab_top, Direction::Up),
            None,
        );
        assert_eq!(slab[0].sky, 144);
    }

    #[test]
    fn shade_brightness_follows_vanillas_overrides() {
        let world = World::new(LightLevels(0));
        let data = |name, props: &[(&str, &str)]| {
            world
                .table
                .block_state_data(world.table.find_state(name, props))
        };
        assert_eq!(shade_brightness(data("stone", &[])), 0.2);
        assert_eq!(shade_brightness(data("glass", &[])), 1.0);
        assert_eq!(shade_brightness(data("air", &[])), 1.0);
        assert_eq!(shade_brightness(data("snow", &[("layers", "8")])), 0.2);
        assert_eq!(shade_brightness(data("snow", &[("layers", "1")])), 1.0);
        assert_eq!(shade_brightness(data("soul_sand", &[])), 0.2);
    }

    #[test]
    fn quad_emission_rounds_to_whole_levels() {
        let raised = Smooth {
            block: 100.0,
            sky: 20.0,
        }
        .with_emission(7);
        assert_eq!((raised.block, raised.sky), (112.0, 112.0));
    }
}
