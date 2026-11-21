//! Tests of the client's wire code: one per translation rewriter, and the
//! client's own encoders and raw handlers against bytes the game encoded
//! (`pomme_protocol::vanilla_fixtures`).

use glam::DVec3;
use pomme_protocol::chunk::{self, ChunkPacket};
use pomme_protocol::container::{self, ContainerPacket};
use pomme_protocol::entity::{self, EntityPacket};
use pomme_protocol::hud::{self, BossOp, HudPacket};
use pomme_protocol::lifecycle::{self, LifecyclePacket};
use pomme_protocol::packets::{Direction, PacketTable, Phase};
use pomme_protocol::player::{self, PlayerPacket};
use pomme_protocol::player_info::{self, PlayerInfoPacket};
use pomme_protocol::scoreboard::{self, ObjectiveMethod, ScoreboardPacket};
use pomme_protocol::sound::{self, SoundPacket};
use pomme_protocol::wire;
use pomme_registry::identifier::Identifier;

use super::RawRead;
use crate::chat_component::ComponentText;

fn table_id(phase: Phase, dir: Direction, name: &str) -> u32 {
    PacketTable::native().id(phase, dir, name).unwrap()
}

/// pomme's size and eye height tables against the game's own
/// `EntityType.getDimensions`, for every entity type.
#[test]
fn entity_dimensions_match_the_game() {
    use pomme_protocol::vanilla_fixtures::{entity_dimensions, entity_type_count};
    use pomme_registry::Registry;
    use pomme_registry::builtin::EntityKind;

    use crate::entity::dimensions::EntityDimensions;

    let mut checked = 0;
    for kind in (0..).map_while(EntityKind::from_u32) {
        let pomme = EntityDimensions::of(kind);
        assert_eq!(
            [
                pomme.width,
                pomme.height,
                EntityDimensions::eye_height(kind)
            ],
            entity_dimensions(kind.to_str()),
            "{kind}"
        );
        checked += 1;
    }
    assert_eq!(checked, entity_type_count());
}

/// pomme's serverbound chat encoders write what the game's own codecs write.
#[test]
fn chat_encoders_match_the_game() {
    use crate::net::chat;
    use crate::net::chat_security::LastSeenUpdate;

    // Messages 0, 2 and 19 of the last twenty seen, as `CodecOracle` sends.
    let update = LastSeenUpdate {
        offset: 3,
        acknowledged: [0b101, 0, 0x08],
        checksum: 0x42,
        last_seen: Vec::new(),
    };
    let cases = [
        (
            "chat",
            "chat",
            chat::encode_outbound_message("hi", 5, -6, Some(&[9; 256]), &update),
        ),
        (
            "chat",
            "chat_unsigned",
            chat::encode_outbound_message("hi", 5, -6, None, &update),
        ),
        (
            "chat_command",
            "chat_command",
            chat::encode_outbound_command("help"),
        ),
        (
            "chat_command_signed",
            "chat_command_signed",
            chat::encode_outbound_signed_command(
                "msg a hi",
                7,
                8,
                &[("message".into(), [4; 256])],
                &update,
            ),
        ),
        (
            "chat_session_update",
            "chat_session_update",
            chat::encode_chat_session_update(uuid::Uuid::from_u128(7), 1234, &[1, 2], &[3]),
        ),
        ("chat_ack", "chat_ack", chat::encode_chat_ack(300)),
    ];
    for (name, case, frame) in cases {
        let expected = game_frame(Direction::Serverbound, name, &format!("serverbound/{case}"));
        assert_eq!(frame, expected, "{case}");
    }
}

/// The wire translation under test for one protocol; the old-layout frames
/// in the tests below are hand-built from that version's decompiled
/// reference codecs (`reference/<version>/decompiled/.../network/`).
fn translation_for(protocol: i32) -> crate::net::translate::TranslationSession {
    crate::net::translate::TranslationSession::for_protocol(protocol).expect("translation data")
}

fn old_id(protocol: i32, dir: Direction, name: &str) -> u32 {
    PacketTable::for_protocol(protocol)
        .unwrap()
        .id(Phase::Game, dir, name)
        .unwrap()
}

fn config_id(protocol: i32, dir: Direction, name: &str) -> u32 {
    PacketTable::for_protocol(protocol)
        .unwrap()
        .id(Phase::Configuration, dir, name)
        .unwrap()
}

fn login_id(protocol: i32, name: &str) -> u32 {
    PacketTable::for_protocol(protocol)
        .unwrap()
        .id(Phase::Login, Direction::Clientbound, name)
        .unwrap()
}

/// A registry entry's id in the given version's table.
fn registry_id(
    table: &pomme_protocol::RegistryTable,
    reg: pomme_protocol::ClientRegistry,
    name: &str,
) -> u32 {
    table.id_of(reg, name).unwrap()
}

/// Translates a hand-built old-version game frame: the native payload of
/// packet `name`, after its id, for the packets pomme has no reader for.
fn translate_to_payload(protocol: i32, old: Vec<u8>, name: &str) -> Vec<u8> {
    translate_and_read(protocol, old, name, |_, r| {
        Some(r.bytes(r.remaining()).map(<[u8]>::to_vec))
    })
}

/// The game's own 26.2 frame for packet `name`, from codec fixture `case`.
fn fixture_frame(name: &str, case: &str) -> Vec<u8> {
    game_frame(Direction::Clientbound, name, case)
}

/// Game packet `name`'s id, then the payload of fixture `case`.
fn game_frame(direction: Direction, name: &str, case: &str) -> Vec<u8> {
    let mut frame = Vec::new();
    wire::write_varint(&mut frame, table_id(Phase::Game, direction, name));
    frame.extend(pomme_protocol::vanilla_fixtures::case(case));
    frame
}

/// The event pomme's raw handler sends for fixture `case` of packet `name`.
fn fixture_event(name: &str, case: &str) -> crate::net::NetworkEvent {
    let (tx, rx) = crossbeam_channel::unbounded();
    let (took, _) = crate::net::handler::handle_raw_for_test(
        &fixture_frame(name, case),
        crate::world::block::BlockStateTable::native(),
        &tx,
    );
    assert!(took, "{case}");
    rx.try_iter().next().expect("an event")
}

/// Translates a hand-built old-version game frame and decodes the result
/// with pomme's `read` for packet `name`, as the raw chain does.
fn translate_and_read<P>(protocol: i32, old: Vec<u8>, name: &str, read: RawRead<P>) -> P {
    let translated = translation_for(protocol)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    read_native_frame(&translated, name, read)
}

/// A pomme-protocol decoder (`chunk::read`, `block::read`, …).
/// A native 26.2 game frame for packet `name`, decoded with `read`.
fn read_native_frame<P>(frame: &[u8], name: &str, read: RawRead<P>) -> P {
    read_frame(Phase::Game, frame, name, read)
}

/// The same for a configuration frame.
fn read_config_frame<P>(frame: &[u8], name: &str, read: RawRead<P>) -> P {
    read_frame(Phase::Configuration, frame, name, read)
}

fn read_frame<P>(phase: Phase, frame: &[u8], name: &str, read: RawRead<P>) -> P {
    let mut r = pomme_protocol::codec::Reader::new(frame);
    assert_eq!(
        r.varint(),
        Ok(table_id(phase, Direction::Clientbound, name))
    );
    let decoded = read(name, &mut r).unwrap().unwrap();
    r.ensure_eof().unwrap();
    decoded
}

/// A `registry_data` frame's entries, in id order, after checking it is the
/// dimension types.
fn dimension_type_entries(frame: &[u8]) -> Vec<pomme_protocol::configuration::RegistryEntry> {
    use pomme_protocol::configuration::{self, ConfigPacket};

    let ConfigPacket::RegistryData(data) =
        read_config_frame(frame, "registry_data", configuration::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(data.registry, "minecraft:dimension_type");
    data.entries
}

/// An item 765 and 26.2 number differently (27 against 54), so a stack that
/// never gets remapped decodes as the wrong item rather than passing by
/// identity.
const SHIFTED_ITEM: u8 = 27;
const SHIFTED_ITEM_KIND: pomme_registry::builtin::ItemKind =
    pomme_registry::builtin::ItemKind::GrassBlock;

/// Exactly the joinable non-native protocols build a translation: a version
/// with embedded tables but no `TRANSLATED` entry (the staging state while
/// its translation is built) must stay un-joinable, and a protocol without
/// tables at all never translates.
#[test]
fn no_translation_without_coverage() {
    use pomme_protocol::version::{NATIVE, VERSIONS};
    for v in VERSIONS {
        assert_eq!(
            crate::net::translate::TranslationSession::for_protocol(v.protocol).is_some(),
            crate::net::translate::joinable(v.protocol) && v.protocol != NATIVE.protocol,
            "{}",
            v.name
        );
    }
    assert!(PacketTable::for_protocol(762).is_none());
    assert!(crate::net::translate::TranslationSession::for_protocol(762).is_none());
}

/// 26.2 appended a trailing session-id UUID to login_finished
/// (`ClientboundLoginFinishedPacket.STREAM_CODEC`); the shim pads a zero one.
#[test]
fn translate_login_finished_26_1() {
    use pomme_protocol::login::{self, LoginPacket};

    let mut old = Vec::new();
    wire::write_varint(&mut old, login_id(775, "login_finished"));
    old.extend_from_slice(&0xfeed_beef_u128.to_be_bytes());
    old.extend_from_slice(&[6, b'P', b'u', b'r', b'd', b'z', b'e']);
    old.push(0); // no properties

    let translated = translation_for(775).translate_login_frame(old.into_boxed_slice());
    let Ok(LoginPacket::Finished(finished)) = login::read_frame(&translated) else {
        panic!("wrong packet");
    };
    assert_eq!(finished.profile.name, "Purdze");
    assert_eq!(finished.profile.id, 0xfeed_beef);
    assert_eq!(finished.session_id, 0);
}

/// 26.2 added `onlineMode` before the trailing `enforcesSecureChat` bool
/// (`ClientboundLoginPacket.write`); the shim inserts `false`.
#[test]
fn translate_game_login_26_1() {
    let mut native = fixture_frame("login", "login/fresh");
    // A 26.1 frame is the same bytes without the online_mode bool, which
    // sits right before the trailing enforces_secure_chat bool.
    let online_mode = native.len() - 2;
    let mut old = native.clone();
    old.remove(online_mode);

    let translated = translation_for(775)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    native[online_mode] = 0;
    assert_eq!(&translated[..], &native[..]);
}

/// 26.1's team `Parameters` order is `displayName, options, visibility,
/// collision, color, prefix, suffix` with color as a `ChatFormatting`
/// ordinal; 26.2 reordered to `displayName, prefix, suffix, visibility,
/// collision, color, options` and changed color to `Optional<TeamColor>`.
/// Translation targets that 26.2 layout, which pomme's team decoder reads.
#[test]
fn translate_set_player_team_26_1() {
    let team_id = table_id(Phase::Game, Direction::Clientbound, "set_player_team");
    // Bare TAG_String roots are valid network components.
    let display: &[u8] = &[8, 0, 4, b'T', b'e', b'a', b'm'];
    let prefix: &[u8] = &[8, 0, 1, b'P'];
    let suffix: &[u8] = &[8, 0, 1, b'S'];

    let mut old = Vec::new();
    old.push(team_id as u8);
    old.extend_from_slice(&[4, b'c', b'r', b'e', b'w']); // name
    old.push(2); // method: change (parameters, no player list)
    old.extend_from_slice(display);
    old.push(3); // options
    old.push(0); // visibility: always
    old.push(1); // collision: never
    old.push(5); // color: ChatFormatting DARK_PURPLE
    old.extend_from_slice(prefix);
    old.extend_from_slice(suffix);

    let translated = translation_for(775)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();

    let mut expected = Vec::new();
    expected.push(team_id as u8);
    expected.extend_from_slice(&[4, b'c', b'r', b'e', b'w']);
    expected.push(2);
    expected.extend_from_slice(display);
    expected.extend_from_slice(prefix);
    expected.extend_from_slice(suffix);
    expected.push(0); // visibility
    expected.push(1); // collision
    expected.push(1); // team color present
    expected.push(5); // TeamColor DARK_PURPLE
    expected.push(3); // options
    assert_eq!(&translated[..], &expected[..]);
}

/// RESET (ChatFormatting ordinal 21) has no 26.2 `TeamColor`, so translation
/// emits an absent optional; the method-0 player list is copied verbatim.
#[test]
fn translate_set_player_team_26_1_reset_color() {
    let team_id = table_id(Phase::Game, Direction::Clientbound, "set_player_team");
    let component: &[u8] = &[8, 0, 1, b'x'];

    let mut old = Vec::new();
    old.push(team_id as u8);
    old.extend_from_slice(&[1, b'a']); // name
    old.push(0); // method: add (parameters + player list)
    old.extend_from_slice(component);
    old.push(0); // options
    old.push(0); // visibility
    old.push(0); // collision
    old.push(21); // color: ChatFormatting RESET
    old.extend_from_slice(component);
    old.extend_from_slice(component);
    old.extend_from_slice(&[1, 3, b'b', b'o', b'b']); // player list

    let translated = translation_for(775)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();

    let mut expected = Vec::new();
    expected.push(team_id as u8);
    expected.extend_from_slice(&[1, b'a']);
    expected.push(0);
    expected.extend_from_slice(component);
    expected.extend_from_slice(component);
    expected.extend_from_slice(component);
    expected.push(0); // visibility
    expected.push(0); // collision
    expected.push(0); // no TeamColor
    expected.push(0); // options
    expected.extend_from_slice(&[1, 3, b'b', b'o', b'b']);
    assert_eq!(&translated[..], &expected[..]);
}

/// Every native registry id must match the runtime enum used by remapping.
#[test]
fn registry_table_matches_the_registry_enums() {
    use pomme_protocol::{ClientRegistry, RegistryTable};
    use pomme_registry::Registry;
    use pomme_registry::builtin::*;

    let mut mismatches = Vec::new();
    macro_rules! check {
        ($kind:ident, $reg:ident) => {{
            let names = RegistryTable::native().names(ClientRegistry::$reg);
            assert!(
                $kind::from_u32(names.len() as u32).is_none(),
                "{} has more ids than the native registry",
                stringify!($kind)
            );
            for (id, name) in names.iter().enumerate() {
                let value = $kind::from_u32(id as u32).expect("builtin id");
                assert_eq!(value.to_u32(), id as u32);
                // The table drops the default namespace, as `toShortString`.
                let entry = value.to_str().trim_start_matches("minecraft:");
                if entry != name {
                    mismatches.push((stringify!($kind), entry.to_owned(), name.clone()));
                }
            }
        }};
    }
    // Exhaustive match makes a new ClientRegistry require an enum comparison.
    for registry in ClientRegistry::ALL {
        match registry {
            ClientRegistry::Attribute => check!(Attribute, Attribute),
            ClientRegistry::BlockEntityType => check!(BlockEntityKind, BlockEntityType),
            ClientRegistry::CommandArgumentType => check!(CommandArgumentKind, CommandArgumentType),
            ClientRegistry::ConsumeEffectType => check!(ConsumeEffectKind, ConsumeEffectType),
            ClientRegistry::DataComponentType => check!(DataComponentKind, DataComponentType),
            ClientRegistry::DebugSubscription => check!(DebugSubscription, DebugSubscription),
            ClientRegistry::EntityType => check!(EntityKind, EntityType),
            ClientRegistry::GameEvent => check!(GameEvent, GameEvent),
            ClientRegistry::Item => check!(ItemKind, Item),
            ClientRegistry::Menu => check!(MenuKind, Menu),
            ClientRegistry::NumberFormatType => check!(NumberFormatKind, NumberFormatType),
            ClientRegistry::ParticleType => check!(ParticleKind, ParticleType),
            ClientRegistry::PositionSourceType => check!(PositionSourceKind, PositionSourceType),
            ClientRegistry::RecipeDisplay => check!(RecipeDisplay, RecipeDisplay),
            ClientRegistry::RecipeSerializer => check!(RecipeSerializer, RecipeSerializer),
            ClientRegistry::SlotDisplay => check!(SlotDisplay, SlotDisplay),
            ClientRegistry::SoundEvent => check!(SoundEvent, SoundEvent),
        }
    }
    assert_eq!(mismatches, []);
}

/// A 26.1 `add_entity`'s type id comes out as the wrong kind in 26.2's
/// numbering (ids past the sulfur_cube insertion shift by one); the remap
/// the raw path applies fixes it.
#[test]
fn remap_add_entity_26_1() {
    use pomme_registry::Registry;
    use pomme_registry::builtin::EntityKind;

    // 26.1 tadpole is id 130, which 26.2 numbers as sulfur_cube.
    assert_eq!(EntityKind::from_u32(130), Some(EntityKind::SulfurCube));
    let native =
        translation_for(775).remap_to_native(pomme_protocol::ClientRegistry::EntityType, 130);
    assert_eq!(
        native.and_then(EntityKind::from_u32),
        Some(EntityKind::Tadpole)
    );
}

/// A creative stack's component values can't be rewritten on a version
/// without an item grammar, so a patch touching a component that version
/// numbers differently is cleared wholesale outbound; unshifted ones survive.
#[test]
fn strip_creative_components_774() {
    use pomme_protocol::item::{ComponentPatch, ItemStack};
    use pomme_protocol::serverbound::GamePacket;
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let component = |name| {
        registry_id(
            RegistryTable::native(),
            ClientRegistry::DataComponentType,
            name,
        )
    };
    let remap = |name| {
        let mut packet = GamePacket::SetCreativeModeSlot {
            slot: 36,
            stack: Some(ItemStack {
                item: registry_id(RegistryTable::native(), ClientRegistry::Item, "stone"),
                count: 1,
                patch: ComponentPatch {
                    added: Vec::new(),
                    removed: vec![component(name)],
                },
            }),
        };
        translation_for(774).remap_outbound(&mut packet);
        let GamePacket::SetCreativeModeSlot { stack, .. } = packet else {
            unreachable!()
        };
        stack.expect("stack cleared").patch
    };

    // max_stack_size is numbered the same in 1.21.11: kept.
    assert_eq!(
        remap("max_stack_size").removed,
        [component("max_stack_size")]
    );
    // lock is shifted in 1.21.11: the patch is cleared.
    assert_eq!(remap("lock"), ComponentPatch::default());
}

/// 1.20.5 to 1.21.4 read a creative stack's component values without the
/// length prefix 1.21.5 added; later versions keep it.
#[test]
fn creative_slot_component_values_lose_their_length_769() {
    use pomme_protocol::item::{ComponentPatch, ItemStack};
    use pomme_protocol::serverbound::GamePacket;
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let native = RegistryTable::native();
    let stone = registry_id(native, ClientRegistry::Item, "stone") as u8;
    let size = registry_id(native, ClientRegistry::DataComponentType, "max_stack_size") as u8;
    let frame = |protocol| {
        let mut packet = GamePacket::SetCreativeModeSlot {
            slot: 36,
            stack: Some(ItemStack {
                item: u32::from(stone),
                count: 2,
                patch: ComponentPatch {
                    added: vec![(u32::from(size), [16].into())],
                    removed: Vec::new(),
                },
            }),
        };
        let translation = translation_for(protocol);
        translation.remap_outbound(&mut packet);
        let frames = translation.translate_outbound_game_frame(packet.encode());
        let [frame] = &frames[..] else {
            panic!("one frame");
        };
        let mut pos = 0;
        assert_eq!(
            wire::read_varint(frame, &mut pos),
            Some(old_id(
                protocol,
                Direction::Serverbound,
                "set_creative_mode_slot"
            ))
        );
        frame[pos..].to_vec()
    };
    // Slot, count, item, one added and no removed components, then the
    // added one's id and value (16).
    assert_eq!(frame(769), [0, 36, 2, stone, 1, 0, size, 16]);
    assert_eq!(frame(774), [0, 36, 2, stone, 1, 0, size, 1, 16]);
}

/// 26.1's game ids match 26.2's, but its item grammar still routes its
/// frames through the translation; 1.21.11's ids diverge.
#[test]
fn outbound_translation_gating() {
    assert!(translation_for(775).translates_items());
    assert!(translation_for(775).translates_outbound());
    assert!(translation_for(774).translates_outbound());
}

/// The id remap alone (`set_health` shifted between the versions).
#[test]
fn remap_game_ids_774() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(774, Direction::Clientbound, "set_health"));
    old.extend_from_slice(&18.0f32.to_be_bytes());
    wire::write_varint(&mut old, 19); // food
    old.extend_from_slice(&4.5f32.to_be_bytes());

    assert_eq!(
        translate_and_read(774, old, "set_health", player::read),
        PlayerPacket::Health {
            health: 18.0,
            food: 19,
            saturation: 4.5,
        }
    );
}

/// The serializer-id walker (see `translate_entity_data`): 1.21.11
/// `cow_variant` is 22, 26.2's is 23.
#[test]
fn translate_entity_data_774() {
    // index 0, serializer byte, value 2; index 17, cow_variant, holder id 4
    assert_eq!(
        entity_data_bytes(774, &[0, 0, 2, 17, 22, 4]),
        [0, 0, 2, 17, 23, 4]
    );
}

#[test]
fn translate_entity_profile_774() {
    // A profile with nothing known and no skin patch, then a humanoid arm.
    let mut values = vec![0, 37];
    values.extend_from_slice(&[0; 8]);
    values.extend_from_slice(&[1, 38, 1]);

    // 26.2's last two serializers.
    let humanoid_arm = pomme_protocol::entity_data::SERIALIZERS_26_2.len() as u8 - 1;
    let mut native = vec![0, humanoid_arm - 1];
    native.extend_from_slice(&[0; 8]);
    native.extend_from_slice(&[1, humanoid_arm, 1]);
    assert_eq!(entity_data_bytes(774, &values), native);
}

#[test]
fn translate_empty_entity_particles_774() {
    // index 10, particles, none; index 11, boolean, true: both serializers
    // number alike on 26.2.
    let values = [10, 17, 0, 11, 8, 1];
    assert_eq!(entity_data_bytes(774, &values), values);
}

