//! Cross-checks of pomme-protocol's vanilla-derived table and encoders
//! against azalea (kept here so pomme-protocol stays azalea-free). On a
//! disagreement the table generated from the decompiled reference is
//! authoritative — azalea's own tables can lag (its 26.2 `Particle` enum is
//! out of sync, see `handler::handle_raw_game_packet`) — so a failure means
//! "investigate which side is wrong", with in-game behavior as tiebreaker.

use azalea_core::entity_id::MinecraftEntityId;
use azalea_protocol::packets::ProtocolPacket;
use azalea_protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket};
use glam::DVec3;
use pomme_protocol::packets::{Direction, PacketTable, Phase};
use pomme_protocol::wire;

fn table_id(dir: Direction, name: &str) -> u32 {
    PacketTable::latest().id(Phase::Game, dir, name).unwrap()
}

#[test]
fn packet_ids_match_azalea() {
    use azalea_protocol::packets::game::{s_attack, s_interact};

    let interact = ServerboundGamePacket::Interact(s_interact::ServerboundInteract {
        entity_id: MinecraftEntityId(0),
        hand: s_interact::InteractionHand::MainHand,
        location: Default::default(),
        using_secondary_action: false,
    });
    assert_eq!(interact.id(), table_id(Direction::Serverbound, "interact"));

    let attack = ServerboundGamePacket::Attack(s_attack::ServerboundAttack {
        entity_id: MinecraftEntityId(0),
    });
    assert_eq!(attack.id(), table_id(Direction::Serverbound, "attack"));

    let teleport = ServerboundGamePacket::TeleportToEntity(
        azalea_protocol::packets::game::s_teleport_to_entity::ServerboundTeleportToEntity {
            uuid: uuid::Uuid::nil(),
        },
    );
    assert_eq!(
        teleport.id(),
        table_id(Direction::Serverbound, "teleport_to_entity")
    );

    let particles = ClientboundGamePacket::LevelParticles(
        azalea_protocol::packets::game::c_level_particles::ClientboundLevelParticles {
            override_limiter: false,
            always_show: false,
            pos: azalea_core::position::Vec3::default(),
            x_dist: 0.0,
            y_dist: 0.0,
            z_dist: 0.0,
            max_speed: 0.0,
            count: 0,
            particle: azalea_entity::particle::Particle::AngryVillager,
        },
    );
    assert_eq!(
        particles.id(),
        table_id(Direction::Clientbound, "level_particles")
    );

    let boss_event = ClientboundGamePacket::BossEvent(
        azalea_protocol::packets::game::c_boss_event::ClientboundBossEvent {
            id: uuid::Uuid::nil(),
            operation: azalea_protocol::packets::game::c_boss_event::Operation::Remove,
        },
    );
    assert_eq!(
        boss_event.id(),
        table_id(Direction::Clientbound, "boss_event")
    );

    let advancements = ClientboundGamePacket::UpdateAdvancements(
        azalea_protocol::packets::game::c_update_advancements::ClientboundUpdateAdvancements {
            reset: false,
            added: Vec::new(),
            removed: Vec::new(),
            progress: Default::default(),
            show_advancements: true,
        },
    );
    assert_eq!(
        advancements.id(),
        table_id(Direction::Clientbound, "update_advancements")
    );

    let recipes = ClientboundGamePacket::RecipeBookAdd(
        azalea_protocol::packets::game::c_recipe_book_add::ClientboundRecipeBookAdd {
            entries: Vec::new(),
            replace: false,
        },
    );
    assert_eq!(
        recipes.id(),
        table_id(Direction::Clientbound, "recipe_book_add")
    );

    use azalea_protocol::packets::game::{
        c_clear_titles, c_set_subtitle_text, c_set_title_text, c_set_titles_animation,
    };

    let title = ClientboundGamePacket::SetTitleText(c_set_title_text::ClientboundSetTitleText {
        text: azalea_chat::FormattedText::default(),
    });
    assert_eq!(
        title.id(),
        table_id(Direction::Clientbound, "set_title_text")
    );

    let subtitle =
        ClientboundGamePacket::SetSubtitleText(c_set_subtitle_text::ClientboundSetSubtitleText {
            text: azalea_chat::FormattedText::default(),
        });
    assert_eq!(
        subtitle.id(),
        table_id(Direction::Clientbound, "set_subtitle_text")
    );

    let animation = ClientboundGamePacket::SetTitlesAnimation(
        c_set_titles_animation::ClientboundSetTitlesAnimation {
            fade_in: 10,
            stay: 70,
            fade_out: 20,
        },
    );
    assert_eq!(
        animation.id(),
        table_id(Direction::Clientbound, "set_titles_animation")
    );

    let clear = ClientboundGamePacket::ClearTitles(c_clear_titles::ClientboundClearTitles {
        reset_times: true,
    });
    assert_eq!(clear.id(), table_id(Direction::Clientbound, "clear_titles"));
}

/// Round-trip through azalea's `LpVec3` decoder to cross-check the port.
fn decode_lp_vec3(bytes: &[u8]) -> DVec3 {
    use azalea_buf::AzBuf;
    let mut cursor = std::io::Cursor::new(bytes);
    let lp = azalea_core::delta::LpVec3::azalea_read(&mut cursor).unwrap();
    assert_eq!(cursor.position() as usize, bytes.len(), "leftover bytes");
    let v = azalea_core::position::Vec3::from(lp);
    DVec3::new(v.x, v.y, v.z)
}

/// The wire translation under test for one protocol; the old-layout frames
/// in the tests below are hand-built from that version's decompiled
/// reference codecs (`reference/<version>/decompiled/.../network/`).
fn translation_for(protocol: i32) -> crate::net::translate::Translation {
    crate::net::translate::Translation::for_protocol(protocol).expect("translation data")
}

fn old_id(protocol: i32, dir: Direction, name: &str) -> u32 {
    PacketTable::for_protocol(protocol)
        .unwrap()
        .id(Phase::Game, dir, name)
        .unwrap()
}

/// A registry entry's id in the given version's table.
fn registry_id(
    table: &pomme_protocol::RegistryTable,
    reg: pomme_protocol::ClientRegistry,
    name: &str,
) -> u32 {
    table.names(reg).iter().position(|n| n == name).unwrap() as u32
}

/// Translates a hand-built old-version game frame and decodes the result
/// with azalea's 26.2 codecs.
fn translate_and_decode(protocol: i32, old: Vec<u8>) -> ClientboundGamePacket {
    let translated = translation_for(protocol)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    azalea_protocol::read::deserialize_packet(&mut std::io::Cursor::new(&translated)).unwrap()
}

/// `translate_and_decode` plus the id remap the connection applies next, for
/// packets whose frame rewrite leaves item ids in the wire version's space.
fn translate_decode_and_remap(protocol: i32, old: Vec<u8>) -> ClientboundGamePacket {
    let mut packet = translate_and_decode(protocol, old);
    assert!(translation_for(protocol).remap_inbound(&mut packet));
    packet
}

/// An item 765 and 26.2 number differently (27 against 54), so a stack that
/// never gets remapped decodes as the wrong item rather than passing by
/// identity.
const SHIFTED_ITEM: u8 = 27;
const SHIFTED_ITEM_KIND: azalea_registry::builtin::ItemKind =
    azalea_registry::builtin::ItemKind::GrassBlock;

