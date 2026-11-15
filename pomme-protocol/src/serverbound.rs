//! The serverbound game packets as vanilla 26.2 lays them out, each encoded
//! as its `STREAM_CODEC` writes it.

use glam::DVec3;

use crate::item::ItemStack;
use crate::packets::{PacketTable, Phase};
use crate::wire::{write_block_pos, write_utf, write_varint};

#[derive(Clone, Debug, PartialEq)]
pub enum GamePacket {
    AcceptTeleportation {
        id: i32,
    },
    BundleItemSelected {
        slot: i32,
        /// -1 for none.
        index: i32,
    },
    /// `GameType.getId`.
    ChangeGameMode {
        mode: u32,
    },
    ChunkBatchReceived {
        desired_chunks_per_tick: f32,
    },
    ClientCommand(ClientCommand),
    ClientInformation(ClientInformation),
    ClientTickEnd,
    CommandSuggestion {
        id: i32,
        command: String,
    },
    ConfigurationAcknowledged,
    ContainerButtonClick {
        container_id: i32,
        button_id: i32,
    },
    ContainerClick(ContainerClick),
    ContainerClose {
        container_id: i32,
    },
    ContainerSlotStateChanged {
        slot: i32,
        container_id: i32,
        new_state: bool,
    },
    /// A payload vanilla has no type for: its id, then its bytes.
    CustomPayload {
        id: String,
        data: Vec<u8>,
    },
    KeepAlive {
        id: i64,
    },
    MovePlayerPos {
        pos: DVec3,
        flags: MoveFlags,
    },
    MovePlayerPosRot {
        pos: DVec3,
        y_rot: f32,
        x_rot: f32,
        flags: MoveFlags,
    },
    MovePlayerRot {
        y_rot: f32,
        x_rot: f32,
        flags: MoveFlags,
    },
    MovePlayerStatusOnly {
        flags: MoveFlags,
    },
    PlayerAbilities {
        flying: bool,
    },
    PlayerAction {
        action: PlayerAction,
        pos: (i32, i32, i32),
        direction: Direction,
        sequence: i32,
    },
    PlayerCommand {
        entity: i32,
        action: PlayerCommand,
        data: i32,
    },
    PlayerInput(Input),
    PlayerLoaded,
    Pong {
        id: i32,
    },
    RenameItem {
        name: String,
    },
    ResourcePack {
        id: u128,
        action: ResourcePackAction,
    },
    SetCarriedItem {
        slot: i16,
    },
    SetCreativeModeSlot {
        slot: i16,
        stack: Option<ItemStack>,
    },
    SignUpdate {
        pos: (i32, i32, i32),
        is_front_text: bool,
        lines: [String; 4],
    },
    Swing {
        hand: InteractionHand,
    },
    TeleportToEntity {
        uuid: u128,
    },
    UseItem {
        hand: InteractionHand,
        sequence: i32,
        y_rot: f32,
        x_rot: f32,
    },
    UseItemOn {
        hand: InteractionHand,
        hit: BlockHit,
        sequence: i32,
    },
}

/// `ServerboundClientCommandPacket.Action`, in ordinal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientCommand {
    PerformRespawn,
    RequestStats,
    RequestGameRuleValues,
}

/// `ClientInformation`. The enums travel as their ordinals: `ChatVisiblity`
/// (full, system, hidden), `HumanoidArm` (left, right) and `ParticleStatus`
/// (all, decreased, minimal).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientInformation {
    pub language: String,
    pub view_distance: u8,
    pub chat_visibility: u32,
    pub chat_colors: bool,
    /// `PlayerModelPart` mask bits.
    pub model_customisation: u8,
    pub main_hand: u32,
    pub text_filtering_enabled: bool,
    pub allows_listing: bool,
    pub particle_status: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerClick {
    pub container_id: i32,
    pub state_id: i32,
    pub slot: i16,
    pub button: i8,
    pub input: ContainerInput,
    pub changed_slots: Vec<(i16, Option<HashedStack>)>,
    pub carried: Option<HashedStack>,
}

/// `ContainerInput`, in id order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerInput {
    Pickup,
    QuickMove,
    Swap,
    Clone,
    Throw,
    QuickCraft,
    PickupAll,
}

/// `HashedStack.ActualItem`: a stack with each component's value replaced by
/// its CRC32C hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashedStack {
    pub item: u32,
    pub count: i32,
    /// Component-type id and value hash.
    pub added: Vec<(u32, i32)>,
    pub removed: Vec<u32>,
}