/// A stack component the walker doesn't know falls back to the verbatim-tail
/// copy instead of dropping the packet. `damage` (id 3 in both 774 and the
/// native registry, varint payload) keeps the verbatim bytes decodable.
#[test]
fn translate_entity_item_stack_fallback_774() {
    // index 5, serializer 7 (item stack): count 1, item 1, 1 added
    // component, 0 removed, component 3 (damage) = 7.
    let entries = entity_data_entries(774, &[5, 7, 1, 1, 1, 0, 3, 7]);
    let pomme_protocol::entity_data::Value::Item(Some(stack)) = &entries[0].value else {
        panic!("wrong value");
    };
    assert_eq!(stack.patch.added, [(3, Box::from([7u8]))]);
}

/// `translate_item_stack`'s component ids against the native registry table.
#[test]
fn component_id_anchors() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let table = RegistryTable::native();
    assert_eq!(
        table.name_of(
            ClientRegistry::DataComponentType,
            super::translate::COMPONENT_MAP_ID
        ),
        Some("map_id")
    );
    assert_eq!(
        table.name_of(
            ClientRegistry::DataComponentType,
            super::translate::COMPONENT_PROFILE
        ),
        Some("profile")
    );
}

/// A `level_chunk_with_light` at chunk (3, -2) with no heightmaps, holding
/// `buffer` as its section data.
fn chunk_frame(protocol: i32, buffer: &[u8]) -> Vec<u8> {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(protocol, Direction::Clientbound, "level_chunk_with_light"),
    );
    old.extend_from_slice(&3i32.to_be_bytes()); // chunk x
    old.extend_from_slice(&(-2i32).to_be_bytes()); // chunk z
    old.push(0); // no heightmaps
    wire::write_varint(&mut old, buffer.len() as u32);
    old.extend_from_slice(buffer);
    old.push(0); // no block entities
    old.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // empty light masks + lists
    old
}

/// One section: block count 1, single-value palettes for states/biomes.
const SINGLE_VALUE_SECTION: [u8; 6] = [0, 1, 0, 5, 0, 0];

/// The per-section `fluidCount` insertion (see `translate_chunk`).
#[test]
fn translate_chunk_774() {
    let ChunkPacket::Level { x, z, sections, .. } = translate_and_read(
        774,
        chunk_frame(774, &SINGLE_VALUE_SECTION),
        "level_chunk_with_light",
        chunk::read,
    ) else {
        panic!("wrong packet");
    };
    assert_eq!((x, z), (3, -2));
    assert_eq!(sections[..], [0, 1, 0, 0, 0, 5, 0, 0]);
}

/// 1.21.5's section buffer ends in a zero byte per container, sized for the
/// length varints it no longer writes; the padding is dropped, and a buffer
/// without it misframes.
#[test]
fn translate_chunk_770() {
    let padded = [&SINGLE_VALUE_SECTION[..], &[0, 0]].concat();
    let ChunkPacket::Level { sections, .. } = translate_and_read(
        770,
        chunk_frame(770, &padded),
        "level_chunk_with_light",
        chunk::read,
    ) else {
        panic!("wrong packet");
    };
    assert_eq!(sections[..], [0, 1, 0, 0, 0, 5, 0, 0]);

    let unpadded = chunk_frame(770, &SINGLE_VALUE_SECTION).into_boxed_slice();
    assert!(
        translation_for(770)
            .translate_game_frame(unpadded)
            .is_none()
    );
}

/// The world-clock map synthesis (see `translate_set_time`).
#[test]
fn translate_set_time_774() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(774, Direction::Clientbound, "set_time"));
    old.extend_from_slice(&12000u64.to_be_bytes()); // game time
    old.extend_from_slice(&6000u64.to_be_bytes()); // day time
    old.push(1); // tickDayTime

    let LifecyclePacket::SetTime { game_time, clocks } =
        translate_and_read(774, old, "set_time", lifecycle::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(game_time, 12000);
    let [(_, clock)] = &clocks[..] else {
        panic!("{clocks:?}");
    };
    assert_eq!((clock.total_ticks, clock.rate), (6000, 1.0));
}

/// Neither 1.21.11 nor 1.21.10 has a serverbound `attack`; the frame
/// becomes an `interact` with the ATTACK action (`ServerboundInteractPacket`
/// action bodies in the references), through each version's id table.
#[test]
fn translate_attack_old_versions() {
    for protocol in [774, 773, 772, 771, 770, 769, 768, 767, 766, 765, 764, 763] {
        let frames =
            translation_for(protocol).translate_outbound_game_frame(wire::encode_attack(42));
        let interact = old_id(protocol, Direction::Serverbound, "interact");
        assert_eq!(frames, [[interact as u8, 42, 1, 0]], "{protocol}");
    }
}

/// A 26.2 off-hand `interact` (hand + LpVec3 location) becomes the 1.21.11
/// pair a vanilla client sends: INTERACT_AT (raw floats, then hand) then
/// INTERACT.
#[test]
fn translate_interact_774() {
    let location = DVec3::new(0.5, 1.25, -0.25);
    let frames = translation_for(774)
        .translate_outbound_game_frame(wire::encode_interact(42, 1, location, true));
    assert_eq!(frames.len(), 2);

    let interact = old_id(774, Direction::Serverbound, "interact") as u8;
    let at = &frames[0];
    assert_eq!(at[..3], [interact, 42, 2]);
    let float_at = |i: usize| f32::from_be_bytes(at[3 + 4 * i..7 + 4 * i].try_into().unwrap());
    for (i, expected) in [location.x, location.y, location.z].iter().enumerate() {
        // Bounded by the LpVec3 quantization the 26.2 frame already carries.
        assert!((f64::from(float_at(i)) - expected).abs() < 1e-3);
    }
    assert_eq!(at[15..], [1, 1]); // off hand, sneaking

    assert_eq!(frames[1], [interact, 42, 0, 1, 1]);
}

/// 26.2-only serverbound packets with no 1.21.11 equivalent are suppressed
/// rather than sent under a wrong id.
#[test]
fn suppress_unknown_outbound_774() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(Phase::Game, Direction::Serverbound, "set_game_rule"),
    );
    frame.push(0);
    assert!(
        translation_for(774)
            .translate_outbound_game_frame(frame)
            .is_empty()
    );
}

/// Outbound frames whose layout didn't change get the id remap only
/// (`swing` is 60 on 1.21.11, 63 on 26.2).
#[test]
fn remap_outbound_ids_774() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(Phase::Game, Direction::Serverbound, "swing"),
    );
    frame.push(0); // main hand
    let frames = translation_for(774).translate_outbound_game_frame(frame);
    assert_eq!(
        frames,
        [[old_id(774, Direction::Serverbound, "swing") as u8, 0]]
    );
}

// 1.21.10's game tables and layouts match 1.21.11's except clientbound 40
// (`horse_screen_open`, which 1.21.11 renamed `mount_screen_open`) and the
// serializer interleave, so the shared frame rewriters are covered by the
// 774 tests above; the tests below pin what's 1.21.10-specific.

/// The 1.21.10 serializer interleave: `sniffer_state` is 30 there (31 on
/// 1.21.11, 35 on 26.2), past the `zombie_nautilus_variant` insertion the
/// 1.21.11 map doesn't account for.
#[test]
fn translate_entity_data_773() {
    // index 0, serializer byte, value 2; index 17, sniffer_state, ordinal 2
    assert_eq!(
        entity_data_bytes(773, &[0, 0, 2, 17, 30, 2]),
        [0, 0, 2, 17, 35, 2]
    );
}

/// 1.21.11 renamed `horse_screen_open` -> `mount_screen_open` with identical
/// fields (containerId, inventoryColumns varints, entityId int in both
/// references); the name alias keeps the frame flowing under the new id.
#[test]
fn translate_horse_screen_open_773() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(773, Direction::Clientbound, "horse_screen_open"),
    );
    old.push(1); // container id
    old.push(3); // inventory columns
    old.extend_from_slice(&42i32.to_be_bytes());

    assert_eq!(
        translate_to_payload(773, old, "mount_screen_open"),
        [1, 3, 0, 0, 0, 42]
    );
}

// 1.21.8's serverbound layouts and the 26.x-era clientbound rewrites match
// 1.21.10's (chunk fluidCount, set_time, game login, login_finished, team,
// attack/interact are covered by the 774/773 tests above); the tests below
// pin the layouts and serializer set 1.21.9 changed.

/// The id remap through the 1.21.9 debug-packet insertions (`set_health`
/// shifted) plus the 1.21.8 serializer interleave: `sniffer_state` is 31
/// there (`compound_tag` still sits at 16), 35 on 26.2. The older versions
/// sharing the serializer set run through it too.
#[test]
fn translate_entity_data_old_versions() {
    for protocol in [772, 771, 770] {
        // index 0, serializer byte, value 2; index 17, sniffer_state,
        // ordinal 2
        assert_eq!(
            entity_data_bytes(protocol, &[0, 0, 2, 17, 31, 2]),
            [0, 0, 2, 17, 35, 2],
            "{protocol}"
        );
    }
}

/// A `compound_tag` entry (16, removed in 1.21.9) is stripped — an empty
/// NBT compound between two live entries — instead of failing the packet.
#[test]
fn translate_entity_data_compound_tag_old_versions() {
    for protocol in [772, 771, 770] {
        // index 0, serializer byte, value 2; the shoulder parrot compound;
        // index 8, boolean, true
        assert_eq!(
            entity_data_bytes(protocol, &[0, 0, 2, 19, 16, 0x0A, 0x00, 8, 8, 1]),
            [0, 0, 2, 8, 8, 1],
            "{protocol}"
        );
    }
}

/// A 772 particle-list value (`LivingEntity.EFFECT_PARTICLES`, serializer 18
/// there) has its particle type ids remapped and the `entity_effect` color
/// copied, so entries after it keep translating (a verbatim tail would leave
/// their shifted serializer ids in place). Byte-exact against the native
/// layout.
#[test]
fn translate_entity_data_particles_772() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let entity_effect =
        |table: &RegistryTable| registry_id(table, ClientRegistry::ParticleType, "entity_effect");
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(772, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 9); // entity id
    old.extend_from_slice(&[10, 18, 1]); // index 10, particles, one particle
    wire::write_varint(
        &mut old,
        entity_effect(RegistryTable::for_protocol(772).unwrap()),
    );
    old.extend_from_slice(&0x11223344u32.to_be_bytes()); // ARGB color
    old.extend_from_slice(&[19, 19, 1, 2, 3]); // index 19, villager_data, 3 varints
    old.push(0xFF);

    let translated = translation_for(772)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();

    let mut expected = Vec::new();
    wire::write_varint(
        &mut expected,
        table_id(Phase::Game, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut expected, 9);
    expected.extend_from_slice(&[10, 17, 1]); // 26.2 particles serializer
    wire::write_varint(&mut expected, entity_effect(RegistryTable::native()));
    expected.extend_from_slice(&0x11223344u32.to_be_bytes());
    expected.extend_from_slice(&[19, 18, 1, 2, 3]); // 26.2 villager_data serializer
    expected.push(0xFF);
    assert_eq!(&translated[..], &expected[..]);
}

/// A 1.21.8 `profile` item component (61 there; bare optional name/uuid +
/// properties) is rewrapped into 26.2's `ResolvableProfile` partial arm
/// with an empty skin patch (see `translate_old_profile`).
#[test]
fn translate_entity_item_profile_772() {
    use pomme_item::components::{Component, Profile, ProfileContents};

    // index 5, serializer 7 (item stack): count 1, item 5, 1 added, 0
    // removed, component 61 (profile).
    let mut values = vec![5, 7, 1, 5, 1, 0, 61];
    values.extend_from_slice(&[1, 5, b'S', b't', b'e', b'v', b'e']); // name
    values.push(0); // no uuid
    values.push(0); // no properties

    let entries = entity_data_entries(772, &values);
    let pomme_protocol::entity_data::Value::Item(Some(stack)) = &entries[0].value else {
        panic!("wrong value");
    };
    let [(_, profile)] = &stack.patch.added[..] else {
        panic!("components: {:?}", stack.patch.added);
    };
    let profile = Profile::from_bytes(profile).expect("profile component");
    let ProfileContents::Partial { name, .. } = profile.contents else {
        panic!("expected the partial-profile arm");
    };
    assert_eq!(name.as_deref(), Some("Steve"));
    assert_eq!(profile.skin_patch, Default::default());
}

/// The velocity the shorts -> `LpVec3` tests write and expect back.
const VELOCITY_772: [f64; 3] = [0.25, -0.5, 0.0];

fn write_velocity_shorts(out: &mut Vec<u8>) {
    for c in VELOCITY_772 {
        out.extend_from_slice(&((c * 8000.0) as i16).to_be_bytes());
    }
}

fn write_doubles(out: &mut Vec<u8>, values: impl IntoIterator<Item = f64>) {
    for v in values {
        out.extend_from_slice(&v.to_be_bytes());
    }
}

fn write_floats(out: &mut Vec<u8>, values: impl IntoIterator<Item = f32>) {
    for v in values {
        out.extend_from_slice(&v.to_be_bytes());
    }
}

/// Compares within the `LpVec3` quantization error.
fn assert_velocity(v: DVec3) {
    for (got, expected) in v.to_array().into_iter().zip(VELOCITY_772) {
        assert!((got - expected).abs() < 1e-3, "{got} != {expected}");
    }
}

/// The velocity move: three trailing shorts (1/8000 block) on 1.21.8, an
/// `LpVec3` between position and rotations on 26.2
/// (`ClientboundAddEntityPacket` read bodies in both references). The older
/// versions sharing the layout run through it too, pinning that the
/// 1.21.9-era rewrites apply to them.
#[test]
fn translate_add_entity_old_versions() {
    for protocol in [772, 771, 770, 769, 768, 767, 766, 765, 764, 763] {
        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(protocol, Direction::Clientbound, "add_entity"),
        );
        wire::write_varint(&mut old, 7); // entity id
        old.extend_from_slice(&[0; 16]); // uuid
        wire::write_varint(&mut old, 20); // entity type (remapped after decode)
        for c in [100.5f64, 64.0, -20.25] {
            old.extend_from_slice(&c.to_be_bytes());
        }
        old.extend_from_slice(&[64, 128, 192]); // x/y/head rotation
        wire::write_varint(&mut old, 0); // data
        write_velocity_shorts(&mut old);

        let EntityPacket::Add {
            id,
            position,
            movement,
            x_rot,
            y_rot,
            y_head_rot,
            ..
        } = translate_and_read(protocol, old, "add_entity", entity::read)
        else {
            panic!("wrong packet");
        };
        assert_eq!(id, 7, "{protocol}");
        assert_eq!(position, DVec3::new(100.5, 64.0, -20.25), "{protocol}");
        assert_velocity(movement);
        assert_eq!(
            (x_rot, y_rot, y_head_rot),
            (90.0, -180.0, -90.0),
            "{protocol}"
        );
    }
}

/// The same shorts -> `LpVec3` switch on `set_entity_motion`.
#[test]
fn translate_set_entity_motion_772() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(772, Direction::Clientbound, "set_entity_motion"),
    );
    wire::write_varint(&mut old, 9); // entity id
    write_velocity_shorts(&mut old);

    let EntityPacket::Motion { id, velocity } =
        translate_and_read(772, old, "set_entity_motion", entity::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(id, 9);
    assert_velocity(velocity);
}

/// 1.21.9 added a relative-rotation bool after each `player_rotation`
/// angle; the shim synthesizes absolute.
#[test]
fn translate_player_rotation_772() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(772, Direction::Clientbound, "player_rotation"),
    );
    old.extend_from_slice(&90.0f32.to_be_bytes()); // y rot
    old.extend_from_slice(&(-10.0f32).to_be_bytes()); // x rot

    assert_eq!(
        translate_and_read(772, old, "player_rotation", lifecycle::read),
        LifecyclePacket::PlayerRotation {
            y_rot: 90.0,
            relative_y: false,
            x_rot: -10.0,
            relative_x: false,
        }
    );
}

/// `BlockPos + angle` -> `RespawnData` with a synthesized overworld
/// dimension and zero pitch.
#[test]
fn translate_set_default_spawn_772() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(772, Direction::Clientbound, "set_default_spawn_position"),
    );
    // Packed BlockPos (0, 64, 0): just the y bits.
    old.extend_from_slice(&64u64.to_be_bytes());
    old.extend_from_slice(&45.0f32.to_be_bytes()); // angle

    let mut native = Vec::new();
    wire::write_utf(&mut native, "minecraft:overworld");
    native.extend_from_slice(&64u64.to_be_bytes());
    native.extend_from_slice(&45.0f32.to_be_bytes()); // yaw
    native.extend_from_slice(&0.0f32.to_be_bytes()); // pitch
    assert_eq!(
        translate_to_payload(772, old, "set_default_spawn_position"),
        native
    );
}

/// 1.21.9 inserted `radius`/`blockCount` after the explosion center and
/// appended a block-particle list; the particle and sound ids between them
/// are remapped into 26.2's space (a 1.21.8 server sends `explosion` 22 and
/// the `entity.generic.explode` holder 615 + 1; 26.2 numbers them 30 and
/// 699).
#[test]
fn translate_explode_772() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let particle_id = |table, name| registry_id(table, ClientRegistry::ParticleType, name);
    let sound_id = |table, name| registry_id(table, ClientRegistry::SoundEvent, name);
    let old_table = RegistryTable::for_protocol(772).unwrap();
    let native_table = RegistryTable::native();

    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(772, Direction::Clientbound, "explode"));
    for c in [1.0f64, 65.0, -2.0] {
        old.extend_from_slice(&c.to_be_bytes());
    }
    old.push(1); // knockback present
    for c in [0.1f64, 0.2, 0.3] {
        old.extend_from_slice(&c.to_be_bytes());
    }
    wire::write_varint(&mut old, particle_id(old_table, "explosion"));
    wire::write_varint(&mut old, sound_id(old_table, "entity.generic.explode") + 1);

    let translated = translation_for(772)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();

    let mut expected = Vec::new();
    wire::write_varint(
        &mut expected,
        table_id(Phase::Game, Direction::Clientbound, "explode"),
    );
    for c in [1.0f64, 65.0, -2.0] {
        expected.extend_from_slice(&c.to_be_bytes());
    }
    expected.extend_from_slice(&0f32.to_be_bytes()); // radius
    expected.extend_from_slice(&0i32.to_be_bytes()); // block count
    expected.push(1); // knockback present
    for c in [0.1f64, 0.2, 0.3] {
        expected.extend_from_slice(&c.to_be_bytes());
    }
    wire::write_varint(&mut expected, particle_id(native_table, "explosion"));
    wire::write_varint(
        &mut expected,
        sound_id(native_table, "entity.generic.explode") + 1,
    );
    expected.push(0); // no block particles
    assert_eq!(&translated[..], &expected[..]);
}

/// 1.21.5's `player_command` action enum still opens with PRESS/RELEASE_
/// SHIFT_KEY (`ServerboundPlayerCommandPacket.Action` in both references),
/// so a 26.2 action ordinal gains two on the old wire.
#[test]
fn translate_player_command_770() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(Phase::Game, Direction::Serverbound, "player_command"),
    );
    wire::write_varint(&mut frame, 9); // entity id
    wire::write_varint(&mut frame, 1); // action: START_SPRINTING
    wire::write_varint(&mut frame, 0); // data

    let frames = translation_for(770).translate_outbound_game_frame(frame);
    let old = old_id(770, Direction::Serverbound, "player_command") as u8;
    assert_eq!(frames, [[old, 9, 3, 0]]);
}

/// The 1.21.4 serializer interleave: `sniffer_state` is 27 there, 35 on
/// 26.2, past the four variant serializers 1.21.5 added.
#[test]
fn translate_entity_data_769() {
    // index 17, sniffer_state, ordinal 2
    assert_eq!(entity_data_bytes(769, &[17, 27, 2]), [17, 35, 2]);
}

/// 1.21.4 team `Parameters` carry nametag visibility and collision rule as
/// strings (`ClientboundSetPlayerTeamPacket` read body); the shim maps
/// them to the enum ids 1.21.5 introduced.
#[test]
fn translate_set_player_team_769() {
    let nbt_str = |s: &str| {
        let mut v = vec![8u8, 0, s.len() as u8];
        v.extend_from_slice(s.as_bytes());
        v
    };
    let utf = |s: &str| {
        let mut v = vec![s.len() as u8];
        v.extend_from_slice(s.as_bytes());
        v
    };

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(769, Direction::Clientbound, "set_player_team"),
    );
    old.extend_from_slice(&utf("crew")); // team name
    old.push(2); // method: change
    old.extend_from_slice(&nbt_str("c")); // display name
    old.push(3); // options
    old.extend_from_slice(&utf("hideForOtherTeams")); // visibility
    old.extend_from_slice(&utf("pushOwnTeam")); // collision
    old.push(5); // color
    old.extend_from_slice(&nbt_str("p")); // prefix
    old.extend_from_slice(&nbt_str("s")); // suffix

    let translated = translation_for(769)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    let mut expected = Vec::new();
    wire::write_varint(
        &mut expected,
        table_id(Phase::Game, Direction::Clientbound, "set_player_team"),
    );
    expected.extend_from_slice(&utf("crew"));
    expected.push(2);
    expected.extend_from_slice(&nbt_str("c"));
    expected.extend_from_slice(&nbt_str("p"));
    expected.extend_from_slice(&nbt_str("s"));
    expected.push(2); // visibility: hideForOtherTeams
    expected.push(3); // collision: pushOwnTeam
    expected.push(1); // TeamColor present
    expected.push(5); // DARK_PURPLE
    expected.push(3); // options
    assert_eq!(&translated[..], &expected[..]);
}

