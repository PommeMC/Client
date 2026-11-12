//! What the chunk mesher asks of every block and each of its neighbours,
//! resolved once per state when the table is built. The rules compare block
//! names, which the mesher's per-block and per-face loops can't afford.

use super::BlockStateData;
use super::model::Direction;
use crate::physics::block_shape::is_collision_shape_full_block;

/// How the chunk mesher draws a state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MeshKind {
    /// Nothing to mesh: air, the invisible blocks, and blocks the block-entity
    /// pipeline draws.
    Air,
    Water,
    Lava,
    Solid,
}

/// How much of one face a state's occlusion shape covers (vanilla
/// `BlockStateBase.getFaceOcclusionShape`); a partial cover has its mask in
/// `LightProps::face_occlusion`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FaceCover {
    Full,
    Empty,
    Partial,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RenderProps {
    pub(crate) mesh_kind: MeshKind,
    pub(crate) emissive_rendering: bool,
    /// Whether vanilla `getShadeBrightness` gives 0.2 rather than 1.0.
    shades: bool,
    /// One bit per `Direction` ordinal.
    culls_same: u8,
    /// By `Direction` ordinal.
    face_cover: [FaceCover; 6],
}

impl RenderProps {
    /// A table entry's value until [`Self::of`] has read the rest of it.
    pub(super) const UNSET: Self = Self {
        mesh_kind: MeshKind::Air,
        emissive_rendering: false,
        shades: false,
        culls_same: 0,
        face_cover: [FaceCover::Empty; 6],
    };

    pub(super) fn of(data: &BlockStateData) -> Self {
        Self {
            mesh_kind: mesh_kind(data),
            emissive_rendering: emissive_rendering(data),
            shades: shades(data),
            culls_same: Direction::VALUES
                .iter()
                .filter(|&&direction| culls_same(data, direction))
                .fold(0, |bits, &direction| bits | 1 << direction as u8),
            face_cover: Direction::VALUES.map(|direction| face_cover(data, direction)),
        }
    }

    pub(crate) fn shade_brightness(self) -> f32 {
        if self.shades { 0.2 } else { 1.0 }
    }

    /// Vanilla `skipRendering` against a neighbour of its own kind across
    /// `direction`.
    pub(crate) fn culls_same(self, direction: Direction) -> bool {
        self.culls_same & 1 << direction as u8 != 0
    }

    pub(crate) fn face_cover(self, direction: Direction) -> FaceCover {
        self.face_cover[direction as usize]
    }
}

fn mesh_kind(data: &BlockStateData) -> MeshKind {
    if data.is_air() {
        return MeshKind::Air;
    }
    match data.id() {
        "cave_air" | "void_air" | "light" | "barrier" | "structure_void" | "moving_piston" => {
            MeshKind::Air
        }
        "water" | "bubble_column" => MeshKind::Water,
        "lava" => MeshKind::Lava,
        // Drawn by the block-entity pipeline; nothing to mesh.
        id if crate::world::block_entity::rendered_kind(id).is_some() => MeshKind::Air,
        _ => MeshKind::Solid,
    }
}

/// Vanilla `Properties.emissiveRendering` predicates (`Blocks`): magma blocks,
/// and sculk sensors while active.
fn emissive_rendering(data: &BlockStateData) -> bool {
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
fn shades(data: &BlockStateData) -> bool {
    let id = data.id();
    match id {
        "barrier" | "light" | "structure_void" | "glass" | "tinted_glass" => false,
        _ if id.ends_with("stained_glass") || id.ends_with("copper_grate") => false,
        "mud" | "soul_sand" => true,
        "snow" => data.properties().get("layers") == Some("8"),
        _ => is_collision_shape_full_block(data),
    }
}

/// Vanilla `skipRendering`: `HalfTransparentBlock`, `LiquidBlock` and powder
/// snow on every side, mangrove roots only above and below.
fn culls_same(data: &BlockStateData, direction: Direction) -> bool {
    match data.id() {
        "mangrove_roots" => matches!(direction, Direction::Up | Direction::Down),
        id => {
            data.is_fluid_block()
                || matches!(
                    id,
                    "glass"
                        | "tinted_glass"
                        | "ice"
                        | "frosted_ice"
                        | "blue_ice"
                        | "slime_block"
                        | "honey_block"
                        | "powder_snow"
                )
                || id.ends_with("_stained_glass")
                || id.ends_with("copper_grate")
        }
    }
}

fn face_cover(data: &BlockStateData, direction: Direction) -> FaceCover {
    if data.is_air() || !data.light_props().can_occlude {
        return FaceCover::Empty;
    }
    match data.light_props().face_occlusion {
        Some(masks) => {
            let mask = &masks[direction as usize];
            if mask.iter().all(|&row| row == 0xFFFF) {
                FaceCover::Full
            } else if mask.iter().all(|&row| row == 0) {
                FaceCover::Empty
            } else {
                FaceCover::Partial
            }
        }
        // TODO: masks are only dumped for useShapeForLightOcclusion states, so
        // other partial occlusion shapes (fence posts, chests) count as empty.
        None if data.occludes_neighbor_faces() => FaceCover::Full,
        None => FaceCover::Empty,
    }
}
