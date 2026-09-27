//! Generates pomme's per-version block data from Mojang's data-generator
//! reports (`java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar
//! --reports`).
//!
//! Usage:
//!   blockgen blocks <reports/blocks.json> <version> <out.json>
//!   blockgen behavior <azalea generated.rs> <out.json>
//!   blockgen state <generated/state.json> <blocks-<v>.json> <out.json>
//!
//! `blocks` flattens the report into a compact per-block table (name, first
//! state id, default state id, ordered property lists). Every explicit state
//! id + property set in the report is cross-checked against the cartesian
//! reconstruction the client uses, and the id space is verified dense — the
//! tool hard-fails rather than emit silently-wrong data.
//!
//! `behavior` seeds the name-keyed destroy-time table from an azalea
//! `generated.rs` (e.g. `~/.cargo/git/checkouts/azalea-*/<rev>/azalea-block/
//! src/generated.rs`); new blocks the seed doesn't know must be appended by
//! hand from the decompiled `Blocks.java`. Hand-added entries survive a
//! regen (existing keys the seed doesn't produce are carried over and
//! listed), but the seed wins for keys it does produce — a hand-correction
//! to a seeded value does not survive.
//!
//! `state` compacts the raw per-state property dump produced by running
//! vanilla (`tools/stategen/StateDump.java`, see `just stategen`) into the
//! per-block table the client embeds: each field is a scalar when uniform
//! across the block's states, else a per-state array, and face-occlusion
//! masks are deduped into a dictionary. State counts and value ranges are
//! cross-checked against the version's blocks table.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, report, version, out] if cmd == "blocks" => gen_blocks(report, version, out),
        [cmd, generated, out] if cmd == "behavior" => gen_behavior(generated, out),
        [cmd, dump, blocks, out] if cmd == "state" => gen_state(dump, blocks, out),
        _ => Err("usage: blockgen blocks <blocks.json> <version> <out.json>\n       blockgen behavior <generated.rs> <out.json>\n       blockgen state <state.json> <blocks-<v>.json> <out.json>".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("blockgen: {e}");
            ExitCode::FAILURE
        }
    }
}

type Error = Box<dyn std::error::Error>;

struct Block {
    name: String,
    first_id: u32,
    default_id: u32,
    /// Property (key, values) pairs in the report's listed order; the last
    /// property varies fastest in the state-id cartesian product.
    props: Vec<(String, Vec<String>)>,
}

fn gen_blocks(report_path: &str, version: &str, out_path: &str) -> Result<(), Error> {
    let report: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(report_path)?)?;

    let mut blocks = Vec::new();
    for (name, entry) in &report {
        blocks.push(parse_block(name, entry)?);
    }
    blocks.sort_by_key(|b| b.first_id);

    // The id space must tile densely from 0 with no gaps or overlaps.
    let mut expected_id = 0u32;
    for block in &blocks {
        if block.first_id != expected_id {
            return Err(format!(
                "id space not dense: block '{}' starts at {} but expected {}",
                block.name, block.first_id, expected_id
            )
            .into());
        }
        expected_id += state_count(block);
    }

    let mut out = String::new();
    writeln!(out, "{{")?;
    writeln!(out, "  \"version\": {},", serde_json::to_string(version)?)?;
    writeln!(out, "  \"state_count\": {expected_id},")?;
    writeln!(out, "  \"blocks\": [")?;
    for (i, block) in blocks.iter().enumerate() {
        let comma = if i + 1 < blocks.len() { "," } else { "" };
        let mut line = format!(
            "    {{\"name\": {}, \"first_id\": {}, \"default_id\": {}",
            serde_json::to_string(&block.name)?,
            block.first_id,
            block.default_id
        );
        if !block.props.is_empty() {
            let props: Vec<serde_json::Value> = block
                .props
                .iter()
                .map(|(k, vs)| serde_json::json!([k, vs]))
                .collect();
            write!(line, ", \"props\": {}", serde_json::to_string(&props)?)?;
        }
        writeln!(out, "{line}}}{comma}")?;
    }
    writeln!(out, "  ]")?;
    writeln!(out, "}}")?;

    std::fs::write(out_path, &out)?;
    println!(
        "wrote {} blocks / {} states for {} to {}",
        blocks.len(),
        expected_id,
        version,
        out_path
    );
    Ok(())
}