/// A 769 `level_chunk_with_light` at chunk (3, -2) holding `section`, its
/// heightmaps the pre-1.21.5 NBT compound `{ MOTION_BLOCKING: [7L] }`.
fn chunk_frame_769(section: &[u8]) -> Vec<u8> {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(769, Direction::Clientbound, "level_chunk_with_light"),
    );
    old.extend_from_slice(&3i32.to_be_bytes()); // chunk x
    old.extend_from_slice(&(-2i32).to_be_bytes()); // chunk z
    old.push(10);
    old.push(12); // long-array tag
    old.extend_from_slice(&15u16.to_be_bytes());
    old.extend_from_slice(b"MOTION_BLOCKING");
    old.extend_from_slice(&1i32.to_be_bytes());
    old.extend_from_slice(&7u64.to_be_bytes());
    old.push(0); // compound end
    wire::write_varint(&mut old, section.len() as u32);
    old.extend_from_slice(section);
    old.push(0); // no block entities
    old.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // empty light masks + lists
    old
}

/// The pre-1.21.5 NBT heightmap compound becomes the packed list, and each
/// single-value container's empty long array loses its `0` length, on top of
/// the shared per-section `fluidCount` insertion.
#[test]
fn translate_chunk_769() {
    // Block count 1; states and biomes each a single value, then length 0.
    let section = [0u8, 1, 0, 5, 0, 0, 0, 0];
    let ChunkPacket::Level {
        x,
        z,
        heightmaps,
        sections,
        ..
    } = translate_and_read(
        769,
        chunk_frame_769(&section),
        "level_chunk_with_light",
        chunk::read,
    )
    else {
        panic!("wrong packet");
    };
    assert_eq!((x, z), (3, -2));
    assert_eq!(heightmaps.len(), 1);
    assert_eq!(&*heightmaps[0].1, &[7u64]);
    assert_eq!(sections[..], [0, 1, 0, 0, 0, 5, 0, 0]);
}

/// An indirect 4-bit block palette's 256 longs keep their bytes and lose the
/// length before them; a length the bits don't imply drops the packet.
#[test]
fn translate_chunk_769_indirect() {
    let longs: Vec<u8> = (0..256u64).flat_map(|i| i.to_be_bytes()).collect();
    let section = |length: u32| {
        let mut section = vec![0u8, 1, 4, 2, 0, 5]; // 4 bits, palette [0, 5]
        wire::write_varint(&mut section, length);
        section.extend_from_slice(&longs);
        section.extend_from_slice(&[0, 0, 0]); // single-value biomes, length 0
        section
    };

    let ChunkPacket::Level { sections, .. } = translate_and_read(
        769,
        chunk_frame_769(&section(256)),
        "level_chunk_with_light",
        chunk::read,
    ) else {
        panic!("wrong packet");
    };
    let mut expected = vec![0u8, 1, 0, 0, 4, 2, 0, 5];
    expected.extend_from_slice(&longs);
    expected.extend_from_slice(&[0, 0]);
    assert_eq!(sections[..], expected[..]);

    let frame = chunk_frame_769(&section(255)).into_boxed_slice();
    assert!(translation_for(769).translate_game_frame(frame).is_none());
}

/// `chunks_biomes` carries biome containers in their pre-1.21.6 layout up to
/// 1.21.5, so it drops there; from 1.21.6 it passes through.
#[test]
fn chunks_biomes_drop_before_1_21_6() {
    let empty = |protocol| {
        let mut frame = Vec::new();
        wire::write_varint(
            &mut frame,
            old_id(protocol, Direction::Clientbound, "chunks_biomes"),
        );
        wire::write_varint(&mut frame, 0);
        frame.into_boxed_slice()
    };
    for protocol in [769, 770] {
        assert!(
            translation_for(protocol)
                .translate_game_frame(empty(protocol))
                .is_none()
        );
    }
    assert!(
        translation_for(772)
            .translate_game_frame(empty(772))
            .is_some()
    );
}

/// A `player_chat` frame with everything but the chat type fixed: 765 sends a
/// direct registry id where 769 already sends a holder.
fn player_chat_frame(protocol: i32, chat_type: u32) -> Vec<u8> {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(protocol, Direction::Clientbound, "player_chat"),
    );
    old.extend(player_chat_body(chat_type));
    old
}

/// The fields of that frame, which 26.2 keeps after its `globalIndex`.
fn player_chat_body(chat_type: u32) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&[7; 16]); // sender uuid
    wire::write_varint(&mut body, 3); // index
    body.push(0); // no signature
    body.extend_from_slice(&[2, b'h', b'i']); // content
    body.extend_from_slice(&11u64.to_be_bytes()); // timestamp
    body.extend_from_slice(&13u64.to_be_bytes()); // salt
    body.push(0); // no last-seen entries
    body.push(0); // no unsigned content
    body.push(0); // filter: pass-through
    wire::write_varint(&mut body, chat_type);
    body.extend_from_slice(&[8, 0, 1, b'n']); // name: NBT string
    body.push(0); // no target
    body
}

/// Pre-1.20.5 sends the chat type as a direct registry id where 26.2 wants a
/// holder. Id 0 (`chat`, the ordinary message) is the holder's "inline value
/// follows" sentinel, so leaving it alone makes normal chat undecodable.
#[test]
fn translate_player_chat_765() {
    // A zero `globalIndex`, then the chat type as the holder of id 0.
    assert_eq!(
        translate_to_payload(765, player_chat_frame(765, 0), "player_chat"),
        [&[0][..], &player_chat_body(1)].concat()
    );
}

/// `disguised_chat` carries the same `ChatType.Bound` tail, after the message
/// component rather than a signed body.
#[test]
fn translate_disguised_chat_765() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(765, Direction::Clientbound, "disguised_chat"),
    );
    old.extend_from_slice(&[8, 0, 2, b'h', b'i']); // message: NBT string
    wire::write_varint(&mut old, 0); // chat type: direct registry id
    old.extend_from_slice(&[8, 0, 1, b'n']); // name: NBT string
    old.push(0); // no target

    assert_eq!(
        translate_to_payload(765, old, "disguised_chat"),
        // The chat type as the holder of id 0.
        [8, 0, 2, b'h', b'i', 1, 8, 0, 1, b'n', 0]
    );
}

/// 1.21.5 prepended `globalIndex` to `player_chat`; the shim synthesizes
/// zero ahead of an otherwise identical body.
#[test]
fn translate_player_chat_769() {
    assert_eq!(
        translate_to_payload(769, player_chat_frame(769, 1), "player_chat"),
        [&[0][..], &player_chat_body(1)].concat()
    );
}

/// A 26.2 `container_click` carries hashed stacks; 1.21.4 wants full item
/// stacks, reconstructed bare (`ServerboundContainerClickPacket` in both
/// references).
#[test]
fn translate_container_click_769() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(Phase::Game, Direction::Serverbound, "container_click"),
    );
    wire::write_varint(&mut frame, 1); // container id
    wire::write_varint(&mut frame, 2); // state id
    frame.extend_from_slice(&5i16.to_be_bytes()); // slot
    frame.push(0); // button
    wire::write_varint(&mut frame, 0); // click type: pickup
    wire::write_varint(&mut frame, 1); // one changed slot
    frame.extend_from_slice(&5i16.to_be_bytes());
    frame.push(1); // hashed stack present
    wire::write_varint(&mut frame, 4); // item
    wire::write_varint(&mut frame, 2); // count
    wire::write_varint(&mut frame, 1); // one hashed component
    wire::write_varint(&mut frame, 3); // component id
    frame.extend_from_slice(&0x1234i32.to_be_bytes()); // hash
    wire::write_varint(&mut frame, 0); // no removed components
    frame.push(0); // carried: empty

    let frames = translation_for(769).translate_outbound_game_frame(frame);
    let old = old_id(769, Direction::Serverbound, "container_click") as u8;
    assert_eq!(frames, [[old, 1, 2, 0, 5, 0, 0, 1, 0, 5, 2, 4, 0, 0, 0]]);
}

/// A `level_particles` position, spread, speed and count, which every
/// version before 26.3 sends alike.
fn level_particles_fields() -> Vec<u8> {
    let mut fields = Vec::new();
    for c in [1.0f64, 65.0, -2.0] {
        fields.extend_from_slice(&c.to_be_bytes());
    }
    for d in [0.5f32, 0.5, 0.5, 0.1] {
        fields.extend_from_slice(&d.to_be_bytes());
    }
    fields.extend_from_slice(&7i32.to_be_bytes()); // count
    fields
}

/// 1.21.4 inserted `alwaysShow` after `overrideLimiter`; the shim
/// synthesizes false for 1.21.3-and-older frames.
#[test]
fn translate_level_particles_768() {
    let mut fields = level_particles_fields();
    wire::write_varint(&mut fields, 30); // particle (26.2 id space)

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(768, Direction::Clientbound, "level_particles"),
    );
    old.push(1); // override limiter
    old.extend_from_slice(&fields);

    assert_eq!(
        translate_to_payload(768, old, "level_particles"),
        // The override limiter, then `alwaysShow`.
        [&[1, 0][..], &fields].concat()
    );
}

/// 1.21.4 appended UPDATE_HAT after UPDATE_LIST_ORDER rather than inserting
/// it, so a 768 mask is a prefix of 26.2's and passes through untouched.
#[test]
fn player_info_passthrough_768() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(768, Direction::Clientbound, "player_info_update"),
    );
    old.push(0x40); // actions: UPDATE_LIST_ORDER (1.21.3 bit 6)
    wire::write_varint(&mut old, 1); // one entry
    old.extend_from_slice(&[9; 16]); // uuid
    wire::write_varint(&mut old, 5); // list order

    let PlayerInfoPacket::Update { actions, entries } =
        translate_and_read(768, old, "player_info_update", player_info::read)
    else {
        panic!("wrong packet");
    };
    assert!(actions.update_list_order && !actions.update_hat);
    assert_eq!(entries[0].list_order, 5);
}

/// The 1.21.2 `player_position` rework (`PositionMoveRotation`): the 767
/// layout's trailing teleport id moves to the front and a zero delta plus
/// widened relative bits are synthesized. Each position bit is mirrored into
/// its `DELTA_*` bit, so 1.21.1's "keep the current velocity on a relative
/// axis" survives 26.2's `calculateDelta`.
#[test]
fn translate_player_position_767() {
    let build = |relatives: u8| {
        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(767, Direction::Clientbound, "player_position"),
        );
        for c in [100.5f64, 64.0, -20.25] {
            old.extend_from_slice(&c.to_be_bytes());
        }
        old.extend_from_slice(&90.0f32.to_be_bytes()); // y rot
        old.extend_from_slice(&(-10.0f32).to_be_bytes()); // x rot
        old.push(relatives);
        wire::write_varint(&mut old, 42); // teleport id
        old
    };

    let read = |old| translate_and_read(767, old, "player_position", lifecycle::read);
    let LifecyclePacket::PlayerPosition {
        id,
        change,
        relative,
    } = read(build(0b0001_1000))
    // relative: Y_ROT | X_ROT
    else {
        panic!("wrong packet");
    };
    assert_eq!(id, 42);
    assert_eq!(change.position, DVec3::new(100.5, 64.0, -20.25));
    assert_eq!(change.delta, DVec3::ZERO);
    assert_eq!((change.y_rot, change.x_rot), (90.0, -10.0));
    // Absolute on every position axis, so the velocity zeroes as 1.21.1's did.
    assert_eq!(
        relative,
        pomme_protocol::entity::Relative {
            y_rot: true,
            x_rot: true,
            ..Default::default()
        }
    );

    let LifecyclePacket::PlayerPosition { relative, .. } = read(build(0b0000_0101))
    // relative: X | Z
    else {
        panic!("wrong packet");
    };
    assert_eq!(
        relative,
        pomme_protocol::entity::Relative {
            x: true,
            z: true,
            delta_x: true,
            delta_z: true,
            ..Default::default()
        }
    );
}

/// 1.21.1's `teleport_entity` only synced position and rotation, so it becomes
/// 26.2's `entity_position_sync` (which leaves velocity alone) rather than its
/// `teleport_entity` (whose zero delta would zero it). Rotations are
/// packed-degree bytes.
#[test]
fn translate_teleport_entity_767() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(767, Direction::Clientbound, "teleport_entity"),
    );
    wire::write_varint(&mut old, 9); // entity id
    for c in [1.0f64, 70.0, 2.0] {
        old.extend_from_slice(&c.to_be_bytes());
    }
    old.push(64); // y rot: 90 degrees packed
    old.push(0); // x rot
    old.push(1); // on ground

    assert_eq!(
        translate_and_read(767, old, "entity_position_sync", entity::read),
        EntityPacket::PositionSync {
            id: 9,
            values: entity::PositionMoveRotation {
                position: DVec3::new(1.0, 70.0, 2.0),
                delta: DVec3::ZERO,
                y_rot: 90.0,
                x_rot: 0.0,
            },
            on_ground: true,
        }
    );
}

/// 767's two-long `set_time` with a negated (frozen) dayTime feeds the
/// shared world-clock synthesis.
#[test]
fn translate_set_time_767() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(767, Direction::Clientbound, "set_time"));
    old.extend_from_slice(&12000u64.to_be_bytes()); // game time
    old.extend_from_slice(&(-6000i64).to_be_bytes()); // frozen day time

    let LifecyclePacket::SetTime { game_time, clocks } =
        translate_and_read(767, old, "set_time", lifecycle::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(game_time, 12000);
    let [(_, clock)] = &clocks[..] else {
        panic!("{clocks:?}");
    };
    assert_eq!((clock.total_ticks, clock.rate), (6000, 0.0));
}

/// 767 `container_set_slot` sentinels: container -1 becomes
/// `set_cursor_item`, -2 becomes `set_player_inventory`, and plain ids pass
/// with the byte-to-varint widening.
#[test]
fn translate_container_set_slot_767() {
    let build = |container: i8, slot: i16| {
        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(767, Direction::Clientbound, "container_set_slot"),
        );
        old.push(container as u8);
        wire::write_varint(&mut old, 2); // state id
        old.extend_from_slice(&slot.to_be_bytes());
        wire::write_varint(&mut old, 0); // empty stack
        old
    };

    let read = |name, old| translate_and_read(767, old, name, container::read);
    assert_eq!(
        read("set_cursor_item", build(-1, 0)),
        ContainerPacket::Cursor { item: None }
    );
    assert_eq!(
        read("set_player_inventory", build(-2, 7)),
        ContainerPacket::PlayerInventory {
            slot: 7,
            item: None
        }
    );
    assert_eq!(
        read("container_set_slot", build(3, 1)),
        ContainerPacket::Slot {
            container_id: 3,
            state_id: 2,
            slot: 1,
            item: None
        }
    );
}

/// A 767 `cooldown` item id becomes the item's cooldown group.
#[test]
fn translate_cooldown_767() {
    let shield = registry_id(
        pomme_protocol::RegistryTable::for_protocol(767).unwrap(),
        pomme_protocol::ClientRegistry::Item,
        "shield",
    );
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(767, Direction::Clientbound, "cooldown"));
    wire::write_varint(&mut old, shield);
    wire::write_varint(&mut old, 100); // duration

    assert_eq!(
        translate_and_read(767, old, "cooldown", pomme_protocol::player::read),
        pomme_protocol::player::PlayerPacket::Cooldown {
            group: "minecraft:shield".into(),
            duration: 100,
        }
    );
}

/// The clientbound `set_carried_item` -> `set_held_slot` rename alias.
#[test]
fn translate_set_carried_item_767() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(767, Direction::Clientbound, "set_carried_item"),
    );
    old.push(3);

    assert_eq!(
        translate_and_read(767, old, "set_held_slot", player::read),
        PlayerPacket::HeldSlot { slot: 3 }
    );
}

/// Outbound `player_input` becomes 767's vehicle-steering layout (axis
/// floats + jump/shift flags).
#[test]
fn translate_player_input_767() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(Phase::Game, Direction::Serverbound, "player_input"),
    );
    frame.push(0b0011_0101); // forward, left, jump, shift

    let frames = translation_for(767).translate_outbound_game_frame(frame);
    let mut expected = Vec::new();
    wire::write_varint(
        &mut expected,
        old_id(767, Direction::Serverbound, "player_input"),
    );
    expected.extend_from_slice(&1.0f32.to_be_bytes()); // xxa: left
    expected.extend_from_slice(&1.0f32.to_be_bytes()); // zza: forward
    expected.push(3); // jump | shift
    assert_eq!(frames, [expected]);
}

/// Outbound move_player flags drop 1.21.2's horizontal-collision bit,
/// which a 767 server would read as onGround.
#[test]
fn translate_move_player_flags_767() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(
            Phase::Game,
            Direction::Serverbound,
            "move_player_status_only",
        ),
    );
    frame.push(2); // horizontal collision only, not on ground

    let frames = translation_for(767).translate_outbound_game_frame(frame);
    assert_eq!(
        frames,
        [[
            old_id(767, Direction::Serverbound, "move_player_status_only") as u8,
            0
        ]]
    );
}

/// 1.21.2 appended `particleStatus` to `client_information`; older servers
/// get it stripped in the game phase and in configuration.
#[test]
fn strip_particle_status_767() {
    let information = super::client_information(
        8,
        crate::ui::chat::ChatOptions::default(),
        crate::ui::menu::SkinOptions::default(),
    );
    // 1.21.1's `ClientInformation.write`: language, view distance, chat
    // visibility (FULL), chat colors, every skin part, main hand (RIGHT), text
    // filtering, allows listing.
    let old_body = [5, b'e', b'n', b'_', b'u', b's', 8, 0, 1, 0x7f, 1, 0, 1];
    let with_id = |id: u32| {
        let mut out = Vec::new();
        wire::write_varint(&mut out, id);
        out.extend_from_slice(&old_body);
        out
    };

    let information = pomme_protocol::serverbound::GamePacket::ClientInformation(information);
    let game = information.encode();
    for protocol in [767, 763] {
        let old = old_id(protocol, Direction::Serverbound, "client_information");
        assert_eq!(
            translation_for(protocol).translate_outbound_game_frame(game.clone()),
            [with_id(old)],
            "{protocol}"
        );
    }

    let config = information.encode_in(Phase::Configuration).unwrap();
    for protocol in [767, 764] {
        let old = config_id(protocol, Direction::Serverbound, "client_information");
        assert_eq!(
            translation_for(protocol).translate_outbound_config_frame(config.clone()),
            Some(with_id(old)),
            "{protocol}"
        );
    }
}

/// 1.21 turned `update_attributes` modifier UUIDs into resource locations;
/// the shim synthesizes a hex name.
#[test]
fn translate_update_attributes_766() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(766, Direction::Clientbound, "update_attributes"),
    );
    wire::write_varint(&mut old, 9); // entity id
    wire::write_varint(&mut old, 1); // one attribute
    wire::write_varint(&mut old, 16); // generic.max_health (766 space)
    old.extend_from_slice(&20.0f64.to_be_bytes()); // base
    wire::write_varint(&mut old, 2); // two modifiers
    old.extend_from_slice(&[0xAB; 16]); // uuid
    old.extend_from_slice(&4.0f64.to_be_bytes()); // amount
    wire::write_varint(&mut old, 0); // operation
    // `SPEED_MODIFIER_SPRINTING_UUID`, which 1.21 names.
    old.extend_from_slice(&0x662A6B8D_DA3E_4C1C_8813_96EA6097278D_u128.to_be_bytes());
    old.extend_from_slice(&0.3f64.to_be_bytes());
    wire::write_varint(&mut old, 2);

    use pomme_registry::Registry;

    assert_eq!(
        translate_and_read(766, old, "update_attributes", player::read),
        PlayerPacket::Attributes {
            entity: 9,
            snapshots: vec![player::AttributeSnapshot {
                attribute: pomme_registry::builtin::Attribute::MaxHealth.to_u32(),
                base: 20.0,
                modifiers: vec![
                    player::AttributeModifier {
                        id: format!("minecraft:{}", "ab".repeat(16)),
                        amount: 4.0,
                        operation: 0,
                    },
                    player::AttributeModifier {
                        id: "minecraft:sprinting".into(),
                        amount: 0.3,
                        operation: 2,
                    },
                ],
            }],
        }
    );
}

/// 1.21 collapsed `projectile_power`'s acceleration vector into its
/// magnitude.
#[test]
fn translate_projectile_power_766() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(766, Direction::Clientbound, "projectile_power"),
    );
    wire::write_varint(&mut old, 9); // entity id
    for c in [0.0f64, 3.0, 4.0] {
        old.extend_from_slice(&c.to_be_bytes());
    }

    assert_eq!(
        translate_to_payload(766, old, "projectile_power"),
        [&[9][..], &5.0f64.to_be_bytes()].concat()
    );
}

/// Outbound `use_item` drops the rotation floats 1.21 appended.
#[test]
fn translate_use_item_766() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(Phase::Game, Direction::Serverbound, "use_item"),
    );
    frame.push(0); // main hand
    wire::write_varint(&mut frame, 7); // sequence
    frame.extend_from_slice(&90.0f32.to_be_bytes());
    frame.extend_from_slice(&(-10.0f32).to_be_bytes());

    let frames = translation_for(766).translate_outbound_game_frame(frame);
    assert_eq!(
        frames,
        [[old_id(766, Direction::Serverbound, "use_item") as u8, 0, 7]]
    );
}

/// A 1.20.4 optional item (`bool + item + i8 count + NBT`) translates bare
/// through `container_set_content` (whose trailing carried stack survives);
/// the NBT drops.
#[test]
fn translate_container_set_content_765() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(765, Direction::Clientbound, "container_set_content"),
    );
    old.push(1); // container id
    wire::write_varint(&mut old, 2); // state id
    wire::write_varint(&mut old, 1); // one slot
    old.push(1); // present
    wire::write_varint(&mut old, 1); // stone
    old.push(3); // count
    old.extend_from_slice(&[10, 1, 0, 1, b'd', 5, 0]); // NBT {d: 5b}
    old.push(0); // carried: empty

    assert_eq!(
        translate_and_read(765, old, "container_set_content", container::read),
        ContainerPacket::Content {
            container_id: 1,
            state_id: 2,
            items: vec![pomme_stack(pomme_registry::builtin::ItemKind::Stone, 3)],
            carried: None,
        }
    );
}

/// `count` of `kind` with no components, as pomme decodes it.
fn pomme_stack(
    kind: pomme_registry::builtin::ItemKind,
    count: i32,
) -> Option<pomme_protocol::item::ItemStack> {
    use pomme_registry::Registry;

    Some(pomme_protocol::item::ItemStack {
        item: kind.to_u32(),
        count,
        patch: Default::default(),
    })
}