/// `ServerboundMovePlayerPacket.packFlags`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MoveFlags {
    pub on_ground: bool,
    pub horizontal_collision: bool,
}

/// `ServerboundPlayerActionPacket.Action`, in ordinal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerAction {
    StartDestroyBlock,
    AbortDestroyBlock,
    StopDestroyBlock,
    DropAllItems,
    DropItem,
    ReleaseUseItem,
    SwapItemWithOffhand,
    Stab,
}

/// `Direction`, by `get3DDataValue`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

/// `ServerboundPlayerCommandPacket.Action`, in ordinal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerCommand {
    StopSleeping,
    StartSprinting,
    StopSprinting,
    StartRidingJump,
    StopRidingJump,
    OpenInventory,
    StartFallFlying,
}

/// `Input`, one flag bit per key in field order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub shift: bool,
    pub sprint: bool,
}

/// `ServerboundResourcePackPacket.Action`, in ordinal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourcePackAction {
    SuccessfullyLoaded,
    Declined,
    FailedDownload,
    Accepted,
    Downloaded,
    InvalidUrl,
    FailedReload,
    Discarded,
}

/// `InteractionHand`, in ordinal order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InteractionHand {
    #[default]
    MainHand,
    OffHand,
}

/// `BlockHitResult` as `FriendlyByteBuf.writeBlockHitResult` sends it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockHit {
    pub pos: (i32, i32, i32),
    pub direction: Direction,
    /// The hit point in world space; the wire carries its offset from `pos`.
    pub location: DVec3,
    pub inside: bool,
    pub world_border: bool,
}

