//! Generates pomme's per-version packet-id tables from the decompiled
//! vanilla protocol registrations (`<Phase>Protocols.java`). Wire ids are
//! implicit from registration order, so the table is the ordered list of
//! packet resource names per phase and direction; id == index.
//!
//! Usage:
//!   protogen <decompiled-root> <version> <out.json> [--protocol N]
//!   e.g. protogen reference/26.2/decompiled 26.2 \
//!        pomme-protocol/src/data/protocol-26.2.json
//!
//! The protocol number is parsed from `SharedConstants.getProtocolVersion()`;
//! `--protocol` overrides it (and is required if the method body isn't a bare
//! integer literal). The parser hard-fails on anything it can't resolve
//! rather than emit silently-wrong data.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

type Error = Box<dyn std::error::Error>;

/// (JSON key, `<Phase>Protocols.java` path, has a clientbound template).
const PHASES: [(&str, &str, bool); 5] = [
    ("handshake", "handshake/HandshakeProtocols.java", false),
    ("status", "status/StatusProtocols.java", true),
    ("login", "login/LoginProtocols.java", true),
    (
        "configuration",
        "configuration/ConfigurationProtocols.java",
        true,
    ),
    ("game", "game/GameProtocols.java", true),
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (root, version, out, protocol_override) = match args.as_slice() {
        [root, version, out] => (root, version, out, None),
        [root, version, out, flag, n] if flag == "--protocol" => match n.parse::<i32>() {
            Ok(n) => (root, version, out, Some(n)),
            Err(_) => {
                eprintln!("protogen: --protocol expects an integer, got {n}");
                return ExitCode::FAILURE;
            }
        },
        _ => {
            eprintln!("usage: protogen <decompiled-root> <version> <out.json> [--protocol N]");
            return ExitCode::FAILURE;
        }
    };
    match generate(Path::new(root), version, out, protocol_override) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("protogen: {e}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Direction {
    Serverbound,
    Clientbound,
}

/// Resource name + direction for every `PacketType<...>` constant, keyed two
/// ways: by `TypesClass.CONSTANT` (addPacket args) and by packet class name
/// (the bundle-delimiter instance in `withBundlePacket`).
struct TypeMaps {
    by_constant: HashMap<(String, String), (Direction, String)>,
    by_class: HashMap<String, (Direction, String)>,
}

fn generate(
    root: &Path,
    version: &str,
    out_path: &str,
    protocol_override: Option<i32>,
) -> Result<(), Error> {
    let proto_dir = root.join("net/minecraft/network/protocol");
    let protocol = resolve_protocol_number(root, protocol_override)?;
    let maps = collect_packet_types(&proto_dir)?;

    let mut out = String::from("{\n");
    writeln!(out, "  \"version\": \"{version}\",")?;
    writeln!(out, "  \"protocol\": {protocol},")?;

    for (i, (key, file, has_clientbound)) in PHASES.iter().enumerate() {
        let source =
            std::fs::read_to_string(proto_dir.join(file)).map_err(|e| format!("{file}: {e}"))?;
        let serverbound = parse_template(&source, file, Direction::Serverbound, &maps)?
            .ok_or_else(|| format!("{file}: no SERVERBOUND_TEMPLATE"))?;
        let clientbound = parse_template(&source, file, Direction::Clientbound, &maps)?;
        match (clientbound.is_some(), has_clientbound) {
            (false, true) => return Err(format!("{file}: no CLIENTBOUND_TEMPLATE").into()),
            (true, false) => {
                return Err(format!("{file}: unexpected CLIENTBOUND_TEMPLATE").into());
            }
            _ => {}
        }
        let clientbound = clientbound.unwrap_or_default();

        println!(
            "{key}: {} serverbound, {} clientbound",
            serverbound.len(),
            clientbound.len()
        );
        writeln!(out, "  \"{key}\": {{")?;
        write_list(&mut out, "serverbound", &serverbound, ",")?;
        write_list(&mut out, "clientbound", &clientbound, "")?;
        writeln!(out, "  }}{}", if i + 1 < PHASES.len() { "," } else { "" })?;
    }
    out.push_str("}\n");

    std::fs::write(out_path, out)?;
    println!("wrote {out_path}");
    Ok(())
}

/// `SharedConstants.getProtocolVersion()`'s literal return value, unless
/// overridden (the override wins with a warning on disagreement).
fn resolve_protocol_number(root: &Path, over: Option<i32>) -> Result<i32, Error> {
    let path = root.join("net/minecraft/SharedConstants.java");
    let source = std::fs::read_to_string(&path).map_err(|e| format!("{path:?}: {e}"))?;
    let parsed = source
        .find("public static int getProtocolVersion()")
        .and_then(|at| {
            let body = &source[at..source[at..].find('}')? + at];
            let ret = &body[body.find("return ")? + "return ".len()..];
            ret[..ret.find(';')?].trim().parse::<i32>().ok()
        });
    match (parsed, over) {
        (Some(p), Some(o)) if p != o => {
            eprintln!("warning: SharedConstants says {p}, --protocol {o} wins");
            Ok(o)
        }
        (_, Some(o)) => Ok(o),
        (Some(p), None) => Ok(p),
        (None, None) => {
            Err("getProtocolVersion() body is not a bare integer literal; pass --protocol N".into())
        }
    }
}

/// Scans every `*PacketTypes.java` under the protocol dir for
/// `public static final PacketType<Class> CONSTANT =
/// Types.create⟨Dir⟩bound("res");`.
fn collect_packet_types(proto_dir: &Path) -> Result<TypeMaps, Error> {
    let mut maps = TypeMaps {
        by_constant: HashMap::new(),
        by_class: HashMap::new(),
    };
    let mut stack = vec![proto_dir.to_path_buf()];
    let mut files: Vec<PathBuf> = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("PacketTypes.java"))
            {
                files.push(path);
            }
        }
    }
    if files.is_empty() {
        return Err(format!("no *PacketTypes.java found under {proto_dir:?}").into());
    }

    for path in files {
        let types_class = path.file_stem().unwrap().to_str().unwrap().to_string();
        let source = std::fs::read_to_string(&path)?;
        for line in source.lines() {
            let Some(rest) = line.trim().strip_prefix("public static final PacketType<") else {
                continue;
            };
            let (class, constant, direction, resource) = parse_type_decl(rest)
                .ok_or_else(|| format!("{types_class}: unparsable PacketType line: {line}"))?;
            maps.by_constant.insert(
                (types_class.clone(), constant),
                (direction, resource.clone()),
            );
            maps.by_class.insert(class, (direction, resource));
        }
    }
    Ok(maps)
}