/// `set_equipment` slot/item pairs, high slot bit continuing the list.
#[test]
fn translate_set_equipment_765() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(765, Direction::Clientbound, "set_equipment"),
    );
    wire::write_varint(&mut old, 9); // entity id
    old.push(0x85); // offhand, more follow
    old.extend_from_slice(&[1, SHIFTED_ITEM, 1, 0]); // bare item, empty NBT
    old.push(0); // head
    old.push(0); // empty stack

    let ContainerPacket::Equipment { entity, slots } =
        translate_and_read(765, old, "set_equipment", container::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(entity, 9);
    let [(5, Some(offhand)), (0, None)] = &slots[..] else {
        panic!("slots: {slots:?}");
    };
    assert_eq!(offhand.count, 1);
    assert_eq!(native_item(765, offhand.item), Some(SHIFTED_ITEM_KIND));
}

/// A wire-version item id as the native item, as `container_wire` remaps it.
fn native_item(protocol: i32, id: u32) -> Option<pomme_registry::builtin::ItemKind> {
    use pomme_registry::Registry;

    let native =
        translation_for(protocol).remap_to_native(pomme_protocol::ClientRegistry::Item, id);
    native.and_then(pomme_registry::builtin::ItemKind::from_u32)
}

/// The `-2` sentinel's stack, split into `set_player_inventory`, keeps its
/// wire id until the raw path remaps it.
#[test]
fn translate_set_player_inventory_remaps_765() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(765, Direction::Clientbound, "container_set_slot"),
    );
    old.push(-2i8 as u8);
    wire::write_varint(&mut old, 2); // state id
    old.extend_from_slice(&7i16.to_be_bytes());
    old.extend_from_slice(&[1, SHIFTED_ITEM, 1, 0]); // bare item, empty NBT
    let ContainerPacket::PlayerInventory {
        slot: 7,
        item: Some(item),
    } = translate_and_read(765, old, "set_player_inventory", container::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(native_item(765, item.item), Some(SHIFTED_ITEM_KIND));
}

/// 1.20.1's menu registry predates `crafter_3x3`, so its anvil is one id
/// below the native one.
#[test]
fn remap_menu_763() {
    use pomme_protocol::{ClientRegistry, RegistryTable};
    use pomme_registry::Registry;
    use pomme_registry::builtin::MenuKind;

    let anvil = RegistryTable::for_protocol(763)
        .unwrap()
        .id_of(ClientRegistry::Menu, "anvil")
        .unwrap();
    assert_ne!(MenuKind::from_u32(anvil), Some(MenuKind::Anvil));
    let native = translation_for(763).remap_to_native(ClientRegistry::Menu, anvil);
    assert_eq!(native.and_then(MenuKind::from_u32), Some(MenuKind::Anvil));
}

/// `merchant_offers` costs were plain stacks before 1.20.5's `ItemCost`
/// (with its explicit optional second cost).
#[test]
fn translate_merchant_offers_765() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(765, Direction::Clientbound, "merchant_offers"),
    );
    wire::write_varint(&mut old, 1); // container id
    wire::write_varint(&mut old, 1); // one offer
    old.extend_from_slice(&[1, SHIFTED_ITEM, 3, 0]); // costA: 3 of it
    old.extend_from_slice(&[1, 4, 1, 0]); // result
    old.push(0); // costB: empty
    old.push(0); // out of stock
    for n in [5i32, 12, 2, 0] {
        old.extend_from_slice(&n.to_be_bytes()); // uses, maxUses, xp, special
    }
    old.extend_from_slice(&0.05f32.to_be_bytes()); // price multiplier
    old.extend_from_slice(&1i32.to_be_bytes()); // demand
    wire::write_varint(&mut old, 2); // villager level
    wire::write_varint(&mut old, 30); // villager xp
    old.extend_from_slice(&[1, 1]); // show progress, can restock

    // Items keep their 765 ids, as they do in the container packets.
    // TODO: nothing reads `merchant_offers` yet; its reader has to remap
    // them (`remap_item_to_native`) the way `container_wire` does.
    let mut native = vec![1, 1]; // container id, one offer
    native.extend_from_slice(&[SHIFTED_ITEM, 3, 0]); // costA: 3 of it, no component predicates
    native.extend_from_slice(&[1, 4, 0, 0]); // result: one, no components
    native.push(0); // no costB
    native.push(0); // out of stock
    for n in [5i32, 12, 2, 0] {
        native.extend_from_slice(&n.to_be_bytes());
    }
    native.extend_from_slice(&0.05f32.to_be_bytes());
    native.extend_from_slice(&1i32.to_be_bytes());
    native.extend_from_slice(&[2, 30, 1, 1]);
    assert_eq!(translate_to_payload(765, old, "merchant_offers"), native);
}

/// An offer whose second cost 26.2 lacks is left out, not shown cheaper;
/// the offers around it remap and keep their order.
#[test]
fn translate_merchant_offers_777() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let wire_item = |name| {
        registry_id(
            RegistryTable::for_protocol(777).unwrap(),
            ClientRegistry::Item,
            name,
        )
    };
    let native_item = |name| registry_id(RegistryTable::native(), ClientRegistry::Item, name);
    let offer = |old: &mut Vec<u8>, emerald: u32, base: u32, second: Option<u32>| {
        wire::write_varint(old, base);
        old.extend_from_slice(&[2, 0]); // count, no component predicates
        old.push(1); // result: one
        wire::write_varint(old, emerald);
        old.extend_from_slice(&[0, 0]); // no components
        old.push(u8::from(second.is_some()));
        if let Some(second) = second {
            wire::write_varint(old, second);
            old.extend_from_slice(&[1, 0]);
        }
        old.push(0); // out of stock
        for n in [0i32, 12, 2, 0] {
            old.extend_from_slice(&n.to_be_bytes()); // uses, maxUses, xp, special
        }
        old.extend_from_slice(&0.05f32.to_be_bytes()); // price multiplier
        old.extend_from_slice(&0i32.to_be_bytes()); // demand
    };
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(777, Direction::Clientbound, "merchant_offers"),
    );
    wire::write_varint(&mut old, 1); // container id
    wire::write_varint(&mut old, 3); // offers
    let emerald = wire_item("emerald");
    offer(&mut old, emerald, wire_item("diamond"), None);
    offer(
        &mut old,
        emerald,
        wire_item("diamond"),
        Some(wire_item("poplar_planks")),
    );
    offer(
        &mut old,
        emerald,
        wire_item("bread"),
        Some(wire_item("diamond")),
    );
    let tail = [2, 30, 1, 1]; // villager level and xp, show progress, can restock
    old.extend_from_slice(&tail);

    let mut native = vec![1, 2]; // container id, the two offers left
    let emerald = native_item("emerald");
    offer(&mut native, emerald, native_item("diamond"), None);
    offer(
        &mut native,
        emerald,
        native_item("bread"),
        Some(native_item("diamond")),
    );
    native.extend_from_slice(&tail);
    assert_eq!(translate_to_payload(777, old, "merchant_offers"), native);
}

/// 1.20.5 widened `update_mob_effect`'s amplifier from a byte to a varint
/// and dropped the trailing factor-data NBT.
#[test]
fn translate_update_mob_effect_765() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(765, Direction::Clientbound, "update_mob_effect"),
    );
    wire::write_varint(&mut old, 7); // entity id
    wire::write_varint(&mut old, 1); // effect
    old.push(2); // amplifier
    wire::write_varint(&mut old, 600); // duration
    old.push(0); // flags
    old.push(0); // factor data: absent

    let PlayerPacket::UpdateEffect {
        entity,
        amplifier,
        duration,
        flags,
        ..
    } = translate_and_read(765, old, "update_mob_effect", player::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!((entity, amplifier, duration, flags), (7, 2, 600, 0));
}

/// 1.20.4 `update_attributes` keys attributes by resource location (mapped
/// through the 765 registry) and modifiers by UUID (hex name synthesized).
#[test]
fn translate_update_attributes_765() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(765, Direction::Clientbound, "update_attributes"),
    );
    wire::write_varint(&mut old, 9); // entity id
    wire::write_varint(&mut old, 1); // one attribute
    let key = "minecraft:generic.max_health";
    wire::write_varint(&mut old, key.len() as u32);
    old.extend_from_slice(key.as_bytes());
    old.extend_from_slice(&20.0f64.to_be_bytes()); // base
    wire::write_varint(&mut old, 1); // one modifier
    old.extend_from_slice(&[0xAB; 16]); // uuid
    old.extend_from_slice(&4.0f64.to_be_bytes()); // amount
    old.push(0); // operation

    use pomme_registry::Registry;

    assert_eq!(
        translate_and_read(765, old, "update_attributes", player::read),
        PlayerPacket::Attributes {
            entity: 9,
            snapshots: vec![player::AttributeSnapshot {
                attribute: pomme_registry::builtin::Attribute::MaxHealth.to_u32(),
                base: 20.0,
                modifiers: vec![player::AttributeModifier {
                    id: format!("minecraft:{}", "ab".repeat(16)),
                    amount: 4.0,
                    operation: 0,
                }],
            }],
        }
    );
}

/// 1.20.4 `level_particles` led with the particle type id; it moves to
/// just before the payload and `alwaysShow` is synthesized.
#[test]
fn translate_level_particles_765() {
    let fields = level_particles_fields();

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(765, Direction::Clientbound, "level_particles"),
    );
    wire::write_varint(&mut old, 30); // particle (26.2 id space)
    old.push(1); // override limiter
    old.extend_from_slice(&fields);

    assert_eq!(
        translate_to_payload(765, old, "level_particles"),
        // The override limiter and `alwaysShow` first, the particle last.
        [&[1, 0][..], &fields, &[30]].concat()
    );
}

/// The serializer-id walker at 765: 1.20.5 inserted `particles` (18),
/// `wolf_variant` (23) and `armadillo_state` (28), so 765's
/// `sniffer_state` sits at 25 (26.2's is 27).
#[test]
fn translate_entity_data_765() {
    // index 17, sniffer_state, ordinal 2
    assert_eq!(
        entity_data_entries(765, &[17, 25, 2])[0].value,
        pomme_protocol::entity_data::Value::Int(2)
    );
}

/// Translates a `set_entity_data` frame for entity 9 carrying `values` (the
/// entry list without its terminator) and reads the entries as the raw
/// handler does.
fn entity_data_entries(
    protocol: i32,
    values: &[u8],
) -> Vec<pomme_protocol::entity_data::DataValue> {
    let payload = translated_entity_data(protocol, values);
    let (id, entries) =
        super::entity_data::read_entity_data(&payload, Some(&translation_for(protocol))).unwrap();
    assert_eq!(id, 9);
    entries
}

/// The same translation's entries as their native bytes, which pins the
/// serializer ids a reader that skips a value never looks at.
fn entity_data_bytes(protocol: i32, values: &[u8]) -> Vec<u8> {
    let payload = translated_entity_data(protocol, values);
    let [9, entries @ .., 0xFF] = &payload[..] else {
        panic!("payload: {payload:?}");
    };
    entries.to_vec()
}

/// A `set_entity_data` frame for entity 9 carrying `values` (the entry list
/// without its terminator), translated: the native payload.
fn translated_entity_data(protocol: i32, values: &[u8]) -> Vec<u8> {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(protocol, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 9); // entity id
    old.extend_from_slice(values);
    old.push(0xFF);
    translate_to_payload(protocol, old, "set_entity_data")
}

/// The item of an entry's stack; `None` for an empty one.
fn entity_data_item(
    entry: &pomme_protocol::entity_data::DataValue,
) -> Option<pomme_registry::builtin::ItemKind> {
    use pomme_registry::Registry;

    let pomme_protocol::entity_data::Value::Item(stack) = &entry.value else {
        panic!("wrong value");
    };
    let stack = stack.as_ref()?;
    Some(pomme_registry::builtin::ItemKind::from_u32(stack.item).expect("a native item"))
}

/// `translate_entity_data` already remaps item stacks, so the read must not
/// remap them again: 26.3 `oak_log` (163) lands as native `oak_log` (161).
#[test]
fn entity_data_item_remaps_once_777() {
    // index 8, item_stack serializer, count 1, 26.3 oak_log (varint 163),
    // empty patch
    let entries = entity_data_entries(777, &[8, 7, 1, 0xA3, 1, 0, 0]);
    assert_eq!(
        entity_data_item(&entries[0]),
        Some(pomme_registry::builtin::ItemKind::OakLog)
    );
}

/// A component the stack walker can't size (`damage`, id 3 in both 26.3 and
/// 26.2) takes the verbatim-tail fallback, which must still remap the item.
#[test]
fn entity_data_item_with_components_remaps_once_777() {
    // oak_log with one added component (damage 5), none removed
    let entries = entity_data_entries(777, &[8, 7, 1, 0xA3, 1, 1, 0, 3, 5]);
    assert_eq!(
        entity_data_item(&entries[0]),
        Some(pomme_registry::builtin::ItemKind::OakLog)
    );
}

/// An item 26.2 lacks (26.3 `poplar_log`, 172) empties the stack; a walkable
/// one keeps the entries after it.
#[test]
fn entity_data_unmapped_item_empties_777() {
    // poplar_log (varint 172), empty patch; then index 9, int serializer, 7
    let entries = entity_data_entries(777, &[8, 7, 1, 0xAC, 1, 0, 0, 9, 1, 7]);
    assert_eq!(entity_data_item(&entries[0]), None);
    assert_eq!(entries.len(), 2);
}

/// 26.3's item grammar sizes an unmapped item's components (`damage`, 5), so
/// the stack empties and the list carries on past it.
#[test]
fn entity_data_unmapped_item_with_components_keeps_the_list_777() {
    let entries = entity_data_entries(777, &[8, 7, 1, 0xAC, 1, 1, 0, 3, 5, 9, 1, 7]);
    assert_eq!(entity_data_item(&entries[0]), None);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].index, 9);
}

/// `translate_item_765` leaves the wire item id, so the read must still remap
/// `set_entity_data` stacks on 765 wires.
#[test]
fn entity_data_item_remaps_765() {
    // index 8, item_stack serializer, present, 765 item id, count 1, empty NBT
    let entries = entity_data_entries(765, &[8, 7, 1, SHIFTED_ITEM, 1, 0x0A, 0x00]);
    assert_eq!(entity_data_item(&entries[0]), Some(SHIFTED_ITEM_KIND));
}

/// `container_set_slot`'s -1/-2 sentinels split like 767's, but with the
/// old-form item body.
#[test]
fn translate_container_set_slot_765() {
    let build = |container: i8, slot: i16| {
        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(765, Direction::Clientbound, "container_set_slot"),
        );
        old.push(container as u8);
        wire::write_varint(&mut old, 2); // state id
        old.extend_from_slice(&slot.to_be_bytes());
        old.extend_from_slice(&[1, 1, 2, 0]); // 2 stone, empty NBT
        old
    };

    let read = |name, old| translate_and_read(765, old, name, container::read);
    assert_eq!(
        read("set_cursor_item", build(-1, 0)),
        ContainerPacket::Cursor {
            item: pomme_stack(pomme_registry::builtin::ItemKind::Stone, 2)
        }
    );
    assert_eq!(
        read("set_player_inventory", build(-2, 7)),
        ContainerPacket::PlayerInventory {
            slot: 7,
            item: pomme_stack(pomme_registry::builtin::ItemKind::Stone, 2)
        }
    );
    assert_eq!(
        read("container_set_slot", build(3, 1)),
        ContainerPacket::Slot {
            container_id: 3,
            state_id: 2,
            slot: 1,
            item: pomme_stack(pomme_registry::builtin::ItemKind::Stone, 2)
        }
    );
}

/// A `dripstone_block` carrying `salmon/size`: both ids sit lower in 26.1
/// than in 26.2 (items past 26.2's inserts, components past
/// `sulfur_cube_content`), with the value in its length-prefixed form.
fn dripstone_with_size(table: &pomme_protocol::RegistryTable) -> (u32, u32, Vec<u8>) {
    use pomme_protocol::ClientRegistry;

    let item = registry_id(table, ClientRegistry::Item, "dripstone_block");
    let size = registry_id(table, ClientRegistry::DataComponentType, "salmon/size");
    let mut stack = vec![1]; // count
    wire::write_varint(&mut stack, item);
    stack.extend_from_slice(&[1, 0]); // one added, none removed
    wire::write_varint(&mut stack, size);
    (item, size, stack)
}

/// [`dripstone_with_size`]'s stack in native ids, its size `medium`.
fn native_dripstone_with_size() -> pomme_protocol::item::ItemStack {
    let (item, size, _) = dripstone_with_size(pomme_protocol::RegistryTable::native());
    let mut stack = pomme_protocol::item::ItemStack {
        item,
        count: 1,
        patch: Default::default(),
    };
    stack.patch.added.push((size, [1].into()));
    stack
}

/// A native creative stack goes out in 26.1's item and component ids, each
/// component value still length-prefixed (`OPTIONAL_UNTRUSTED_STREAM_CODEC`).
#[test]
fn translate_creative_slot_775() {
    use pomme_protocol::RegistryTable;

    let (native_item, native_size, _) = dripstone_with_size(RegistryTable::native());
    let frame = pomme_protocol::serverbound::GamePacket::SetCreativeModeSlot {
        slot: 36,
        stack: Some(native_dripstone_with_size()),
    }
    .encode();

    let (wire_item, wire_size, wire_stack) =
        dripstone_with_size(RegistryTable::for_protocol(775).unwrap());
    assert_ne!((wire_item, wire_size), (native_item, native_size));
    let mut expected = Vec::new();
    wire::write_varint(
        &mut expected,
        old_id(775, Direction::Serverbound, "set_creative_mode_slot"),
    );
    expected.extend_from_slice(&36i16.to_be_bytes());
    expected.extend_from_slice(&wire_stack);
    expected.extend_from_slice(&[1, 1]); // value length, medium
    assert_eq!(
        translation_for(775).translate_outbound_game_frame(frame),
        vec![expected]
    );
}

/// The same stack inbound lands in native ids, decoded by pomme's own
/// container reader.
#[test]
fn translate_container_set_slot_775() {
    use pomme_protocol::RegistryTable;

    let (_, _, wire_stack) = dripstone_with_size(RegistryTable::for_protocol(775).unwrap());
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(775, Direction::Clientbound, "container_set_slot"),
    );
    old.extend_from_slice(&[0, 4]); // container, state id
    old.extend_from_slice(&36i16.to_be_bytes());
    old.extend_from_slice(&wire_stack);
    old.push(1); // medium

    assert_eq!(
        translate_and_read(775, old, "container_set_slot", container::read),
        ContainerPacket::Slot {
            container_id: 0,
            state_id: 4,
            slot: 36,
            item: Some(native_dripstone_with_size())
        }
    );
}

/// 26.3's item and component ids shift past 26.2's; a slot holding a dyed
/// diamond lands in native ids, decoded by pomme's own container reader.
#[test]
fn translate_container_set_slot_777() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let wire_table = RegistryTable::for_protocol(777).unwrap();
    let native_table = RegistryTable::native();
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(777, Direction::Clientbound, "container_set_slot"),
    );
    old.extend_from_slice(&[0, 4]); // container, state id
    old.extend_from_slice(&36i16.to_be_bytes());
    old.push(1); // count
    wire::write_varint(
        &mut old,
        registry_id(wire_table, ClientRegistry::Item, "diamond"),
    );
    old.extend_from_slice(&[1, 0]); // one added, none removed
    wire::write_varint(
        &mut old,
        registry_id(wire_table, ClientRegistry::DataComponentType, "dyed_color"),
    );
    old.extend_from_slice(&0x123456i32.to_be_bytes());

    let mut item = pomme_stack(pomme_registry::builtin::ItemKind::Diamond, 1).unwrap();
    item.patch.added.push((
        registry_id(
            native_table,
            ClientRegistry::DataComponentType,
            "dyed_color",
        ),
        0x123456i32.to_be_bytes().into(),
    ));
    assert_eq!(
        translate_and_read(777, old, "container_set_slot", container::read),
        ContainerPacket::Slot {
            container_id: 0,
            state_id: 4,
            slot: 36,
            item: Some(item)
        }
    );
}

/// 765's single-NBT `registry_data` splits into per-registry frames whose
/// entries reorder by their explicit ids (wire order is the id space); the
/// captured dimension-type order then keys the 765 spawn-info rewrites,
/// whose dimension type is a resource key string.
#[test]
fn translate_config_registry_data_765() {
    use simdnbt::owned::NbtTag;

    let root = dimension_registry_codec(&[
        ("minecraft:custom_end", 1),
        ("minecraft:custom_overworld", 0),
    ]);

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        config_id(765, Direction::Clientbound, "registry_data"),
    );
    NbtTag::Compound(root).write(&mut old);

    let translation = translation_for(765);
    let frames = translation.translate_config_frame(old.into_boxed_slice());
    assert_eq!(frames.len(), 1);
    let entries = dimension_type_entries(&frames[0]);
    assert_eq!(
        entries.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
        ["minecraft:custom_overworld", "minecraft:custom_end"]
    );
    // The element alone, not the {name, id, element} wrapper 765 nests it in.
    let element = entries[0].data.as_deref().expect("entry NBT");
    let element = crate::net::chat::read_nbt_tag(element, &mut 0).unwrap();
    let element = element.compound().expect("a compound");
    assert_eq!(element.int("height"), Some(384));
    assert_eq!(element.string("name"), None);

    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(765, Direction::Clientbound, "respawn"));
    write_spawn_info_765(&mut old, "minecraft:custom_end", "minecraft:overworld");
    old.push(3); // keep-data flags

    let LifecyclePacket::Respawn {
        spawn,
        data_to_keep,
    } = read_native_frame(
        &translation
            .translate_game_frame(old.into_boxed_slice())
            .unwrap(),
        "respawn",
        lifecycle::read,
    )
    else {
        panic!("wrong packet");
    };
    assert_eq!(spawn.dimension_type, 1);
    assert_eq!(spawn.dimension, "minecraft:overworld");
    assert_eq!((spawn.seed, data_to_keep), (42, 3));
}

