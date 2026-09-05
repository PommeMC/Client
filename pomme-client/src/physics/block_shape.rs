//! Per-block-state collision shapes for the handful of blocks whose hitbox
//! isn't a full cube. Ported from the vanilla block classes (`SlabBlock`,
//! `StairBlock`, etc.). Boxes are block-local (0..1); the caller offsets them
//! to the block position.
//!
//! TODO: walls, fences, fence gates, panes, trapdoors, doors, beds, chests,
//! cake, etc. still fall back to a full cube.
//!
//! TODO: blocks with no collision but a small outline (torches, flowers,
//! buttons, plants, redstone dust) fall back to a full cube too, so the
//! crosshair still reaches them from a block away.

use azalea_block::BlockState;

use crate::world::block::PropMap;

/// A block-local axis-aligned box: `[min_x, min_y, min_z, max_x, max_y,
/// max_z]`.
pub type LocalBox = [f64; 6];

/// Cached collision boxes for `state`: `None` for a full cube, `Some(&[])` for
/// no collision, `Some(boxes)` for a partial shape.
pub fn partial_shape(state: BlockState) -> Option<&'static [LocalBox]> {
    crate::world::block::block_shape(state)
}

const FULL_CUBE_SHAPE: &[LocalBox] = &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];

/// Boxes the interaction raycast clips against (vanilla `getShape`). Unlike
/// `partial_shape` the full-cube case is already resolved, so an empty slice
/// means "not targetable" rather than "no collision".
pub fn outline_shape(state: BlockState) -> &'static [LocalBox] {
    crate::world::block::block_outline(state).unwrap_or(FULL_CUBE_SHAPE)
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