impl GamePacket {
    /// The packet's name in the packet table.
    pub fn name(&self) -> &'static str {
        match self {
            Self::AcceptTeleportation { .. } => "accept_teleportation",
            Self::BundleItemSelected { .. } => "bundle_item_selected",
            Self::ChangeGameMode { .. } => "change_game_mode",
            Self::ChunkBatchReceived { .. } => "chunk_batch_received",
            Self::ClientCommand(_) => "client_command",
            Self::ClientInformation(_) => "client_information",
            Self::ClientTickEnd => "client_tick_end",
            Self::CommandSuggestion { .. } => "command_suggestion",
            Self::ConfigurationAcknowledged => "configuration_acknowledged",
            Self::ContainerButtonClick { .. } => "container_button_click",
            Self::ContainerClick(_) => "container_click",
            Self::ContainerClose { .. } => "container_close",
            Self::ContainerSlotStateChanged { .. } => "container_slot_state_changed",
            Self::CustomPayload { .. } => "custom_payload",
            Self::KeepAlive { .. } => "keep_alive",
            Self::MovePlayerPos { .. } => "move_player_pos",
            Self::MovePlayerPosRot { .. } => "move_player_pos_rot",
            Self::MovePlayerRot { .. } => "move_player_rot",
            Self::MovePlayerStatusOnly { .. } => "move_player_status_only",
            Self::PlayerAbilities { .. } => "player_abilities",
            Self::PlayerAction { .. } => "player_action",
            Self::PlayerCommand { .. } => "player_command",
            Self::PlayerInput(_) => "player_input",
            Self::PlayerLoaded => "player_loaded",
            Self::Pong { .. } => "pong",
            Self::RenameItem { .. } => "rename_item",
            Self::ResourcePack { .. } => "resource_pack",
            Self::SetCarriedItem { .. } => "set_carried_item",
            Self::SetCreativeModeSlot { .. } => "set_creative_mode_slot",
            Self::SignUpdate { .. } => "sign_update",
            Self::Swing { .. } => "swing",
            Self::TeleportToEntity { .. } => "teleport_to_entity",
            Self::UseItem { .. } => "use_item",
            Self::UseItemOn { .. } => "use_item_on",
        }
    }

    /// The native frame: the packet id, then the body.
    pub fn encode(&self) -> Vec<u8> {
        self.encode_in(Phase::Game)
            .unwrap_or_else(|| panic!("{} missing from packet table", self.name()))
    }

    /// The native frame of a packet `phase` shares with the game phase (the
    /// common packets); `None` when `phase` has no such packet.
    pub fn encode_in(&self, phase: Phase) -> Option<Vec<u8>> {
        let mut buf = PacketTable::native().serverbound_frame(phase, self.name())?;
        self.write(&mut buf);
        Some(buf)
    }

    fn write(&self, buf: &mut Vec<u8>) {
        match self {
            Self::AcceptTeleportation { id } => var_i32(buf, *id),
            Self::BundleItemSelected { slot, index } => {
                var_i32(buf, *slot);
                var_i32(buf, *index);
            }
            Self::ChangeGameMode { mode } => write_varint(buf, *mode),
            Self::ChunkBatchReceived {
                desired_chunks_per_tick,
            } => buf.extend(desired_chunks_per_tick.to_be_bytes()),
            Self::ClientCommand(action) => write_varint(buf, *action as u32),
            // ClientInformation.java:27-37
            Self::ClientInformation(info) => {
                write_utf(buf, &info.language);
                buf.push(info.view_distance);
                write_varint(buf, info.chat_visibility);
                buf.push(u8::from(info.chat_colors));
                buf.push(info.model_customisation);
                write_varint(buf, info.main_hand);
                buf.push(u8::from(info.text_filtering_enabled));
                buf.push(u8::from(info.allows_listing));
                write_varint(buf, info.particle_status);
            }
            Self::ClientTickEnd | Self::ConfigurationAcknowledged | Self::PlayerLoaded => {}
            Self::CommandSuggestion { id, command } => {
                var_i32(buf, *id);
                write_utf(buf, command);
            }
            Self::ContainerButtonClick {
                container_id,
                button_id,
            } => {
                var_i32(buf, *container_id);
                var_i32(buf, *button_id);
            }
            // ServerboundContainerClickPacket.java:27-28
            Self::ContainerClick(click) => {
                var_i32(buf, click.container_id);
                var_i32(buf, click.state_id);
                buf.extend(click.slot.to_be_bytes());
                buf.push(click.button as u8);
                write_varint(buf, click.input as u32);
                write_varint(buf, click.changed_slots.len() as u32);
                for (slot, stack) in &click.changed_slots {
                    buf.extend(slot.to_be_bytes());
                    hashed_stack(buf, stack.as_ref());
                }
                hashed_stack(buf, click.carried.as_ref());
            }
            Self::ContainerClose { container_id } => var_i32(buf, *container_id),
            Self::ContainerSlotStateChanged {
                slot,
                container_id,
                new_state,
            } => {
                var_i32(buf, *slot);
                var_i32(buf, *container_id);
                buf.push(u8::from(*new_state));
            }
            Self::CustomPayload { id, data } => {
                write_utf(buf, id);
                buf.extend_from_slice(data);
            }
            Self::KeepAlive { id } => buf.extend(id.to_be_bytes()),
            Self::MovePlayerPos { pos, flags } => {
                vec3(buf, *pos);
                buf.push(flags.pack());
            }
            Self::MovePlayerPosRot {
                pos,
                y_rot,
                x_rot,
                flags,
            } => {
                vec3(buf, *pos);
                rotation(buf, *y_rot, *x_rot);
                buf.push(flags.pack());
            }
            Self::MovePlayerRot {
                y_rot,
                x_rot,
                flags,
            } => {
                rotation(buf, *y_rot, *x_rot);
                buf.push(flags.pack());
            }
            Self::MovePlayerStatusOnly { flags } => buf.push(flags.pack()),
            // ServerboundPlayerAbilitiesPacket.java:29-35: only FLAG_FLYING.
            Self::PlayerAbilities { flying } => buf.push(u8::from(*flying) << 1),
            Self::PlayerAction {
                action,
                pos,
                direction,
                sequence,
            } => {
                write_varint(buf, *action as u32);
                write_block_pos(buf, *pos);
                buf.push(*direction as u8);
                var_i32(buf, *sequence);
            }
            Self::PlayerCommand {
                entity,
                action,
                data,
            } => {
                var_i32(buf, *entity);
                write_varint(buf, *action as u32);
                var_i32(buf, *data);
            }
            // Input.java:10-30
            Self::PlayerInput(input) => buf.push(
                [
                    input.forward,
                    input.backward,
                    input.left,
                    input.right,
                    input.jump,
                    input.shift,
                    input.sprint,
                ]
                .iter()
                .enumerate()
                .fold(0, |flags, (bit, &held)| flags | u8::from(held) << bit),
            ),
            Self::Pong { id } => buf.extend(id.to_be_bytes()),
            Self::RenameItem { name } => write_utf(buf, name),
            Self::ResourcePack { id, action } => {
                buf.extend(id.to_be_bytes());
                write_varint(buf, *action as u32);
            }
            Self::SetCarriedItem { slot } => buf.extend(slot.to_be_bytes()),
            // `ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC`, whose patch
            // length-prefixes each component value.
            Self::SetCreativeModeSlot { slot, stack } => {
                buf.extend(slot.to_be_bytes());
                crate::item::write_stack(buf, stack.as_ref(), true);
            }
            Self::SignUpdate {
                pos,
                is_front_text,
                lines,
            } => {
                write_block_pos(buf, *pos);
                buf.push(u8::from(*is_front_text));
                for line in lines {
                    write_utf(buf, line);
                }
            }
            Self::Swing { hand } => write_varint(buf, *hand as u32),
            Self::TeleportToEntity { uuid } => buf.extend(uuid.to_be_bytes()),
            Self::UseItem {
                hand,
                sequence,
                y_rot,
                x_rot,
            } => {
                write_varint(buf, *hand as u32);
                var_i32(buf, *sequence);
                rotation(buf, *y_rot, *x_rot);
            }
            // FriendlyByteBuf.java:653-663
            Self::UseItemOn {
                hand,
                hit,
                sequence,
            } => {
                write_varint(buf, *hand as u32);
                write_block_pos(buf, hit.pos);
                write_varint(buf, hit.direction as u32);
                let (x, y, z) = hit.pos;
                for (at, origin) in [
                    (hit.location.x, x),
                    (hit.location.y, y),
                    (hit.location.z, z),
                ] {
                    buf.extend(((at - f64::from(origin)) as f32).to_be_bytes());
                }
                buf.push(u8::from(hit.inside));
                buf.push(u8::from(hit.world_border));
                var_i32(buf, *sequence);
            }
        }
    }
}