/// Exactly the joinable non-native protocols build a translation: a version
/// with embedded tables but no `TRANSLATED` entry (the staging state while
/// its translation is built) must stay un-joinable, and a protocol without
/// tables at all never translates.
#[test]
fn no_translation_without_coverage() {
    use pomme_protocol::version::{LATEST, VERSIONS};
    for v in VERSIONS {
        assert_eq!(
            crate::net::translate::Translation::for_protocol(v.protocol).is_some(),
            crate::net::translate::joinable(v.protocol) && v.protocol != LATEST.protocol,
            "{}",
            v.name
        );
    }
    assert!(PacketTable::for_protocol(763).is_none());
    assert!(crate::net::translate::Translation::for_protocol(763).is_none());
}

/// 26.2 appended a trailing session-id UUID to login_finished
/// (`ClientboundLoginFinishedPacket.STREAM_CODEC`); the shim pads a zero one.
#[test]
fn translate_login_finished_26_1() {
    use azalea_protocol::packets::login::ClientboundLoginPacket;
    use azalea_protocol::packets::login::c_login_finished::ClientboundLoginFinished;

    let packet = ClientboundLoginPacket::LoginFinished(ClientboundLoginFinished {
        game_profile: azalea_auth::game_profile::GameProfile {
            uuid: uuid::Uuid::from_u128(0xfeed_beef),
            name: "Purdze".into(),
            properties: Default::default(),
        },
        session_id: uuid::Uuid::from_u128(0xdead),
    });
    let frame = azalea_protocol::write::serialize_packet(&packet).unwrap();
    // A 26.1 frame is the same bytes without the trailing UUID.
    let old = frame[..frame.len() - 16].to_vec().into_boxed_slice();

    let translated = translation_for(775).translate_login_frame(old);
    let decoded: ClientboundLoginPacket =
        azalea_protocol::read::deserialize_packet(&mut std::io::Cursor::new(&translated)).unwrap();
    let ClientboundLoginPacket::LoginFinished(decoded) = decoded else {
        panic!("wrong packet: {decoded:?}");
    };
    assert_eq!(decoded.game_profile.name, "Purdze");
    assert_eq!(
        decoded.game_profile.uuid,
        uuid::Uuid::from_u128(0xfeed_beef)
    );
    assert_eq!(decoded.session_id, uuid::Uuid::nil());
}

/// 26.2 added `onlineMode` before the trailing `enforcesSecureChat` bool
/// (`ClientboundLoginPacket.write`); the shim inserts `false`.
#[test]
fn translate_game_login_26_1() {
    use azalea_core::game_type::{GameMode, OptionalGameType};
    use azalea_protocol::packets::game::ClientboundGamePacket;
    use azalea_protocol::packets::game::c_login::ClientboundLogin;
    use azalea_registry::DataRegistry;

    let packet = ClientboundGamePacket::Login(ClientboundLogin {
        player_id: MinecraftEntityId(7),
        hardcore: false,
        levels: vec!["minecraft:overworld".into()],
        max_players: 20,
        chunk_radius: 12,
        simulation_distance: 10,
        reduced_debug_info: false,
        show_death_screen: true,
        do_limited_crafting: false,
        common: azalea_protocol::packets::common::CommonPlayerSpawnInfo {
            dimension_type: azalea_registry::data::DimensionKind::new_raw(0),
            dimension: "minecraft:overworld".into(),
            seed: 42,
            game_type: GameMode::Survival,
            previous_game_type: OptionalGameType(None),
            is_debug: false,
            is_flat: false,
            last_death_location: None,
            portal_cooldown: 0,
            sea_level: 63,
        },
        online_mode: false,
        enforces_secure_chat: true,
    });
    let frame = azalea_protocol::write::serialize_packet(&packet).unwrap();
    // A 26.1 frame is the same bytes without the online_mode bool, which
    // sits right before the trailing enforces_secure_chat bool.
    let mut old = frame.to_vec();
    old.remove(old.len() - 2);

    let translated = translation_for(775)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    assert_eq!(&translated[..], &frame[..]);
}

/// 26.1's team `Parameters` order is `displayName, options, visibility,
/// collision, color, prefix, suffix` with color as a `ChatFormatting`
/// ordinal; 26.2 reordered to `displayName, prefix, suffix, visibility,
/// collision, color, options` (`ClientboundSetPlayerTeamPacket.Parameters`
/// in both references). Vanilla 26.2 also changed color to
/// `Optional<TeamColor>`, but azalea (ffedf17) still decodes a plain
/// `ChatFormatting` ordinal, so these frames target azalea's layout — the
/// ordinal is copied through unchanged. Teams on native 26.2 servers
/// misdecode until azalea catches up.
#[test]
fn translate_set_player_team_26_1() {
    let team_id = table_id(Direction::Clientbound, "set_player_team");
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
    expected.push(5);
    expected.push(3); // options
    assert_eq!(&translated[..], &expected[..]);
}

/// RESET (ChatFormatting ordinal 21) passes through as-is — azalea's enum
/// has all 22 formatting variants; the method-0 player list is copied
/// verbatim.
#[test]
fn translate_set_player_team_26_1_reset_color() {
    let team_id = table_id(Direction::Clientbound, "set_player_team");
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
    expected.push(21);
    expected.push(0); // options
    expected.extend_from_slice(&[1, 3, b'b', b'o', b'b']);
    assert_eq!(&translated[..], &expected[..]);
}

/// The registry tables must agree with azalea's 26.2 enums on the id anchors
/// the remaps pivot around.
#[test]
fn registry_table_matches_azalea() {
    use azalea_registry::Registry;
    use azalea_registry::builtin::{Attribute, BlockEntityKind, EntityKind};
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let t = RegistryTable::latest();
    let index = |reg, name: &str| t.names(reg).iter().position(|n| n == name).unwrap() as u32;
    assert_eq!(
        EntityKind::SulfurCube.to_u32(),
        index(ClientRegistry::EntityType, "sulfur_cube")
    );
    assert_eq!(
        Attribute::AirDragModifier.to_u32(),
        index(ClientRegistry::Attribute, "air_drag_modifier")
    );
    assert_eq!(
        BlockEntityKind::PotentSulfur.to_u32(),
        index(ClientRegistry::BlockEntityType, "potent_sulfur")
    );
}

/// A 26.1 `add_entity` decoded with the 26.2 enum comes out as the wrong
/// kind (ids past the sulfur_cube insertion shift by one); the remap fixes
/// it in place.
#[test]
fn remap_add_entity_26_1() {
    use azalea_protocol::packets::game::ClientboundGamePacket;
    use azalea_protocol::packets::game::c_add_entity::ClientboundAddEntity;
    use azalea_registry::Registry;
    use azalea_registry::builtin::EntityKind;

    // 26.1 tadpole is id 130, which the 26.2 enum decodes as sulfur_cube.
    let mut packet = ClientboundGamePacket::AddEntity(ClientboundAddEntity {
        id: MinecraftEntityId(1),
        uuid: uuid::Uuid::nil(),
        entity_type: EntityKind::from_u32(130).unwrap(),
        position: Default::default(),
        movement: Default::default(),
        x_rot: 0,
        y_rot: 0,
        y_head_rot: 0,
        data: 0,
    });
    assert!(translation_for(775).remap_inbound(&mut packet));
    let ClientboundGamePacket::AddEntity(p) = &packet else {
        unreachable!()
    };
    assert_eq!(p.entity_type, EntityKind::Tadpole);
}