/// A 765 `CommonPlayerSpawnInfo`: dimension-type key, dimension name, then
/// the shared tail (seed 42, survival, no previous type, no death location).
fn write_spawn_info_765(old: &mut Vec<u8>, dimension_type: &str, dimension: &str) {
    for key in [dimension_type, dimension] {
        write_utf(old, key);
    }
    old.extend_from_slice(&42u64.to_be_bytes()); // hashed seed
    old.push(0); // game type
    old.push(0xFF); // previous game type: none
    old.extend_from_slice(&[0, 0, 0]); // debug, flat, no death location
    wire::write_varint(old, 0); // portal cooldown
}

/// The 1.20.4 game `login`: the fixed prefix copies, the spawn info's
/// dimension key becomes a registry index (0 when unseeded), and
/// enforcesSecureChat plus the newer seaLevel/onlineMode synthesize.
#[test]
fn translate_game_login_765() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(765, Direction::Clientbound, "login"));
    old.extend_from_slice(&7i32.to_be_bytes()); // player id
    old.push(0); // hardcore
    wire::write_varint(&mut old, 1); // one level
    let level = "minecraft:the_nether";
    wire::write_varint(&mut old, level.len() as u32);
    old.extend_from_slice(level.as_bytes());
    wire::write_varint(&mut old, 20); // max players
    wire::write_varint(&mut old, 12); // chunk radius
    wire::write_varint(&mut old, 10); // simulation distance
    old.extend_from_slice(&[0, 1, 0]); // reducedDebug, deathScreen, crafting
    write_spawn_info_765(&mut old, "minecraft:the_nether", "minecraft:the_nether");

    let LifecyclePacket::Login(p) = translate_and_read(765, old, "login", lifecycle::read) else {
        panic!("wrong packet");
    };
    assert_eq!(p.player_id, 7);
    assert_eq!(p.levels, ["minecraft:the_nether"]);
    assert_eq!(p.max_players, 20);
    assert!(p.show_death_screen);
    assert_eq!(p.spawn.dimension, "minecraft:the_nether");
    assert!(!p.enforces_secure_chat);
}

/// Config ids remap by name at 765 and 26.2-only packets suppress
/// (`select_known_packs` predates the known-packs handshake).
#[test]
fn translate_config_ids_765() {
    let t = translation_for(765);
    assert!(t.translates_config());
    assert!(translation_for(767).translates_config());
    assert!(!translation_for(768).translates_config());
    assert!(!translation_for(763).translates_config());

    // Inbound finish_configuration lands on its 26.2 id.
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        config_id(765, Direction::Clientbound, "finish_configuration"),
    );
    let frames = t.translate_config_frame(old.into_boxed_slice());
    let mut expected = Vec::new();
    wire::write_varint(
        &mut expected,
        config_id(776, Direction::Clientbound, "finish_configuration"),
    );
    assert_eq!(frames, [expected.into_boxed_slice()]);

    // Outbound finish_configuration remaps back; select_known_packs drops.
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        config_id(776, Direction::Serverbound, "finish_configuration"),
    );
    let old = config_id(765, Direction::Serverbound, "finish_configuration");
    assert_eq!(
        t.translate_outbound_config_frame(frame),
        Some(vec![old as u8])
    );
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        config_id(776, Direction::Serverbound, "select_known_packs"),
    );
    wire::write_varint(&mut frame, 0);
    assert_eq!(t.translate_outbound_config_frame(frame), None);
}

/// 1.20.5/1.21-era `game_profile` carried a trailing strictErrorHandling
/// bool where 1.21.2 put the session UUID; it pops before the zero pad.
#[test]
fn translate_login_finished_766() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, login_id(766, "game_profile"));
    old.extend_from_slice(&[9; 16]); // uuid
    old.extend_from_slice(&[4, b'p', b'o', b'm', b'e']); // name
    wire::write_varint(&mut old, 0); // no properties
    old.push(1); // strictErrorHandling

    for protocol in [766, 767] {
        let translated = translation_for(protocol).translate_login_frame(old.clone().into());
        let mut expected = old[..old.len() - 1].to_vec();
        expected.extend_from_slice(&[0; 16]);
        assert_eq!(&translated[..], expected);
    }
    // 765 has no trailing bool: nothing pops.
    let mut bare = old.clone();
    bare.pop();
    let translated = translation_for(765).translate_login_frame(bare.clone().into());
    bare.extend_from_slice(&[0; 16]);
    assert_eq!(&translated[..], bare);
}

/// 1.20.5 appended shouldAuthenticate to login `hello`; 765 frames gain a
/// synthesized true.
#[test]
fn translate_login_hello_765() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, login_id(765, "hello"));
    old.push(0); // server id (empty string)
    wire::write_varint(&mut old, 1); // public key
    old.push(0xAA);
    wire::write_varint(&mut old, 1); // challenge
    old.push(0xBB);

    let translated = translation_for(765).translate_login_frame(old.clone().into());
    old.push(1);
    assert_eq!(&translated[..], old);
}

/// A 26.2 `container_click`'s hashed stacks become bare old-form items
/// (`bool + item + i8 count + empty NBT`).
#[test]
fn translate_container_click_765() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(Phase::Game, Direction::Serverbound, "container_click"),
    );
    wire::write_varint(&mut frame, 1); // container id
    wire::write_varint(&mut frame, 2); // state id
    frame.extend_from_slice(&5i16.to_be_bytes()); // slot
    frame.push(0); // button
    wire::write_varint(&mut frame, 0); // click type: pickup
    wire::write_varint(&mut frame, 1); // one changed slot
    frame.extend_from_slice(&5i16.to_be_bytes());
    frame.push(1); // hashed stack present
    wire::write_varint(&mut frame, 4); // item
    wire::write_varint(&mut frame, 2); // count
    wire::write_varint(&mut frame, 1); // one hashed component
    wire::write_varint(&mut frame, 3); // component id
    frame.extend_from_slice(&0x1234i32.to_be_bytes()); // hash
    wire::write_varint(&mut frame, 0); // no removed components
    frame.push(0); // carried: empty

    let frames = translation_for(765).translate_outbound_game_frame(frame);
    let old = old_id(765, Direction::Serverbound, "container_click") as u8;
    assert_eq!(frames, [[old, 1, 2, 0, 5, 0, 0, 1, 0, 5, 1, 4, 2, 0, 0]]);
}

/// A 26.2 `set_creative_mode_slot` component stack becomes a bare old-form
/// item.
#[test]
fn translate_creative_slot_765() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(
            Phase::Game,
            Direction::Serverbound,
            "set_creative_mode_slot",
        ),
    );
    frame.extend_from_slice(&36i16.to_be_bytes()); // slot
    wire::write_varint(&mut frame, 2); // count
    wire::write_varint(&mut frame, 4); // item
    wire::write_varint(&mut frame, 0); // no added components
    wire::write_varint(&mut frame, 0); // no removed components

    let frames = translation_for(765).translate_outbound_game_frame(frame);
    let old = old_id(765, Direction::Serverbound, "set_creative_mode_slot") as u8;
    assert_eq!(frames, [[old, 0, 36, 1, 4, 2, 0]]);
}

/// 1.20.4 `chat_command` is always the signed form; empty timestamp, salt,
/// signatures and last-seen update append.
#[test]
fn translate_chat_command_765() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(Phase::Game, Direction::Serverbound, "chat_command"),
    );
    frame.extend_from_slice(&[3, b's', b'a', b'y']);

    let frames = translation_for(765).translate_outbound_game_frame(frame);
    let mut expected = vec![old_id(765, Direction::Serverbound, "chat_command") as u8];
    expected.extend_from_slice(&[3, b's', b'a', b'y']);
    expected.extend_from_slice(&[0; 16]); // timestamp, salt
    expected.extend_from_slice(&[0, 0]); // no signatures, last-seen offset
    expected.extend_from_slice(&[0; 3]); // acknowledged bit set
    assert_eq!(frames, [expected]);
}

/// Appends a 1.20.2 length-prefixed JSON component.
fn write_json(out: &mut Vec<u8>, json: &str) {
    write_utf(out, json);
}

/// A length-prefixed string, the wire form of every resource key and name.
fn write_utf(out: &mut Vec<u8>, s: &str) {
    wire::write_varint(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

/// A `{d: 5}` compound in 763's form: the tag byte, then the empty root name
/// `NbtIo.write` emitted before 1.20.2, then the payload.
fn write_named_nbt_d5(out: &mut Vec<u8>) {
    out.extend_from_slice(&[10, 0, 0, 3, 0, 1, b'd']);
    out.extend_from_slice(&5i32.to_be_bytes());
    out.push(0);
}

/// The 1.20.2 JSON -> NBT component transcode, end to end through
/// `system_chat` (leading component): nested styling and a mixed `extra`
/// array (bare string beside a compound) both come through.
#[test]
fn translate_system_chat_764() {
    let json = r#"{"text":"a","color":"red","extra":["b",{"text":"c","bold":true}]}"#;
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(764, Direction::Clientbound, "system_chat"));
    write_json(&mut old, json);
    old.push(0); // not an action bar

    let payload = translate_to_payload(764, old, "system_chat");
    let mut pos = 0;
    let text = crate::net::chat::read_component(&payload, &mut pos).unwrap();
    assert_eq!(payload[pos..], [0]);
    assert_eq!(text.plain_text(), "abc");
    assert_eq!(text.style.color, Some(0xFF5555));
    assert_eq!(text.siblings[1].style.bold, Some(true));
}

/// `tab_list` carries two back-to-back components.
#[test]
fn translate_tab_list_764() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(764, Direction::Clientbound, "tab_list"));
    write_json(&mut old, r#"{"text":"head"}"#);
    write_json(&mut old, r#""foot""#); // bare JSON literal

    let HudPacket::TabList { header, footer } = translate_and_read(764, old, "tab_list", hud::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(
        (plain(&header), plain(&footer)),
        ("head".into(), "foot".into())
    );
}

/// `set_title_text`'s single component.
#[test]
fn translate_set_title_text_764() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(764, Direction::Clientbound, "set_title_text"),
    );
    write_json(&mut old, r#"{"text":"hello"}"#);

    let HudPacket::Title(text) = translate_and_read(764, old, "set_title_text", hud::read) else {
        panic!("wrong packet");
    };
    assert_eq!(plain(&text), "hello");
}

/// A text component's plain text.
fn plain(nbt: &[u8]) -> String {
    crate::net::chat::read_component(nbt, &mut 0)
        .unwrap()
        .plain_text()
}

/// `player_combat_kill`'s trailing component.
#[test]
fn translate_combat_kill_764() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(764, Direction::Clientbound, "player_combat_kill"),
    );
    wire::write_varint(&mut old, 9); // player id
    write_json(&mut old, r#"{"text":"died"}"#);

    let PlayerPacket::CombatKill { player, message } =
        translate_and_read(764, old, "player_combat_kill", player::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(player, 9);
    assert_eq!(plain(&message), "died");
}

/// `boss_event`'s name sits inside the Add op body.
#[test]
fn translate_boss_event_764() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(764, Direction::Clientbound, "boss_event"));
    old.extend_from_slice(&[3; 16]); // bar uuid
    wire::write_varint(&mut old, 0); // add
    write_json(&mut old, r#"{"text":"boss"}"#);
    old.extend_from_slice(&0.5f32.to_be_bytes()); // progress
    wire::write_varint(&mut old, 2); // color
    wire::write_varint(&mut old, 0); // overlay
    old.push(0); // flags

    let HudPacket::BossEvent {
        op: BossOp::Add { name, progress, .. },
        ..
    } = translate_and_read(764, old, "boss_event", hud::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(plain(&name), "boss");
    assert_eq!(progress, 0.5);
}

/// The 764 team parameters (JSON components in the 767-era order) convert,
/// then the shared `translate_team` reorder runs on the result.
#[test]
fn translate_set_player_team_764() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(764, Direction::Clientbound, "set_player_team"),
    );
    old.extend_from_slice(&[3, b'r', b'e', b'd']); // team name
    old.push(0); // method: add
    write_json(&mut old, r#"{"text":"Reds"}"#);
    old.push(1); // options
    old.extend_from_slice(&[6, b'a', b'l', b'w', b'a', b'y', b's']); // visibility
    old.extend_from_slice(&[6, b'a', b'l', b'w', b'a', b'y', b's']); // collision
    wire::write_varint(&mut old, 12); // color
    write_json(&mut old, r#"{"text":"[R] "}"#);
    write_json(&mut old, r#"{"text":""}"#);
    wire::write_varint(&mut old, 1); // one player
    old.extend_from_slice(&[3, b'b', b'o', b'b']);

    let translated = translation_for(764)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    let (tx, rx) = crossbeam_channel::bounded(1);
    assert!(
        crate::net::handler::handle_raw_for_test(
            &translated,
            crate::world::block::BlockStateTable::for_protocol(764),
            &tx
        )
        .0
    );
    match rx.recv().unwrap() {
        crate::net::NetworkEvent::ScoreboardTeam {
            display_name,
            prefix,
            fill_color,
            members,
            ..
        } => {
            assert_eq!(
                display_name
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>(),
                "Reds"
            );
            assert_eq!(
                prefix
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>(),
                "[R] "
            );
            assert_eq!(fill_color, Some(0xFF5555));
            assert_eq!(members, Some(vec!["bob".to_string()]));
        }
        _ => panic!("wrong packet"),
    }
}

/// The full `player_chat` walk: nullable unsigned content transcodes and
/// the direct chat-type id becomes 26.2's holder (with the globalIndex
/// prepend); the last-seen full-signature arm exercises the walk.
#[test]
fn translate_player_chat_764() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(764, Direction::Clientbound, "player_chat"));
    old.extend_from_slice(&[7; 16]); // sender uuid
    wire::write_varint(&mut old, 3); // index
    old.push(0); // no signature
    old.extend_from_slice(&[2, b'h', b'i']); // content
    old.extend_from_slice(&11u64.to_be_bytes()); // timestamp
    old.extend_from_slice(&13u64.to_be_bytes()); // salt
    wire::write_varint(&mut old, 1); // one last-seen entry
    wire::write_varint(&mut old, 0); // carrying a full signature
    old.extend_from_slice(&[0xCD; 256]);
    old.push(1); // unsigned content present
    write_json(&mut old, r#"{"text":"hey"}"#);
    old.push(0); // filter: pass-through
    wire::write_varint(&mut old, 4); // chat type: direct registry id
    write_json(&mut old, r#"{"text":"n"}"#);
    old.push(0); // no target

    let payload = translate_to_payload(764, old, "player_chat");
    let mut r = pomme_protocol::codec::Reader::new(&payload);
    assert_eq!(r.varint(), Ok(0)); // global index
    r.bytes(16).unwrap(); // sender uuid
    assert_eq!(r.varint(), Ok(3)); // index
    assert_eq!(r.bool(), Ok(false)); // no signature
    assert_eq!(r.utf(256), Ok("hi"));
    r.bytes(16).unwrap(); // timestamp, salt
    assert_eq!(r.varint(), Ok(1)); // one last-seen entry
    assert_eq!(r.varint(), Ok(0)); // carrying a full signature
    r.bytes(256).unwrap();
    assert_eq!(r.bool(), Ok(true)); // unsigned content present
    assert_eq!(plain(r.nbt().unwrap()), "hey");
    assert_eq!(r.varint(), Ok(0)); // filter: pass-through
    assert_eq!(r.varint(), Ok(5)); // the holder of chat type 4
    assert_eq!(plain(r.nbt().unwrap()), "n");
    assert_eq!(r.bool(), Ok(false)); // no target
    r.ensure_eof().unwrap();
}

/// `disguised_chat`: message transcode plus the chat-type holder bump.
#[test]
fn translate_disguised_chat_764() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(764, Direction::Clientbound, "disguised_chat"),
    );
    write_json(&mut old, r#"{"text":"psst"}"#);
    wire::write_varint(&mut old, 2); // chat type: direct registry id
    write_json(&mut old, r#"{"text":"n"}"#);
    old.push(0); // no target

    let payload = translate_to_payload(764, old, "disguised_chat");
    let mut r = pomme_protocol::codec::Reader::new(&payload);
    assert_eq!(plain(r.nbt().unwrap()), "psst");
    assert_eq!(r.varint(), Ok(3)); // the holder of chat type 2
    assert_eq!(plain(r.nbt().unwrap()), "n");
    assert_eq!(r.bool(), Ok(false)); // no target
    r.ensure_eof().unwrap();
}

/// `player_info_update`: the display name (last action) transcodes after
/// the per-entry action payload walk.
#[test]
fn translate_player_info_764() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(764, Direction::Clientbound, "player_info_update"),
    );
    old.push(0x21); // add_player | update_display_name
    wire::write_varint(&mut old, 1); // one entry
    old.extend_from_slice(&[9; 16]); // uuid
    old.extend_from_slice(&[3, b'b', b'o', b'b']); // name
    wire::write_varint(&mut old, 0); // no properties
    old.push(1); // display name present
    write_json(&mut old, r#"{"text":"Bob"}"#);

    let PlayerInfoPacket::Update { actions, entries } =
        translate_and_read(764, old, "player_info_update", player_info::read)
    else {
        panic!("wrong packet");
    };
    assert!(actions.add_player && actions.update_display_name);
    let entry = &entries[0];
    assert_eq!(entry.name, "bob");
    assert_eq!(plain(entry.display_name.as_deref().unwrap()), "Bob");
}

/// `command_suggestions`: nullable tooltips inside the suggestion list.
#[test]
fn translate_command_suggestions_764() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(764, Direction::Clientbound, "command_suggestions"),
    );
    wire::write_varint(&mut old, 5); // transaction id
    wire::write_varint(&mut old, 1); // range start
    wire::write_varint(&mut old, 4); // range length
    wire::write_varint(&mut old, 1); // one suggestion
    old.extend_from_slice(&[4, b'/', b'h', b'e', b'y']);
    old.push(1); // tooltip present
    write_json(&mut old, r#"{"text":"tip"}"#);

    let payload = translate_to_payload(764, old, "command_suggestions");
    let mut r = pomme_protocol::codec::Reader::new(&payload);
    assert_eq!(r.varint(), Ok(5)); // transaction id
    assert_eq!((r.varint(), r.varint()), (Ok(1), Ok(4))); // range
    assert_eq!(r.varint(), Ok(1)); // one suggestion
    assert_eq!(r.utf(256), Ok("/hey"));
    assert_eq!(r.bool(), Ok(true)); // tooltip present
    assert_eq!(plain(r.nbt().unwrap()), "tip");
    r.ensure_eof().unwrap();
}

/// The 1.20.2 `set_score` split: CHANGE gains absent display/numberFormat,
/// REMOVE re-emits as the `reset_score` packet 1.20.3 added (with an empty
/// objective meaning every objective).
#[test]
fn translate_set_score_764() {
    let build = |method: u32, objective: &[u8], score: bool| {
        let mut old = Vec::new();
        wire::write_varint(&mut old, old_id(764, Direction::Clientbound, "set_score"));
        old.extend_from_slice(&[3, b'b', b'o', b'b']); // owner
        wire::write_varint(&mut old, method);
        old.push(objective.len() as u8);
        old.extend_from_slice(objective);
        if score {
            wire::write_varint(&mut old, 42);
        }
        old
    };

    let read = |name, old| translate_and_read(764, old, name, scoreboard::read);
    assert_eq!(
        read("set_score", build(0, b"obj", true)),
        ScoreboardPacket::Score {
            owner: "bob".into(),
            objective: "obj".into(),
            score: 42,
            display: None,
            number_format: None,
        }
    );
    assert_eq!(
        read("reset_score", build(1, b"obj", false)),
        ScoreboardPacket::ResetScore {
            owner: "bob".into(),
            objective: Some("obj".into()),
        }
    );
    assert_eq!(
        read("reset_score", build(1, b"", false)),
        ScoreboardPacket::ResetScore {
            owner: "bob".into(),
            objective: None,
        }
    );
}

