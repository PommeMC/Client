//! Generates pomme's per-version block data from Mojang's data-generator
//! reports (`java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar
//! --reports`).
//!
//! Usage:
//!   blockgen blocks <reports/blocks.json> <version> <out.json>
//!   blockgen behavior <azalea generated.rs> <out.json>
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
//! hand from the decompiled `Blocks.java`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, report, version, out] if cmd == "blocks" => gen_blocks(report, version, out),
        [cmd, generated, out] if cmd == "behavior" => gen_behavior(generated, out),
        _ => Err("usage: blockgen blocks <blocks.json> <version> <out.json>\n       blockgen behavior <generated.rs> <out.json>".into()),
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
    Ok(())
}

fn extract_float_arg(text: &str, method: &str) -> Option<f32> {
    let start = text.find(method)? + method.len();
    let rest = &text[start..];
    let end = rest.find([',', ')'])?;
    rest[..end].trim().parse().ok()
}