/// azalea's typed encoder always writes 26.2 component-type ids, so a
/// creative stack whose patch touches a shifted id (78+, where 26.2 inserted
/// `sulfur_cube_content`) is cleared wholesale outbound; unshifted
/// components survive.
#[test]
fn strip_creative_components_26_1() {
    use azalea_inventory::{DataComponentPatch, ItemStack, ItemStackData};
    use azalea_protocol::packets::game::ServerboundGamePacket;
    use azalea_protocol::packets::game::s_set_creative_mode_slot::ServerboundSetCreativeModeSlot;
    use azalea_registry::builtin::{DataComponentKind, ItemKind};

    let remap = |kind: DataComponentKind| {
        let mut patch = DataComponentPatch::default();
        // A removal marker carries no typed value, making it the safe way to
        // put an arbitrary kind in the otherwise-opaque patch.
        unsafe { patch.unchecked_insert_component(kind, None) };
        let mut packet =
            ServerboundGamePacket::SetCreativeModeSlot(ServerboundSetCreativeModeSlot {
                slot_num: 36,
                item_stack: ItemStack::Present(ItemStackData {
                    kind: ItemKind::Stone,
                    count: 1,
                    component_patch: patch,
                }),
            });
        translation_for(775).remap_outbound(&mut packet);
        let ServerboundGamePacket::SetCreativeModeSlot(p) = packet else {
            unreachable!()
        };
        let ItemStack::Present(data) = p.item_stack else {
            panic!("stack cleared");
        };
        data.component_patch
    };

    // max_stack_size (id 1) is numbered the same in 26.1: kept.
    assert_eq!(remap(DataComponentKind::MaxStackSize).iter().count(), 1);
    // lock (79 in 26.2, 78 in 26.1) is shifted: the patch is cleared.
    assert_eq!(remap(DataComponentKind::Lock).iter().count(), 0);
}

/// 26.1's game ids match 26.2, so its frames pass through without the id
/// remap or the outbound reroute; 1.21.11's diverge, so they don't.
#[test]
fn outbound_translation_gating() {
    assert!(!translation_for(775).translates_outbound());
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

    let ClientboundGamePacket::SetHealth(p) = translate_and_decode(774, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.health, 18.0);
    assert_eq!(p.food, 19);
    assert_eq!(p.saturation, 4.5);
}

/// The serializer-id walker (see `translate_entity_data`): 1.21.11
/// `cow_variant` is 22, 26.2's is 23.
#[test]
fn translate_entity_data_774() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(774, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 9); // entity id
    old.extend_from_slice(&[0, 0, 2]); // index 0, serializer byte, value 2
    old.extend_from_slice(&[17, 22, 4]); // index 17, cow_variant, holder id 4
    old.push(0xFF);

    let ClientboundGamePacket::SetEntityData(p) = translate_and_decode(774, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.id, MinecraftEntityId(9));
    let items = &p.packed_items.0;
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].index, 0);
    assert!(matches!(
        items[0].value,
        azalea_entity::EntityDataValue::Byte(2)
    ));
    assert_eq!(items[1].index, 17);
    assert!(matches!(
        items[1].value,
        azalea_entity::EntityDataValue::CowVariant(_)
    ));
}

#[test]
fn translate_entity_profile_774() {
    use azalea_buf::AzBuf;
    use azalea_inventory::components::Profile;

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(774, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 9);
    old.extend_from_slice(&[0, 37]);
    Profile::default().azalea_write(&mut old).unwrap();
    old.extend_from_slice(&[1, 38, 1, 0xFF]);

    let ClientboundGamePacket::SetEntityData(p) = translate_and_decode(774, old) else {
        panic!("wrong packet");
    };
    assert!(matches!(
        p.packed_items.0[0].value,
        azalea_entity::EntityDataValue::ResolvableProfile(_)
    ));
    assert!(matches!(
        p.packed_items.0[1].value,
        azalea_entity::EntityDataValue::HumanoidArm(_)
    ));
}

#[test]
fn translate_empty_entity_particles_774() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(774, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 9);
    old.extend_from_slice(&[10, 17, 0, 11, 8, 1, 0xFF]);

    let ClientboundGamePacket::SetEntityData(p) = translate_and_decode(774, old) else {
        panic!("wrong packet");
    };
    assert!(matches!(
        p.packed_items.0[0].value,
        azalea_entity::EntityDataValue::Particles(_)
    ));
    assert!(matches!(
        p.packed_items.0[1].value,
        azalea_entity::EntityDataValue::Boolean(true)
    ));
}

/// A stack component the walker doesn't know falls back to the verbatim-tail
/// copy instead of dropping the packet. `damage` (id 3 in both 774 and the
/// latest registry, varint payload) keeps the verbatim bytes decodable.
#[test]
fn translate_entity_item_stack_fallback_774() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(774, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 9);
    // index 5, serializer 7 (item stack): count 1, item 1, 1 added
    // component, 0 removed, component 3 (damage) = 7.
    old.extend_from_slice(&[5, 7, 1, 1, 1, 0, 3, 7, 0xFF]);

    let ClientboundGamePacket::SetEntityData(p) = translate_and_decode(774, old) else {
        panic!("wrong packet");
    };
    assert!(matches!(
        p.packed_items.0[0].value,
        azalea_entity::EntityDataValue::ItemStack(_)
    ));
}

/// `translate_item_stack`'s component ids against the latest registry table.
#[test]
fn component_id_anchors() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let table = RegistryTable::latest();
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

/// The per-section `fluidCount` insertion (see `translate_chunk`).
#[test]
fn translate_chunk_774() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(774, Direction::Clientbound, "level_chunk_with_light"),
    );
    old.extend_from_slice(&3i32.to_be_bytes()); // chunk x
    old.extend_from_slice(&(-2i32).to_be_bytes()); // chunk z
    old.push(0); // no heightmaps
    // One section: block count 1, single-value palettes for states/biomes.
    let section = [0u8, 1, 0, 5, 0, 0];
    wire::write_varint(&mut old, section.len() as u32);
    old.extend_from_slice(&section);
    old.push(0); // no block entities
    old.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // empty light masks + lists

    let ClientboundGamePacket::LevelChunkWithLight(p) = translate_and_decode(774, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.x, 3);
    assert_eq!(p.z, -2);
    assert_eq!(p.chunk_data.data[..], [0, 1, 0, 0, 0, 5, 0, 0]);
}

/// The world-clock map synthesis (see `translate_set_time`).
#[test]
fn translate_set_time_774() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(774, Direction::Clientbound, "set_time"));
    old.extend_from_slice(&12000u64.to_be_bytes()); // game time
    old.extend_from_slice(&6000u64.to_be_bytes()); // day time
    old.push(1); // tickDayTime

    let ClientboundGamePacket::SetTime(p) = translate_and_decode(774, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.game_time, 12000);
    let clock = p.clock_updates.values().next().unwrap();
    assert_eq!(clock.total_ticks, 6000);
    assert_eq!(clock.rate, 1.0);
}