/// The 1.20.2 `set_objective` ends at the render type; the display
/// transcodes and the numberFormat synthesizes absent.
#[test]
fn translate_set_objective_764() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(764, Direction::Clientbound, "set_objective"),
    );
    old.extend_from_slice(&[3, b'o', b'b', b'j']);
    old.push(0); // method: add
    write_json(&mut old, r#"{"text":"Deaths"}"#);
    wire::write_varint(&mut old, 1); // render type: hearts

    let ScoreboardPacket::Objective {
        method: ObjectiveMethod::Add(parameters),
        ..
    } = translate_and_read(764, old, "set_objective", scoreboard::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(plain(&parameters.display_name), "Deaths");
    assert_eq!(parameters.render_type, 1);
    assert_eq!(parameters.number_format, None);
}

/// The unsplit 1.20.2 `resource_pack` maps to `resource_pack_push` with a
/// synthesized zero UUID and a transcoded prompt.
#[test]
fn translate_resource_pack_764() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(764, Direction::Clientbound, "resource_pack"),
    );
    old.extend_from_slice(&[4, b'h', b't', b't', b'p']); // url
    old.extend_from_slice(&[2, b'a', b'b']); // hash
    old.push(1); // required
    old.push(1); // prompt present
    write_json(&mut old, r#"{"text":"pls"}"#);

    use pomme_protocol::resource_pack::{self, ResourcePackPacket};

    let ResourcePackPacket::Push {
        id,
        url,
        hash,
        required,
        prompt,
    } = translate_and_read(764, old, "resource_pack_push", resource_pack::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(
        (id, url.as_str(), hash.as_str(), required),
        (0, "http", "ab", true)
    );
    assert_eq!(plain(&prompt.unwrap()), "pls");
}

/// The serverbound `resource_pack` reply loses its UUID at 764 and the
/// post-1.20.2 action values clamp.
#[test]
fn translate_resource_pack_response_764() {
    let build = |action: u32| {
        let mut frame = Vec::new();
        wire::write_varint(
            &mut frame,
            table_id(Phase::Game, Direction::Serverbound, "resource_pack"),
        );
        frame.extend_from_slice(&[9; 16]); // pack uuid
        wire::write_varint(&mut frame, action);
        frame
    };
    let old = old_id(764, Direction::Serverbound, "resource_pack") as u8;
    let t = translation_for(764);
    assert_eq!(t.translate_outbound_game_frame(build(3)), [[old, 3]]);
    assert_eq!(t.translate_outbound_game_frame(build(4)), [[old, 3]]); // downloaded
    assert_eq!(t.translate_outbound_game_frame(build(5)), [[old, 2]]); // invalid url
    assert_eq!(t.translate_outbound_game_frame(build(7)), [[old, 1]]); // discarded
}

/// The 764 config phase: disconnect's JSON component transcodes and the
/// unsplit resource_pack becomes push, both after the id remap; the
/// serverbound reply strips its UUID.
#[test]
fn translate_config_764() {
    use pomme_protocol::resource_pack::{self, ResourcePackPacket};

    let t = translation_for(764);

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        config_id(764, Direction::Clientbound, "disconnect"),
    );
    write_json(&mut old, r#"{"text":"bye"}"#);
    let frames = t.translate_config_frame(old.into_boxed_slice());
    assert_eq!(frames.len(), 1);
    let reason = read_config_frame(&frames[0], "disconnect", |_, r| Some(r.nbt().map(plain)));
    assert_eq!(reason, "bye");

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        config_id(764, Direction::Clientbound, "resource_pack"),
    );
    old.extend_from_slice(&[1, b'u']); // url
    old.extend_from_slice(&[1, b'h']); // hash
    old.extend_from_slice(&[0, 0]); // not required, no prompt
    let frames = t.translate_config_frame(old.into_boxed_slice());
    assert_eq!(
        read_config_frame(&frames[0], "resource_pack_push", resource_pack::read),
        ResourcePackPacket::Push {
            id: 0,
            url: "u".into(),
            hash: "h".into(),
            required: false,
            prompt: None,
        }
    );

    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        config_id(776, Direction::Serverbound, "resource_pack"),
    );
    frame.extend_from_slice(&[9; 16]);
    wire::write_varint(&mut frame, 4); // downloaded
    let old = config_id(764, Direction::Serverbound, "resource_pack");
    assert_eq!(
        t.translate_outbound_config_frame(frame),
        Some(vec![old as u8, 3])
    );
}

/// Entity-data component serializers (5/6) carry JSON at 764; a skip would
/// desync the list, so they transcode in place.
#[test]
fn translate_entity_data_764() {
    use pomme_protocol::entity_data::Value;

    let mut values = vec![2, 6, 1]; // index 2, optional_component, present
    write_json(&mut values, r#"{"text":"Named"}"#);
    values.extend_from_slice(&[3, 0, 1]); // index 3, byte serializer

    let entries = entity_data_entries(764, &values);
    let Value::OptionalComponent(Some(name)) = &entries[0].value else {
        panic!("wrong serializer");
    };
    assert_eq!(plain(name), "Named");
    assert_eq!(entries[1].value, Value::Byte(1));
}

/// `update_attributes` names its attribute by registry id, and those shift
/// between versions (1.21.2 also dropped the category prefixes), so the frame
/// rewrite remaps each into the native space, once: the raw decode after it
/// must not remap again. Unremapped, an old `max_health` decodes as whatever
/// attribute holds that id in 26.2.
#[test]
fn translate_update_attributes_old_versions() {
    use pomme_protocol::player::PlayerPacket;
    use pomme_protocol::{ClientRegistry, RegistryTable};

    for protocol in [775, 774, 773, 772, 771, 770, 769, 768, 767] {
        let table = RegistryTable::for_protocol(protocol).unwrap();
        let max_health = table
            .names(ClientRegistry::Attribute)
            .iter()
            .position(|n| n == "max_health" || n == "generic.max_health")
            .unwrap() as u32;

        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(protocol, Direction::Clientbound, "update_attributes"),
        );
        wire::write_varint(&mut old, 9); // entity id
        wire::write_varint(&mut old, 1); // one attribute
        wire::write_varint(&mut old, max_health);
        old.extend_from_slice(&20.0f64.to_be_bytes()); // base
        wire::write_varint(&mut old, 1); // one modifier
        let name = "minecraft:test";
        wire::write_varint(&mut old, name.len() as u32);
        old.extend_from_slice(name.as_bytes());
        old.extend_from_slice(&4.0f64.to_be_bytes()); // amount
        wire::write_varint(&mut old, 0); // operation

        let PlayerPacket::Attributes { snapshots, .. } = translate_and_read(
            protocol,
            old,
            "update_attributes",
            pomme_protocol::player::read,
        ) else {
            panic!("wrong packet for {protocol}");
        };
        let attribute = crate::attribute::AttributeKind::from_id(snapshots[0].attribute);
        assert_eq!(
            attribute,
            Some(crate::attribute::AttributeKind::MaxHealth),
            "{protocol}"
        );
        assert_eq!(snapshots[0].base, 20.0, "{protocol}");
        assert_eq!(snapshots[0].modifiers[0].amount, 4.0, "{protocol}");
    }
}

/// A pre-1.20.5 registry codec holding one dimension-type registry, in the
/// `{registry: {type, value: [{name, id, element}]}}` shape.
fn dimension_registry_codec(dimensions: &[(&str, i32)]) -> simdnbt::owned::NbtCompound {
    use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

    let entries: Vec<NbtCompound> = dimensions
        .iter()
        .map(|&(name, id)| {
            let mut element = NbtCompound::new();
            element.insert("height", NbtTag::Int(384));
            let mut e = NbtCompound::new();
            e.insert("name", NbtTag::String(name.into()));
            e.insert("id", NbtTag::Int(id));
            e.insert("element", NbtTag::Compound(element));
            e
        })
        .collect();
    let mut registry = NbtCompound::new();
    registry.insert("type", NbtTag::String("minecraft:dimension_type".into()));
    registry.insert("value", NbtTag::List(NbtList::from(entries)));
    let mut root = NbtCompound::new();
    root.insert("minecraft:dimension_type", NbtTag::Compound(registry));
    root
}

/// The same codec in 1.20.1's form: the empty root name `NbtIo.write` emitted
/// before 1.20.2 sits between the tag byte and the payload.
fn write_registry_codec_763(old: &mut Vec<u8>, dimensions: &[(&str, i32)]) {
    use simdnbt::owned::NbtTag;

    let mut unnamed = Vec::new();
    NbtTag::Compound(dimension_registry_codec(dimensions)).write(&mut unnamed);
    old.push(unnamed[0]); // tag type
    old.extend_from_slice(&0u16.to_be_bytes()); // the empty root name
    old.extend_from_slice(&unnamed[1..]);
}

/// A 1.20.1 game `login`: the registry codec rides inline ahead of the spawn
/// info, whose fields hadn't been gathered into `CommonPlayerSpawnInfo` yet,
/// and `doLimitedCrafting` doesn't exist.
fn write_login_763(old: &mut Vec<u8>, dimension_type: &str, dimensions: &[(&str, i32)]) {
    wire::write_varint(old, old_id(763, Direction::Clientbound, "login"));
    old.extend_from_slice(&7i32.to_be_bytes()); // player id
    old.push(0); // hardcore
    old.push(0); // game type: survival
    old.push(0xFF); // previous game type: none
    wire::write_varint(old, 1); // one level
    write_utf(old, "minecraft:overworld");
    write_registry_codec_763(old, dimensions);
    for key in [dimension_type, "minecraft:overworld"] {
        write_utf(old, key);
    }
    old.extend_from_slice(&42u64.to_be_bytes()); // hashed seed
    wire::write_varint(old, 20); // max players
    wire::write_varint(old, 12); // chunk radius
    wire::write_varint(old, 10); // simulation distance
    old.extend_from_slice(&[0, 1]); // reducedDebugInfo, showDeathScreen
    old.extend_from_slice(&[0, 0, 0]); // isDebug, isFlat, no death location
    wire::write_varint(old, 8); // portal cooldown
}

/// 1.20.1 ships the registries inside the game `login` packet, which the join
/// splits into the per-registry frames later versions send in the
/// configuration phase; the captured order then keys the spawn-info rewrite.
#[test]
fn translate_login_registries_763() {
    let mut old = Vec::new();
    write_login_763(
        &mut old,
        "minecraft:custom_end",
        &[
            ("minecraft:custom_end", 1),
            ("minecraft:custom_overworld", 0),
        ],
    );

    let translation = translation_for(763);
    let frames = translation
        .split_login_registries(&old)
        .expect("registry codec");
    assert_eq!(frames.len(), 1);
    let entries = dimension_type_entries(&frames[0]);
    assert_eq!(
        entries.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
        ["minecraft:custom_overworld", "minecraft:custom_end"]
    );

    // The same frame then decodes as a game login, its dimension key resolved
    // against the order just captured.
    let LifecyclePacket::Login(p) = read_native_frame(
        &translation
            .translate_game_frame(old.into_boxed_slice())
            .unwrap(),
        "login",
        lifecycle::read,
    ) else {
        panic!("wrong packet");
    };
    assert_eq!(p.player_id, 7);
    assert_eq!(p.levels, ["minecraft:overworld"]);
    assert_eq!(p.max_players, 20);
    assert!(p.show_death_screen);
    assert!(!p.enforces_secure_chat);
    assert_eq!(p.spawn.dimension_type, 1);
    assert_eq!(p.spawn.dimension, "minecraft:overworld");
    assert_eq!(p.spawn.seed, 42);
}

/// 1.20.2 moved `dataToKeep` from the middle of the spawn info to the end.
#[test]
fn translate_respawn_763() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(763, Direction::Clientbound, "respawn"));
    for key in ["minecraft:the_nether", "minecraft:the_nether"] {
        write_utf(&mut old, key);
    }
    old.extend_from_slice(&42u64.to_be_bytes()); // hashed seed
    old.push(0); // game type
    old.push(0xFF); // previous game type: none
    old.extend_from_slice(&[0, 0]); // isDebug, isFlat
    old.push(3); // dataToKeep, ahead of the death location at 763
    old.push(0); // no death location
    wire::write_varint(&mut old, 8); // portal cooldown

    let LifecyclePacket::Respawn {
        spawn,
        data_to_keep,
    } = translate_and_read(763, old, "respawn", lifecycle::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(data_to_keep, 3);
    assert_eq!(spawn.dimension, "minecraft:the_nether");
    assert_eq!(spawn.seed, 42);
}

/// 1.20.2 packed `forget_level_chunk`'s two ints into a ChunkPos long, whose
/// big-endian halves put z first.
#[test]
fn translate_forget_level_chunk_763() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(763, Direction::Clientbound, "forget_level_chunk"),
    );
    old.extend_from_slice(&5i32.to_be_bytes()); // x
    old.extend_from_slice(&(-3i32).to_be_bytes()); // z

    assert_eq!(
        translate_and_read(763, old, "forget_level_chunk", chunk::read),
        ChunkPacket::Forget { x: 5, z: -3 }
    );
}

/// 1.20.1 spawns other players with `add_player`, which 1.20.2 dropped for
/// `add_entity`: the type is synthesized, the rotation bytes swap into
/// pitch/yaw order and the head yaw repeats the body yaw.
#[test]
fn translate_add_player_763() {
    let uuid = uuid::Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(763, Direction::Clientbound, "add_player"));
    wire::write_varint(&mut old, 42); // entity id
    old.extend_from_slice(uuid.as_bytes());
    old.extend_from_slice(&1.5f64.to_be_bytes());
    old.extend_from_slice(&64.0f64.to_be_bytes());
    old.extend_from_slice(&(-2.5f64).to_be_bytes());
    old.push(20); // yaw, ahead of pitch at 763
    old.push(10); // pitch

    let translation = translation_for(763);
    let pomme_protocol::entity::EntityPacket::Add {
        id,
        uuid: wire_uuid,
        entity_type,
        position,
        x_rot,
        y_rot,
        y_head_rot,
        data,
        ..
    } = translate_and_read(763, old, "add_entity", pomme_protocol::entity::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(id, 42);
    assert_eq!(wire_uuid, uuid.as_u128());
    let native =
        translation.remap_to_native(pomme_protocol::ClientRegistry::EntityType, entity_type);
    assert_eq!(
        native
            .and_then(<pomme_registry::builtin::EntityKind as pomme_registry::Registry>::from_u32),
        Some(pomme_registry::builtin::EntityKind::Player)
    );
    assert_eq!(position, glam::DVec3::new(1.5, 64.0, -2.5));
    let angle = pomme_protocol::entity::unpack_degrees;
    assert_eq!(
        (x_rot, y_rot, y_head_rot),
        (angle(10), angle(20), angle(20))
    );
    assert_eq!(data, 0);
}

/// 1.20.2 made the serverbound login hello's profile id mandatory; 763 wraps
/// it in a presence bool.
#[test]
fn translate_hello_763() {
    let name = "pomme";
    let uuid = uuid::Uuid::from_u128(9);
    let id = PacketTable::native()
        .id(Phase::Login, Direction::Serverbound, "hello")
        .unwrap();

    let mut frame = Vec::new();
    wire::write_varint(&mut frame, id);
    write_utf(&mut frame, name);
    frame.extend_from_slice(uuid.as_bytes());

    let out = translation_for(763).translate_outbound_login_frame(frame.clone());
    let split = frame.len() - 16;
    assert_eq!(&out[..split], &frame[..split]);
    assert_eq!(out[split], 1); // the profile id is present
    assert_eq!(&out[split + 1..], uuid.as_bytes());

    // 1.20.2 made it mandatory, so nothing is inserted from there on.
    assert_eq!(
        translation_for(764).translate_outbound_login_frame(frame.clone()),
        frame
    );
}

/// Item NBT carries an empty root name at 763 (`NbtIo.write`), so the stack
/// walk has to consume one where the later versions have none.
#[test]
fn translate_container_set_content_763() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(763, Direction::Clientbound, "container_set_content"),
    );
    old.push(1); // container id
    wire::write_varint(&mut old, 2); // state id
    wire::write_varint(&mut old, 1); // one slot
    old.push(1); // present
    wire::write_varint(&mut old, 1); // stone
    old.push(3); // count
    old.extend_from_slice(&[10, 0, 0, 1, 0, 1, b'd', 5, 0]); // named-root {d: 5b}
    old.push(0); // carried: empty

    assert_eq!(
        translate_and_read(763, old, "container_set_content", container::read),
        ContainerPacket::Content {
            container_id: 1,
            state_id: 2,
            items: vec![pomme_stack(pomme_registry::builtin::ItemKind::Stone, 3)],
            carried: None,
        }
    );
}

/// A 763 chunk names both its heightmap root and every block entity's tag,
/// and pads its section buffer for the single-value block container.
#[test]
fn translate_chunk_763() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(763, Direction::Clientbound, "level_chunk_with_light"),
    );
    old.extend_from_slice(&3i32.to_be_bytes()); // chunk x
    old.extend_from_slice(&(-2i32).to_be_bytes()); // chunk z
    // Named-root compound { MOTION_BLOCKING: [long; 1] }.
    old.push(10);
    old.extend_from_slice(&0u16.to_be_bytes()); // the empty root name
    old.push(12); // long-array tag
    old.extend_from_slice(&15u16.to_be_bytes());
    old.extend_from_slice(b"MOTION_BLOCKING");
    old.extend_from_slice(&1i32.to_be_bytes());
    old.extend_from_slice(&7u64.to_be_bytes());
    old.push(0); // compound end
    let section = [0u8, 1, 0, 5, 0, 0, 0, 0, 0]; // one byte of padding
    wire::write_varint(&mut old, section.len() as u32);
    old.extend_from_slice(&section);
    wire::write_varint(&mut old, 1); // one block entity
    old.push(0x11); // packed xz
    old.extend_from_slice(&64i16.to_be_bytes()); // y
    wire::write_varint(&mut old, 0); // block entity type
    write_named_nbt_d5(&mut old);
    old.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // empty light masks + lists

    let ChunkPacket::Level {
        x,
        z,
        heightmaps,
        sections,
        block_entities,
        ..
    } = translate_and_read(763, old, "level_chunk_with_light", chunk::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!((x, z), (3, -2));
    assert_eq!(heightmaps.len(), 1);
    assert_eq!(&*heightmaps[0].1, &[7u64]);
    assert_eq!(sections[..], [0, 1, 0, 0, 0, 5, 0, 0]);
    let [be] = &block_entities[..] else {
        panic!("block entities: {block_entities:?}");
    };
    assert_eq!(be.y, 64);
    let tag = crate::net::chat::read_nbt_tag(be.tag.as_deref().unwrap(), &mut 0).unwrap();
    assert_eq!(tag.compound().and_then(|c| c.int("d")), Some(5));
}

/// A standalone 763 `block_entity_data` names its tag's root too, where the
/// chunk's inline block entities were already handled.
#[test]
fn translate_block_entity_data_763() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(763, Direction::Clientbound, "block_entity_data"),
    );
    old.extend_from_slice(&0u64.to_be_bytes()); // block pos
    wire::write_varint(&mut old, 0); // block entity type
    write_named_nbt_d5(&mut old);

    let pomme_protocol::block::BlockPacket::EntityData { tag, .. } =
        translate_and_read(763, old, "block_entity_data", pomme_protocol::block::read)
    else {
        panic!("wrong packet");
    };
    let tag = crate::net::chat::read_nbt_tag(&tag, &mut 0).unwrap();
    assert_eq!(tag.compound().and_then(|c| c.int("d")), Some(5));
}

/// Old block entity type ids reach the raw `block_entity_data` path in the
/// wire version's numbering.
#[test]
fn remap_block_entity_type_old_versions() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let native = RegistryTable::for_protocol(776).unwrap();
    let id_in = |table: &RegistryTable, name: &str| {
        table
            .names(ClientRegistry::BlockEntityType)
            .iter()
            .position(|n| n == name)
            .map(|i| i as u32)
    };
    for protocol in [775, 774, 773, 772, 771, 770, 769, 768, 767] {
        let table = RegistryTable::for_protocol(protocol).unwrap();
        let translation = translation_for(protocol);
        let sign = id_in(table, "sign").unwrap();
        assert_eq!(
            translation.remap_to_native(ClientRegistry::BlockEntityType, sign),
            id_in(native, "sign"),
            "{protocol}"
        );
        if let Some(bed) = id_in(table, "bed") {
            assert_eq!(
                translation.remap_to_native(ClientRegistry::BlockEntityType, bed),
                id_in(native, "bed"),
                "{protocol}"
            );
        }
    }
}

/// `tag_query` carries the same named root after its transaction id.
#[test]
fn translate_tag_query_763() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(763, Direction::Clientbound, "tag_query"));
    wire::write_varint(&mut old, 7); // transaction id
    write_named_nbt_d5(&mut old);

    assert_eq!(
        translate_to_payload(763, old, "tag_query"),
        // The transaction id, then `{d: 5}` without its root name.
        [7, 10, 3, 0, 1, b'd', 0, 0, 0, 5, 0]
    );
}

/// Every element pomme embeds for `minecraft:core` has to reach the registry
/// holder: one that goes missing shifts the protocol id of every entry after
/// it. Claiming the pack is only safe while this holds, for every version
/// with a table. A registry whose elements aren't compounds is left out
/// whole, as it is when a server sends it.
#[test]
fn embedded_known_pack_elements_all_reach_the_holder() {
    use crate::net::config_wire::RegistryError;

    for protocol in [pomme_protocol::version::NATIVE.protocol, 777] {
        let table = pomme_protocol::KnownPackTable::for_protocol(protocol).expect("a table");
        for (registry, elements) in table.registries() {
            let holder = match crate::net::known_packs::filled_holder(protocol, registry) {
                Ok(holder) => holder,
                Err(RegistryError::Unsupported(_)) if registry == "block_transformer" => continue,
                Err(e) => panic!("{protocol} {registry}: {e}"),
            };
            let id = Identifier::new(format!("minecraft:{registry}"));
            let kept = holder.get(&id).map_or(0, |registry| registry.map.len());
            assert_eq!(
                kept,
                elements.count(),
                "{protocol} {registry}: entries were dropped"
            );

            // The values have to survive the JSON -> NBT conversion too:
            // `dimension_info` reads these two off the holder.
            if registry == "dimension_type" {
                let overworld =
                    &holder.get(&id).unwrap().map[&Identifier::new("minecraft:overworld")];
                assert_eq!(overworld.int("height"), Some(384));
                assert_eq!(overworld.int("min_y"), Some(-64));
            }
        }
    }
}

/// Pomme claims the game's core pack at the native version and no other.
#[test]
fn only_the_core_pack_is_claimed() {
    use pomme_protocol::KnownPack;

    let core_of = |namespace: &str| KnownPack {
        namespace: namespace.to_owned(),
        id: "core".to_owned(),
        version: pomme_protocol::version::NATIVE.name.to_owned(),
    };
    let offered = [core_of("minecraft"), core_of("pomme")];
    let known_packs =
        crate::net::known_packs::select_packs(pomme_protocol::version::NATIVE.protocol, &offered);
    assert_eq!(known_packs, offered[..1]);
}