fn state_count(block: &Block) -> u32 {
    block.props.iter().map(|(_, vs)| vs.len() as u32).product()
}

fn parse_block(name: &str, entry: &serde_json::Value) -> Result<Block, Error> {
    let name = name.strip_prefix("minecraft:").unwrap_or(name).to_string();

    // Value arrays are in variant order, but the report's property KEY order
    // is the builder order, not the state-enumeration order (vanilla sorts
    // properties by name for the state definition). Rather than assume the
    // sort rule, each property's stride is derived from the explicit state
    // ids below and the properties reordered to match.
    let mut props: Vec<(String, Vec<String>)> = Vec::new();
    if let Some(properties) = entry.get("properties") {
        let map = properties
            .as_object()
            .ok_or_else(|| format!("{name}: properties is not an object"))?;
        for (key, values) in map {
            let values = values
                .as_array()
                .ok_or_else(|| format!("{name}: property {key} values not an array"))?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(String::from)
                        .ok_or_else(|| format!("{name}: property {key} has non-string value"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            props.push((key.clone(), values));
        }
    }

    let states = entry
        .get("states")
        .and_then(|s| s.as_array())
        .ok_or_else(|| format!("{name}: missing states array"))?;

    let expected: u32 = props.iter().map(|(_, vs)| vs.len() as u32).product();
    if states.len() as u32 != expected {
        return Err(format!(
            "{name}: {} states but property product is {expected}",
            states.len()
        )
        .into());
    }

    let first_id = states
        .iter()
        .filter_map(|s| s.get("id").and_then(|i| i.as_u64()))
        .min()
        .ok_or_else(|| format!("{name}: states missing ids"))? as u32;

    // Index the explicit states by their full property assignment.
    let mut by_props: std::collections::HashMap<BTreeMap<&str, &str>, u32> =
        std::collections::HashMap::new();
    for state in states {
        let id = state
            .get("id")
            .and_then(|i| i.as_u64())
            .ok_or_else(|| format!("{name}: state missing id"))? as u32;
        let mut key = BTreeMap::new();
        if let Some(map) = state.get("properties").and_then(|p| p.as_object()) {
            for (k, v) in map {
                key.insert(
                    k.as_str(),
                    v.as_str()
                        .ok_or_else(|| format!("{name}: non-string property value"))?,
                );
            }
        }
        by_props.insert(key, id);
    }

    // The base state (first id) must sit at every property's first value,
    // and flipping one property to its second value reveals that property's
    // stride in the enumeration.
    let base: BTreeMap<&str, &str> = props
        .iter()
        .map(|(k, vs)| (k.as_str(), vs[0].as_str()))
        .collect();
    if by_props.get(&base) != Some(&first_id) {
        return Err(format!(
            "{name}: base state (all first values) is not the first id — enumeration isn't a plain cartesian product"
        )
        .into());
    }
    let mut strides: Vec<u32> = Vec::with_capacity(props.len());
    for (key, values) in &props {
        if values.len() == 1 {
            strides.push(0);
            continue;
        }
        let mut flipped = base.clone();
        flipped.insert(key.as_str(), values[1].as_str());
        let id = by_props
            .get(&flipped)
            .ok_or_else(|| format!("{name}: no state for {key}={}", values[1]))?;
        strides.push(id - first_id);
    }

    // Reorder to enumeration order: largest stride first (single-value
    // properties contribute factor 1 and can go last).
    let mut order: Vec<usize> = (0..props.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(strides[i]));
    let props: Vec<(String, Vec<String>)> = order.into_iter().map(|i| props[i].clone()).collect();

    let mut default_id = None;

    // Cross-check every explicit state against the cartesian reconstruction
    // (derived property order, last property varying fastest).
    for state in states {
        let id = state
            .get("id")
            .and_then(|i| i.as_u64())
            .ok_or_else(|| format!("{name}: state missing id"))? as u32;
        let offset = id
            .checked_sub(first_id)
            .ok_or_else(|| format!("{name}: state id {id} below first id {first_id}"))?;

        let mut stride: u32 = props.iter().map(|(_, vs)| vs.len() as u32).product();
        for (key, values) in &props {
            stride /= values.len() as u32;
            let index = (offset / stride) as usize % values.len();
            let reconstructed = &values[index];
            let reported = state
                .get("properties")
                .and_then(|p| p.get(key))
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("{name}: state {id} missing property {key}"))?;
            if reconstructed != reported {
                return Err(format!(
                    "{name}: state {id} property {key} reconstructs to '{reconstructed}' but report says '{reported}' — enumeration order changed, extend the format"
                )
                .into());
            }
        }

        if state.get("default").and_then(|d| d.as_bool()) == Some(true) {
            default_id = Some(id);
        }
    }

    let default_id = default_id.ok_or_else(|| format!("{name}: no default state"))?;
    Ok(Block {
        name,
        first_id,
        default_id,
        props,
    })
}

/// Extracts `destroy_time` + `requires_correct_tool_for_drops` per block from
/// azalea's machine-generated `generated.rs` block list (uniform shape:
/// `name => BlockBehavior::new().strength(a, b)..., {`).
fn gen_behavior(generated_path: &str, out_path: &str) -> Result<(), Error> {
    let source = std::fs::read_to_string(generated_path)?;
    let mut entries: BTreeMap<String, (f32, bool)> = BTreeMap::new();

    for line in source.lines() {
        let trimmed = line.trim_start();
        let Some((name, rest)) = trimmed.split_once(" => BlockBehavior::new()") else {
            continue;
        };
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            continue;
        }
        let destroy_time = extract_float_arg(rest, ".strength(")
            .or_else(|| extract_float_arg(rest, ".destroy_time("))
            .unwrap_or(0.0);
        let requires_tool = rest.contains(".requires_correct_tool_for_drops()");
        entries.insert(name.to_string(), (destroy_time, requires_tool));
    }

    if entries.is_empty() {
        return Err("no block behavior entries found — wrong input file?".into());
    }

    // Carry over hand-appended keys the seed doesn't know.
    let mut carried: Vec<String> = Vec::new();
    match std::fs::read_to_string(out_path) {
        Ok(existing) => {
            let existing: BTreeMap<String, BehaviorJson> =
                serde_json::from_str(&existing).map_err(|e| format!("{out_path}: {e}"))?;
            for (name, b) in existing {
                if !entries.contains_key(&name) {
                    entries.insert(name.clone(), (b.destroy_time, b.requires_correct_tool));
                    carried.push(name);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{out_path}: {e}").into()),
    }

    let mut out = String::new();
    writeln!(out, "{{")?;
    let len = entries.len();
    for (i, (name, (destroy_time, requires_tool))) in entries.iter().enumerate() {
        let comma = if i + 1 < len { "," } else { "" };
        writeln!(
            out,
            "  {}: {{\"destroy_time\": {destroy_time}, \"requires_correct_tool\": {requires_tool}}}{comma}",
            serde_json::to_string(name)?
        )?;
    }
    writeln!(out, "}}")?;

    std::fs::write(out_path, &out)?;
    println!("wrote {len} behavior entries to {out_path}");
    if !carried.is_empty() {
        println!(
            "carried over {} hand-added entries: {}",
            carried.len(),
            carried.join(", ")
        );
    }
    Ok(())
}

/// An entry of the emitted `block_behavior.json`, read back on regen.
#[derive(serde::Deserialize)]
struct BehaviorJson {
    destroy_time: f32,
    requires_correct_tool: bool,
}

fn extract_float_arg(text: &str, method: &str) -> Option<f32> {
    let start = text.find(method)? + method.len();
    let rest = &text[start..];
    let end = rest.find([',', ')'])?;
    rest[..end].trim().parse().ok()
}

/// Raw per-state dump written by `tools/stategen/StateDump.java`.
#[derive(serde::Deserialize)]
struct StateDumpFile {
    version: String,
    state_count: u32,
    emission: Vec<u8>,
    dampening: Vec<u8>,
    propagates_skylight_down: Vec<u8>,
    can_occlude: Vec<u8>,
    use_shape_for_light_occlusion: Vec<u8>,
    /// `BlockBehaviour.hasCollision`; block-level, so uniform across each
    /// block's states.
    has_collision: Vec<u8>,
    /// Vanilla `BlockState.blocksMotion()`; absent from versions without the
    /// method (26.3 dropped it).
    #[serde(default)]
    blocks_motion: Option<Vec<u8>>,
    /// Vanilla cached `BlockState.isSolid()` / `legacySolid`.
    legacy_solid: Vec<u8>,
    /// Vanilla cached `BlockState.canBeReplaced()` property.
    replaceable: Vec<u8>,
    /// Packed six Direction ordinal bits for
    /// `BlockState.isFaceSturdy(..., SupportType.FULL)`.
    full_face_sturdy: Vec<u8>,
    /// Whether vanilla's logical collision/outline shape actually applies the
    /// block state's positional offset. Render/model offsets are independent.
    collision_shape_uses_offset: Vec<u8>,
    outline_shape_uses_offset: Vec<u8>,
    /// `BlockBehaviour.OffsetType`: 0 none, 1 XZ, 2 XYZ.
    position_offset_type: Vec<u8>,
    /// Runtime clamp parameters used by vanilla's registered offset function.
    max_horizontal_offset: Vec<f32>,
    max_vertical_offset: Vec<f32>,
    /// Flattened vanilla AABBs (`[min_x,min_y,min_z,max_x,max_y,max_z,...]`),
    /// one entry per state. `StateDump` obtains these from vanilla itself.
    collision_shapes: Vec<Vec<f64>>,
    outline_shapes: Vec<Vec<f64>>,
    /// State id (as string) -> 6 face masks, 64 hex chars each, present
    /// exactly for states with `can_occlude && use_shape_for_light_occlusion`.
    face_masks: std::collections::HashMap<String, [String; 6]>,
}

#[derive(serde::Deserialize)]
struct BlocksFile {
    version: String,
    state_count: u32,
    blocks: Vec<BlocksEntry>,
}

#[derive(serde::Deserialize)]
struct BlocksEntry {
    name: String,
    first_id: u32,
    #[serde(default)]
    props: Vec<(String, Vec<String>)>,
}

fn gen_state(dump_path: &str, blocks_path: &str, out_path: &str) -> Result<(), Error> {
    let dump: StateDumpFile = serde_json::from_str(&std::fs::read_to_string(dump_path)?)?;
    let blocks: BlocksFile = serde_json::from_str(&std::fs::read_to_string(blocks_path)?)?;

    if dump.version != blocks.version {
        return Err(format!(
            "version mismatch: state dump is '{}', blocks table is '{}'",
            dump.version, blocks.version
        )
        .into());
    }
    if dump.state_count != blocks.state_count {
        return Err(format!(
            "state count mismatch: state dump has {}, blocks table has {}",
            dump.state_count, blocks.state_count
        )
        .into());
    }
    let n = dump.state_count as usize;
    for (key, len) in [
        ("emission", dump.emission.len()),
        ("dampening", dump.dampening.len()),
        (
            "propagates_skylight_down",
            dump.propagates_skylight_down.len(),
        ),
        ("can_occlude", dump.can_occlude.len()),
        (
            "use_shape_for_light_occlusion",
            dump.use_shape_for_light_occlusion.len(),
        ),
        ("has_collision", dump.has_collision.len()),
        (
            "blocks_motion",
            dump.blocks_motion.as_ref().map_or(n, Vec::len),
        ),
        ("legacy_solid", dump.legacy_solid.len()),
        ("replaceable", dump.replaceable.len()),
        ("full_face_sturdy", dump.full_face_sturdy.len()),
        (
            "collision_shape_uses_offset",
            dump.collision_shape_uses_offset.len(),
        ),
        (
            "outline_shape_uses_offset",
            dump.outline_shape_uses_offset.len(),
        ),
        ("position_offset_type", dump.position_offset_type.len()),
        ("max_horizontal_offset", dump.max_horizontal_offset.len()),
        ("max_vertical_offset", dump.max_vertical_offset.len()),
        ("collision_shapes", dump.collision_shapes.len()),
        ("outline_shapes", dump.outline_shapes.len()),
    ] {
        if len != n {
            return Err(format!("{key} has {len} entries, expected {n}").into());
        }
    }
    for i in 0..n {
        if dump.emission[i] > 15 || dump.dampening[i] > 15 {
            return Err(format!("state {i}: light value out of 0..=15 range").into());
        }
        for (key, v) in [
            ("propagates_skylight_down", dump.propagates_skylight_down[i]),
            ("can_occlude", dump.can_occlude[i]),
            (
                "use_shape_for_light_occlusion",
                dump.use_shape_for_light_occlusion[i],
            ),
            ("has_collision", dump.has_collision[i]),
            (
                "blocks_motion",
                dump.blocks_motion.as_ref().map_or(0, |m| m[i]),
            ),
            ("legacy_solid", dump.legacy_solid[i]),
            ("replaceable", dump.replaceable[i]),
            (
                "collision_shape_uses_offset",
                dump.collision_shape_uses_offset[i],
            ),
            (
                "outline_shape_uses_offset",
                dump.outline_shape_uses_offset[i],
            ),
        ] {
            if v > 1 {
                return Err(format!("state {i}: {key} is {v}, expected 0/1").into());
            }
        }
        if dump.full_face_sturdy[i] > 0x3f {
            return Err(format!(
                "state {i}: full_face_sturdy is {}, expected a 6-bit mask",
                dump.full_face_sturdy[i]
            )
            .into());
        }
        let offset_type = dump.position_offset_type[i];
        if offset_type > 2 {
            return Err(format!(
                "state {i}: position_offset_type is {offset_type}, expected 0..=2"
            )
            .into());
        }
        let max_horizontal = dump.max_horizontal_offset[i];
        let max_vertical = dump.max_vertical_offset[i];
        if !max_horizontal.is_finite()
            || !max_vertical.is_finite()
            || max_horizontal < 0.0
            || max_vertical < 0.0
        {
            return Err(format!(
                "state {i}: invalid offset bounds {max_horizontal}/{max_vertical}"
            )
            .into());
        }
        if (offset_type == 0 && (max_horizontal != 0.0 || max_vertical != 0.0))
            || (offset_type == 1 && max_vertical != 0.0)
        {
            return Err(format!(
                "state {i}: offset type {offset_type} is inconsistent with bounds {max_horizontal}/{max_vertical}"
            )
            .into());
        }
        if offset_type == 0
            && (dump.collision_shape_uses_offset[i] != 0 || dump.outline_shape_uses_offset[i] != 0)
        {
            return Err(format!("state {i}: shape uses an offset but has no offset type").into());
        }
    }

    // Dedupe collision + outline shapes into one dictionary keyed on exact bits.
    let mut shape_dict: Vec<Vec<f64>> = Vec::new();
    let mut shape_dict_index: std::collections::HashMap<Vec<u64>, u32> =
        std::collections::HashMap::new();
    let mut state_shapes = [Vec::with_capacity(n), Vec::with_capacity(n)];
    for i in 0..n {
        for (k, (kind, shape)) in [
            ("collision", &dump.collision_shapes[i]),
            ("outline", &dump.outline_shapes[i]),
        ]
        .into_iter()
        .enumerate()
        {
            if shape.len() % 6 != 0 {
                return Err(format!(
                    "state {i}: {kind} shape has {} coordinates, expected a multiple of 6",
                    shape.len()
                )
                .into());
            }
            for (box_index, b) in shape.as_chunks::<6>().0.iter().enumerate() {
                if !b.iter().all(|v| v.is_finite()) {
                    return Err(format!(
                        "state {i}: {kind} shape box {box_index} has a non-finite coordinate"
                    )
                    .into());
                }
                if b[0] > b[3] || b[1] > b[4] || b[2] > b[5] {
                    return Err(format!(
                        "state {i}: {kind} shape box {box_index} has inverted bounds"
                    )
                    .into());
                }
            }
            state_shapes[k].push(shape_index(shape, &mut shape_dict, &mut shape_dict_index));
        }
    }
    let [state_collision_shapes, state_outline_shapes] = state_shapes;

    // Dedupe face masks into a dictionary, iterating in ascending state-id
    // order so the output is deterministic.
    let mut masks_by_state: BTreeMap<u32, &[String; 6]> = BTreeMap::new();
    for (key, masks) in &dump.face_masks {
        let id: u32 = key
            .parse()
            .map_err(|_| format!("face_masks key '{key}' is not a state id"))?;
        if id as usize >= n {
            return Err(format!("face_masks state id {id} out of range").into());
        }
        for mask in masks {
            if mask.len() != 64 || !mask.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!("state {id}: face mask '{mask}' is not 64 hex chars").into());
            }
        }
        masks_by_state.insert(id, masks);
    }
    for i in 0..n {
        let shaped = dump.can_occlude[i] == 1 && dump.use_shape_for_light_occlusion[i] == 1;
        if shaped != masks_by_state.contains_key(&(i as u32)) {
            return Err(format!(
                "state {i}: face masks {} but can_occlude && use_shape is {shaped}",
                if shaped { "missing" } else { "present" }
            )
            .into());
        }
    }
    let mut dict: Vec<&str> = Vec::new();
    let mut dict_index: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    let mut state_masks: Vec<Option<[u32; 6]>> = vec![None; n];
    for (&id, masks) in &masks_by_state {
        let mut indices = [0u32; 6];
        for (slot, mask) in indices.iter_mut().zip(masks.iter()) {
            *slot = *dict_index.entry(mask).or_insert_with(|| {
                dict.push(mask);
                dict.len() as u32 - 1
            });
        }
        state_masks[id as usize] = Some(indices);
    }

    let mut out = String::new();
    writeln!(out, "{{")?;
    writeln!(
        out,
        "  \"version\": {},",
        serde_json::to_string(&dump.version)?
    )?;
    writeln!(out, "  \"state_count\": {n},")?;
    writeln!(out, "  \"masks\": {},", serde_json::to_string(&dict)?)?;
    writeln!(
        out,
        "  \"shapes\": {},",
        serde_json::to_string(&shape_dict)?
    )?;
    writeln!(out, "  \"blocks\": [")?;
    let mut expected_id = 0u32;
    for (i, block) in blocks.blocks.iter().enumerate() {
        if block.first_id != expected_id {
            return Err(format!(
                "blocks table not dense at '{}': starts at {} expected {expected_id}",
                block.name, block.first_id
            )
            .into());
        }
        let count: u32 = block.props.iter().map(|(_, vs)| vs.len() as u32).product();
        let range = block.first_id as usize..(block.first_id + count) as usize;
        expected_id += count;

        let mut line = format!("    {{\"name\": {}", serde_json::to_string(&block.name)?);
        for (key, values) in [
            ("e", &dump.emission),
            ("d", &dump.dampening),
            ("p", &dump.propagates_skylight_down),
            ("o", &dump.can_occlude),
            ("u", &dump.use_shape_for_light_occlusion),
        ] {
            write!(line, ", \"{key}\": {}", scalar_or_array(values, &range)?)?;
        }
        let collision = &dump.has_collision[range.clone()];
        if collision.iter().any(|&c| c != collision[0]) {
            return Err(format!("{}: has_collision varies across states", block.name).into());
        }
        write!(line, ", \"c\": {}", collision[0])?;
        if let Some(blocks_motion) = &dump.blocks_motion {
            write!(line, ", \"m\": {}", scalar_or_array(blocks_motion, &range)?)?;
        }
        for (key, values) in [
            ("l", scalar_or_array(&dump.legacy_solid, &range)?),
            ("v", scalar_or_array(&dump.replaceable, &range)?),
            ("t", scalar_or_array(&dump.full_face_sturdy, &range)?),
            (
                "co",
                scalar_or_array(&dump.collision_shape_uses_offset, &range)?,
            ),
            (
                "oo",
                scalar_or_array(&dump.outline_shape_uses_offset, &range)?,
            ),
            ("q", scalar_or_array(&dump.position_offset_type, &range)?),
            ("h", scalar_or_array(&dump.max_horizontal_offset, &range)?),
            ("y", scalar_or_array(&dump.max_vertical_offset, &range)?),
            ("s", scalar_or_array(&state_collision_shapes, &range)?),
            ("r", scalar_or_array(&state_outline_shapes, &range)?),
        ] {
            write!(line, ", \"{key}\": {values}")?;
        }
        let masks = &state_masks[range];
        if masks.iter().any(Option::is_some) {
            if masks.iter().all(|m| *m == masks[0]) {
                // Uniform across the block: a single 6-index tuple.
                write!(
                    line,
                    ", \"f\": {}",
                    serde_json::to_string(&masks[0].unwrap())?
                )?;
            } else {
                // Per state: 6-index tuple or null.
                write!(line, ", \"f\": {}", serde_json::to_string(masks)?)?;
            }
        }
        let comma = if i + 1 < blocks.blocks.len() { "," } else { "" };
        writeln!(out, "{line}}}{comma}")?;
    }
    if expected_id as usize != n {
        return Err(format!("blocks cover {expected_id} states, dump has {n}").into());
    }
    writeln!(out, "  ]")?;
    writeln!(out, "}}")?;

    std::fs::write(out_path, &out)?;
    println!(
        "wrote state data for {} states ({} light-shaped, {} distinct masks, {} distinct block shapes) to {out_path}",
        n,
        masks_by_state.len(),
        dict.len(),
        shape_dict.len()
    );
    Ok(())
}

fn shape_index(
    shape: &[f64],
    dict: &mut Vec<Vec<f64>>,
    index: &mut std::collections::HashMap<Vec<u64>, u32>,
) -> u32 {
    let key: Vec<u64> = shape.iter().map(|v| v.to_bits()).collect();
    *index.entry(key).or_insert_with(|| {
        dict.push(shape.to_vec());
        dict.len() as u32 - 1
    })
}

/// A single JSON value when every entry in `range` serializes identically, else
/// the full array.
fn scalar_or_array<T: serde::Serialize>(
    all: &[T],
    range: &std::ops::Range<usize>,
) -> Result<String, Error> {
    let values = &all[range.clone()];
    let first = serde_json::to_string(values.first().ok_or("block with zero states")?)?;
    for value in &values[1..] {
        if serde_json::to_string(value)? != first {
            return Ok(serde_json::to_string(values)?);
        }
    }
    Ok(first)
}
