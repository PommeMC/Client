//! Per-block-state collision and interaction-outline shapes. Generated tables
//! carry vanilla's own `VoxelShape` AABBs under `CollisionContext.empty()`;
//! older tables fall back to the native computations below. Boxes are
//! block-local; callers translate them through the shape offset helpers.
//!
//! [`collision_shape`] and [`outline_shape_holding`] layer the entity-context
//! branches of `ScaffoldingBlock`, `PowderSnowBlock` and `LightBlock` on top.
//! TODO: `LiquidBlock.getCollisionShape`'s `canStandOnFluid` branch.

use azalea_block::BlockState;

use crate::world::block::{PropMap, block_id, block_properties, has_collision};

/// A block-local axis-aligned box: `[min_x, min_y, min_z, max_x, max_y,
/// max_z]`.
pub type LocalBox = [f64; 6];

/// Cached collision boxes for `state`: `None` for a full cube, `Some(&[])` for
/// no collision, `Some(boxes)` for a partial shape.
pub fn partial_shape(state: BlockState) -> Option<&'static [LocalBox]> {
    crate::world::block::block_shape(state)
}

const FULL_CUBE_SHAPE: &[LocalBox] = &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];

/// `ScaffoldingBlock.SHAPE_UNSTABLE_BOTTOM`.
const SCAFFOLDING_UNSTABLE_BOTTOM: &[LocalBox] = &[[0.0, 0.0, 0.0, 1.0, 2.0 / 16.0, 1.0]];

/// Vanilla `EntityCollisionContext`, reduced to what block shapes read.
#[derive(Clone, Copy, Debug)]
pub struct CollisionContext {
    /// The entity's feet Y; `None` for `CollisionContext.empty()`.
    entity_bottom: Option<f64>,
    descending: bool,
    walks_on_powder_snow: bool,
}

impl CollisionContext {
    pub const EMPTY: Self = Self {
        entity_bottom: None,
        descending: false,
        walks_on_powder_snow: false,
    };

    /// `CollisionContext.of(entity)`: `descending` is `isDescending` (the
    /// shift key for players), `walks_on_powder_snow` is
    /// `PowderSnowBlock.canEntityWalkOnPowderSnow`.
    pub fn entity(bottom: f64, descending: bool, walks_on_powder_snow: bool) -> Self {
        Self {
            entity_bottom: Some(bottom),
            descending,
            walks_on_powder_snow,
        }
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
    state: BlockState,
    pos_y: i32,
    ctx: &CollisionContext,
) -> Option<&'static [LocalBox]> {
    match block_id(state) {
        // `ScaffoldingBlock.getCollisionShape`: the table holds SHAPE_STABLE.
        "scaffolding" => {
            if ctx.is_above(1.0, pos_y, true) && !ctx.descending {
                return partial_shape(state);
            }
            let props = block_properties(state);
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
        // TODO: the `fallDistance > 2.5` shape once fall distance is tracked.
        "powder_snow" => {
            let walkable = ctx.entity_bottom.is_some()
                && ctx.walks_on_powder_snow
                && ctx.is_above(1.0, pos_y, false)
                && !ctx.descending;
            if walkable { None } else { Some(&[]) }
        }
        _ if !has_collision(state) => Some(&[]),
        _ => partial_shape(state),
    }
}

/// Boxes the interaction raycast clips against (vanilla `getShape`). Unlike
/// `partial_shape` the full-cube case is already resolved, so an empty slice
/// means "not targetable" rather than "no collision".
pub fn outline_shape(state: BlockState) -> &'static [LocalBox] {
    crate::world::block::block_outline(state).unwrap_or(FULL_CUBE_SHAPE)
}

/// [`outline_shape`] under a player's context: `LightBlock` and
/// `ScaffoldingBlock.getShape` fill the cell while their own item is held.
pub fn outline_shape_holding(state: BlockState, held_item: Option<&str>) -> &'static [LocalBox] {
    let id = block_id(state);
    if matches!(id, "light" | "scaffolding") && held_item == Some(id) {
        FULL_CUBE_SHAPE
    } else {
        outline_shape(state)
    }
}

/// Computes one state's shape. Takes id/props rather than a `BlockState` so
/// the block-table build can call it without re-entering the table.
pub(crate) fn compute_shape(id: &str, props: &PropMap) -> Option<Vec<LocalBox>> {
    if id.ends_with("_slab") {
        return Some(match props.get("type") {
            Some("top") => vec![[0.0, 0.5, 0.0, 1.0, 1.0, 1.0]],
            Some("double") => return None,             // full cube
            _ => vec![[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]], // bottom
        });
    }

    if id.ends_with("_stairs") {
        return Some(stair_boxes(
            props.get("half").unwrap_or("bottom"),
            props.get("facing").unwrap_or("north"),
            props.get("shape").unwrap_or("straight"),
        ));
    }

    match id {
        "dirt_path" | "farmland" => Some(vec![[0.0, 0.0, 0.0, 1.0, 0.9375, 1.0]]),
        _ if id.ends_with("_carpet") => Some(vec![[0.0, 0.0, 0.0, 1.0, 0.0625, 1.0]]),
        // `SnowLayerBlock.getCollisionShape` is one layer shorter than its
        // outline, so a single layer has no collision at all.
        "snow" => Some(snow_shape(snow_layers(props) - 1)),
        _ => None,
    }
}

