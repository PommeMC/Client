//! Per-block-state collision and interaction-outline shapes. Every version's
//! table carries vanilla's own `VoxelShape` AABBs under
//! `CollisionContext.empty()`. Boxes are block-local; callers translate them
//! through the shape offset helpers.
//!
//! [`collision_shape`] and [`outline_shape_holding_data`] layer the
//! entity-context branches of `ScaffoldingBlock`, `PowderSnowBlock` and
//! `LightBlock` on top. TODO: `LiquidBlock.getCollisionShape`'s
//! `canStandOnFluid` branch.

use crate::world::block::{BlockStateData, SpecialCollision};

/// A block-local axis-aligned box: `[min_x, min_y, min_z, max_x, max_y,
/// max_z]`.
pub type LocalBox = [f64; 6];

/// Cached collision boxes for resolved data: `None` for a full cube,
/// `Some(&[])` for no collision, `Some(boxes)` for a partial shape.
pub fn partial_shape(block_state_data: &BlockStateData) -> Option<&'static [LocalBox]> {
    block_state_data.collision_boxes()
}

const FULL_CUBE_SHAPE: &[LocalBox] = &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];

/// `ScaffoldingBlock.SHAPE_UNSTABLE_BOTTOM`.
const SCAFFOLDING_UNSTABLE_BOTTOM: &[LocalBox] = &[[0.0, 0.0, 0.0, 1.0, 2.0 / 16.0, 1.0]];

/// `PowderSnowBlock.FALLING_COLLISION_SHAPE`.
const POWDER_SNOW_FALLING: &[LocalBox] = &[[0.0, 0.0, 0.0, 1.0, 0.9_f32 as f64, 1.0]];

/// Vanilla `EntityCollisionContext`, reduced to what block shapes read.
#[derive(Clone, Copy, Debug)]
pub struct CollisionContext {
    /// The entity's feet Y; `None` for `CollisionContext.empty()`.
    entity_bottom: Option<f64>,
    descending: bool,
    walks_on_powder_snow: bool,
    fall_distance: f64,
}

impl CollisionContext {
    /// Vanilla `isDescending`: the sneak key is down.
    pub fn descending(&self) -> bool {
        self.descending
    }

    pub const EMPTY: Self = Self {
        entity_bottom: None,
        descending: false,
        walks_on_powder_snow: false,
        fall_distance: 0.0,
    };

    /// `CollisionContext.of(entity)`: `descending` is `isDescending` (the
    /// shift key for players), `walks_on_powder_snow` is
    /// `PowderSnowBlock.canEntityWalkOnPowderSnow`.
    pub fn entity(bottom: f64, descending: bool, walks_on_powder_snow: bool) -> Self {
        Self {
            entity_bottom: Some(bottom),
            descending,
            walks_on_powder_snow,
            fall_distance: 0.0,
        }
    }

    /// The entity's `fallDistance`, which powder snow reads.
    pub fn with_fall_distance(self, fall_distance: f64) -> Self {
        Self {
            fall_distance,
            ..self
        }
    }

    /// `CollisionContext.positionContext(y)`: never descending, and not an
    /// entity context, so powder snow stays empty.
    pub fn position(y: f64) -> Self {
        Self::entity(y, false, false)
    }

    /// `isAbove(shape, pos, default)` for a shape whose top is `shape_max_y`.
    fn is_above(&self, shape_max_y: f64, pos_y: i32, default: bool) -> bool {
        self.entity_bottom.map_or(default, |bottom| {
            bottom > f64::from(pos_y) + shape_max_y - f64::from(1.0e-5_f32)
        })
    }
}

/// `context.getCollisionShape(state, level, pos)` in `partial_shape`'s
/// encoding. Scaffolding is `noCollision()` but overrides the shape anyway.
pub fn collision_shape(
    block_state_data: &BlockStateData,
    pos_y: i32,
    ctx: &CollisionContext,
) -> Option<&'static [LocalBox]> {
    match block_state_data.special_collision() {
        // `ScaffoldingBlock.getCollisionShape`: the table holds SHAPE_STABLE.
        SpecialCollision::Scaffolding => {
            if ctx.is_above(1.0, pos_y, true) && !ctx.descending {
                return partial_shape(block_state_data);
            }
            let props = block_state_data.properties();
            if props.get("distance") != Some("0")
                && props.get("bottom") == Some("true")
                && ctx.is_above(0.0, pos_y, true)
            {
                Some(SCAFFOLDING_UNSTABLE_BOTTOM)
            } else {
                Some(&[])
            }
        }
        // `PowderSnowBlock.getCollisionShape`.
        SpecialCollision::PowderSnow => {
            if ctx.fall_distance > 2.5 {
                return Some(POWDER_SNOW_FALLING);
            }
            let walkable =
                ctx.walks_on_powder_snow && ctx.is_above(1.0, pos_y, false) && !ctx.descending;
            if walkable { None } else { Some(&[]) }
        }
        // Explicit shapes beat `noCollision` (`PitcherCropBlock`,
        // `WallHangingSignBlock`); only the full-cube default needs the flag.
        _ => match partial_shape(block_state_data) {
            None if !block_state_data.has_collision() => Some(&[]),
            shape => shape,
        },
    }
}

/// Vanilla `isCollisionShapeFullBlock`: the empty-context collision shape
/// fills the cell.
pub fn is_collision_shape_full_block(block_state_data: &BlockStateData) -> bool {
    collision_shape(block_state_data, 0, &CollisionContext::EMPTY).is_none()
}