/// 26.3's clientbound insertions shift ids, and the layouts 26.2 rewrote
/// for older versions (`login_finished`, `set_player_team`, `set_time`)
/// pass through untouched above it.
#[test]
fn translate_passthrough_777() {
    let t = translation_for(777);
    let native_config = |name| {
        PacketTable::native()
            .id(Phase::Configuration, Direction::Clientbound, name)
            .unwrap()
    };
    assert_eq!(
        old_id(777, Direction::Clientbound, "respawn"),
        table_id(Phase::Game, Direction::Clientbound, "respawn") + 2
    );
    assert_eq!(
        config_id(777, Direction::Clientbound, "store_cookie"),
        native_config("store_cookie") + 1
    );

    let mut login_finished = Vec::new();
    wire::write_varint(&mut login_finished, login_id(777, "login_finished"));
    login_finished.extend_from_slice(&[1, 2, 3]);
    assert_eq!(
        &t.translate_login_frame(login_finished.clone().into_boxed_slice())[..],
        &login_finished[..]
    );

    for name in ["set_player_team", "set_time"] {
        let mut old = Vec::new();
        wire::write_varint(&mut old, old_id(777, Direction::Clientbound, name));
        old.extend_from_slice(&[0xAA; 5]);
        let mut expected = Vec::new();
        wire::write_varint(
            &mut expected,
            table_id(Phase::Game, Direction::Clientbound, name),
        );
        expected.extend_from_slice(&[0xAA; 5]);
        assert_eq!(
            &t.translate_game_frame(old.into_boxed_slice()).unwrap()[..],
            &expected[..],
            "{name}"
        );
    }

    let mut post_effects = Vec::new();
    wire::write_varint(
        &mut post_effects,
        config_id(777, Direction::Clientbound, "post_effects"),
    );
    post_effects.push(0);
    assert!(
        t.translate_config_frame(post_effects.into_boxed_slice())
            .is_empty()
    );
    let mut cookie = Vec::new();
    wire::write_varint(
        &mut cookie,
        config_id(777, Direction::Clientbound, "store_cookie"),
    );
    cookie.push(7);
    let mut expected = Vec::new();
    wire::write_varint(&mut expected, native_config("store_cookie"));
    expected.push(7);
    assert_eq!(
        t.translate_config_frame(cookie.into_boxed_slice()),
        vec![expected.into_boxed_slice()]
    );
}

/// 26.3's `move_entity_pos*` pack onGround and a step count into a
/// `properties` varint and carry a `VecDelta` (`ClientboundMoveEntityPacket`
/// and `VecDelta` in the 26.3 reference): a plain short triple maps byte
/// for byte, stepped ones collapse to the summed displacement, and
/// `move_entity_rot` moved onGround ahead of the rotation bytes.
#[test]
fn translate_move_entity_777() {
    let shorts = |out: &mut Vec<u8>, d: [i16; 3]| {
        for c in d {
            out.extend_from_slice(&c.to_be_bytes());
        }
    };
    let head = |name, properties| {
        let mut old = Vec::new();
        wire::write_varint(&mut old, old_id(777, Direction::Clientbound, name));
        wire::write_varint(&mut old, 7);
        wire::write_varint(&mut old, properties);
        old
    };
    // 26.2's layout: the entity, the delta, then onGround.
    let pos = |delta, on_ground| {
        let mut native = vec![7];
        shorts(&mut native, delta);
        native.push(on_ground);
        native
    };

    let mut old = head("move_entity_pos", 1);
    shorts(&mut old, [100, -200, 300]);
    assert_eq!(
        translate_to_payload(777, old, "move_entity_pos"),
        pos([100, -200, 300], 1)
    );

    // Two steps (properties = 2 << 1 | onGround) summing to the same delta.
    let mut old = head("move_entity_pos", (2 << 1) | 1);
    for (ticks, d) in [(1, [40, -50, 100]), (2, [60, -150, 200])] {
        wire::write_varint(&mut old, ticks);
        shorts(&mut old, d);
    }
    assert_eq!(
        translate_to_payload(777, old, "move_entity_pos"),
        pos([100, -200, 300], 1)
    );

    let rotation = [10, (-20i8) as u8];
    let mut old = head("move_entity_pos_rot", 0);
    shorts(&mut old, [100, -200, 300]);
    old.extend_from_slice(&rotation);
    let mut native = vec![7];
    shorts(&mut native, [100, -200, 300]);
    native.extend_from_slice(&rotation);
    native.push(0);
    assert_eq!(
        translate_to_payload(777, old, "move_entity_pos_rot"),
        native
    );

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(777, Direction::Clientbound, "move_entity_rot"),
    );
    wire::write_varint(&mut old, 7);
    old.push(1);
    old.extend_from_slice(&rotation);
    assert_eq!(
        translate_to_payload(777, old, "move_entity_rot"),
        [7, rotation[0], rotation[1], 1]
    );

    // A summed displacement past the short range saturates.
    let mut old = head("move_entity_pos", 2 << 1);
    for _ in 0..2 {
        wire::write_varint(&mut old, 1);
        shorts(&mut old, [30000, 0, -30000]);
    }
    assert_eq!(
        translate_to_payload(777, old, "move_entity_pos"),
        pos([i16::MAX, 0, i16::MIN], 0)
    );
}

/// 26.3's `entity_position_sync` carries a `PositionPath` (`PositionPath`
/// in the 26.3 reference: a tagged `Vec3`, or a counted `Vec3` + tick offset
/// list ending at the last step) and no delta.
#[test]
fn translate_entity_position_sync_777() {
    let finish = |old: &mut Vec<u8>| {
        write_floats(old, [90.0, -10.0]);
        old.push(1);
    };
    let head = || {
        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(777, Direction::Clientbound, "entity_position_sync"),
        );
        wire::write_varint(&mut old, 9);
        old
    };

    let mut linear = head();
    wire::write_varint(&mut linear, 0);
    write_doubles(&mut linear, [1.0, 65.0, -2.0]);
    finish(&mut linear);
    let mut stepped = head();
    wire::write_varint(&mut stepped, 1);
    wire::write_varint(&mut stepped, 2);
    write_doubles(&mut stepped, [0.0, 64.0, 0.0]);
    wire::write_varint(&mut stepped, 1);
    write_doubles(&mut stepped, [1.0, 65.0, -2.0]);
    wire::write_varint(&mut stepped, 2);
    finish(&mut stepped);
    for old in [linear, stepped] {
        assert_eq!(
            translate_and_read(777, old, "entity_position_sync", entity::read),
            EntityPacket::PositionSync {
                id: 9,
                values: entity::PositionMoveRotation {
                    position: DVec3::new(1.0, 65.0, -2.0),
                    delta: DVec3::ZERO,
                    y_rot: 90.0,
                    x_rot: -10.0,
                },
                on_ground: true,
            }
        );
    }
}

/// 26.3's `explode` appends a `playSound` bool; its particle and sound ids
/// (the weighted block-particle list's too) are remapped into native space,
/// block states staying in wire space.
#[test]
fn translate_explode_777() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let particle_id = |table, name| registry_id(table, ClientRegistry::ParticleType, name);
    let sound_id = |table, name| registry_id(table, ClientRegistry::SoundEvent, name);
    let old_table = RegistryTable::for_protocol(777).unwrap();
    let native_table = RegistryTable::native();
    let build = |id, table, play_sound: Option<u8>| {
        let mut out = Vec::new();
        wire::write_varint(&mut out, id);
        write_doubles(&mut out, [1.0, 65.0, -2.0]);
        out.extend_from_slice(&4f32.to_be_bytes()); // radius
        out.extend_from_slice(&12i32.to_be_bytes()); // block count
        out.push(0); // no knockback
        wire::write_varint(&mut out, particle_id(table, "explosion"));
        wire::write_varint(&mut out, sound_id(table, "entity.generic.explode") + 1);
        wire::write_varint(&mut out, 1); // one block particle
        wire::write_varint(&mut out, particle_id(table, "block"));
        wire::write_varint(&mut out, 27); // a wire-space block state
        out.extend_from_slice(&1f32.to_be_bytes()); // scaling
        out.extend_from_slice(&0.5f32.to_be_bytes()); // speed
        wire::write_varint(&mut out, 3); // weight
        out.extend(play_sound);
        out
    };
    assert_ne!(
        sound_id(old_table, "entity.generic.explode"),
        sound_id(native_table, "entity.generic.explode")
    );
    let old = build(
        old_id(777, Direction::Clientbound, "explode"),
        old_table,
        Some(1),
    );
    let translated = translation_for(777)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    let expected = build(
        table_id(Phase::Game, Direction::Clientbound, "explode"),
        native_table,
        None,
    );
    assert_eq!(&translated[..], &expected[..]);
}

/// 26.3's `CommonPlayerSpawnInfo` sends `previousGameType` as an optional
/// varint (0 = none, else id + 1) where 26.2 reads a signed byte; it sits
/// a fixed distance from the end of `login` and `respawn`.
#[test]
fn translate_spawn_info_777() {
    // The byte precedes debug/flat/death/portal/sea level and the packet's
    // own trailer (two bools, or the data-to-keep byte); the respawn
    // fixture's death location is 29 bytes longer than none.
    for (name, fixture, from_end) in [
        ("login", "login/fresh", 8),
        ("respawn", "respawn/keep_all", 7 + 29),
    ] {
        // (its 26.2 byte, its 26.3 byte): none, survival, creative
        for (native_byte, wire_byte) in [(0xFF, 0), (0, 1), (1, 2)] {
            let mut native = fixture_frame(name, fixture);
            let at = native.len() - from_end;
            native[at] = native_byte;
            let mut old = native.clone();
            old[at] = wire_byte;
            old[0] = old_id(777, Direction::Clientbound, name) as u8;
            let translated = translation_for(777)
                .translate_game_frame(old.into_boxed_slice())
                .unwrap();
            assert_eq!(&translated[..], &native[..], "{name}");
        }
    }
}

/// 26.3 renumbered `animate` (wake up 0, critical hit 1, magic critical hit
/// 2) and moved the swings to `swing_animation`, rewritten back onto
/// `animate` by hand; a `none` animation drops.
#[test]
fn translate_animate_777() {
    // `ClientboundAnimatePacket`'s 26.2 actions.
    const SWING_MAIN_HAND: u8 = 0;
    const WAKE_UP: u8 = 2;
    const SWING_OFF_HAND: u8 = 3;
    const CRITICAL_HIT: u8 = 4;
    const MAGIC_CRITICAL_HIT: u8 = 5;

    let animate = |old: Vec<u8>| translate_and_read(777, old, "animate", entity::read);
    for (wire, action) in [(0, WAKE_UP), (1, CRITICAL_HIT), (2, MAGIC_CRITICAL_HIT)] {
        let mut old = Vec::new();
        wire::write_varint(&mut old, old_id(777, Direction::Clientbound, "animate"));
        wire::write_varint(&mut old, 7);
        old.push(wire);
        assert_eq!(animate(old), EntityPacket::Animate { id: 7, action });
    }
    let swing = |hand, kind| {
        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(777, Direction::Clientbound, "swing_animation"),
        );
        wire::write_varint(&mut old, 7);
        wire::write_varint(&mut old, hand);
        wire::write_varint(&mut old, kind);
        wire::write_varint(&mut old, 6); // duration
        old
    };
    for (hand, action) in [(0, SWING_MAIN_HAND), (1, SWING_OFF_HAND)] {
        assert_eq!(
            animate(swing(hand, 1)),
            EntityPacket::Animate { id: 7, action }
        );
    }
    let none = swing(0, 0).into_boxed_slice();
    assert!(translation_for(777).translate_game_frame(none).is_none());
}

/// 26.3's `level_particles` leads with the particle, splits `maxSpeed` per
/// axis, sends `count` as a varint and appends a randomization type. The
/// particle keeps its wire-space id and payload (sized per option codec:
/// none, a block state, dust's color + scale, an item template with a
/// component patch).
#[test]
fn translate_level_particles_777() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let old_table = RegistryTable::for_protocol(777).unwrap();
    let particle = |name| registry_id(old_table, ClientRegistry::ParticleType, name);
    let body = |out: &mut Vec<u8>| {
        out.extend_from_slice(&[1, 0]); // override limiter, always show
        write_doubles(out, [1.0, 65.0, -2.0]);
        write_floats(out, [0.5, 0.25, 0.125]);
    };
    let build = |particle: &[u8]| {
        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(777, Direction::Clientbound, "level_particles"),
        );
        old.extend_from_slice(particle);
        body(&mut old);
        write_floats(&mut old, [0.1, 0.2, 0.3]);
        wire::write_varint(&mut old, 12); // count
        wire::write_varint(&mut old, 0); // default randomization
        old
    };
    let expect = |particle: &[u8]| {
        let mut expected = Vec::new();
        wire::write_varint(
            &mut expected,
            table_id(Phase::Game, Direction::Clientbound, "level_particles"),
        );
        body(&mut expected);
        expected.extend_from_slice(&0.1f32.to_be_bytes());
        expected.extend_from_slice(&12i32.to_be_bytes());
        expected.extend_from_slice(particle);
        expected
    };

    let mut flame = Vec::new();
    wire::write_varint(&mut flame, particle("flame"));
    let mut block = Vec::new();
    wire::write_varint(&mut block, particle("block"));
    wire::write_varint(&mut block, 27);
    let mut dust = Vec::new();
    wire::write_varint(&mut dust, particle("dust"));
    dust.extend_from_slice(&0xFF0000i32.to_be_bytes());
    dust.extend_from_slice(&1f32.to_be_bytes());
    // stone, one, then a patch adding custom_name (numbered alike on both)
    // as the NBT string "hi".
    let mut item = Vec::new();
    wire::write_varint(&mut item, particle("item"));
    item.extend_from_slice(&[1, 1, 1, 0]);
    wire::write_varint(
        &mut item,
        registry_id(old_table, ClientRegistry::DataComponentType, "custom_name"),
    );
    item.extend_from_slice(&[8, 0, 2, b'h', b'i']);
    for p in [flame, block, dust, item] {
        let translated = translation_for(777)
            .translate_game_frame(build(&p).into_boxed_slice())
            .unwrap();
        assert_eq!(&translated[..], &expect(&p)[..]);
    }
}

/// One `update_advancements` entry as protocol `protocol` lays it out: a
/// display whose icon is `item` (a name in that version's registry) unless
/// `item` is `None`, one requirement, and the screen position (1.5, -2).
fn advancement_entry(protocol: i32, name: &str, item: Option<&str>) -> Vec<u8> {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let mut out = Vec::new();
    wire::write_utf(&mut out, name);
    out.push(0); // no parent
    out.push(u8::from(item.is_some()));
    if let Some(item) = item {
        let table = RegistryTable::for_protocol(protocol).unwrap();
        let item = registry_id(table, ClientRegistry::Item, item);
        for text in ["Title", "Description"] {
            out.extend_from_slice(&[8, 0, text.len() as u8]);
            out.extend_from_slice(text.as_bytes());
        }
        // Through 1.21.11 the icon is an `ItemStack`, count first.
        if protocol <= 774 {
            out.push(1);
            wire::write_varint(&mut out, item);
        } else {
            wire::write_varint(&mut out, item);
            out.push(1);
        }
        out.extend_from_slice(&[0, 0]); // empty patch
        out.push(2); // goal
        out.extend_from_slice(&2i32.to_be_bytes()); // show toast, no background
    }
    let position = [1.5f32.to_be_bytes(), (-2.0f32).to_be_bytes()].concat();
    // 26.3 sends the position after the entry, display or not.
    if item.is_some() && protocol < 777 {
        out.extend_from_slice(&position);
    }
    out.push(1); // one requirement group
    out.push(1);
    wire::write_utf(&mut out, "a");
    out.push(0); // no telemetry
    if protocol >= 777 {
        out.extend_from_slice(&position);
    }
    out
}

/// An `update_advancements` frame for `protocol` adding `entries`.
fn advancements_frame(protocol: i32, id: u32, entries: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    wire::write_varint(&mut out, id);
    out.push(1); // reset
    wire::write_varint(&mut out, entries.len() as u32);
    out.extend(entries.iter().flatten());
    out.extend_from_slice(&[0, 0]); // no removals, no progress
    // 1.21.5 appended `showAdvancements`.
    if protocol >= 770 {
        out.push(1);
    }
    out
}

/// Every translated version's `update_advancements` lands as the frame a
/// 26.2 server sends: the icon an `ItemStackTemplate` in native ids, the
/// position inside the display, `showAdvancements` set.
#[test]
fn translate_update_advancements_to_native() {
    let native_id = table_id(Phase::Game, Direction::Clientbound, "update_advancements");
    let entries = |protocol| {
        vec![
            advancement_entry(protocol, "minecraft:story/root", Some("wooden_pickaxe")),
            advancement_entry(protocol, "minecraft:story/mine_stone", None),
        ]
    };
    // 26.1's frame is the native one but for its ids.
    let expected = advancements_frame(
        775,
        native_id,
        &entries(pomme_protocol::version::NATIVE.protocol),
    );
    for protocol in [766, 769, 772, 774, 775, 777] {
        let old = advancements_frame(
            protocol,
            old_id(protocol, Direction::Clientbound, "update_advancements"),
            &entries(protocol),
        );
        let translated = translation_for(protocol)
            .translate_game_frame(old.into_boxed_slice())
            .unwrap();
        assert_eq!(&translated[..], &expected[..], "{protocol}");
    }

    let mut r = pomme_protocol::codec::Reader::new(&expected);
    r.varint().unwrap();
    let update = super::advancement_wire::read_update(&expected[r.pos()..]).unwrap();
    assert!(update.show_advancements);
    let display = update.added[0].1.display.as_ref().unwrap();
    assert_eq!(display.icon_item.as_deref(), Some("wooden_pickaxe"));
    assert!(update.added[1].1.display.is_none());
}

/// An icon the native version lacks (26.3 `poplar_log`) takes its display
/// with it; the entry and the ones after it stay.
#[test]
fn advancement_with_unmapped_icon_loses_its_display_777() {
    let old = advancements_frame(
        777,
        old_id(777, Direction::Clientbound, "update_advancements"),
        &[
            advancement_entry(777, "minecraft:a", Some("poplar_log")),
            advancement_entry(777, "minecraft:b", Some("stone")),
        ],
    );
    let translated = translation_for(777)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    let expected = advancements_frame(
        775,
        table_id(Phase::Game, Direction::Clientbound, "update_advancements"),
        &[
            advancement_entry(
                pomme_protocol::version::NATIVE.protocol,
                "minecraft:a",
                None,
            ),
            advancement_entry(
                pomme_protocol::version::NATIVE.protocol,
                "minecraft:b",
                Some("stone"),
            ),
        ],
    );
    assert_eq!(&translated[..], &expected[..]);
}

/// 26.3 appended the `dye_color` serializer (43); an entry using it drops
/// and the rest of the list survives.
#[test]
fn translate_entity_data_777() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(777, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 7);
    old.extend_from_slice(&[0, 0, 5]); // index 0, byte, value
    old.extend_from_slice(&[12, 43, 3]); // index 12, dye_color, red
    old.extend_from_slice(&[1, 1, 9]); // index 1, int, value
    old.push(0xFF);
    let translated = translation_for(777)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    let mut expected = Vec::new();
    wire::write_varint(
        &mut expected,
        table_id(Phase::Game, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut expected, 7);
    expected.extend_from_slice(&[0, 0, 5, 1, 1, 9, 0xFF]);
    assert_eq!(&translated[..], &expected[..]);
}

/// Outbound on 26.3: `swing` becomes the payload-less, main-hand-only
/// `punch`; `player_action` actions after the first shift up by one;
/// `sign_update` carries its text side last; and `accept_teleportation`
/// takes the pose of the `move_player_pos_rot` following it, which 26.3 no
/// longer sends.
#[test]
fn translate_outbound_777() {
    let t = translation_for(777);
    assert!(t.translates_outbound());
    let sb = |name| table_id(Phase::Game, Direction::Serverbound, name);
    let old_sb = |name| old_id(777, Direction::Serverbound, name);
    let frame = |id, payload: &[u8]| {
        let mut out = Vec::new();
        wire::write_varint(&mut out, id);
        out.extend_from_slice(payload);
        out
    };

    assert_eq!(
        t.translate_outbound_game_frame(frame(sb("swing"), &[0])),
        vec![frame(old_sb("punch"), &[])]
    );
    assert!(
        t.translate_outbound_game_frame(frame(sb("swing"), &[1]))
            .is_empty()
    );

    let rest = [0, 0, 0, 0, 0, 0, 0, 0, 1, 5]; // block pos, direction, sequence
    for (action, wire_action) in [(0, 0), (2, 3), (6, 7)] {
        let mut payload = vec![action];
        payload.extend_from_slice(&rest);
        let mut expected = vec![wire_action];
        expected.extend_from_slice(&rest);
        assert_eq!(
            t.translate_outbound_game_frame(frame(sb("player_action"), &payload)),
            vec![frame(old_sb("player_action"), &expected)]
        );
    }

    // The text side moves from before the four lines to after them.
    let (sign_pos, lines) = ([0, 0, 0, 0, 0, 0, 0, 9], [2, b'h', b'i', 0, 0, 0]);
    for front in [0, 1] {
        let native = [&sign_pos[..], &[front], &lines].concat();
        let wire = [&sign_pos[..], &lines, &[front]].concat();
        assert_eq!(
            t.translate_outbound_game_frame(frame(sb("sign_update"), &native)),
            vec![frame(old_sb("sign_update"), &wire)]
        );
    }

    // The accept is held back and folded into the PosRot sent right after
    // it, which carries the applied pose; a later PosRot is an ordinary move.
    let mut accept = Vec::new();
    wire::write_varint(&mut accept, 9);
    assert!(
        t.translate_outbound_game_frame(frame(sb("accept_teleportation"), &accept))
            .is_empty()
    );
    let mut pose = Vec::new();
    write_doubles(&mut pose, [11.0, 22.0, 33.0]);
    write_floats(&mut pose, [4.0, 5.0]);
    let mut pos_rot = pose.clone();
    pos_rot.push(0); // flags
    let mut expected = Vec::new();
    wire::write_varint(&mut expected, 9);
    expected.extend_from_slice(&pose);
    assert_eq!(
        t.translate_outbound_game_frame(frame(sb("move_player_pos_rot"), &pos_rot)),
        vec![frame(old_sb("accept_teleportation"), &expected)]
    );
    assert_eq!(
        t.translate_outbound_game_frame(frame(sb("move_player_pos_rot"), &pos_rot)),
        vec![frame(old_sb("move_player_pos_rot"), &pos_rot)]
    );

    assert!(!t.reports_use_swings());
    assert!(translation_for(775).reports_use_swings());
}

