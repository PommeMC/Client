//! `commands` (`ClientboundCommandsPacket`) as vanilla 26.2 lays it out: the
//! server's Brigadier tree as a flat node list and the root's index.

use crate::codec::{DecodeResult, MAX_STRING_LENGTH, Reader};
use crate::registries::{ClientRegistry, RegistryTable};

#[derive(Clone, Debug, PartialEq)]
pub struct CommandsPacket {
    pub nodes: Vec<Node>,
    pub root_index: u32,
}

/// `ClientboundCommandsPacket.Entry`, with its flags unpacked.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub executable: bool,
    pub restricted: bool,
    pub children: Vec<u32>,
    pub redirect: Option<u32>,
    pub stub: NodeStub,
}

impl Node {
    pub fn name(&self) -> Option<&str> {
        match &self.stub {
            NodeStub::Root => None,
            NodeStub::Literal { name } | NodeStub::Argument { name, .. } => Some(name),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeStub {
    Root,
    Literal {
        name: String,
    },
    Argument {
        name: String,
        argument: ArgumentType,
        /// The `SuggestionProviders` id the server completes it with.
        suggestions: Option<String>,
    },
}

/// An `ArgumentTypeInfo.Template`, one variant per `command_argument_type`
/// entry.
#[derive(Clone, Debug, PartialEq)]
pub enum ArgumentType {
    Bool,
    Float(Bounds<f32>),
    Double(Bounds<f64>),
    Integer(Bounds<i32>),
    Long(Bounds<i64>),
    String(StringType),
    Entity(EntityArgument),
    GameProfile,
    BlockPos,
    ColumnPos,
    Vec3,
    Vec2,
    BlockState,
    BlockPredicate,
    ItemStack,
    ItemPredicate,
    TeamColor,
    HexColor,
    Component,
    Style,
    Message,
    NbtCompoundTag,
    NbtTag,
    NbtPath,
    Objective,
    ObjectiveCriteria,
    Operation,
    Particle,
    Angle,
    Rotation,
    ScoreboardSlot,
    ScoreHolder { allows_multiple: bool },
    Swizzle,
    Team,
    ItemSlot,
    ItemSlots,
    Identifier,
    Function,
    EntityAnchor,
    IntRange,
    FloatRange,
    Dimension,
    GameMode,
    Time { min: i32 },
    ResourceOrTag { registry: String },
    ResourceOrTagKey { registry: String },
    Resource { registry: String },
    ResourceKey { registry: String },
    ResourceSelector { registry: String },
    TemplateMirror,
    TemplateRotation,
    Heightmap,
    LootTable,
    LootPredicate,
    LootModifier,
    Dialog,
    Uuid,
}

/// A number argument's bounds; `None` is the type's extreme, which vanilla
/// leaves off the wire.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds<T> {
    pub min: Option<T>,
    pub max: Option<T>,
}

/// `StringArgumentType.StringType`, in ordinal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringType {
    SingleWord,
    QuotablePhrase,
    GreedyPhrase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityArgument {
    pub single: bool,
    pub players_only: bool,
}

const MASK_TYPE: u8 = 3;
const FLAG_EXECUTABLE: u8 = 4;
const FLAG_REDIRECT: u8 = 8;
const FLAG_CUSTOM_SUGGESTIONS: u8 = 16;
const FLAG_RESTRICTED: u8 = 32;
const TYPE_LITERAL: u8 = 1;
const TYPE_ARGUMENT: u8 = 2;

/// Decodes packet `name`, or `None` if it isn't `commands`.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<CommandsPacket>> {
    (name == "commands").then(|| {
        // ClientboundCommandsPacket.java:76-80
        let nodes = r.list(usize::MAX, read_node)?;
        let root_index = r.varint()?;
        validate(r, &nodes, |node, unbuilt| {
            node.redirect
                .is_none_or(|redirect| !pending(unbuilt, redirect))
        })?;
        validate(r, &nodes, |node, unresolved| {
            node.children
                .iter()
                .all(|&child| !pending(unresolved, child))
        })?;
        Ok(CommandsPacket { nodes, root_index })
    })
}

/// `ClientboundCommandsPacket.readNode`.
fn read_node(r: &mut Reader) -> DecodeResult<Node> {
    let flags = r.u8()?;
    // `readVarIntArray`, at most one element per readable byte.
    let children = r.list(usize::MAX, Reader::varint)?;
    let redirect = if flags & FLAG_REDIRECT != 0 {
        Some(r.varint()?)
    } else {
        None
    };
    let stub = match flags & MASK_TYPE {
        TYPE_ARGUMENT => NodeStub::Argument {
            name: r.utf(MAX_STRING_LENGTH)?.to_owned(),
            argument: argument_type(r)?,
            suggestions: if flags & FLAG_CUSTOM_SUGGESTIONS != 0 {
                Some(r.identifier()?.to_owned())
            } else {
                None
            },
        },
        TYPE_LITERAL => NodeStub::Literal {
            name: r.utf(MAX_STRING_LENGTH)?.to_owned(),
        },
        // `read` returns no stub for the root and the unused fourth type.
        _ => NodeStub::Root,
    };
    Ok(Node {
        executable: flags & FLAG_EXECUTABLE != 0,
        restricted: flags & FLAG_RESTRICTED != 0,
        children,
        redirect,
        stub,
    })
}

/// A `command_argument_type` id and its info's `deserializeFromNetwork`.
/// Vanilla leaves an unknown id's node without a stub and misreads the rest
/// of the packet; this fails the decode there instead.
fn argument_type(r: &mut Reader) -> DecodeResult<ArgumentType> {
    use ArgumentType as A;
    let registry = |r: &mut Reader| r.identifier().map(str::to_owned);
    let name = RegistryTable::native()
        .name_of(ClientRegistry::CommandArgumentType, r.varint()?)
        .ok_or_else(|| r.error("unknown command argument type"))?;
    Ok(match name {
        "brigadier:bool" => A::Bool,
        "brigadier:float" => A::Float(bounds(r, Reader::f32)?),
        "brigadier:double" => A::Double(bounds(r, Reader::f64)?),
        "brigadier:integer" => A::Integer(bounds(r, Reader::i32)?),
        "brigadier:long" => A::Long(bounds(r, Reader::i64)?),
        "brigadier:string" => A::String(match r.varint()? {
            0 => StringType::SingleWord,
            1 => StringType::QuotablePhrase,
            2 => StringType::GreedyPhrase,
            _ => return Err(r.error("unknown string argument type")),
        }),
        "entity" => {
            // EntityArgument.java:184
            let flags = r.u8()?;
            A::Entity(EntityArgument {
                single: flags & 1 != 0,
                players_only: flags & 2 != 0,
            })
        }
        "game_profile" => A::GameProfile,
        "block_pos" => A::BlockPos,
        "column_pos" => A::ColumnPos,
        "vec3" => A::Vec3,
        "vec2" => A::Vec2,
        "block_state" => A::BlockState,
        "block_predicate" => A::BlockPredicate,
        "item_stack" => A::ItemStack,
        "item_predicate" => A::ItemPredicate,
        "team_color" => A::TeamColor,
        "hex_color" => A::HexColor,
        "component" => A::Component,
        "style" => A::Style,
        "message" => A::Message,
        "nbt_compound_tag" => A::NbtCompoundTag,
        "nbt_tag" => A::NbtTag,
        "nbt_path" => A::NbtPath,
        "objective" => A::Objective,
        "objective_criteria" => A::ObjectiveCriteria,
        "operation" => A::Operation,
        "particle" => A::Particle,
        "angle" => A::Angle,
        "rotation" => A::Rotation,
        "scoreboard_slot" => A::ScoreboardSlot,
        // ScoreHolderArgument.java:217
        "score_holder" => A::ScoreHolder {
            allows_multiple: r.u8()? & 1 != 0,
        },
        "swizzle" => A::Swizzle,
        "team" => A::Team,
        "item_slot" => A::ItemSlot,
        "item_slots" => A::ItemSlots,
        "resource_location" => A::Identifier,
        "function" => A::Function,
        "entity_anchor" => A::EntityAnchor,
        "int_range" => A::IntRange,
        "float_range" => A::FloatRange,
        "dimension" => A::Dimension,
        "gamemode" => A::GameMode,
        "time" => A::Time { min: r.i32()? },
        "resource_or_tag" => A::ResourceOrTag {
            registry: registry(r)?,
        },
        "resource_or_tag_key" => A::ResourceOrTagKey {
            registry: registry(r)?,
        },
        "resource" => A::Resource {
            registry: registry(r)?,
        },
        "resource_key" => A::ResourceKey {
            registry: registry(r)?,
        },
        "resource_selector" => A::ResourceSelector {
            registry: registry(r)?,
        },
        "template_mirror" => A::TemplateMirror,
        "template_rotation" => A::TemplateRotation,
        "heightmap" => A::Heightmap,
        "loot_table" => A::LootTable,
        "loot_predicate" => A::LootPredicate,
        "loot_modifier" => A::LootModifier,
        "dialog" => A::Dialog,
        "uuid" => A::Uuid,
        _ => return Err(r.error("command argument type pomme doesn't know")),
    })
}

/// The number infos' `deserializeFromNetwork`: `ArgumentUtils` flags (1 for
/// a minimum, 2 for a maximum), then each bound present.
fn bounds<'a, T>(
    r: &mut Reader<'a>,
    read: fn(&mut Reader<'a>) -> DecodeResult<T>,
) -> DecodeResult<Bounds<T>> {
    let flags = r.u8()?;
    let min = if flags & 1 != 0 { Some(read(r)?) } else { None };
    let max = if flags & 2 != 0 { Some(read(r)?) } else { None };
    Ok(Bounds { min, max })
}

/// `ClientboundCommandsPacket.validateEntries`: repeatedly drops the nodes
/// `ready` accepts given the ones still pending, failing when a pass drops
/// none.
fn validate(
    r: &Reader,
    nodes: &[Node],
    ready: impl Fn(&Node, &[bool]) -> bool,
) -> DecodeResult<()> {
    let mut pending = vec![true; nodes.len()];
    let mut left = nodes.len();
    while left > 0 {
        let before = left;
        for (i, node) in nodes.iter().enumerate() {
            if pending[i] && ready(node, &pending) {
                pending[i] = false;
                left -= 1;
            }
        }
        if left == before {
            return Err(r.error("impossible command tree"));
        }
    }
    Ok(())
}

/// Whether node `index` is still pending; an index past the list never is,
/// as in vanilla's set.
fn pending(set: &[bool], index: u32) -> bool {
    set.get(index as usize).copied().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vanilla_fixtures::case;

    fn decode(bytes: &[u8]) -> DecodeResult<CommandsPacket> {
        let mut r = Reader::new(bytes);
        let packet = read("commands", &mut r).expect("a commands packet")?;
        r.ensure_eof()?;
        Ok(packet)
    }

    fn arguments(packet: &CommandsPacket) -> Vec<(&str, &ArgumentType)> {
        packet
            .nodes
            .iter()
            .filter_map(|node| match &node.stub {
                NodeStub::Argument { name, argument, .. } => Some((name.as_str(), argument)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_tree_reads_as_vanilla_writes_it() {
        let packet = decode(&case("commands/tree")).unwrap();
        let root = &packet.nodes[packet.root_index as usize];
        assert_eq!(root.stub, NodeStub::Root);
        let names: Vec<_> = root
            .children
            .iter()
            .map(|&i| packet.nodes[i as usize].name().unwrap())
            .collect();
        assert_eq!(names, ["tp", "say", "give", "alias"]);

        let alias = packet
            .nodes
            .iter()
            .find(|n| n.name() == Some("alias"))
            .unwrap();
        assert_eq!(
            alias.redirect.map(|i| packet.nodes[i as usize].name()),
            Some(Some("tp"))
        );
        let say = packet
            .nodes
            .iter()
            .find(|n| n.name() == Some("say"))
            .unwrap();
        assert!(say.restricted);
        assert!(!say.executable);

        let message = packet
            .nodes
            .iter()
            .find(|n| n.name() == Some("message"))
            .unwrap();
        assert!(message.executable);
        let NodeStub::Argument { suggestions, .. } = &message.stub else {
            panic!("message is an argument");
        };
        assert_eq!(suggestions.as_deref(), Some("minecraft:ask_server"));

        // Nodes are numbered breadth first from the root.
        assert_eq!(
            arguments(&packet),
            [
                (
                    "target",
                    &ArgumentType::Entity(EntityArgument {
                        single: true,
                        players_only: false
                    })
                ),
                ("message", &ArgumentType::Message),
                (
                    "item",
                    &ArgumentType::ResourceKey {
                        registry: "minecraft:item".into()
                    }
                ),
                ("pos", &ArgumentType::Vec3),
                (
                    "count",
                    &ArgumentType::Integer(Bounds {
                        min: Some(1),
                        max: Some(64)
                    })
                ),
                (
                    "scale",
                    &ArgumentType::Double(Bounds {
                        min: None,
                        max: Some(2.5)
                    })
                ),
                ("word", &ArgumentType::String(StringType::GreedyPhrase)),
                ("delay", &ArgumentType::Time { min: 1 }),
                (
                    "holders",
                    &ArgumentType::ScoreHolder {
                        allows_multiple: true
                    }
                ),
                ("id", &ArgumentType::Uuid),
            ]
        );
    }

    #[test]
    fn an_impossible_tree_fails_to_decode() {
        // Two literal nodes, each the other's only child.
        let mut bytes = vec![2];
        for (child, name) in [(1, b'a'), (0, b'b')] {
            bytes.extend_from_slice(&[TYPE_LITERAL, 1, child, 1, name]);
        }
        bytes.push(0);
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn a_redirect_cycle_fails_to_decode() {
        // A root whose only node redirects to itself.
        let bytes = [1, FLAG_REDIRECT, 0, 0, 0];
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn an_unknown_argument_type_fails_to_decode() {
        let bytes = [1, TYPE_ARGUMENT, 0, 1, b'x', 57, 0];
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn every_native_argument_type_is_known() {
        let table = RegistryTable::native();
        for (id, name) in table
            .names(ClientRegistry::CommandArgumentType)
            .iter()
            .enumerate()
        {
            // Zeros cover every type's properties: no bounds, the first
            // string type, no flags, a zero `time` minimum, an empty key.
            let bytes = [id as u8, 0, 0, 0, 0];
            assert!(argument_type(&mut Reader::new(&bytes)).is_ok(), "{name}");
        }
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("set_camera", &mut Reader::new(&[])).is_none());
    }
}