/// Neither 1.21.11 nor 1.21.10 has a serverbound `attack`; the frame
/// becomes an `interact` with the ATTACK action (`ServerboundInteractPacket`
/// action bodies in the references), through each version's id table.
#[test]
fn translate_attack_old_versions() {
    for protocol in [774, 773, 772, 771, 770, 769, 768, 767, 766, 765] {
        let frames =
            translation_for(protocol).translate_outbound_game_frame(wire::encode_attack(42));
        let interact = old_id(protocol, Direction::Serverbound, "interact");
        assert_eq!(frames, [[interact as u8, 42, 1, 0]], "{protocol}");
    }
}

/// A 26.2 `interact` (hand + LpVec3 location) becomes the 1.21.11 pair a
/// vanilla client sends: INTERACT_AT (raw floats, then hand) then INTERACT.
#[test]
fn translate_interact_774() {
    let location = DVec3::new(0.5, 1.25, -0.25);
    let frames = translation_for(774)
        .translate_outbound_game_frame(wire::encode_interact(42, location, true));
    assert_eq!(frames.len(), 2);

    let interact = old_id(774, Direction::Serverbound, "interact") as u8;
    let at = &frames[0];
    assert_eq!(at[..3], [interact, 42, 2]);
    let float_at = |i: usize| f32::from_be_bytes(at[3 + 4 * i..7 + 4 * i].try_into().unwrap());
    for (i, expected) in [location.x, location.y, location.z].iter().enumerate() {
        // Bounded by the LpVec3 quantization the 26.2 frame already carries.
        assert!((f64::from(float_at(i)) - expected).abs() < 1e-3);
    }
    assert_eq!(at[15..], [0, 1]); // main hand, sneaking

    assert_eq!(frames[1], [interact, 42, 0, 0, 1]);
}

/// 26.2-only serverbound packets with no 1.21.11 equivalent are suppressed
/// rather than sent under a wrong id.
#[test]
fn suppress_unknown_outbound_774() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(Direction::Serverbound, "set_game_rule"),
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
    wire::write_varint(&mut frame, table_id(Direction::Serverbound, "swing"));
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
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(773, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 9); // entity id
    old.extend_from_slice(&[0, 0, 2]); // index 0, serializer byte, value 2
    old.extend_from_slice(&[17, 30, 2]); // index 17, sniffer_state, ordinal 2
    old.push(0xFF);

    let ClientboundGamePacket::SetEntityData(p) = translate_and_decode(773, old) else {
        panic!("wrong packet");
    };
    let items = &p.packed_items.0;
    assert_eq!(items.len(), 2);
    assert_eq!(items[1].index, 17);
    assert!(matches!(
        items[1].value,
        azalea_entity::EntityDataValue::SnifferState(_)
    ));
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

    let ClientboundGamePacket::MountScreenOpen(p) = translate_and_decode(773, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.container_id, 1);
    assert_eq!(p.inventory_columns, 3);
    assert_eq!(p.entity_id, MinecraftEntityId(42));
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
        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(protocol, Direction::Clientbound, "set_entity_data"),
        );
        wire::write_varint(&mut old, 9); // entity id
        old.extend_from_slice(&[0, 0, 2]); // index 0, serializer byte, value 2
        old.extend_from_slice(&[17, 31, 2]); // index 17, sniffer_state, ordinal 2
        old.push(0xFF);

        let ClientboundGamePacket::SetEntityData(p) = translate_and_decode(protocol, old) else {
            panic!("wrong packet");
        };
        let items = &p.packed_items.0;
        assert_eq!(items.len(), 2, "{protocol}");
        assert_eq!(items[1].index, 17, "{protocol}");
        assert!(
            matches!(
                items[1].value,
                azalea_entity::EntityDataValue::SnifferState(_)
            ),
            "{protocol}"
        );
    }
}

/// A `compound_tag` entry (16, removed in 1.21.9) is stripped — an empty
/// NBT compound between two live entries — instead of failing the packet.
#[test]
fn translate_entity_data_compound_tag_old_versions() {
    for protocol in [772, 771, 770] {
        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(protocol, Direction::Clientbound, "set_entity_data"),
        );
        wire::write_varint(&mut old, 9); // entity id
        old.extend_from_slice(&[0, 0, 2]); // index 0, serializer byte, value 2
        old.extend_from_slice(&[19, 16, 0x0A, 0x00]); // shoulder parrot compound
        old.extend_from_slice(&[8, 8, 1]); // index 8, boolean, true
        old.push(0xFF);

        let ClientboundGamePacket::SetEntityData(p) = translate_and_decode(protocol, old) else {
            panic!("wrong packet");
        };
        let items = &p.packed_items.0;
        assert_eq!(items.len(), 2, "{protocol}");
        assert_eq!(items[0].index, 0, "{protocol}");
        assert_eq!(items[1].index, 8, "{protocol}");
        assert!(
            matches!(
                items[1].value,
                azalea_entity::EntityDataValue::Boolean(true)
            ),
            "{protocol}"
        );
    }
}

/// A 772 particle-list value (`LivingEntity.EFFECT_PARTICLES`, serializer 18
/// there) has its particle type ids remapped and the `entity_effect` color
/// copied, so entries after it keep translating (a verbatim tail would leave
/// their shifted serializer ids in place). Byte-exact against the native
/// layout: azalea's out-of-sync `Particle` ordinals can't decode it.
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
        table_id(Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut expected, 9);
    expected.extend_from_slice(&[10, 17, 1]); // 26.2 particles serializer
    wire::write_varint(&mut expected, entity_effect(RegistryTable::latest()));
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
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(772, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 9); // entity id
    // index 5, serializer 7 (item stack): count 1, item 5, 1 added, 0
    // removed, component 61 (profile).
    old.extend_from_slice(&[5, 7, 1, 5, 1, 0, 61]);
    old.extend_from_slice(&[1, 5, b'S', b't', b'e', b'v', b'e']); // name
    old.push(0); // no uuid
    old.push(0); // no properties
    old.push(0xFF);

    let ClientboundGamePacket::SetEntityData(p) = translate_and_decode(772, old) else {
        panic!("wrong packet");
    };
    let azalea_entity::EntityDataValue::ItemStack(stack) = &p.packed_items.0[0].value else {
        panic!("wrong value");
    };
    let azalea_inventory::ItemStack::Present(data) = stack else {
        panic!("empty stack");
    };
    let profile = data
        .component_patch
        .get::<azalea_inventory::components::Profile>()
        .expect("profile component");
    let azalea_inventory::components::PartialOrFullProfile::Partial(partial) = &*profile.unpack
    else {
        panic!("expected the partial-profile arm");
    };
    assert_eq!(partial.name.as_deref(), Some("Steve"));
}

/// The velocity the shorts -> `LpVec3` tests write and expect back.
const VELOCITY_772: [f64; 3] = [0.25, -0.5, 0.0];

fn write_velocity_shorts(out: &mut Vec<u8>) {
    for c in VELOCITY_772 {
        out.extend_from_slice(&((c * 8000.0) as i16).to_be_bytes());
    }
}