/// Parses `Class> CONSTANT = Types.create⟨Dir⟩bound("res");` (the remainder
/// after the stripped declaration prefix).
fn parse_type_decl(rest: &str) -> Option<(String, String, Direction, String)> {
    let (class, rest) = rest.split_once('>')?;
    let (constant, rest) = rest.trim().split_once(" = ")?;
    let (_, call) = rest.split_once('.')?;
    let direction = if call.starts_with("createServerbound(") {
        Direction::Serverbound
    } else if call.starts_with("createClientbound(") {
        Direction::Clientbound
    } else {
        return None;
    };
    let resource = call.split_once('"')?.1.split_once('"')?.0;
    Some((
        class.trim().to_string(),
        constant.trim().to_string(),
        direction,
        resource.to_string(),
    ))
}

/// Extracts one direction's ordered resource names from a Protocols file, or
/// `None` if the direction has no template (handshake clientbound).
fn parse_template(
    source: &str,
    file: &str,
    direction: Direction,
    maps: &TypeMaps,
) -> Result<Option<Vec<String>>, Error> {
    let needle = match direction {
        Direction::Serverbound => " SERVERBOUND_TEMPLATE = ",
        Direction::Clientbound => " CLIENTBOUND_TEMPLATE = ",
    };
    let Some(start) = source.find(needle) else {
        return Ok(None);
    };
    let statement = &source[start..];
    let statement = &statement[..statement
        .find(';')
        .ok_or_else(|| format!("{file}: unterminated template statement"))?];

    // Registration order is wire-id order, so collect both call kinds by
    // position; `withBundlePacket` registers the *delimiter*'s type.
    let mut calls: Vec<(usize, bool)> = statement
        .match_indices(".addPacket(")
        .map(|(at, _)| (at, false))
        .chain(
            statement
                .match_indices(".withBundlePacket(")
                .map(|(at, _)| (at, true)),
        )
        .collect();
    calls.sort_unstable_by_key(|&(at, _)| at);
    if calls.is_empty() {
        return Err(format!("{file}: template has no packet registrations").into());
    }

    let mut names = Vec::with_capacity(calls.len());
    for (at, is_bundle) in calls {
        let args = &statement[statement[at..].find('(').unwrap() + at + 1..];
        let (resolved_dir, resource) = if is_bundle {
            // Third argument is `new <DelimiterClass>()`; that class's
            // PacketType is what occupies the wire id.
            let class = args
                .split(',')
                .nth(2)
                .and_then(|arg| arg.trim().strip_prefix("new "))
                .and_then(|arg| arg.split('(').next())
                .ok_or_else(|| format!("{file}: unparsable withBundlePacket args"))?;
            maps.by_class
                .get(class.trim())
                .ok_or_else(|| format!("{file}: no PacketType for bundle class {class}"))?
        } else {
            let constant = args
                .split(',')
                .next()
                .ok_or_else(|| format!("{file}: empty addPacket args"))?
                .trim();
            let (types_class, name) = constant
                .split_once('.')
                .ok_or_else(|| format!("{file}: unqualified packet constant {constant}"))?;
            maps.by_constant
                .get(&(types_class.to_string(), name.to_string()))
                .ok_or_else(|| format!("{file}: unknown packet constant {constant}"))?
        };
        if *resolved_dir != direction {
            return Err(format!(
                "{file}: {resource} registered {direction:?} but declared {resolved_dir:?}"
            )
            .into());
        }
        if names.contains(resource) {
            return Err(format!("{file}: duplicate packet {resource}").into());
        }
        names.push(resource.clone());
    }
    Ok(Some(names))
}

fn write_list(out: &mut String, key: &str, names: &[String], trailing: &str) -> Result<(), Error> {
    if names.is_empty() {
        writeln!(out, "    \"{key}\": []{trailing}")?;
        return Ok(());
    }
    writeln!(out, "    \"{key}\": [")?;
    for (i, name) in names.iter().enumerate() {
        let comma = if i + 1 < names.len() { "," } else { "" };
        writeln!(out, "      \"{name}\"{comma}")?;
    }
    writeln!(out, "    ]{trailing}")?;
    Ok(())
}