/// Vanilla `getShape` where it differs from `getCollisionShape`. `None` means
/// the two agree, so `compute_shape`'s result doubles as the outline.
pub(crate) fn compute_outline(id: &str, props: &PropMap) -> Option<Vec<LocalBox>> {
    match id {
        "snow" => Some(snow_shape(snow_layers(props))),
        // `LiquidBlock.getShape` and `BubbleColumnBlock.getShape` are
        // `Shapes.empty()`: the pick ray clips straight through them.
        "water" | "lava" | "bubble_column" => Some(Vec::new()),
        _ => None,
    }
}

fn snow_layers(props: &PropMap) -> i32 {
    props
        .get("layers")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
}

/// Vanilla `SnowLayerBlock.SHAPES[layers]`, two pixels per layer; index 0 is
/// empty.
fn snow_shape(layers: i32) -> Vec<LocalBox> {
    if layers <= 0 {
        return Vec::new();
    }
    vec![[0.0, 0.0, 0.0, 1.0, layers as f64 * 2.0 / 16.0, 1.0]]
}

/// Vanilla `StairBlock` shape: a half-slab plus 1–3 upper corner pillars,
/// rotated to `facing`/`shape` and Y-flipped for the top half.
fn stair_boxes(half: &str, facing: &str, shape: &str) -> Vec<LocalBox> {
    // Base shape faces north, bottom half. SHAPE_OUTER is the half-slab plus one
    // corner; STRAIGHT adds its 90° rotation; INNER adds a third corner.
    let mut boxes = vec![[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]];
    let corner: LocalBox = [0.0, 0.5, 0.0, 0.5, 1.0, 0.5];
    match shape {
        "inner_left" | "inner_right" => {
            boxes.push(corner);
            boxes.push(rot_y90(corner));
            boxes.push(rot_y90(rot_y90(corner)));
        }
        "outer_left" | "outer_right" => boxes.push(corner),
        _ => {
            boxes.push(corner);
            boxes.push(rot_y90(corner));
        }
    }

    if half == "top" {
        for b in &mut boxes {
            *b = invert_y(*b);
        }
    }

    // Vanilla derives the lookup direction from facing and shape.
    let dir = match shape {
        "inner_left" => ccw(facing),
        "outer_right" => cw(facing),
        _ => facing,
    };
    for _ in 0..dir_steps(dir) {
        for b in &mut boxes {
            *b = rot_y90(*b);
        }
    }

    boxes
}

/// Rotate a box 90° about the block's vertical center axis: `(x, z)` -> `(1-z,
/// x)`.
fn rot_y90([x0, y0, z0, x1, y1, z1]: LocalBox) -> LocalBox {
    [1.0 - z1, y0, x0, 1.0 - z0, y1, x1]
}

fn invert_y([x0, y0, z0, x1, y1, z1]: LocalBox) -> LocalBox {
    [x0, 1.0 - y1, z0, x1, 1.0 - y0, z1]
}

fn dir_steps(facing: &str) -> u32 {
    match facing {
        "east" => 1,
        "south" => 2,
        "west" => 3,
        _ => 0, // north
    }
}

fn cw(facing: &str) -> &'static str {
    match facing {
        "north" => "east",
        "east" => "south",
        "south" => "west",
        _ => "north",
    }
}

fn ccw(facing: &str) -> &'static str {
    match facing {
        "north" => "west",
        "west" => "south",
        "south" => "east",
        _ => "north",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::find_state;

    #[test]
    fn held_item_fills_light_and_scaffolding_outlines() {
        crate::world::block::init("26.2");
        let light = find_state("light", &[]);
        assert!(outline_shape_holding(light, None).is_empty());
        assert!(outline_shape_holding(light, Some("scaffolding")).is_empty());
        assert_eq!(outline_shape_holding(light, Some("light")), FULL_CUBE_SHAPE);

        let scaffolding = find_state("scaffolding", &[]);
        assert_ne!(outline_shape_holding(scaffolding, None), FULL_CUBE_SHAPE);
        assert_eq!(
            outline_shape_holding(scaffolding, Some("scaffolding")),
            FULL_CUBE_SHAPE
        );
    }
}