/// Compares within the `LpVec3` quantization error.
fn assert_velocity(v: azalea_core::position::Vec3) {
    for (got, expected) in [v.x, v.y, v.z].into_iter().zip(VELOCITY_772) {
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
    for protocol in [772, 771, 770, 769, 768, 767, 766, 765] {
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
        old.extend_from_slice(&[10, 20, 30]); // x/y/head rotation
        wire::write_varint(&mut old, 0); // data
        write_velocity_shorts(&mut old);

        let ClientboundGamePacket::AddEntity(p) = translate_and_decode(protocol, old) else {
            panic!("wrong packet");
        };
        assert_eq!(p.id, MinecraftEntityId(7), "{protocol}");
        assert_eq!(
            (p.position.x, p.position.y, p.position.z),
            (100.5, 64.0, -20.25),
            "{protocol}"
        );
        assert_velocity(azalea_core::position::Vec3::from(p.movement));
        assert_eq!((p.x_rot, p.y_rot, p.y_head_rot), (10, 20, 30), "{protocol}");
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

    let ClientboundGamePacket::SetEntityMotion(p) = translate_and_decode(772, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.id, MinecraftEntityId(9));
    assert_velocity(azalea_core::position::Vec3::from(p.delta));
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

    let ClientboundGamePacket::PlayerRotation(p) = translate_and_decode(772, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.y_rot, 90.0);
    assert_eq!(p.x_rot, -10.0);
    assert!(!p.relative_y);
    assert!(!p.relative_x);
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

    let ClientboundGamePacket::SetDefaultSpawnPosition(p) = translate_and_decode(772, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.global_pos.dimension.to_string(), "minecraft:overworld");
    assert_eq!(
        (p.global_pos.pos.x, p.global_pos.pos.y, p.global_pos.pos.z),
        (0, 64, 0)
    );
    assert_eq!(p.yaw, 45.0);
    assert_eq!(p.pitch, 0.0);
}

/// 1.21.9 inserted `radius`/`blockCount` after the explosion center and
/// appended a block-particle list; the particle and sound ids between them
/// are remapped into 26.2's space (a 1.21.8 server sends `explosion` 22 and
/// the `entity.generic.explode` holder 615 + 1; 26.2 numbers them 30 and
/// 699). Byte-exact against a hand-built native frame so azalea's decode
/// quirks can't mask an id drift.
#[test]
fn translate_explode_772() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let particle_id = |table, name| registry_id(table, ClientRegistry::ParticleType, name);
    let sound_id = |table, name| registry_id(table, ClientRegistry::SoundEvent, name);
    let old_table = RegistryTable::for_protocol(772).unwrap();
    let latest_table = RegistryTable::latest();

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
    wire::write_varint(&mut expected, table_id(Direction::Clientbound, "explode"));
    for c in [1.0f64, 65.0, -2.0] {
        expected.extend_from_slice(&c.to_be_bytes());
    }
    expected.extend_from_slice(&0f32.to_be_bytes()); // radius
    expected.extend_from_slice(&0i32.to_be_bytes()); // block count
    expected.push(1); // knockback present
    for c in [0.1f64, 0.2, 0.3] {
        expected.extend_from_slice(&c.to_be_bytes());
    }
    wire::write_varint(&mut expected, particle_id(latest_table, "explosion"));
    wire::write_varint(
        &mut expected,
        sound_id(latest_table, "entity.generic.explode") + 1,
    );
    expected.push(0); // no block particles
    assert_eq!(&translated[..], &expected[..]);

    let packet =
        azalea_protocol::read::deserialize_packet(&mut std::io::Cursor::new(&translated)).unwrap();
    let ClientboundGamePacket::Explode(p) = packet else {
        panic!("wrong packet");
    };
    assert_eq!((p.center.x, p.center.y, p.center.z), (1.0, 65.0, -2.0));
    assert_eq!(p.radius, 0.0);
    assert_eq!(p.block_count, 0);
    let knockback = p.player_knockback.expect("knockback");
    assert_eq!((knockback.x, knockback.y, knockback.z), (0.1, 0.2, 0.3));
    assert!(p.block_particles.is_empty());
}

/// 1.21.5's `player_command` action enum still opens with PRESS/RELEASE_
/// SHIFT_KEY (`ServerboundPlayerCommandPacket.Action` in both references),
/// so a 26.2 action ordinal gains two on the old wire.
#[test]
fn translate_player_command_770() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(Direction::Serverbound, "player_command"),
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
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(769, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 9); // entity id
    old.extend_from_slice(&[17, 27, 2]); // index 17, sniffer_state, ordinal 2
    old.push(0xFF);

    let ClientboundGamePacket::SetEntityData(p) = translate_and_decode(769, old) else {
        panic!("wrong packet");
    };
    assert!(matches!(
        p.packed_items.0[0].value,
        azalea_entity::EntityDataValue::SnifferState(_)
    ));
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

    let ClientboundGamePacket::SetPlayerTeam(p) = translate_and_decode(769, old) else {
        panic!("wrong packet");
    };
    use azalea_protocol::packets::game::c_set_player_team::{
        CollisionRule, Method, NameTagVisibility,
    };
    let Method::Change(params) = p.method else {
        panic!("wrong method");
    };
    assert!(matches!(
        params.nametag_visibility,
        NameTagVisibility::HideForOtherTeams
    ));
    assert!(matches!(params.collision_rule, CollisionRule::PushOwnTeam));
}

/// The pre-1.21.5 NBT heightmap compound becomes the packed list, on top
/// of the shared per-section `fluidCount` insertion.
#[test]
fn translate_chunk_769() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(769, Direction::Clientbound, "level_chunk_with_light"),
    );
    old.extend_from_slice(&3i32.to_be_bytes()); // chunk x
    old.extend_from_slice(&(-2i32).to_be_bytes()); // chunk z
    // NBT compound { MOTION_BLOCKING: [long; 1] }.
    old.push(10);
    old.push(12); // long-array tag
    old.extend_from_slice(&15u16.to_be_bytes());
    old.extend_from_slice(b"MOTION_BLOCKING");
    old.extend_from_slice(&1i32.to_be_bytes());
    old.extend_from_slice(&7u64.to_be_bytes());
    old.push(0); // compound end
    let section = [0u8, 1, 0, 5, 0, 0];
    wire::write_varint(&mut old, section.len() as u32);
    old.extend_from_slice(&section);
    old.push(0); // no block entities
    old.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // empty light masks + lists

    let ClientboundGamePacket::LevelChunkWithLight(p) = translate_and_decode(769, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.x, 3);
    assert_eq!(p.z, -2);
    assert_eq!(p.chunk_data.heightmaps.len(), 1);
    assert_eq!(&*p.chunk_data.heightmaps[0].1, &[7u64]);
    assert_eq!(p.chunk_data.data[..], [0, 1, 0, 0, 0, 5, 0, 0]);
}

/// A `player_chat` frame with everything but the chat type fixed: 765 sends a
/// direct registry id where 769 already sends a holder.
fn player_chat_frame(protocol: i32, chat_type: u32) -> Vec<u8> {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(protocol, Direction::Clientbound, "player_chat"),
    );
    old.extend_from_slice(&[7; 16]); // sender uuid
    wire::write_varint(&mut old, 3); // index
    old.push(0); // no signature
    old.extend_from_slice(&[2, b'h', b'i']); // content
    old.extend_from_slice(&11u64.to_be_bytes()); // timestamp
    old.extend_from_slice(&13u64.to_be_bytes()); // salt
    old.push(0); // no last-seen entries
    old.push(0); // no unsigned content
    old.push(0); // filter: pass-through
    wire::write_varint(&mut old, chat_type);
    old.extend_from_slice(&[8, 0, 1, b'n']); // name: NBT string
    old.push(0); // no target
    old
}