/// 26.3 writes light masks with `ByteBufCodecs.BIT_SET` (a varint byte
/// count, `BitSet.toByteArray`'s little-endian bytes) where 26.2 reads a
/// long array; the rest of the light data and the chunk data are unchanged.
#[test]
fn translate_light_777() {
    // (26.3 byte form, 26.2 long form) per mask: empty, 3 bytes, 9 bytes.
    let masks: [(Vec<u8>, Vec<u8>); 4] = [
        (vec![0], vec![0]),
        (vec![3, 1, 2, 3], {
            let mut v = vec![1];
            v.extend_from_slice(&0x03_02_01u64.to_be_bytes());
            v
        }),
        (vec![9, 1, 2, 3, 4, 5, 6, 7, 8, 9], {
            let mut v = vec![2];
            v.extend_from_slice(&0x08_07_06_05_04_03_02_01u64.to_be_bytes());
            v.extend_from_slice(&9u64.to_be_bytes());
            v
        }),
        (vec![1, 0x80], {
            let mut v = vec![1];
            v.extend_from_slice(&0x80u64.to_be_bytes());
            v
        }),
    ];
    let light = |wire: bool| {
        let mut out = Vec::new();
        for (bytes, longs) in &masks {
            out.extend_from_slice(if wire { bytes } else { longs });
        }
        out.push(1); // one sky section
        wire::write_varint(&mut out, 2048);
        out.extend_from_slice(&[0x5A; 2048]);
        out.push(0); // no block sections
        out
    };

    let update = |id, wire| {
        let mut out = Vec::new();
        wire::write_varint(&mut out, id);
        wire::write_varint(&mut out, 3); // x
        wire::write_varint(&mut out, -4i32 as u32); // z
        out.extend_from_slice(&light(wire));
        out
    };
    let translated = translation_for(777)
        .translate_game_frame(
            update(old_id(777, Direction::Clientbound, "light_update"), true).into_boxed_slice(),
        )
        .unwrap();
    let expected = update(
        table_id(Phase::Game, Direction::Clientbound, "light_update"),
        false,
    );
    assert_eq!(&translated[..], &expected[..]);
    let ChunkPacket::Light {
        light: light_data, ..
    } = translate_and_read(
        777,
        update(old_id(777, Direction::Clientbound, "light_update"), true),
        "light_update",
        chunk::read,
    )
    else {
        panic!("wrong packet");
    };
    assert_eq!(light_data.sky_updates.len(), 1);

    let chunk = |id, wire| {
        let mut out = Vec::new();
        wire::write_varint(&mut out, id);
        out.extend_from_slice(&3i32.to_be_bytes());
        out.extend_from_slice(&(-4i32).to_be_bytes());
        out.push(0); // no heightmaps
        out.extend_from_slice(&[2, 0xAB, 0xCD]); // section buffer
        out.push(1); // one block entity
        out.extend_from_slice(&[0x12, 0, 64, 7]); // packed xz, y, type
        out.extend_from_slice(&[10, 0]); // empty compound
        out.extend_from_slice(&light(wire));
        out
    };
    let translated = translation_for(777)
        .translate_game_frame(
            chunk(
                old_id(777, Direction::Clientbound, "level_chunk_with_light"),
                true,
            )
            .into_boxed_slice(),
        )
        .unwrap();
    let expected = chunk(
        table_id(
            Phase::Game,
            Direction::Clientbound,
            "level_chunk_with_light",
        ),
        false,
    );
    assert_eq!(&translated[..], &expected[..]);
}

/// 26.3's `player_chat` sends a `PARTIALLY_FILTERED` mask as a
/// `ByteBufCodecs.BIT_SET`; the body around it (`SignedMessageBody.Packed`,
/// the optional unsigned content, `ChatType.Bound`) is unchanged.
#[test]
fn translate_player_chat_777() {
    let chat = |id, mask: &[u8]| {
        let mut out = Vec::new();
        wire::write_varint(&mut out, id);
        out.push(1); // global index
        out.extend_from_slice(&[0x11; 16]); // sender
        out.push(0); // index
        out.push(0); // no signature
        out.extend_from_slice(&[2, b'h', b'i']); // content
        out.extend_from_slice(&[0; 16]); // timestamp, salt
        out.push(2); // two last-seen entries
        out.push(3); // cached id 2
        out.push(0); // a full signature
        out.extend_from_slice(&[0x22; 256]);
        out.push(0); // no unsigned content
        out.push(2); // partially filtered
        out.extend_from_slice(mask);
        out.push(1); // chat type 0
        out.extend_from_slice(&[8, 0, 2, b'm', b'e']); // name
        out.push(0); // no target
        out
    };
    let mut longs = vec![1];
    longs.extend_from_slice(&0x0105u64.to_be_bytes());
    let translated = translation_for(777)
        .translate_game_frame(
            chat(
                old_id(777, Direction::Clientbound, "player_chat"),
                &[2, 5, 1],
            )
            .into_boxed_slice(),
        )
        .unwrap();
    let expected = chat(
        table_id(Phase::Game, Direction::Clientbound, "player_chat"),
        &longs,
    );
    assert_eq!(&translated[..], &expected[..]);
}

/// A 26.3 component whose value layout changed (`pot_decorations` became
/// four optional item templates) is sized by 26.3's item grammar, so the
/// particle carrying it still arrives.
#[test]
fn translate_changed_component_777() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let old_table = RegistryTable::for_protocol(777).unwrap();
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(777, Direction::Clientbound, "level_particles"),
    );
    wire::write_varint(
        &mut old,
        registry_id(old_table, ClientRegistry::ParticleType, "item"),
    );
    old.extend_from_slice(&[1, 1, 1, 0]); // stone, one, a patch adding one
    wire::write_varint(
        &mut old,
        registry_id(
            old_table,
            ClientRegistry::DataComponentType,
            "pot_decorations",
        ),
    );
    old.extend_from_slice(&[0; 4]); // four absent sides
    old.extend_from_slice(&[1, 0]);
    write_doubles(&mut old, [1.0, 65.0, -2.0]);
    write_floats(&mut old, [0.0; 6]);
    old.extend_from_slice(&[1, 0]); // count, randomization
    // Pomme reads only the particle's type, so its payload passes through.
    assert!(
        translation_for(777)
            .translate_game_frame(old.into_boxed_slice())
            .is_some()
    );
}

/// 26.3 inserted five singleton argument types (`ArgumentTypeInfos`): shared
/// parsers remap by name with their properties copied, and a 26.3-only one
/// becomes the parser 26.2 sent for it (`/place feature` read a
/// `resource_key` over configured features).
#[test]
fn translate_commands_777() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let old_table = RegistryTable::for_protocol(777).unwrap();
    let old_parser = |name| registry_id(old_table, ClientRegistry::CommandArgumentType, name);
    let native_parser = |name| {
        registry_id(
            RegistryTable::native(),
            ClientRegistry::CommandArgumentType,
            name,
        )
    };
    let tree = |id, feature: &[u8], uuid: u32, integer: u32| {
        let mut out = Vec::new();
        wire::write_varint(&mut out, id);
        out.push(5); // five nodes
        out.extend_from_slice(&[0, 2, 1, 4]); // root -> place, n
        out.extend_from_slice(&[1, 2, 2, 3, 5, b'p', b'l', b'a', b'c', b'e']);
        out.extend_from_slice(&[2 | 0x04, 0, 1, b'f']);
        out.extend_from_slice(feature);
        out.extend_from_slice(&[2 | 0x04, 0, 1, b'u']);
        wire::write_varint(&mut out, uuid);
        out.extend_from_slice(&[2 | 0x04 | 0x10, 0, 1, b'n']);
        wire::write_varint(&mut out, integer);
        out.push(0x03); // min and max
        out.extend_from_slice(&1i32.to_be_bytes());
        out.extend_from_slice(&9i32.to_be_bytes());
        let suggestions = b"minecraft:ask_server";
        out.push(suggestions.len() as u8);
        out.extend_from_slice(suggestions);
        out.push(0); // root
        out
    };
    let mut old_feature = Vec::new();
    wire::write_varint(&mut old_feature, old_parser("feature"));
    let mut native_feature = Vec::new();
    wire::write_varint(&mut native_feature, native_parser("resource_key"));
    let registry = b"minecraft:worldgen/configured_feature";
    native_feature.push(registry.len() as u8);
    native_feature.extend_from_slice(registry);

    let old = tree(
        old_id(777, Direction::Clientbound, "commands"),
        &old_feature,
        old_parser("uuid"),
        old_parser("brigadier:integer"),
    );
    let translated = translation_for(777)
        .translate_game_frame(old.clone().into_boxed_slice())
        .unwrap();
    let expected = tree(
        table_id(Phase::Game, Direction::Clientbound, "commands"),
        &native_feature,
        native_parser("uuid"),
        native_parser("brigadier:integer"),
    );
    assert_ne!(old_parser("uuid"), native_parser("uuid"));
    assert_eq!(&translated[..], &expected[..]);
    let p = translate_and_read(777, old, "commands", pomme_protocol::commands::read);
    assert_eq!(p.nodes.len(), 5);
}

/// A pre-1.20.5 `server_data` frame: `motd`, no icon, then `tail`.
fn server_data_frame(id: u32, motd: &[u8], tail: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    wire::write_varint(&mut out, id);
    out.extend_from_slice(motd);
    out.push(0); // no icon
    out.extend_from_slice(tail);
    out
}

/// A motd as 1.20.3+ sends it: a string NBT tag.
const NBT_MOTD: &[u8] = &[8, 0, 2, b'h', b'i'];

/// 1.20.4's `server_data` loses its trailing `enforcesSecureChat` bool; a
/// frame that doesn't end in exactly that bool drops.
#[test]
fn translate_server_data_765() {
    let frame = |id, tail: &[u8]| server_data_frame(id, NBT_MOTD, tail);
    let old_id = old_id(765, Direction::Clientbound, "server_data");

    let translated = translation_for(765)
        .translate_game_frame(frame(old_id, &[1]).into_boxed_slice())
        .unwrap();
    let native = table_id(Phase::Game, Direction::Clientbound, "server_data");
    assert_eq!(&translated[..], &frame(native, &[])[..]);
    let stray = frame(old_id, &[1, 7]).into_boxed_slice();
    assert!(translation_for(765).translate_game_frame(stray).is_none());
}

/// The secure-chat flag reads out of `server_data` in both pre-1.20.5 motd
/// forms, and out of nothing else.
#[test]
fn server_data_secure_chat_765() {
    let nbt = server_data_frame(
        old_id(765, Direction::Clientbound, "server_data"),
        NBT_MOTD,
        &[1],
    );
    assert_eq!(
        translation_for(765).server_data_secure_chat(&nbt),
        Some(true)
    );

    let json = server_data_frame(
        old_id(764, Direction::Clientbound, "server_data"),
        &[4, b'"', b'h', b'i', b'"'],
        &[0],
    );
    assert_eq!(
        translation_for(764).server_data_secure_chat(&json),
        Some(false)
    );

    let mut other = Vec::new();
    wire::write_varint(
        &mut other,
        old_id(765, Direction::Clientbound, "keep_alive"),
    );
    other.extend_from_slice(&[0; 8]);
    assert_eq!(translation_for(765).server_data_secure_chat(&other), None);
}

/// 1.20.1's parser ids sit where 26.2 had since inserted parsers, and its
/// `color` is 26.2's `team_color`; both remap by name, properties intact.
#[test]
fn translate_commands_763() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let parser = |table, name| registry_id(table, ClientRegistry::CommandArgumentType, name);
    let old_table = RegistryTable::for_protocol(763).unwrap();
    let tree = |id, resource: u32, color: u32| {
        let mut out = Vec::new();
        wire::write_varint(&mut out, id);
        out.push(3); // three nodes
        out.extend_from_slice(&[0, 1, 1]); // root -> e
        out.extend_from_slice(&[2, 1, 2, 1, b'e']); // e -> c
        wire::write_varint(&mut out, resource);
        let registry = b"minecraft:entity_type";
        out.push(registry.len() as u8);
        out.extend_from_slice(registry);
        out.extend_from_slice(&[2 | 0x04, 0, 1, b'c']);
        wire::write_varint(&mut out, color);
        out.push(0); // root
        out
    };

    let old = tree(
        old_id(763, Direction::Clientbound, "commands"),
        parser(old_table, "resource"),
        parser(old_table, "color"),
    );
    let translated = translation_for(763)
        .translate_game_frame(old.clone().into_boxed_slice())
        .unwrap();
    let native = RegistryTable::native();
    let expected = tree(
        table_id(Phase::Game, Direction::Clientbound, "commands"),
        parser(native, "resource"),
        parser(native, "team_color"),
    );
    assert_ne!(parser(old_table, "resource"), parser(native, "resource"));
    assert_eq!(&translated[..], &expected[..]);
    let p = translate_and_read(763, old, "commands", pomme_protocol::commands::read);
    assert_eq!(p.nodes.len(), 3);
}

/// 26.3's `tag` slot display carries a `HolderSet`: a named tag becomes the
/// tag id 26.2 sent, and a direct item list (every ingredient on 26.3,
/// `Ingredient.display()`) the `composite` of `item` displays 26.2 sent.
#[test]
fn translate_recipe_display_777() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let slot = |name| registry_id(RegistryTable::native(), ClientRegistry::SlotDisplay, name) as u8;
    let recipe =
        |name| registry_id(RegistryTable::native(), ClientRegistry::RecipeDisplay, name) as u8;
    let tag = b"minecraft:planks";
    let ghost = |id, input: &[u8], station: &[u8]| {
        let mut out = Vec::new();
        wire::write_varint(&mut out, id);
        out.push(1); // container
        out.push(recipe("stonecutter"));
        out.extend_from_slice(input);
        out.extend_from_slice(&[slot("item"), 3]); // result
        out.extend_from_slice(station);
        out
    };
    let named = |prefix: &[u8]| {
        let mut out = prefix.to_vec();
        out.push(tag.len() as u8);
        out.extend_from_slice(tag);
        out
    };
    let old = ghost(
        old_id(777, Direction::Clientbound, "place_ghost_recipe"),
        &[slot("tag"), 3, 5, 7], // direct set of two items
        &named(&[slot("tag"), 0]),
    );
    let translated = translation_for(777)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    let expected = ghost(
        table_id(Phase::Game, Direction::Clientbound, "place_ghost_recipe"),
        &[slot("composite"), 2, slot("item"), 5, slot("item"), 7],
        &named(&[slot("tag")]),
    );
    assert_eq!(&translated[..], &expected[..]);
}

/// The level-load gate's `LEVEL_CHUNKS_LOAD_START` (game event 13, unchanged
/// in 26.3's `ClientboundGameEventPacket`) reaches the handler on a 777 wire.
#[test]
fn translate_game_event_777() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(777, Direction::Clientbound, "game_event"));
    old.push(13);
    old.extend_from_slice(&0f32.to_be_bytes());
    assert_eq!(
        translate_and_read(777, old, "game_event", player::read),
        PlayerPacket::GameEvent {
            event: 13,
            param: 0.0,
        }
    );
}

/// The game's own `update_advancements` reads as the advancements, the removal
/// and the progress `CodecOracle.advancements` put in it.
#[test]
fn update_advancements_fixture_reads() {
    let update = super::advancement_wire::read_update(&pomme_protocol::vanilla_fixtures::case(
        "update_advancements/root_and_child",
    ))
    .unwrap();

    assert!(update.reset);
    assert!(update.show_advancements);
    assert_eq!(update.removed, ["minecraft:old"]);

    let [(root, shown), (child, hidden)] = &update.added[..] else {
        panic!("two advancements");
    };
    assert_eq!(
        (root.as_str(), child.as_str()),
        ("minecraft:story/root", "minecraft:story/child")
    );
    assert_eq!(shown.requirements, [["a"]]);
    assert_eq!(hidden.requirements, [vec!["a", "b"], vec!["c"]]);
    assert!(hidden.display.is_none());
    let display = shown.display.as_ref().unwrap();
    assert_eq!(
        display.title.iter().map(|s| &*s.text).collect::<String>(),
        "hi"
    );
    assert!(matches!(
        display.frame,
        crate::ui::toast::AdvancementFrame::Goal
    ));
    assert!(display.show_toast);
    assert_eq!(display.icon_item.as_deref(), Some("stone"));

    let [(of, criteria)] = &update.progress[..] else {
        panic!("one advancement's progress");
    };
    assert_eq!(of, "minecraft:story/root");
    assert_eq!(
        *criteria,
        std::collections::HashMap::from([("a".to_owned(), true), ("b".to_owned(), false)])
    );
}

/// A 1.21.8 `sound` keeps its wire-space event id through translation; the
/// raw decoder remaps it into 26.2's numbering.
#[test]
fn translate_sound_772() {
    use pomme_protocol::sound::SoundEvent as PommeSound;
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let sound_id = |table, name| registry_id(table, ClientRegistry::SoundEvent, name);
    let old_table = RegistryTable::for_protocol(772).unwrap();
    let old_explode = sound_id(old_table, "entity.generic.explode");
    let native_explode = sound_id(RegistryTable::native(), "entity.generic.explode");
    assert_ne!(old_explode, native_explode);

    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(772, Direction::Clientbound, "sound"));
    wire::write_varint(&mut old, old_explode + 1);
    wire::write_varint(&mut old, 4); // blocks
    for c in [8i32, 16, 24] {
        old.extend_from_slice(&c.to_be_bytes());
    }
    old.extend_from_slice(&1f32.to_be_bytes());
    old.extend_from_slice(&1f32.to_be_bytes());
    old.extend_from_slice(&0i64.to_be_bytes());

    let SoundPacket::Sound {
        sound: PommeSound::Registered(id),
        ..
    } = translate_and_read(772, old, "sound", sound::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(id, old_explode);
    assert_eq!(
        translation_for(772).remap_to_native(ClientRegistry::SoundEvent, id),
        Some(native_explode)
    );
}

#[test]
fn client_information_reports_the_skin_options() {
    let skin = crate::ui::menu::SkinOptions {
        hat: false,
        main_hand_right: false,
        ..Default::default()
    };
    let info = super::client_information(8, crate::ui::chat::ChatOptions::default(), skin);
    // `HumanoidArm.LEFT`, and every `PlayerModelPart` bit but the hat's.
    assert_eq!(info.main_hand, 0);
    assert_eq!(info.model_customisation, 0x3f);
}

/// Reconnecting on the same protocol cannot inherit a previous dimension order.
#[test]
fn reconnect_resets_translation_dimensions() {
    use std::sync::Arc;

    use super::conn::{Conn, memory_pipes};

    let (end, _peer) = memory_pipes();
    let mut first = Conn::from_memory(end);
    first.translation = Some(Arc::new(translation_for(763)));
    let mut login = Vec::new();
    write_login_763(
        &mut login,
        "minecraft:custom_end",
        &[
            ("minecraft:custom_overworld", 0),
            ("minecraft:custom_end", 1),
        ],
    );
    let translation = first.translation.as_deref().unwrap();
    translation.split_login_registries(&login).unwrap();
    let translated = translation
        .translate_game_frame(login.into_boxed_slice())
        .unwrap();
    let LifecyclePacket::Login(old) = read_native_frame(&translated, "login", lifecycle::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(old.spawn.dimension_type, 1);
    drop(first);

    let (end, _peer) = memory_pipes();
    let mut second = Conn::from_memory(end);
    second.translation = Some(Arc::new(translation_for(763)));
    let mut login = Vec::new();
    write_login_763(&mut login, "minecraft:custom_end", &[]);
    let translated = second
        .translation
        .as_deref()
        .unwrap()
        .translate_game_frame(login.into_boxed_slice())
        .unwrap();
    let LifecyclePacket::Login(new) = read_native_frame(&translated, "login", lifecycle::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!(
        new.spawn.dimension_type, 0,
        "new connection must not retain custom_end at index 1"
    );
}

/// The game's own frames for the packets pomme decodes itself land as events
/// through pomme's raw handlers.
#[test]
fn player_and_world_fixtures_land_as_events() {
    use crate::net::NetworkEvent;

    let read = fixture_event;

    let id = uuid::Uuid::from_u128(pomme_protocol::vanilla_fixtures::ORACLE_ID);
    assert!(matches!(
        read("set_camera", "set_camera/other"),
        NetworkEvent::SetCamera { id: 300 }
    ));
    let NetworkEvent::MoveVehicle {
        position,
        y_rot_deg,
        x_rot_deg,
    } = read("move_vehicle", "move_vehicle/clientbound")
    else {
        panic!("expected move_vehicle");
    };
    assert_eq!(
        (position, y_rot_deg, x_rot_deg),
        (glam::DVec3::new(1.5, 64.0, -2.25), 90.0, -10.0)
    );
    let NetworkEvent::OpenSignEditor { pos, is_front_text } =
        read("open_sign_editor", "open_sign_editor/back")
    else {
        panic!("expected open_sign_editor");
    };
    assert_eq!(
        (pos, is_front_text),
        (crate::position::BlockPos::new(-12, 70, 345), false)
    );
    let NetworkEvent::ResourcePackPush {
        id: pushed, url, ..
    } = read("resource_pack_push", "resource_pack_push/prompt")
    else {
        panic!("expected resource_pack_push");
    };
    assert_eq!((pushed, url.as_str()), (id, "https://example.com/pack.zip"));
    assert!(matches!(
        read("resource_pack_pop", "resource_pack_pop/all"),
        NetworkEvent::ResourcePackPop { id: None }
    ));
    let NetworkEvent::Waypoint {
        operation,
        waypoint,
    } = read("waypoint", "waypoint/track_position")
    else {
        panic!("expected waypoint");
    };
    assert_eq!(
        operation,
        pomme_protocol::waypoint::WaypointOperation::Track
    );
    assert_eq!(
        waypoint,
        pomme_protocol::waypoint::TrackedWaypoint {
            id: pomme_protocol::waypoint::WaypointId::Uuid(id.as_u128()),
            style: "minecraft:bowtie".into(),
            color: Some([0x12, 0xab, 0x34]),
            data: pomme_protocol::waypoint::WaypointData::Position {
                x: 1,
                y: -64,
                z: 300,
            },
        }
    );
}

/// The game's own command tree lands as an event through pomme's raw handler.
#[test]
fn commands_fixture_lands_as_an_event() {
    let crate::net::NetworkEvent::CommandTree { tree } = fixture_event("commands", "commands/tree")
    else {
        panic!("expected a command tree");
    };
    assert_eq!(tree.root_child_names(), ["tp", "say", "give", "alias"]);
}