/// Boxes the interaction raycast clips against (vanilla `getShape`). Unlike
/// `partial_shape` the full-cube case is already resolved, so an empty slice
/// means "not targetable" rather than "no collision".
pub fn outline_shape(block_state_data: &BlockStateData) -> &'static [LocalBox] {
    block_state_data.outline_boxes().unwrap_or(FULL_CUBE_SHAPE)
}

/// [`outline_shape`] under a player's context: `LightBlock` and
/// `ScaffoldingBlock.getShape` fill the cell while their own item is held.
/// Uses already resolved session metadata.
pub(crate) fn outline_shape_holding_data(
    block_state_data: &BlockStateData,
    held_item: Option<&str>,
) -> &'static [LocalBox] {
    let id = block_state_data.id();
    if matches!(id, "light" | "scaffolding") && held_item == Some(id) {
        FULL_CUBE_SHAPE
    } else {
        outline_shape(block_state_data)
    }
}

/// Vanilla `VoxelShape.forAllEdges`: the edges of the boxes' union, each
/// run of collinear cell edges merged into one line, as block-local
/// `(start, end)` pairs. Ports `DiscreteVoxelShape.forAllAxisEdges` over the
/// grid the boxes' bounds cut.
pub fn outline_edges(boxes: &[LocalBox]) -> Vec<([f64; 3], [f64; 3])> {
    let coords: [Vec<f64>; 3] = std::array::from_fn(|axis| {
        let mut c: Vec<f64> = boxes.iter().flat_map(|b| [b[axis], b[axis + 3]]).collect();
        c.sort_by(f64::total_cmp);
        c.dedup_by(|a, b| (*a - *b).abs() < 1.0e-7);
        c
    });
    let size = coords.each_ref().map(|c| c.len().saturating_sub(1) as i32);
    let full = |cell: [i32; 3]| {
        (0..3).all(|k| (0..size[k]).contains(&cell[k]))
            && boxes.iter().any(|b| {
                (0..3).all(|k| {
                    let mid = (coords[k][cell[k] as usize] + coords[k][cell[k] as usize + 1]) / 2.0;
                    b[k] < mid && mid < b[k + 3]
                })
            })
    };
    let point = |i: [i32; 3]| std::array::from_fn(|k| coords[k][i[k] as usize]);

    let mut edges = Vec::new();
    // `AxisCycle` NONE, FORWARD and BACKWARD by their inverse: `xyz[k]` is
    // `abc[source[k]]`.
    for source in [[0, 1, 2], [1, 2, 0], [2, 0, 1]] {
        let to_xyz = |abc: [i32; 3]| source.map(|s| abc[s]);
        let mut abc_size = [0; 3];
        for k in 0..3 {
            abc_size[source[k]] = size[k];
        }
        for a in 0..=abc_size[0] {
            for b in 0..=abc_size[1] {
                let mut last_start = None;
                for c in 0..=abc_size[2] {
                    let mut full_sectors = 0;
                    let mut odd_sectors = 0;
                    for da in 0..=1 {
                        for db in 0..=1 {
                            if full(to_xyz([a + da - 1, b + db - 1, c])) {
                                full_sectors += 1;
                                odd_sectors ^= da ^ db;
                            }
                        }
                    }
                    if full_sectors == 1
                        || full_sectors == 3
                        || full_sectors == 2 && odd_sectors == 0
                    {
                        last_start.get_or_insert(c);
                    } else if let Some(start) = last_start.take() {
                        edges.push((point(to_xyz([a, b, start])), point(to_xyz([a, b, c]))));
                    }
                }
            }
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_edges_merge_as_vanilla_does() {
        assert_eq!(outline_edges(FULL_CUBE_SHAPE).len(), 12);
        assert_eq!(outline_edges(&[[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]]).len(), 12);
        // A bottom stair: an L-shaped prism, with no seam where its boxes meet.
        let stair = [
            [0.0, 0.0, 0.0, 1.0, 0.5, 1.0],
            [0.0, 0.5, 0.5, 1.0, 1.0, 1.0],
        ];
        let edges = outline_edges(&stair);
        assert_eq!(edges.len(), 18);
        assert!(!edges.contains(&([0.0, 0.5, 0.5], [0.0, 0.5, 1.0])));
        // Two boxes side by side make one box: the shared face's edges merge.
        let halves = [
            [0.0, 0.0, 0.0, 0.5, 1.0, 1.0],
            [0.5, 0.0, 0.0, 1.0, 1.0, 1.0],
        ];
        assert_eq!(outline_edges(&halves).len(), 12);
    }

    #[test]
    fn held_item_fills_light_and_scaffolding_outlines() {
        let block_state_table = crate::world::block::BlockStateTable::native();
        let outline_shape_holding = |state, held_item| {
            outline_shape_holding_data(block_state_table.block_state_data(state), held_item)
        };
        let light = block_state_table.find_state("light", &[]);
        assert!(outline_shape_holding(light, None).is_empty());
        assert!(outline_shape_holding(light, Some("scaffolding")).is_empty());
        assert_eq!(outline_shape_holding(light, Some("light")), FULL_CUBE_SHAPE);

        let scaffolding = block_state_table.find_state("scaffolding", &[]);
        assert_ne!(outline_shape_holding(scaffolding, None), FULL_CUBE_SHAPE);
        assert_eq!(
            outline_shape_holding(scaffolding, Some("scaffolding")),
            FULL_CUBE_SHAPE
        );
    }
}