/// Pre-1.20.5 sends the chat type as a direct registry id where 26.2 wants a
/// holder. Id 0 (`chat`, the ordinary message) is the holder's "inline value
/// follows" sentinel, so leaving it alone makes normal chat undecodable.
#[test]
fn translate_player_chat_765() {
    let ClientboundGamePacket::PlayerChat(p) = translate_and_decode(765, player_chat_frame(765, 0))
    else {
        panic!("wrong packet");
    };
    assert_eq!(p.global_index, 0);
    assert_eq!(p.index, 3);
    assert_eq!(p.body.content, "hi");
    assert_eq!(chat_kind(&p.chat_type), 0);
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

    let ClientboundGamePacket::DisguisedChat(p) = translate_and_decode(765, old) else {
        panic!("wrong packet");
    };
    assert_eq!(chat_kind(&p.chat_type), 0);
}

/// The registry id behind a decoded chat-type holder; a direct (inline) one
/// means the holder bump was missed.
fn chat_kind(bound: &azalea_protocol::packets::game::c_player_chat::ChatTypeBound) -> u32 {
    use azalea_registry::{Holder, Registry};
    match &bound.chat_type {
        Holder::Reference(kind) => kind.to_u32(),
        Holder::Direct(_) => panic!("chat type decoded as an inline value"),
    }
}

/// 1.21.5 prepended `globalIndex` to `player_chat`; the shim synthesizes
/// zero ahead of an otherwise identical body.
#[test]
fn translate_player_chat_769() {
    let ClientboundGamePacket::PlayerChat(p) = translate_and_decode(769, player_chat_frame(769, 1))
    else {
        panic!("wrong packet");
    };
    assert_eq!(p.global_index, 0);
    assert_eq!(p.index, 3);
    assert_eq!(p.body.content, "hi");
}

/// 1.21.5 appended `showAdvancements` to `update_advancements`; the shim
/// synthesizes true.
#[test]
fn translate_update_advancements_769() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(769, Direction::Clientbound, "update_advancements"),
    );
    old.push(1); // reset
    old.extend_from_slice(&[0, 0, 0]); // no added/removed/progress

    let ClientboundGamePacket::UpdateAdvancements(p) = translate_and_decode(769, old) else {
        panic!("wrong packet");
    };
    assert!(p.reset);
    assert!(p.show_advancements);
}

/// A 26.2 `container_click` carries hashed stacks; 1.21.4 wants full item
/// stacks, reconstructed bare (`ServerboundContainerClickPacket` in both
/// references).
#[test]
fn translate_container_click_769() {
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        table_id(Direction::Serverbound, "container_click"),
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

/// 1.21.4 inserted `alwaysShow` after `overrideLimiter`; the shim
/// synthesizes false for 1.21.3-and-older frames.
#[test]
fn translate_level_particles_768() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(768, Direction::Clientbound, "level_particles"),
    );
    old.push(1); // override limiter
    for c in [1.0f64, 65.0, -2.0] {
        old.extend_from_slice(&c.to_be_bytes());
    }
    for d in [0.5f32, 0.5, 0.5, 0.1] {
        old.extend_from_slice(&d.to_be_bytes());
    }
    old.extend_from_slice(&7i32.to_be_bytes()); // count
    wire::write_varint(&mut old, 30); // particle (26.2 id space)

    let ClientboundGamePacket::LevelParticles(p) = translate_and_decode(768, old) else {
        panic!("wrong packet");
    };
    assert!(p.override_limiter);
    assert!(!p.always_show);
    assert_eq!(p.count, 7);
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

    let ClientboundGamePacket::PlayerInfoUpdate(p) = translate_and_decode(768, old) else {
        panic!("wrong packet");
    };
    assert!(p.actions.update_list_order);
    assert_eq!(p.entries[0].list_order, 5);
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

    let absolute = build(0b0001_1000); // relative: Y_ROT | X_ROT
    let ClientboundGamePacket::PlayerPosition(p) = translate_and_decode(767, absolute) else {
        panic!("wrong packet");
    };
    assert_eq!(p.id, 42);
    assert_eq!(
        (p.change.pos.x, p.change.pos.y, p.change.pos.z),
        (100.5, 64.0, -20.25)
    );
    assert_eq!(
        (p.change.delta.x, p.change.delta.y, p.change.delta.z),
        (0.0, 0.0, 0.0)
    );
    assert_eq!(p.change.look_direction.y_rot(), 90.0);
    assert_eq!(p.change.look_direction.x_rot(), -10.0);
    assert!(p.relative.y_rot && p.relative.x_rot);
    assert!(!p.relative.x && !p.relative.y && !p.relative.z);
    // Absolute on every position axis, so the velocity zeroes as 1.21.1's did.
    assert!(!p.relative.delta_x && !p.relative.delta_y && !p.relative.delta_z);

    let relative_xz = build(0b0000_0101); // relative: X | Z
    let ClientboundGamePacket::PlayerPosition(p) = translate_and_decode(767, relative_xz) else {
        panic!("wrong packet");
    };
    assert!(p.relative.x && p.relative.z && !p.relative.y);
    assert!(p.relative.delta_x && p.relative.delta_z && !p.relative.delta_y);
    assert!(!p.relative.rotate_delta);
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

    let ClientboundGamePacket::EntityPositionSync(p) = translate_and_decode(767, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.id, MinecraftEntityId(9));
    assert_eq!(
        (p.values.pos.x, p.values.pos.y, p.values.pos.z),
        (1.0, 70.0, 2.0)
    );
    assert_eq!(p.values.look_direction.y_rot(), 90.0);
    assert_eq!(p.values.look_direction.x_rot(), 0.0);
    assert!(p.on_ground);
}

/// 767's two-long `set_time` with a negated (frozen) dayTime feeds the
/// shared world-clock synthesis.
#[test]
fn translate_set_time_767() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(767, Direction::Clientbound, "set_time"));
    old.extend_from_slice(&12000u64.to_be_bytes()); // game time
    old.extend_from_slice(&(-6000i64).to_be_bytes()); // frozen day time

    let ClientboundGamePacket::SetTime(p) = translate_and_decode(767, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.game_time, 12000);
    let clock = p.clock_updates.values().next().unwrap();
    assert_eq!(clock.total_ticks, 6000);
    assert_eq!(clock.rate, 0.0);
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

    let ClientboundGamePacket::SetCursorItem(_) = translate_and_decode(767, build(-1, 0)) else {
        panic!("wrong packet for -1");
    };
    let ClientboundGamePacket::SetPlayerInventory(p) = translate_and_decode(767, build(-2, 7))
    else {
        panic!("wrong packet for -2");
    };
    assert_eq!(p.slot, 7);
    let ClientboundGamePacket::ContainerSetSlot(p) = translate_and_decode(767, build(3, 1)) else {
        panic!("wrong packet for plain");
    };
    assert_eq!(p.container_id, 3);
    assert_eq!(p.slot, 1);
}