impl MoveFlags {
    fn pack(self) -> u8 {
        u8::from(self.on_ground) | u8::from(self.horizontal_collision) << 1
    }
}

fn var_i32(buf: &mut Vec<u8>, value: i32) {
    write_varint(buf, value as u32);
}

fn vec3(buf: &mut Vec<u8>, v: DVec3) {
    for c in [v.x, v.y, v.z] {
        buf.extend(c.to_be_bytes());
    }
}

fn rotation(buf: &mut Vec<u8>, y_rot: f32, x_rot: f32) {
    buf.extend(y_rot.to_be_bytes());
    buf.extend(x_rot.to_be_bytes());
}

/// `HashedStack.STREAM_CODEC`: an optional `ActualItem` (item, count, then
/// `HashedPatchMap`'s added hashes and removed types).
fn hashed_stack(buf: &mut Vec<u8>, stack: Option<&HashedStack>) {
    let Some(stack) = stack else {
        buf.push(0);
        return;
    };
    buf.push(1);
    write_varint(buf, stack.item);
    var_i32(buf, stack.count);
    write_varint(buf, stack.added.len() as u32);
    for &(component, hash) in &stack.added {
        write_varint(buf, component);
        buf.extend(hash.to_be_bytes());
    }
    write_varint(buf, stack.removed.len() as u32);
    for &component in &stack.removed {
        write_varint(buf, component);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::Reader;
    use crate::packets::Direction as Bound;
    use crate::registries::{ClientRegistry, RegistryTable};
    use crate::vanilla_fixtures::case;

    const UUID: u128 = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210;
    const POS: (i32, i32, i32) = (-12, 70, 345);

    /// One of every packet with a fixture, carrying what `CodecOracle`'s
    /// `serverbound` encoded.
    fn packets() -> Vec<GamePacket> {
        let registries = RegistryTable::native();
        let id = |registry, name| registries.id_of(registry, name).unwrap();
        let flags = |on_ground, horizontal_collision| MoveFlags {
            on_ground,
            horizontal_collision,
        };
        let pos = DVec3::new(1.5, 64.0, -2.25);
        vec![
            GamePacket::AcceptTeleportation { id: 300 },
            GamePacket::BundleItemSelected {
                slot: 36,
                index: -1,
            },
            GamePacket::ChangeGameMode { mode: 3 },
            GamePacket::ChunkBatchReceived {
                desired_chunks_per_tick: 12.5,
            },
            GamePacket::ClientCommand(ClientCommand::RequestStats),
            GamePacket::ClientInformation(ClientInformation {
                language: "en_gb".into(),
                view_distance: 12,
                chat_visibility: 1,
                chat_colors: true,
                model_customisation: 0x7f,
                main_hand: 0,
                text_filtering_enabled: false,
                allows_listing: true,
                particle_status: 2,
            }),
            GamePacket::ClientTickEnd,
            GamePacket::CommandSuggestion {
                id: 7,
                command: "/give @s sto".into(),
            },
            GamePacket::ConfigurationAcknowledged,
            GamePacket::ContainerButtonClick {
                container_id: 3,
                button_id: 200,
            },
            GamePacket::ContainerClick(ContainerClick {
                container_id: 3,
                state_id: 300,
                slot: -999,
                button: 1,
                input: ContainerInput::Throw,
                changed_slots: vec![(
                    5,
                    Some(HashedStack {
                        item: id(ClientRegistry::Item, "diamond_sword"),
                        count: 1,
                        added: vec![(id(ClientRegistry::DataComponentType, "damage"), 0x1234_5678)],
                        removed: vec![id(ClientRegistry::DataComponentType, "custom_name")],
                    }),
                )],
                carried: None,
            }),
            GamePacket::ContainerClose { container_id: 3 },
            GamePacket::ContainerSlotStateChanged {
                slot: 8,
                container_id: 3,
                new_state: true,
            },
            GamePacket::CustomPayload {
                id: "minecraft:brand".into(),
                data: b"\x05pomme".to_vec(),
            },
            GamePacket::KeepAlive { id: -2 },
            GamePacket::MovePlayerPos {
                pos,
                flags: flags(true, false),
            },
            GamePacket::MovePlayerPosRot {
                pos,
                y_rot: 90.0,
                x_rot: -10.0,
                flags: flags(false, true),
            },
            GamePacket::MovePlayerRot {
                y_rot: 90.0,
                x_rot: -10.0,
                flags: flags(true, true),
            },
            GamePacket::MovePlayerStatusOnly {
                flags: flags(false, false),
            },
            GamePacket::PlayerAbilities { flying: true },
            GamePacket::PlayerAction {
                action: PlayerAction::StopDestroyBlock,
                pos: POS,
                direction: Direction::West,
                sequence: 300,
            },
            GamePacket::PlayerCommand {
                entity: 300,
                action: PlayerCommand::StartRidingJump,
                data: 100,
            },
            GamePacket::PlayerInput(Input {
                forward: true,
                right: true,
                jump: true,
                sprint: true,
                ..Input::default()
            }),
            GamePacket::PlayerLoaded,
            GamePacket::Pong { id: -5 },
            GamePacket::RenameItem {
                name: "Sting".into(),
            },
            GamePacket::ResourcePack {
                id: UUID,
                action: ResourcePackAction::FailedReload,
            },
            GamePacket::SetCarriedItem { slot: 8 },
            GamePacket::SignUpdate {
                pos: POS,
                is_front_text: false,
                lines: ["one", "", "thrée", "4"].map(str::to_owned),
            },
            GamePacket::Swing {
                hand: InteractionHand::OffHand,
            },
            GamePacket::TeleportToEntity { uuid: UUID },
            GamePacket::UseItem {
                hand: InteractionHand::MainHand,
                sequence: 300,
                y_rot: 90.0,
                x_rot: -10.0,
            },
            GamePacket::UseItemOn {
                hand: InteractionHand::OffHand,
                hit: BlockHit {
                    pos: POS,
                    direction: Direction::Up,
                    location: DVec3::new(-11.25, 71.0, 345.5),
                    inside: true,
                    world_border: false,
                },
                sequence: 300,
            },
        ]
    }

    #[test]
    fn every_packet_encodes_as_vanilla_does() {
        let table = PacketTable::native();
        for packet in packets() {
            let name = packet.name();
            let frame = packet.encode();
            let mut r = Reader::new(&frame);
            assert_eq!(
                r.varint().ok(),
                table.id(Phase::Game, Bound::Serverbound, name),
                "{name}"
            );
            assert_eq!(
                &frame[r.pos()..],
                &case(&format!("serverbound/{name}"))[..],
                "{name}"
            );
        }
    }

    #[test]
    fn a_creative_slot_length_prefixes_each_component_value() {
        let stack = ItemStack {
            item: 1,
            count: 2,
            patch: crate::item::ComponentPatch {
                added: vec![(3, [5].into())],
                removed: vec![7],
            },
        };
        let frame = GamePacket::SetCreativeModeSlot {
            slot: 36,
            stack: Some(stack),
        }
        .encode();
        let id =
            PacketTable::native().id(Phase::Game, Bound::Serverbound, "set_creative_mode_slot");
        let mut r = Reader::new(&frame);
        assert_eq!(r.varint().ok(), id);
        // Slot, count, item, one added and one removed, then the added
        // component's id, value length and value, and the removed id.
        assert_eq!(&frame[r.pos()..], [0, 36, 2, 1, 1, 1, 3, 1, 5, 7]);
    }
}