/// 767 `cooldown` item ids remap into the latest registry space (azalea
/// still decodes the item id form).
#[test]
fn translate_cooldown_767() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(767, Direction::Clientbound, "cooldown"));
    wire::write_varint(&mut old, 5); // item, unshifted below the divergence
    wire::write_varint(&mut old, 100); // duration

    let ClientboundGamePacket::Cooldown(p) = translate_and_decode(767, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.duration, 100);
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

    let ClientboundGamePacket::SetHeldSlot(p) = translate_and_decode(767, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.slot, 3);
}

/// Outbound `player_input` becomes 767's vehicle-steering layout (axis
/// floats + jump/shift flags).
#[test]
fn translate_player_input_767() {
    let mut frame = Vec::new();
    wire::write_varint(&mut frame, table_id(Direction::Serverbound, "player_input"));
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
        table_id(Direction::Serverbound, "move_player_status_only"),
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
    wire::write_varint(&mut old, 1); // one modifier
    old.extend_from_slice(&[0xAB; 16]); // uuid
    old.extend_from_slice(&4.0f64.to_be_bytes()); // amount
    wire::write_varint(&mut old, 0); // operation

    let ClientboundGamePacket::UpdateAttributes(p) = translate_and_decode(766, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.values.len(), 1);
    assert_eq!(
        p.values[0].attribute,
        azalea_registry::builtin::Attribute::MaxHealth
    );
    assert_eq!(p.values[0].base, 20.0);
    let modifier = &p.values[0].modifiers[0];
    assert_eq!(modifier.amount, 4.0);
    assert_eq!(
        modifier.id.to_string(),
        format!("minecraft:{}", "ab".repeat(16))
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

    let ClientboundGamePacket::ProjectilePower(p) = translate_and_decode(766, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.id, MinecraftEntityId(9));
    assert_eq!(p.acceleration_power, 5.0);
}

/// Outbound `use_item` drops the rotation floats 1.21 appended.
#[test]
fn translate_use_item_766() {
    let mut frame = Vec::new();
    wire::write_varint(&mut frame, table_id(Direction::Serverbound, "use_item"));
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

    let ClientboundGamePacket::ContainerSetContent(p) = translate_and_decode(765, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.container_id, 1);
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    assert_eq!(data.kind, azalea_registry::builtin::ItemKind::Stone);
    assert_eq!(data.count, 3);
    assert_eq!(data.component_patch.iter().count(), 0);
    assert!(matches!(p.carried_item, azalea_inventory::ItemStack::Empty));
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

    let ClientboundGamePacket::SetEquipment(p) = translate_decode_and_remap(765, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.entity_id, MinecraftEntityId(9));
    assert_eq!(p.slots.slots.len(), 2);
    let azalea_inventory::ItemStack::Present(data) = &p.slots.slots[0].1 else {
        panic!("empty stack");
    };
    assert_eq!(data.count, 1);
    assert_eq!(data.kind, SHIFTED_ITEM_KIND);
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

    let ClientboundGamePacket::MerchantOffers(p) = translate_decode_and_remap(765, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.container_id, 1);
    let offer = &p.offers[0];
    assert_eq!(offer.base_cost_a.count, 3);
    assert_eq!(offer.base_cost_a.item, SHIFTED_ITEM_KIND);
    assert!(offer.cost_b.is_none());
    assert_eq!(offer.uses, 5);
    assert_eq!(offer.max_uses, 12);
    assert_eq!(p.villager_level, 2);
    assert!(p.can_restock);
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

    let ClientboundGamePacket::UpdateMobEffect(p) = translate_and_decode(765, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.entity_id, MinecraftEntityId(7));
    assert_eq!(p.data.amplifier, 2);
    assert_eq!(p.data.duration, 600);
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

    let ClientboundGamePacket::UpdateAttributes(p) = translate_and_decode(765, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.values.len(), 1);
    assert_eq!(
        p.values[0].attribute,
        azalea_registry::builtin::Attribute::MaxHealth
    );
    assert_eq!(p.values[0].base, 20.0);
    let modifier = &p.values[0].modifiers[0];
    assert_eq!(modifier.amount, 4.0);
    assert_eq!(
        modifier.id.to_string(),
        format!("minecraft:{}", "ab".repeat(16))
    );
}

/// 1.20.4 `level_particles` led with the particle type id; it moves to
/// just before the payload and `alwaysShow` is synthesized.
#[test]
fn translate_level_particles_765() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(765, Direction::Clientbound, "level_particles"),
    );
    wire::write_varint(&mut old, 30); // particle (26.2 id space)
    old.push(1); // override limiter
    for c in [1.0f64, 65.0, -2.0] {
        old.extend_from_slice(&c.to_be_bytes());
    }
    for d in [0.5f32, 0.5, 0.5, 0.1] {
        old.extend_from_slice(&d.to_be_bytes());
    }
    old.extend_from_slice(&7i32.to_be_bytes()); // count

    let ClientboundGamePacket::LevelParticles(p) = translate_and_decode(765, old) else {
        panic!("wrong packet");
    };
    assert!(p.override_limiter);
    assert!(!p.always_show);
    assert_eq!(p.count, 7);
}

/// The serializer-id walker at 765: 1.20.5 inserted `particles` (18),
/// `wolf_variant` (23) and `armadillo_state` (28), so 765's
/// `sniffer_state` sits at 25 (26.2's is 27).
#[test]
fn translate_entity_data_765() {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(765, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 9); // entity id
    old.extend_from_slice(&[17, 25, 2]); // index 17, sniffer_state, ordinal 2
    old.push(0xFF);

    let ClientboundGamePacket::SetEntityData(p) = translate_and_decode(765, old) else {
        panic!("wrong packet");
    };
    assert!(matches!(
        p.packed_items.0[0].value,
        azalea_entity::EntityDataValue::SnifferState(_)
    ));
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

    let ClientboundGamePacket::SetCursorItem(_) = translate_and_decode(765, build(-1, 0)) else {
        panic!("wrong packet for -1");
    };
    let ClientboundGamePacket::SetPlayerInventory(p) = translate_and_decode(765, build(-2, 7))
    else {
        panic!("wrong packet for -2");
    };
    assert_eq!(p.slot, 7);
    let ClientboundGamePacket::ContainerSetSlot(p) = translate_and_decode(765, build(3, 1)) else {
        panic!("wrong packet for plain");
    };
    assert_eq!(p.container_id, 3);
    assert_eq!(p.slot, 1);
    let azalea_inventory::ItemStack::Present(data) = &p.item_stack else {
        panic!("empty stack");
    };
    assert_eq!(data.count, 2);
}

/// 765's single-NBT `registry_data` splits into per-registry frames whose
/// entries reorder by their explicit ids (wire order is the id space); the
/// captured dimension-type order then keys the 765 spawn-info rewrites,
/// whose dimension type is a resource key string.
#[test]
fn translate_config_registry_data_765() {
    use azalea_buf::AzBuf;
    use azalea_protocol::packets::config::ClientboundConfigPacket;
    use azalea_registry::DataRegistry;
    use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

    let entry = |name: &str, id: i32| {
        let mut element = NbtCompound::new();
        element.insert("height", NbtTag::Int(384));
        let mut e = NbtCompound::new();
        e.insert("name", NbtTag::String(name.into()));
        e.insert("id", NbtTag::Int(id));
        e.insert("element", NbtTag::Compound(element));
        e
    };
    let mut registry = NbtCompound::new();
    registry.insert("type", NbtTag::String("minecraft:dimension_type".into()));
    registry.insert(
        "value",
        NbtTag::List(NbtList::from(vec![
            entry("minecraft:custom_end", 1),
            entry("minecraft:custom_overworld", 0),
        ])),
    );
    let mut root = NbtCompound::new();
    root.insert("minecraft:dimension_type", NbtTag::Compound(registry));

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        PacketTable::for_protocol(765)
            .unwrap()
            .id(
                Phase::Configuration,
                Direction::Clientbound,
                "registry_data",
            )
            .unwrap(),
    );
    NbtTag::Compound(root).azalea_write(&mut old).unwrap();

    let frames = translation_for(765).translate_config_frame(old.into_boxed_slice());
    assert_eq!(frames.len(), 1);
    let packet: ClientboundConfigPacket =
        azalea_protocol::read::deserialize_packet(&mut std::io::Cursor::new(&frames[0])).unwrap();
    let ClientboundConfigPacket::RegistryData(p) = packet else {
        panic!("wrong packet");
    };
    assert_eq!(p.registry_id.to_string(), "minecraft:dimension_type");
    let names: Vec<String> = p.entries.iter().map(|(n, _)| n.to_string()).collect();
    assert_eq!(
        names,
        ["minecraft:custom_overworld", "minecraft:custom_end"]
    );
    // The element alone, not the {name, id, element} wrapper 765 nests it in.
    let element = p.entries[0].1.as_ref().expect("entry NBT");
    assert_eq!(element.int("height"), Some(384));
    assert_eq!(element.string("name"), None);

    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(765, Direction::Clientbound, "respawn"));
    write_spawn_info_765(&mut old, "minecraft:custom_end", "minecraft:overworld");
    old.push(3); // keep-data flags

    let ClientboundGamePacket::Respawn(p) = translate_and_decode(765, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.common.dimension_type.protocol_id(), 1);
    assert_eq!(p.common.dimension.to_string(), "minecraft:overworld");
    assert_eq!(p.common.seed, 42);
    assert_eq!(p.data_to_keep, 3);
}

/// A 765 `CommonPlayerSpawnInfo`: dimension-type key, dimension name, then
/// the shared tail (seed 42, survival, no previous type, no death location).
fn write_spawn_info_765(old: &mut Vec<u8>, dimension_type: &str, dimension: &str) {
    for key in [dimension_type, dimension] {
        wire::write_varint(old, key.len() as u32);
        old.extend_from_slice(key.as_bytes());
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

    let ClientboundGamePacket::Login(p) = translate_and_decode(765, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.player_id, MinecraftEntityId(7));
    assert_eq!(p.levels, vec!["minecraft:the_nether".into()]);
    assert_eq!(p.max_players, 20);
    assert!(p.show_death_screen);
    assert_eq!(p.common.dimension.to_string(), "minecraft:the_nether");
    assert!(!p.enforces_secure_chat);
}

/// Config ids remap by name at 765 and 26.2-only packets suppress
/// (`select_known_packs` predates the known-packs handshake).
#[test]
fn translate_config_ids_765() {
    let config_id = |protocol: i32, dir, name| {
        PacketTable::for_protocol(protocol)
            .unwrap()
            .id(Phase::Configuration, dir, name)
    };
    let t = translation_for(765);
    assert!(t.translates_config());
    assert!(!translation_for(766).translates_config());

    // Inbound finish_configuration lands on its 26.2 id.
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        config_id(765, Direction::Clientbound, "finish_configuration").unwrap(),
    );
    let frames = t.translate_config_frame(old.into_boxed_slice());
    let mut expected = Vec::new();
    wire::write_varint(
        &mut expected,
        config_id(776, Direction::Clientbound, "finish_configuration").unwrap(),
    );
    assert_eq!(frames, [expected.into_boxed_slice()]);

    // Outbound finish_configuration remaps back; select_known_packs drops.
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        config_id(776, Direction::Serverbound, "finish_configuration").unwrap(),
    );
    let old = config_id(765, Direction::Serverbound, "finish_configuration").unwrap();
    assert_eq!(
        t.translate_outbound_config_frame(frame),
        Some(vec![old as u8])
    );
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        config_id(776, Direction::Serverbound, "select_known_packs").unwrap(),
    );
    wire::write_varint(&mut frame, 0);
    assert_eq!(t.translate_outbound_config_frame(frame), None);
}

/// 1.20.5/1.21-era `game_profile` carried a trailing strictErrorHandling
/// bool where 1.21.2 put the session UUID; it pops before the zero pad.
#[test]
fn translate_login_finished_766() {
    let login_id = |protocol, name| {
        PacketTable::for_protocol(protocol)
            .unwrap()
            .id(Phase::Login, Direction::Clientbound, name)
            .unwrap()
    };
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
    wire::write_varint(
        &mut old,
        PacketTable::for_protocol(765)
            .unwrap()
            .id(Phase::Login, Direction::Clientbound, "hello")
            .unwrap(),
    );
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
        table_id(Direction::Serverbound, "container_click"),
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
        table_id(Direction::Serverbound, "set_creative_mode_slot"),
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
    wire::write_varint(&mut frame, table_id(Direction::Serverbound, "chat_command"));
    frame.extend_from_slice(&[3, b's', b'a', b'y']);

    let frames = translation_for(765).translate_outbound_game_frame(frame);
    let mut expected = vec![old_id(765, Direction::Serverbound, "chat_command") as u8];
    expected.extend_from_slice(&[3, b's', b'a', b'y']);
    expected.extend_from_slice(&[0; 16]); // timestamp, salt
    expected.extend_from_slice(&[0, 0]); // no signatures, last-seen offset
    expected.extend_from_slice(&[0; 3]); // acknowledged bit set
    assert_eq!(frames, [expected]);
}

/// `update_attributes` names its attribute by registry id, and those shift
/// between versions (1.21.2 also dropped the category prefixes), so each is
/// remapped into the latest space. Unremapped, an old `max_health` decodes as
/// whatever attribute holds that id in 26.2.
#[test]
fn translate_update_attributes_old_versions() {
    use azalea_registry::builtin::Attribute;
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

        let ClientboundGamePacket::UpdateAttributes(p) = translate_and_decode(protocol, old) else {
            panic!("wrong packet for {protocol}");
        };
        assert_eq!(p.values[0].attribute, Attribute::MaxHealth, "{protocol}");
        assert_eq!(p.values[0].base, 20.0, "{protocol}");
        assert_eq!(p.values[0].modifiers[0].amount, 4.0, "{protocol}");
    }
}

#[test]
fn lp_vec3_roundtrip() {
    let cases = [
        DVec3::ZERO,
        DVec3::new(0.3, 1.62, -0.21),
        DVec3::new(-0.5, -0.001, 0.5),
        DVec3::new(2.75, -3.5, 1.0),
        DVec3::new(120.0, -64.25, 300.5),
    ];
    for v in cases {
        let mut buf = Vec::new();
        wire::write_lp_vec3(&mut buf, v);
        let decoded = decode_lp_vec3(&buf);
        // Quantization error is bounded by scale / 32766 per component.
        let tolerance = (v.abs().max_element().ceil() / 32766.0).max(1e-9) * 1.01;
        assert!(
            (decoded - v).abs().max_element() <= tolerance,
            "{v:?} decoded as {decoded:?} (tolerance {tolerance})"
        );
    }
}
