//! Cross-checks of pomme-protocol's vanilla-derived table and encoders
//! against azalea (kept here so pomme-protocol stays azalea-free). On a
//! disagreement the table generated from the decompiled reference is
//! authoritative — azalea's own tables can lag (its 26.2 `Particle` enum is
//! out of sync, see `handler::handle_raw_game_packet`) — so a failure means
//! "investigate which side is wrong", with in-game behavior as tiebreaker.

use azalea_buf::{AzBuf, AzBufVar};
use azalea_core::entity_id::MinecraftEntityId;
use azalea_core::sound::CustomSound;
use azalea_protocol::packets::ProtocolPacket;
use azalea_protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket};
use azalea_registry::Holder;
use azalea_registry::builtin::SoundEvent;
use azalea_registry::identifier::Identifier;
use glam::DVec3;
use pomme_protocol::packets::{Direction, PacketTable, Phase};
use pomme_protocol::wire;

fn table_id(phase: Phase, dir: Direction, name: &str) -> u32 {
    PacketTable::native().id(phase, dir, name).unwrap()
}

#[test]
fn native_matches_azalea() {
    assert_eq!(
        azalea_protocol::packets::PROTOCOL_VERSION,
        pomme_protocol::version::NATIVE.protocol
    );
}

/// pomme's serverbound chat encoders, read back by azalea's decoders.
#[test]
fn chat_encoders_round_trip_through_azalea() {
    use crate::net::chat;
    use crate::net::chat_security::LastSeenUpdate;

    let decode = |frame: Vec<u8>| {
        azalea_protocol::read::deserialize_packet::<ServerboundGamePacket>(
            &mut std::io::Cursor::new(&frame[..]),
        )
        .unwrap()
    };
    let update = LastSeenUpdate {
        offset: 3,
        acknowledged: [0b101, 0, 0x08],
        checksum: 0x42,
        last_seen: Vec::new(),
    };
    let check_update =
        |last_seen: &azalea_protocol::packets::game::s_chat::LastSeenMessagesUpdate| {
            assert_eq!(last_seen.offset, 3);
            assert!(last_seen.acknowledged.index(0) && last_seen.acknowledged.index(2));
            assert!(last_seen.acknowledged.index(19) && !last_seen.acknowledged.index(1));
            assert_eq!(last_seen.checksum, 0x42);
        };

    let ServerboundGamePacket::Chat(message) = decode(chat::encode_outbound_message(
        "hi",
        5,
        -6,
        Some(&[9; 256]),
        &update,
    )) else {
        panic!("expected chat");
    };
    assert_eq!(
        (
            message.message.as_str(),
            message.timestamp,
            message.salt as i64
        ),
        ("hi", 5, -6)
    );
    assert_eq!(message.signature.map(|s| s.bytes), Some([9; 256]));
    check_update(&message.last_seen_messages);

    let ServerboundGamePacket::ChatCommandSigned(command) =
        decode(chat::encode_outbound_signed_command(
            "msg a hi",
            7,
            8,
            &[("message".into(), [4; 256])],
            &update,
        ))
    else {
        panic!("expected chat_command_signed");
    };
    assert_eq!(
        (command.command.as_str(), command.timestamp, command.salt),
        ("msg a hi", 7, 8)
    );
    assert_eq!(command.argument_signatures[0].name, "message");
    assert_eq!(command.argument_signatures[0].signature.bytes, [4; 256]);
    check_update(&command.last_seen_messages);

    let ServerboundGamePacket::ChatSessionUpdate(session) = decode(
        chat::encode_chat_session_update(uuid::Uuid::from_u128(7), 1234, &[1, 2], &[3]),
    ) else {
        panic!("expected chat_session_update");
    };
    assert_eq!(session.chat_session.session_id, uuid::Uuid::from_u128(7));
    assert_eq!(session.chat_session.profile_public_key.expires_at, 1234);
    assert_eq!(session.chat_session.profile_public_key.key, [1, 2]);
    assert_eq!(session.chat_session.profile_public_key.key_signature, [3]);

    let ServerboundGamePacket::ChatAck(ack) = decode(chat::encode_chat_ack(300)) else {
        panic!("expected chat_ack");
    };
    assert_eq!(ack.messages, 300);
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
    assert_eq!(
        interact.id(),
        table_id(Phase::Game, Direction::Serverbound, "interact")
    );

    let attack = ServerboundGamePacket::Attack(s_attack::ServerboundAttack {
        entity_id: MinecraftEntityId(0),
    });
    assert_eq!(
        attack.id(),
        table_id(Phase::Game, Direction::Serverbound, "attack")
    );

    // azalea's block variant still carries the pre-1.21.4 `slot` field, which
    // is why `wire.rs` hand-encodes the position and flag; only the id is
    // shared here.
    let pick_block = ServerboundGamePacket::PickItemFromBlock(
        azalea_protocol::packets::game::s_pick_item_from_block::ServerboundPickItemFromBlock {
            slot: 0,
        },
    );
    assert_eq!(
        pick_block.id(),
        table_id(Phase::Game, Direction::Serverbound, "pick_item_from_block")
    );
    let pick_entity = ServerboundGamePacket::PickItemFromEntity(
        azalea_protocol::packets::game::s_pick_item_from_entity::ServerboundPickItemFromEntity {
            id: MinecraftEntityId(0),
            include_data: false,
        },
    );
    assert_eq!(
        pick_entity.id(),
        table_id(Phase::Game, Direction::Serverbound, "pick_item_from_entity")
    );

    let teleport = ServerboundGamePacket::TeleportToEntity(
        azalea_protocol::packets::game::s_teleport_to_entity::ServerboundTeleportToEntity {
            uuid: uuid::Uuid::nil(),
        },
    );
    assert_eq!(
        teleport.id(),
        table_id(Phase::Game, Direction::Serverbound, "teleport_to_entity")
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
        table_id(Phase::Game, Direction::Clientbound, "level_particles")
    );

    let boss_event = ClientboundGamePacket::BossEvent(
        azalea_protocol::packets::game::c_boss_event::ClientboundBossEvent {
            id: uuid::Uuid::nil(),
            operation: azalea_protocol::packets::game::c_boss_event::Operation::Remove,
        },
    );
    assert_eq!(
        boss_event.id(),
        table_id(Phase::Game, Direction::Clientbound, "boss_event")
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
        table_id(Phase::Game, Direction::Clientbound, "update_advancements")
    );

    let recipes = ClientboundGamePacket::RecipeBookAdd(
        azalea_protocol::packets::game::c_recipe_book_add::ClientboundRecipeBookAdd {
            entries: Vec::new(),
            replace: false,
        },
    );
    assert_eq!(
        recipes.id(),
        table_id(Phase::Game, Direction::Clientbound, "recipe_book_add")
    );

    dialog_packet_ids_match_azalea();

    // `net::chat` decodes and encodes these itself; only the ids are shared.
    use azalea_protocol::packets::game::{
        c_set_action_bar_text, c_system_chat, s_chat, s_chat_command,
    };

    let system_chat = ClientboundGamePacket::SystemChat(c_system_chat::ClientboundSystemChat {
        content: azalea_chat::FormattedText::default(),
        overlay: false,
    });
    assert_eq!(
        system_chat.id(),
        table_id(Phase::Game, Direction::Clientbound, "system_chat")
    );
    let action_bar = ClientboundGamePacket::SetActionBarText(
        c_set_action_bar_text::ClientboundSetActionBarText {
            text: azalea_chat::FormattedText::default(),
        },
    );
    assert_eq!(
        action_bar.id(),
        table_id(Phase::Game, Direction::Clientbound, "set_action_bar_text")
    );
    let chat = ServerboundGamePacket::Chat(s_chat::ServerboundChat {
        message: String::new(),
        timestamp: 0,
        salt: 0,
        signature: None,
        last_seen_messages: Default::default(),
    });
    assert_eq!(
        chat.id(),
        table_id(Phase::Game, Direction::Serverbound, "chat")
    );
    let chat_command = ServerboundGamePacket::ChatCommand(s_chat_command::ServerboundChatCommand {
        command: String::new(),
    });
    assert_eq!(
        chat_command.id(),
        table_id(Phase::Game, Direction::Serverbound, "chat_command")
    );

    for (name, body) in [
        ("delete_chat", vec![1]),
        ("command_suggestions", vec![42, 1, 0, 0]),
    ] {
        let mut frame = Vec::new();
        wire::write_varint(
            &mut frame,
            table_id(Phase::Game, Direction::Clientbound, name),
        );
        frame.extend(body);
        let packet = azalea_protocol::read::deserialize_packet::<ClientboundGamePacket>(
            &mut std::io::Cursor::new(&frame[..]),
        )
        .unwrap();
        assert!(
            matches!(
                (name, packet),
                ("delete_chat", ClientboundGamePacket::DeleteChat(_))
                    | (
                        "command_suggestions",
                        ClientboundGamePacket::CommandSuggestions(_)
                    )
            ),
            "{name}"
        );
    }

    use azalea_protocol::packets::game::{
        c_clear_titles, c_set_subtitle_text, c_set_title_text, c_set_titles_animation,
    };

    let title = ClientboundGamePacket::SetTitleText(c_set_title_text::ClientboundSetTitleText {
        text: azalea_chat::FormattedText::default(),
    });
    assert_eq!(
        title.id(),
        table_id(Phase::Game, Direction::Clientbound, "set_title_text")
    );

    let subtitle =
        ClientboundGamePacket::SetSubtitleText(c_set_subtitle_text::ClientboundSetSubtitleText {
            text: azalea_chat::FormattedText::default(),
        });
    assert_eq!(
        subtitle.id(),
        table_id(Phase::Game, Direction::Clientbound, "set_subtitle_text")
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
        table_id(Phase::Game, Direction::Clientbound, "set_titles_animation")
    );

    let clear = ClientboundGamePacket::ClearTitles(c_clear_titles::ClientboundClearTitles {
        reset_times: true,
    });
    assert_eq!(
        clear.id(),
        table_id(Phase::Game, Direction::Clientbound, "clear_titles")
    );

    use azalea_protocol::packets::game::c_hurt_animation;

    let hurt_animation =
        ClientboundGamePacket::HurtAnimation(c_hurt_animation::ClientboundHurtAnimation {
            id: MinecraftEntityId(0),
            yaw: 90.0,
        });
    assert_eq!(
        hurt_animation.id(),
        table_id(Phase::Game, Direction::Clientbound, "hurt_animation")
    );

    use azalea_protocol::packets::game::{c_sound, c_sound_entity, c_stop_sound};

    let sound = ClientboundGamePacket::Sound(c_sound::ClientboundSound {
        sound: sound_holder(),
        source: c_sound::SoundSource::Master,
        x: 0,
        y: 0,
        z: 0,
        volume: 1.0,
        pitch: 1.0,
        seed: 0,
    });
    assert_eq!(
        sound.id(),
        table_id(Phase::Game, Direction::Clientbound, "sound")
    );

    let sound_entity = ClientboundGamePacket::SoundEntity(c_sound_entity::ClientboundSoundEntity {
        sound: sound_holder(),
        source: c_sound::SoundSource::Master,
        id: MinecraftEntityId(0),
        volume: 1.0,
        pitch: 1.0,
        seed: 0,
    });
    assert_eq!(
        sound_entity.id(),
        table_id(Phase::Game, Direction::Clientbound, "sound_entity")
    );

    let stop_sound = ClientboundGamePacket::StopSound(c_stop_sound::ClientboundStopSound {
        source: None,
        name: None,
    });
    assert_eq!(
        stop_sound.id(),
        table_id(Phase::Game, Direction::Clientbound, "stop_sound")
    );
}

fn sound_holder() -> Holder<SoundEvent, CustomSound> {
    Holder::Direct(CustomSound {
        sound_id: Identifier::new("minecraft:test.sound"),
        range: None,
    })
}

/// Pins the azalea bug the raw sound path in `handler::handle_raw_game_packet`
/// works around: azalea's `SoundSource` stops at `Voice = 9`, and the `AzBuf`
/// derive decodes an unknown discriminant as the first variant rather than
/// failing, so vanilla's `UI = 10` silently arrives as `Master`. When azalea
/// gains the variant this fails, and the raw path can be deleted.
#[test]
fn azalea_sound_source_still_misdecodes_ui() {
    use azalea_buf::AzBuf;
    use azalea_protocol::packets::game::c_sound::SoundSource;

    let mut ui = std::io::Cursor::new(&[10u8][..]);
    assert_eq!(
        SoundSource::azalea_read(&mut ui).unwrap(),
        SoundSource::Master
    );

    let mut voice = std::io::Cursor::new(&[9u8][..]);
    assert_eq!(
        SoundSource::azalea_read(&mut voice).unwrap(),
        SoundSource::Voice
    );
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
    use pomme_protocol::version::{NATIVE, VERSIONS};
    for v in VERSIONS {
        assert_eq!(
            crate::net::translate::Translation::for_protocol(v.protocol).is_some(),
            crate::net::translate::joinable(v.protocol) && v.protocol != NATIVE.protocol,
            "{}",
            v.name
        );
    }
    assert!(PacketTable::for_protocol(762).is_none());
    assert!(crate::net::translate::Translation::for_protocol(762).is_none());
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
    expected.push(5);
    expected.push(3); // options
    assert_eq!(&translated[..], &expected[..]);
}

/// RESET (ChatFormatting ordinal 21) passes through as-is — azalea's enum
/// has all 22 formatting variants; the method-0 player list is copied
/// verbatim.
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

    let t = RegistryTable::native();
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

/// 26.1's packet ids match 26.2, but its data-component registry shifts at
/// `sulfur_cube_content`. Keep latest semantic kinds through typed encoding,
/// then remap removal ids in the raw creative frame.
#[test]
fn creative_components_26_1_remap_shifted_ids() {
    use azalea_inventory::{DataComponentPatch, ItemStack, ItemStackData};
    use azalea_protocol::packets::game::ServerboundGamePacket;
    use azalea_protocol::packets::game::s_set_creative_mode_slot::ServerboundSetCreativeModeSlot;
    use azalea_registry::builtin::{DataComponentKind, ItemKind};
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let mut patch = DataComponentPatch::default();
    unsafe {
        patch.unchecked_insert_component(DataComponentKind::MaxStackSize, None);
        patch.unchecked_insert_component(DataComponentKind::Lock, None);
    }
    let mut packet = ServerboundGamePacket::SetCreativeModeSlot(ServerboundSetCreativeModeSlot {
        slot_num: 36,
        item_stack: ItemStack::Present(ItemStackData {
            kind: ItemKind::Stone,
            count: 1,
            component_patch: patch,
        }),
    });

    let translation = translation_for(775);
    translation.remap_outbound(&mut packet);
    let ServerboundGamePacket::SetCreativeModeSlot(p) = &packet else {
        unreachable!()
    };
    let ItemStack::Present(data) = &p.item_stack else {
        panic!("stack cleared");
    };
    assert_eq!(data.component_patch.iter().count(), 2);

    let latest = azalea_protocol::write::serialize_packet(&packet).unwrap();
    let frames = translation.translate_outbound_game_frame(latest.to_vec());
    assert_eq!(frames.len(), 1);
    let frame = &frames[0];
    let mut pos = 0;
    assert_eq!(
        wire::read_varint(frame, &mut pos),
        Some(old_id(
            775,
            Direction::Serverbound,
            "set_creative_mode_slot"
        ))
    );
    pos += 2; // slot
    assert_eq!(wire::read_varint(frame, &mut pos), Some(1)); // count
    assert!(wire::read_varint(frame, &mut pos).is_some()); // item id
    assert_eq!(wire::read_varint(frame, &mut pos), Some(0)); // additions
    assert_eq!(wire::read_varint(frame, &mut pos), Some(2)); // removals

    let table = RegistryTable::for_protocol(775).unwrap();
    let mut removed = [
        wire::read_varint(frame, &mut pos).unwrap(),
        wire::read_varint(frame, &mut pos).unwrap(),
    ];
    removed.sort_unstable();
    let mut expected = [
        registry_id(table, ClientRegistry::DataComponentType, "max_stack_size"),
        registry_id(table, ClientRegistry::DataComponentType, "lock"),
    ];
    expected.sort_unstable();
    assert_eq!(removed, expected);
    assert_eq!(pos, frame.len());
}

#[test]
fn creative_nested_stacks_775_use_template_and_stream_patch_framing() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let protocol = 775;
    let native = RegistryTable::native();
    let target = RegistryTable::for_protocol(protocol).unwrap();
    let native_packet = PacketTable::native()
        .id(
            Phase::Game,
            Direction::Serverbound,
            "set_creative_mode_slot",
        )
        .unwrap();
    let native_dripstone = registry_id(native, ClientRegistry::Item, "dripstone_block");
    let target_dripstone = registry_id(target, ClientRegistry::Item, "dripstone_block");
    let native_lock = registry_id(native, ClientRegistry::DataComponentType, "lock");
    let target_lock = registry_id(target, ClientRegistry::DataComponentType, "lock");
    assert_ne!(native_dripstone, target_dripstone);
    assert_ne!(native_lock, target_lock);

    for component in [
        "use_remainder",
        "charged_projectiles",
        "bundle_contents",
        "container",
    ] {
        let mut frame = Vec::new();
        wire::write_varint(&mut frame, native_packet);
        frame.extend_from_slice(&36u16.to_be_bytes());
        wire::write_varint(&mut frame, 1); // outer ordinary ItemStack count
        wire::write_varint(
            &mut frame,
            registry_id(native, ClientRegistry::Item, "stone"),
        );
        wire::write_varint(&mut frame, 1); // one outer addition
        wire::write_varint(&mut frame, 0); // no outer removals
        wire::write_varint(
            &mut frame,
            registry_id(native, ClientRegistry::DataComponentType, component),
        );

        // Pinned Azalea's native component payload is still ordinary ItemStack.
        if component != "use_remainder" {
            wire::write_varint(&mut frame, 1); // one nested stack
        }
        wire::write_varint(&mut frame, 1); // nested ordinary stack count
        wire::write_varint(&mut frame, native_dripstone);
        wire::write_varint(&mut frame, 1); // nested additions
        wire::write_varint(&mut frame, 0); // nested removals
        wire::write_varint(&mut frame, native_lock);
        write_utf(&mut frame, "pomme:nested_key");

        let translated = translation_for(protocol).translate_outbound_game_frame(frame);
        assert_eq!(translated.len(), 1, "{component}");
        let out = &translated[0];
        let mut pos = 0;
        assert_eq!(
            wire::read_varint(out, &mut pos),
            Some(old_id(
                protocol,
                Direction::Serverbound,
                "set_creative_mode_slot"
            )),
            "{component}"
        );
        pos += 2; // slot
        assert_eq!(wire::read_varint(out, &mut pos), Some(1));
        assert_eq!(
            wire::read_varint(out, &mut pos),
            Some(registry_id(target, ClientRegistry::Item, "stone"))
        );
        assert_eq!(wire::read_varint(out, &mut pos), Some(1));
        assert_eq!(wire::read_varint(out, &mut pos), Some(0));
        assert_eq!(
            wire::read_varint(out, &mut pos),
            Some(registry_id(
                target,
                ClientRegistry::DataComponentType,
                component
            ))
        );
        let payload_len = wire::read_varint(out, &mut pos).unwrap() as usize;
        let payload_end = pos + payload_len;

        if component != "use_remainder" {
            assert_eq!(wire::read_varint(out, &mut pos), Some(1)); // list size
        }
        if component == "container" {
            assert_eq!(out[pos], 1); // Optional<ItemStackTemplate>::Some
            pos += 1;
        }
        // Mojang 26.1 ItemStackTemplate order is item, count, patch.
        assert_eq!(wire::read_varint(out, &mut pos), Some(target_dripstone));
        assert_eq!(wire::read_varint(out, &mut pos), Some(1));
        assert_eq!(wire::read_varint(out, &mut pos), Some(1)); // nested additions
        assert_eq!(wire::read_varint(out, &mut pos), Some(0)); // nested removals
        assert_eq!(wire::read_varint(out, &mut pos), Some(target_lock));
        // Nested DataComponentPatch.STREAM_CODEC has no per-value length prefix.
        let len = wire::read_varint(out, &mut pos).unwrap() as usize;
        assert_eq!(&out[pos..pos + len], b"pomme:nested_key");
        pos += len;
        assert_eq!(pos, payload_end, "{component}");
        assert_eq!(pos, out.len(), "{component}");
    }
}

#[test]
fn creative_container_775_empty_slot_writes_optional_none() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let protocol = 775;
    let native = RegistryTable::native();
    let target = RegistryTable::for_protocol(protocol).unwrap();
    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        PacketTable::native()
            .id(
                Phase::Game,
                Direction::Serverbound,
                "set_creative_mode_slot",
            )
            .unwrap(),
    );
    frame.extend_from_slice(&36u16.to_be_bytes());
    wire::write_varint(&mut frame, 1); // outer stack count
    wire::write_varint(
        &mut frame,
        registry_id(native, ClientRegistry::Item, "stone"),
    );
    wire::write_varint(&mut frame, 1); // outer additions
    wire::write_varint(&mut frame, 0); // outer removals
    wire::write_varint(
        &mut frame,
        registry_id(native, ClientRegistry::DataComponentType, "container"),
    );
    wire::write_varint(&mut frame, 1); // one container slot
    wire::write_varint(&mut frame, 0); // pinned Azalea ItemStack::Empty

    let translated = translation_for(protocol).translate_outbound_game_frame(frame);
    assert_eq!(translated.len(), 1);
    let out = &translated[0];
    let mut pos = 0;
    assert_eq!(
        wire::read_varint(out, &mut pos),
        Some(old_id(
            protocol,
            Direction::Serverbound,
            "set_creative_mode_slot"
        ))
    );
    pos += 2;
    assert_eq!(wire::read_varint(out, &mut pos), Some(1));
    assert!(wire::read_varint(out, &mut pos).is_some());
    assert_eq!(wire::read_varint(out, &mut pos), Some(1));
    assert_eq!(wire::read_varint(out, &mut pos), Some(0));
    assert_eq!(
        wire::read_varint(out, &mut pos),
        Some(registry_id(
            target,
            ClientRegistry::DataComponentType,
            "container"
        ))
    );
    let payload_len = wire::read_varint(out, &mut pos).unwrap() as usize;
    let payload_end = pos + payload_len;
    assert_eq!(wire::read_varint(out, &mut pos), Some(1)); // one slot
    assert_eq!(out[pos], 0); // Optional<ItemStackTemplate>::None
    pos += 1;
    assert_eq!(pos, payload_end);
    assert_eq!(pos, out.len());
}

#[test]
fn creative_tooltip_display_775_remaps_hidden_component_ids() {
    use azalea_inventory::components::{DataComponentUnion, TooltipDisplay};
    use azalea_inventory::{DataComponentPatch, ItemStack, ItemStackData};
    use azalea_protocol::packets::game::ServerboundGamePacket;
    use azalea_protocol::packets::game::s_set_creative_mode_slot::ServerboundSetCreativeModeSlot;
    use azalea_registry::builtin::{DataComponentKind, ItemKind};
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let mut patch = DataComponentPatch::default();
    unsafe {
        patch.unchecked_insert_component(
            DataComponentKind::TooltipDisplay,
            Some(DataComponentUnion::from(TooltipDisplay {
                hide_tooltip: false,
                hidden_components: vec![DataComponentKind::Lock],
            })),
        );
    }
    let mut packet = ServerboundGamePacket::SetCreativeModeSlot(ServerboundSetCreativeModeSlot {
        slot_num: 36,
        item_stack: ItemStack::Present(ItemStackData {
            kind: ItemKind::Stone,
            count: 1,
            component_patch: patch,
        }),
    });

    let protocol = 775;
    let translation = translation_for(protocol);
    translation.remap_outbound(&mut packet);
    let native = azalea_protocol::write::serialize_packet(&packet).unwrap();
    let frames = translation.translate_outbound_game_frame(native.to_vec());
    assert_eq!(frames.len(), 1);
    let frame = &frames[0];
    let target = RegistryTable::for_protocol(protocol).unwrap();
    let mut pos = 0;
    assert_eq!(
        wire::read_varint(frame, &mut pos),
        Some(old_id(
            protocol,
            Direction::Serverbound,
            "set_creative_mode_slot"
        ))
    );
    pos += 2;
    assert_eq!(wire::read_varint(frame, &mut pos), Some(1));
    assert!(wire::read_varint(frame, &mut pos).is_some());
    assert_eq!(wire::read_varint(frame, &mut pos), Some(1));
    assert_eq!(wire::read_varint(frame, &mut pos), Some(0));
    assert_eq!(
        wire::read_varint(frame, &mut pos),
        Some(registry_id(
            target,
            ClientRegistry::DataComponentType,
            "tooltip_display"
        ))
    );
    let payload_len = wire::read_varint(frame, &mut pos).unwrap() as usize;
    let payload_end = pos + payload_len;
    assert_eq!(frame[pos], 0); // hideTooltip=false
    pos += 1;
    assert_eq!(wire::read_varint(frame, &mut pos), Some(1));
    assert_eq!(
        wire::read_varint(frame, &mut pos),
        Some(registry_id(
            target,
            ClientRegistry::DataComponentType,
            "lock"
        ))
    );
    assert_eq!(pos, payload_end);
    assert_eq!(pos, frame.len());
}

#[test]
fn creative_tool_774_remaps_direct_block_ids() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let protocol = 774;
    let latest = RegistryTable::native();
    let target = RegistryTable::for_protocol(protocol).unwrap();
    let (block_name, latest_block, target_block) = (0..10_000u32)
        .find_map(|latest_id| {
            let name = crate::world::block::block_registry_name(776, latest_id)?;
            let target_id = crate::world::block::block_registry_id(protocol, name)?;
            (target_id != latest_id).then_some((name, latest_id, target_id))
        })
        .expect("a block registry shift between 1.21.11 and 26.2");

    let mut frame = Vec::new();
    wire::write_varint(
        &mut frame,
        PacketTable::native()
            .id(
                Phase::Game,
                Direction::Serverbound,
                "set_creative_mode_slot",
            )
            .unwrap(),
    );
    frame.extend_from_slice(&36u16.to_be_bytes());
    wire::write_varint(&mut frame, 1); // stack count
    wire::write_varint(
        &mut frame,
        registry_id(target, ClientRegistry::Item, "iron_pickaxe"),
    );
    wire::write_varint(&mut frame, 1); // one addition
    wire::write_varint(&mut frame, 0); // no removals
    wire::write_varint(
        &mut frame,
        registry_id(latest, ClientRegistry::DataComponentType, "tool"),
    );
    wire::write_varint(&mut frame, 1); // one Tool rule
    wire::write_varint(&mut frame, 2); // direct HolderSet with one block
    wire::write_varint(&mut frame, latest_block);
    frame.push(1); // speed present
    frame.extend_from_slice(&9.0f32.to_be_bytes());
    frame.push(1); // correctForDrops present
    frame.push(1);
    frame.extend_from_slice(&1.0f32.to_be_bytes()); // defaultMiningSpeed
    wire::write_varint(&mut frame, 1); // damagePerBlock
    frame.push(0); // canDestroyBlocksInCreative

    let translated = translation_for(protocol).translate_outbound_game_frame(frame);
    assert_eq!(translated.len(), 1);
    let out = &translated[0];
    let mut pos = 0;
    assert_eq!(
        wire::read_varint(out, &mut pos),
        Some(old_id(
            protocol,
            Direction::Serverbound,
            "set_creative_mode_slot"
        ))
    );
    pos += 2;
    assert_eq!(wire::read_varint(out, &mut pos), Some(1));
    assert_eq!(
        wire::read_varint(out, &mut pos),
        Some(registry_id(target, ClientRegistry::Item, "iron_pickaxe"))
    );
    assert_eq!(wire::read_varint(out, &mut pos), Some(1));
    assert_eq!(wire::read_varint(out, &mut pos), Some(0));
    assert_eq!(
        wire::read_varint(out, &mut pos),
        Some(registry_id(
            target,
            ClientRegistry::DataComponentType,
            "tool"
        ))
    );
    let payload_len = wire::read_varint(out, &mut pos).unwrap() as usize;
    let payload_end = pos + payload_len;
    assert_eq!(wire::read_varint(out, &mut pos), Some(1));
    assert_eq!(wire::read_varint(out, &mut pos), Some(2));
    assert_eq!(wire::read_varint(out, &mut pos), Some(target_block));
    assert_ne!(latest_block, target_block, "{block_name}");
    pos = payload_end;
    assert_eq!(pos, out.len());
}

#[test]
fn creative_attribute_modifiers_771_remap_embedded_attribute_id() {
    use azalea_core::attribute_modifier_operation::AttributeModifierOperation;
    use azalea_inventory::components::{
        AttributeModifier, AttributeModifierDisplay, AttributeModifiers, AttributeModifiersEntry,
        DataComponentUnion, EquipmentSlotGroup,
    };
    use azalea_inventory::{DataComponentPatch, ItemStack, ItemStackData};
    use azalea_protocol::packets::game::ServerboundGamePacket;
    use azalea_protocol::packets::game::s_set_creative_mode_slot::ServerboundSetCreativeModeSlot;
    use azalea_registry::builtin::{Attribute, DataComponentKind, ItemKind};
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let mut patch = DataComponentPatch::default();
    unsafe {
        patch.unchecked_insert_component(
            DataComponentKind::AttributeModifiers,
            Some(DataComponentUnion::from(AttributeModifiers {
                modifiers: vec![AttributeModifiersEntry {
                    kind: Attribute::AttackSpeed,
                    modifier: AttributeModifier {
                        id: Identifier::new("minecraft:test_speed"),
                        amount: 2.0,
                        operation: AttributeModifierOperation::AddValue,
                    },
                    slot: EquipmentSlotGroup::Any,
                    display: AttributeModifierDisplay::Default,
                }],
            })),
        );
    }
    let mut packet = ServerboundGamePacket::SetCreativeModeSlot(ServerboundSetCreativeModeSlot {
        slot_num: 36,
        item_stack: ItemStack::Present(ItemStackData {
            kind: ItemKind::Stone,
            count: 1,
            component_patch: patch,
        }),
    });

    let protocol = 771;
    let translation = translation_for(protocol);
    translation.remap_outbound(&mut packet);
    let latest = azalea_protocol::write::serialize_packet(&packet).unwrap();
    let frames = translation.translate_outbound_game_frame(latest.to_vec());
    assert_eq!(frames.len(), 1);

    let frame = &frames[0];
    let mut pos = 0;
    assert_eq!(
        wire::read_varint(frame, &mut pos),
        Some(old_id(
            protocol,
            Direction::Serverbound,
            "set_creative_mode_slot"
        ))
    );
    pos += 2; // slot
    assert_eq!(wire::read_varint(frame, &mut pos), Some(1)); // stack count
    assert!(wire::read_varint(frame, &mut pos).is_some()); // item id
    assert_eq!(wire::read_varint(frame, &mut pos), Some(1)); // additions
    assert_eq!(wire::read_varint(frame, &mut pos), Some(0)); // removals

    let table = RegistryTable::for_protocol(protocol).unwrap();
    assert_eq!(
        wire::read_varint(frame, &mut pos),
        Some(registry_id(
            table,
            ClientRegistry::DataComponentType,
            "attribute_modifiers"
        ))
    );
    let payload_len = wire::read_varint(frame, &mut pos).unwrap() as usize;
    let payload_end = pos + payload_len;
    assert_eq!(wire::read_varint(frame, &mut pos), Some(1)); // one modifier
    assert_eq!(
        wire::read_varint(frame, &mut pos),
        Some(registry_id(
            table,
            ClientRegistry::Attribute,
            "attack_speed"
        ))
    );
    pos = payload_end;
    assert_eq!(pos, frame.len());
}

#[test]
fn creative_slot_766_scrubs_private_mining_metadata() {
    use std::collections::HashMap;

    use azalea_inventory::components::{CustomData, DataComponentUnion, Enchantments};
    use azalea_inventory::{DataComponentPatch, ItemStack, ItemStackData};
    use azalea_protocol::packets::game::ServerboundGamePacket;
    use azalea_protocol::packets::game::s_set_creative_mode_slot::ServerboundSetCreativeModeSlot;
    use azalea_registry::DataRegistry;
    use azalea_registry::builtin::{DataComponentKind, ItemKind};
    use simdnbt::owned::{Nbt, NbtCompound};

    let mut compound = NbtCompound::new();
    compound.insert("pomme:legacy_efficiency", 4i32);
    compound.insert("pomme:legacy_aqua_affinity", 1i8);
    compound.insert("server:real_value", 42i32);
    let mut patch = DataComponentPatch::default();
    unsafe {
        patch.unchecked_insert_component(
            DataComponentKind::CustomData,
            Some(DataComponentUnion::from(CustomData {
                nbt: Nbt::new("".into(), compound),
            })),
        );
        patch.unchecked_insert_component(
            DataComponentKind::Enchantments,
            Some(DataComponentUnion::from(Enchantments {
                levels: HashMap::from([(azalea_registry::data::Enchantment::new_raw(20), 3)]),
            })),
        );
    }
    let mut packet = ServerboundGamePacket::SetCreativeModeSlot(ServerboundSetCreativeModeSlot {
        slot_num: 36,
        item_stack: ItemStack::Present(ItemStackData {
            kind: ItemKind::Stone,
            count: 1,
            component_patch: patch,
        }),
    });

    let translation = translation_for(766);
    translation.remap_outbound(&mut packet);
    let ServerboundGamePacket::SetCreativeModeSlot(p) = &packet else {
        unreachable!()
    };
    let ItemStack::Present(data) = &p.item_stack else {
        panic!("stack cleared");
    };
    let custom = data
        .component_patch
        .get::<CustomData>()
        .expect("server custom data should survive");
    assert_eq!(custom.nbt.int("server:real_value"), Some(42));
    assert_eq!(custom.nbt.int("pomme:legacy_efficiency"), None);
    assert_eq!(custom.nbt.byte("pomme:legacy_aqua_affinity"), None);
    assert!(data.component_patch.get::<Enchantments>().is_some());

    let latest = azalea_protocol::write::serialize_packet(&packet).unwrap();
    let frames = translation.translate_outbound_game_frame(latest.to_vec());
    assert_eq!(frames.len(), 1);
    let frame = &frames[0];
    let mut pos = 0;
    assert_eq!(
        wire::read_varint(frame, &mut pos),
        Some(old_id(
            766,
            Direction::Serverbound,
            "set_creative_mode_slot"
        ))
    );
    assert_eq!(&frame[pos..pos + 2], &36i16.to_be_bytes());
    pos += 2;
    assert_eq!(wire::read_varint(frame, &mut pos), Some(1)); // count
    assert!(wire::read_varint(frame, &mut pos).is_some()); // old item id
    let added = wire::read_varint(frame, &mut pos).expect("added component count");
    assert_eq!(wire::read_varint(frame, &mut pos), Some(0)); // removals

    let mut saw_custom = false;
    let mut saw_enchantments = false;
    let mut cursor = std::io::Cursor::new(&frame[pos..]);
    for _ in 0..added {
        let component = u32::azalea_read_var(&mut cursor).unwrap();
        match component {
            0 => {
                let nbt = simdnbt::owned::read_unnamed(&mut cursor).unwrap();
                let simdnbt::owned::Nbt::Some(root) = nbt else {
                    panic!("missing custom_data compound");
                };
                let custom = root.as_compound();
                assert_eq!(custom.int("server:real_value"), Some(42));
                assert_eq!(custom.int("pomme:legacy_efficiency"), None);
                assert_eq!(custom.byte("pomme:legacy_aqua_affinity"), None);
                saw_custom = true;
            }
            9 => {
                assert_eq!(u32::azalea_read_var(&mut cursor).unwrap(), 1);
                assert_eq!(u32::azalea_read_var(&mut cursor).unwrap(), 20);
                assert_eq!(i32::azalea_read_var(&mut cursor).unwrap(), 3);
                assert_eq!(u8::azalea_read(&mut cursor).unwrap(), 1); // showInTooltip
                saw_enchantments = true;
            }
            other => panic!("unexpected protocol-766 component {other}"),
        }
    }
    assert!(saw_custom && saw_enchantments);
}

#[test]
fn outbound_translation_gating() {
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
/// native registry, varint payload) keeps the verbatim bytes decodable.
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
    for protocol in [774, 773, 772, 771, 770, 769, 768, 767, 766, 765, 764, 763] {
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

/// 767 `cooldown` item ids remap into the native registry space (azalea
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

    let ClientboundGamePacket::UpdateAttributes(p) = translate_decode_and_remap(766, old) else {
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

/// 1.20.6's enchantments component used the old static enchantment registry.
/// The translator extracts mining-relevant levels into latest custom_data so
/// the legacy mining formula never depends on those old numeric ids.
#[test]
fn translate_enchanted_stack_766() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let table = RegistryTable::for_protocol(766).expect("1.20.6 registry table");
    let iron_pickaxe = registry_id(table, ClientRegistry::Item, "iron_pickaxe");

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(766, Direction::Clientbound, "container_set_content"),
    );
    old.push(1); // container id
    wire::write_varint(&mut old, 2); // state id
    wire::write_varint(&mut old, 1); // one slot
    wire::write_varint(&mut old, 1); // stack count
    wire::write_varint(&mut old, iron_pickaxe);
    wire::write_varint(&mut old, 1); // one added component
    wire::write_varint(&mut old, 0); // no removals
    wire::write_varint(&mut old, 9); // 1.20.6 enchantments component
    wire::write_varint(&mut old, 3); // three enchantments
    wire::write_varint(&mut old, 20); // efficiency
    wire::write_varint(&mut old, 3); // level III
    wire::write_varint(&mut old, 6); // aqua affinity
    wire::write_varint(&mut old, 1); // level I
    wire::write_varint(&mut old, 33); // channeling (shifts to 34 after Lunge)
    wire::write_varint(&mut old, 1); // level I
    old.push(1); // showInTooltip
    wire::write_varint(&mut old, 0); // carried item empty

    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(766, old) else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    assert_eq!(data.kind, azalea_registry::builtin::ItemKind::IronPickaxe);
    let custom = data
        .component_patch
        .get::<azalea_inventory::components::CustomData>()
        .expect("legacy mining metadata");
    assert_eq!(custom.nbt.int("pomme:legacy_efficiency"), Some(3));
    assert_eq!(custom.nbt.byte("pomme:legacy_aqua_affinity"), Some(1));

    use azalea_registry::DataRegistry;
    let enchantments = data
        .component_patch
        .get::<azalea_inventory::components::Enchantments>()
        .expect("translated enchantments component");
    let mut levels = enchantments
        .levels
        .iter()
        .map(|(kind, level)| (kind.protocol_id(), *level))
        .collect::<Vec<_>>();
    levels.sort_unstable();
    assert_eq!(levels, vec![(6, 1), (20, 3), (34, 1)]);
}

#[test]
fn translate_stack_766_skips_unsupported_tool_value_without_dropping_packet() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let table = RegistryTable::for_protocol(766).expect("1.20.6 registry table");
    let iron_pickaxe = registry_id(table, ClientRegistry::Item, "iron_pickaxe");

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(766, Direction::Clientbound, "container_set_content"),
    );
    old.push(1);
    wire::write_varint(&mut old, 2);
    wire::write_varint(&mut old, 1);
    wire::write_varint(&mut old, 1);
    wire::write_varint(&mut old, iron_pickaxe);
    wire::write_varint(&mut old, 2); // Tool + damage
    wire::write_varint(&mut old, 0);
    wire::write_varint(&mut old, 22); // 1.20.6 Tool
    wire::write_varint(&mut old, 1); // one Tool rule
    wire::write_varint(&mut old, 0); // named HolderSet
    write_utf(&mut old, "minecraft:mineable/pickaxe");
    old.push(1); // speed present
    old.extend_from_slice(&6.0f32.to_be_bytes());
    old.push(1); // correctForDrops present
    old.push(1); // true
    old.extend_from_slice(&1.0f32.to_be_bytes()); // defaultMiningSpeed
    wire::write_varint(&mut old, 1); // damagePerBlock
    wire::write_varint(&mut old, 3); // damage component follows Tool
    wire::write_varint(&mut old, 7);
    wire::write_varint(&mut old, 0); // carried item empty

    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(766, old) else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    assert_eq!(data.kind, azalea_registry::builtin::ItemKind::IronPickaxe);
    assert_eq!(
        data.component_patch
            .get::<azalea_inventory::components::Damage>()
            .map(|damage| damage.amount),
        Some(7)
    );
    assert!(
        data.component_patch
            .get::<azalea_inventory::components::Tool>()
            .is_none()
    );
}

#[test]
fn translate_stack_766_old_only_unit_components_do_not_drop_packet() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let table = RegistryTable::for_protocol(766).expect("1.20.6 registry table");
    let stone = registry_id(table, ClientRegistry::Item, "stone");
    for component in [14u32, 15, 21] {
        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(766, Direction::Clientbound, "container_set_content"),
        );
        old.push(1); // container id
        wire::write_varint(&mut old, 2); // state id
        wire::write_varint(&mut old, 1); // one slot
        wire::write_varint(&mut old, 1); // stack count
        wire::write_varint(&mut old, stone);
        wire::write_varint(&mut old, 2); // marker + damage
        wire::write_varint(&mut old, 0); // no removals
        wire::write_varint(&mut old, component); // zero-byte old-only unit payload
        wire::write_varint(&mut old, 3); // damage
        wire::write_varint(&mut old, 7);
        wire::write_varint(&mut old, 0); // carried item empty

        let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(766, old)
        else {
            panic!("wrong packet for old-only component {component}");
        };
        let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
            panic!("empty stack for old-only component {component}");
        };
        assert_eq!(
            data.component_patch
                .get::<azalea_inventory::components::Damage>()
                .map(|damage| damage.amount),
            Some(7),
            "old-only component {component}"
        );
    }
}

#[test]
fn translate_stack_766_old_only_removal_keeps_mapped_removal() {
    use azalea_registry::builtin::DataComponentKind;
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let table = RegistryTable::for_protocol(766).expect("1.20.6 registry table");
    let stone = registry_id(table, ClientRegistry::Item, "stone");
    for component in [14u32, 15, 21] {
        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(766, Direction::Clientbound, "container_set_content"),
        );
        old.push(1);
        wire::write_varint(&mut old, 2);
        wire::write_varint(&mut old, 1);
        wire::write_varint(&mut old, 1);
        wire::write_varint(&mut old, stone);
        wire::write_varint(&mut old, 0); // no additions
        wire::write_varint(&mut old, 2); // old-only + damage removals
        wire::write_varint(&mut old, component);
        wire::write_varint(&mut old, 3); // damage
        wire::write_varint(&mut old, 0); // carried item empty

        let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(766, old)
        else {
            panic!("wrong packet for old-only removal {component}");
        };
        let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
            panic!("empty stack for old-only removal {component}");
        };
        let components: Vec<_> = data.component_patch.iter().collect();
        assert_eq!(components.len(), 1, "old-only component {component}");
        assert_eq!(components[0].0, DataComponentKind::Damage);
        assert!(components[0].1.is_none());
    }
}

#[test]
fn translate_stack_766_tool_removal_suppresses_native_default() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let table = RegistryTable::for_protocol(766).expect("1.20.6 registry table");
    let iron_pickaxe = registry_id(table, ClientRegistry::Item, "iron_pickaxe");

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(766, Direction::Clientbound, "container_set_content"),
    );
    old.push(1);
    wire::write_varint(&mut old, 2);
    wire::write_varint(&mut old, 1);
    wire::write_varint(&mut old, 1);
    wire::write_varint(&mut old, iron_pickaxe);
    wire::write_varint(&mut old, 0);
    wire::write_varint(&mut old, 1); // one removed component
    wire::write_varint(&mut old, 22); // 1.20.6 Tool removal
    wire::write_varint(&mut old, 0); // carried item empty

    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(766, old) else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    assert!(crate::tool::stack_tool(data, 766).is_none());
}

fn write_component_tool_stack_767_773(protocol: i32, direct: bool, removal: bool) -> Vec<u8> {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let table = RegistryTable::for_protocol(protocol).expect("embedded registry table");
    let iron_pickaxe = registry_id(table, ClientRegistry::Item, "iron_pickaxe");
    let tool = registry_id(table, ClientRegistry::DataComponentType, "tool");
    let damage = registry_id(table, ClientRegistry::DataComponentType, "damage");

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(protocol, Direction::Clientbound, "container_set_content"),
    );
    if protocol == 767 {
        old.push(1); // byte container id through 1.21.1
    } else {
        wire::write_varint(&mut old, 1);
    }
    wire::write_varint(&mut old, 2); // state id
    wire::write_varint(&mut old, 1); // one slot
    wire::write_varint(&mut old, 1); // stack count
    wire::write_varint(&mut old, iron_pickaxe);
    if removal {
        wire::write_varint(&mut old, 0);
        wire::write_varint(&mut old, 1);
        wire::write_varint(&mut old, tool);
    } else {
        wire::write_varint(&mut old, 2); // Tool + damage
        wire::write_varint(&mut old, 0);
        wire::write_varint(&mut old, tool);
        wire::write_varint(&mut old, 1); // one Tool rule
        if direct {
            wire::write_varint(&mut old, 2); // direct HolderSet: len + 1
            wire::write_varint(&mut old, 1); // one block holder id
        } else {
            wire::write_varint(&mut old, 0); // named HolderSet
            write_utf(&mut old, "minecraft:mineable/pickaxe");
        }
        old.push(1); // speed present
        old.extend_from_slice(&6.0f32.to_be_bytes());
        old.push(1); // correctForDrops present
        old.push(1); // true
        old.extend_from_slice(&1.0f32.to_be_bytes());
        wire::write_varint(&mut old, 1); // damagePerBlock
        if protocol >= 770 {
            old.push(1); // canDestroyBlocksInCreative, added in 1.21.5
        }
        wire::write_varint(&mut old, damage);
        wire::write_varint(&mut old, 7);
    }
    wire::write_varint(&mut old, 0); // carried item empty
    old
}

#[test]
fn translate_tool_components_767_773_are_safe() {
    for protocol in 767..=773 {
        for direct in [false, true] {
            let old = write_component_tool_stack_767_773(protocol, direct, false);
            let ClientboundGamePacket::ContainerSetContent(p) =
                translate_decode_and_remap(protocol, old)
            else {
                panic!("wrong packet for protocol {protocol}");
            };
            let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
                panic!("empty stack for protocol {protocol}");
            };
            assert_eq!(
                data.component_patch
                    .get::<azalea_inventory::components::Damage>()
                    .map(|damage| damage.amount),
                Some(7),
                "protocol {protocol}, direct={direct}"
            );
            assert!(
                crate::tool::stack_tool(data, protocol).is_some(),
                "unsupported Tool value should fall back to native default on protocol {protocol}"
            );
        }

        let old = write_component_tool_stack_767_773(protocol, false, true);
        let ClientboundGamePacket::ContainerSetContent(p) =
            translate_decode_and_remap(protocol, old)
        else {
            panic!("wrong removal packet for protocol {protocol}");
        };
        let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
            panic!("empty removal stack for protocol {protocol}");
        };
        assert!(
            crate::tool::stack_tool(data, protocol).is_none(),
            "Tool removal must suppress the native default on protocol {protocol}"
        );
    }
}

#[test]
fn translate_tool_component_774_preserves_value() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let protocol = 774;
    let table = RegistryTable::for_protocol(protocol).expect("embedded registry table");
    let iron_pickaxe = registry_id(table, ClientRegistry::Item, "iron_pickaxe");
    let tool = registry_id(table, ClientRegistry::DataComponentType, "tool");

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(protocol, Direction::Clientbound, "container_set_content"),
    );
    wire::write_varint(&mut old, 1); // container id
    wire::write_varint(&mut old, 2); // state id
    wire::write_varint(&mut old, 1); // one slot
    wire::write_varint(&mut old, 1); // stack count
    wire::write_varint(&mut old, iron_pickaxe);
    wire::write_varint(&mut old, 1); // one added component
    wire::write_varint(&mut old, 0); // no removals
    wire::write_varint(&mut old, tool);
    wire::write_varint(&mut old, 0); // no Tool rules
    old.extend_from_slice(&9.0f32.to_be_bytes());
    wire::write_varint(&mut old, 2); // damagePerBlock
    old.push(0); // canDestroyBlocksInCreative
    wire::write_varint(&mut old, 0); // carried item empty

    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(protocol, old)
    else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    let tool = crate::tool::stack_tool(data, protocol).expect("Tool value preserved");
    assert_eq!(tool.default_mining_speed, 9.0);
    assert_eq!(tool.damage_per_block, 2);
    assert!(!tool.can_destroy_blocks_in_creative);
}

#[test]
fn creative_components_767_774_preserve_patch_and_wire_framing() {
    use azalea_inventory::components::{CustomData, Damage, DataComponentUnion};
    use azalea_inventory::{DataComponentPatch, ItemStack, ItemStackData};
    use azalea_protocol::packets::game::ServerboundGamePacket;
    use azalea_protocol::packets::game::s_set_creative_mode_slot::ServerboundSetCreativeModeSlot;
    use azalea_registry::builtin::{DataComponentKind, ItemKind};
    use pomme_protocol::{ClientRegistry, RegistryTable};
    use simdnbt::owned::{Nbt, NbtCompound};

    for protocol in [767, 769, 770, 773, 774] {
        let mut compound = NbtCompound::new();
        compound.insert("server:real_value", 42i32);
        let mut patch = DataComponentPatch::default();
        unsafe {
            patch.unchecked_insert_component(
                DataComponentKind::CustomData,
                Some(DataComponentUnion::from(CustomData {
                    nbt: Nbt::new("".into(), compound),
                })),
            );
            patch.unchecked_insert_component(
                DataComponentKind::Damage,
                Some(DataComponentUnion::from(Damage { amount: 7 })),
            );
            patch.unchecked_insert_component(DataComponentKind::Tool, None);
        }
        let mut packet =
            ServerboundGamePacket::SetCreativeModeSlot(ServerboundSetCreativeModeSlot {
                slot_num: 36,
                item_stack: ItemStack::Present(ItemStackData {
                    kind: ItemKind::IronPickaxe,
                    count: 1,
                    component_patch: patch,
                }),
            });

        let translation = translation_for(protocol);
        translation.remap_outbound(&mut packet);
        let ServerboundGamePacket::SetCreativeModeSlot(p) = &packet else {
            unreachable!()
        };
        let ItemStack::Present(data) = &p.item_stack else {
            panic!("stack cleared for protocol {protocol}");
        };
        assert_eq!(data.component_patch.iter().count(), 3);

        let latest = azalea_protocol::write::serialize_packet(&packet).unwrap();
        let frames = translation.translate_outbound_game_frame(latest.to_vec());
        assert_eq!(frames.len(), 1, "protocol {protocol}");
        let frame = &frames[0];
        let mut pos = 0;
        assert_eq!(
            wire::read_varint(frame, &mut pos),
            Some(old_id(
                protocol,
                Direction::Serverbound,
                "set_creative_mode_slot"
            ))
        );
        assert_eq!(&frame[pos..pos + 2], &36u16.to_be_bytes());
        pos += 2;
        assert_eq!(wire::read_varint(frame, &mut pos), Some(1));
        assert!(wire::read_varint(frame, &mut pos).is_some());
        assert_eq!(wire::read_varint(frame, &mut pos), Some(2));
        assert_eq!(wire::read_varint(frame, &mut pos), Some(1));

        let table = RegistryTable::for_protocol(protocol).expect("registry table");
        let custom_id = registry_id(table, ClientRegistry::DataComponentType, "custom_data");
        let damage_id = registry_id(table, ClientRegistry::DataComponentType, "damage");
        let tool_id = registry_id(table, ClientRegistry::DataComponentType, "tool");
        let mut seen_custom = false;
        let mut seen_damage = false;
        for _ in 0..2 {
            let component = wire::read_varint(frame, &mut pos).unwrap();
            let value = if protocol >= 770 {
                let len = wire::read_varint(frame, &mut pos).unwrap() as usize;
                let slice = &frame[pos..pos + len];
                pos += len;
                slice
            } else {
                &frame[pos..]
            };
            if component == custom_id {
                let mut cur = std::io::Cursor::new(value);
                let nbt = simdnbt::owned::read_unnamed(&mut cur).unwrap();
                let simdnbt::owned::Nbt::Some(root) = nbt else {
                    panic!("missing custom_data for protocol {protocol}");
                };
                assert_eq!(root.as_compound().int("server:real_value"), Some(42));
                if protocol < 770 {
                    pos += cur.position() as usize;
                }
                seen_custom = true;
            } else if component == damage_id {
                let mut cur = std::io::Cursor::new(value);
                assert_eq!(i32::azalea_read_var(&mut cur).unwrap(), 7);
                if protocol < 770 {
                    pos += cur.position() as usize;
                }
                seen_damage = true;
            } else {
                panic!("unexpected component {component} on protocol {protocol}");
            }
        }
        assert!(seen_custom && seen_damage, "protocol {protocol}");
        assert_eq!(wire::read_varint(frame, &mut pos), Some(tool_id));
        assert_eq!(pos, frame.len(), "protocol {protocol}");
    }
}

fn write_component_stack_with_following_damage(
    protocol: i32,
    component_name: &str,
    component_payload: impl FnOnce(&mut Vec<u8>),
) -> Vec<u8> {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let table = RegistryTable::for_protocol(protocol).expect("embedded registry table");
    let stone = registry_id(table, ClientRegistry::Item, "stone");
    let component = registry_id(table, ClientRegistry::DataComponentType, component_name);
    let damage = registry_id(table, ClientRegistry::DataComponentType, "damage");
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(protocol, Direction::Clientbound, "container_set_content"),
    );
    if protocol == 767 {
        old.push(1);
    } else {
        wire::write_varint(&mut old, 1);
    }
    wire::write_varint(&mut old, 2);
    wire::write_varint(&mut old, 1);
    wire::write_varint(&mut old, 1);
    wire::write_varint(&mut old, stone);
    wire::write_varint(&mut old, 2);
    wire::write_varint(&mut old, 0);
    wire::write_varint(&mut old, component);
    component_payload(&mut old);
    wire::write_varint(&mut old, damage);
    wire::write_varint(&mut old, 7);
    wire::write_varint(&mut old, 0);
    old
}

#[test]
fn translate_old_only_unit_components_drop_without_losing_patch() {
    for (protocol, component) in [
        (767, "hide_additional_tooltip"),
        (767, "hide_tooltip"),
        (767, "fire_resistant"),
        (768, "hide_additional_tooltip"),
        (768, "hide_tooltip"),
        (769, "hide_additional_tooltip"),
        (769, "hide_tooltip"),
    ] {
        let old = write_component_stack_with_following_damage(protocol, component, |_| {});
        assert_trailing_damage(protocol, old);
    }
}

#[test]
fn translate_old_only_component_removal_does_not_drop_mapped_removal() {
    use azalea_registry::builtin::DataComponentKind;
    use pomme_protocol::{ClientRegistry, RegistryTable};

    for (protocol, old_only) in [
        (767, "hide_tooltip"),
        (767, "fire_resistant"),
        (768, "hide_additional_tooltip"),
        (769, "hide_tooltip"),
    ] {
        let table = RegistryTable::for_protocol(protocol).unwrap();
        let stone = registry_id(table, ClientRegistry::Item, "stone");
        let old_only = registry_id(table, ClientRegistry::DataComponentType, old_only);
        let damage = registry_id(table, ClientRegistry::DataComponentType, "damage");
        let mut old = Vec::new();
        wire::write_varint(
            &mut old,
            old_id(protocol, Direction::Clientbound, "container_set_content"),
        );
        if protocol == 767 {
            old.push(1);
        } else {
            wire::write_varint(&mut old, 1);
        }
        wire::write_varint(&mut old, 2); // state id
        wire::write_varint(&mut old, 1); // one item
        wire::write_varint(&mut old, 1); // stack count
        wire::write_varint(&mut old, stone);
        wire::write_varint(&mut old, 0); // no additions
        wire::write_varint(&mut old, 2); // two removals
        wire::write_varint(&mut old, old_only);
        wire::write_varint(&mut old, damage);
        wire::write_varint(&mut old, 0); // carried stack empty

        let ClientboundGamePacket::ContainerSetContent(p) =
            translate_decode_and_remap(protocol, old)
        else {
            panic!("wrong packet for protocol {protocol}");
        };
        let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
            panic!("empty stack for protocol {protocol}");
        };
        let components: Vec<_> = data.component_patch.iter().collect();
        assert_eq!(components.len(), 1, "protocol {protocol}");
        assert_eq!(
            components[0].0,
            DataComponentKind::Damage,
            "protocol {protocol}"
        );
        assert!(components[0].1.is_none(), "protocol {protocol}");
    }
}

#[test]
fn translate_food_767_consumes_nested_effects_without_losing_alignment() {
    let old = write_component_stack_with_following_damage(767, "food", |old| {
        wire::write_varint(old, 4); // nutrition
        old.extend_from_slice(&0.3f32.to_be_bytes());
        old.push(1); // canAlwaysEat
        old.extend_from_slice(&1.6f32.to_be_bytes());
        wire::write_varint(old, 0); // usingConvertsTo = empty ItemStack
        wire::write_varint(old, 1); // one PossibleEffect
        wire::write_varint(old, 1); // MobEffect holder id
        wire::write_varint(old, 2); // amplifier
        wire::write_varint(old, 100); // duration
        old.extend_from_slice(&[0, 1, 1, 0]); // ambient, visible, showIcon, no hidden effect
        old.extend_from_slice(&0.5f32.to_be_bytes()); // probability
    });
    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(767, old) else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    assert!(
        data.component_patch
            .get::<azalea_inventory::components::Food>()
            .is_none()
    );
    assert_eq!(
        data.component_patch
            .get::<azalea_inventory::components::Damage>()
            .map(|damage| damage.amount),
        Some(7)
    );
}

#[test]
fn translate_attribute_modifiers_770_has_no_legacy_tooltip_byte() {
    let old = write_component_stack_with_following_damage(770, "attribute_modifiers", |old| {
        wire::write_varint(old, 0); // zero entries; no showInTooltip on 1.21.5
    });
    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(770, old) else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    assert_eq!(
        data.component_patch
            .get::<azalea_inventory::components::Damage>()
            .map(|damage| damage.amount),
        Some(7)
    );
}

#[test]
fn translate_food_768_uses_current_three_field_layout() {
    let old = write_component_stack_with_following_damage(768, "food", |old| {
        wire::write_varint(old, 4);
        old.extend_from_slice(&0.3f32.to_be_bytes());
        old.push(1);
    });
    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(768, old) else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    let food = data
        .component_patch
        .get::<azalea_inventory::components::Food>()
        .expect("1.21.3 food preserved");
    assert_eq!(food.nutrition, 4);
    assert_eq!(food.saturation, 0.3);
    assert!(food.can_always_eat);
    assert_eq!(
        data.component_patch
            .get::<azalea_inventory::components::Damage>()
            .map(|damage| damage.amount),
        Some(7)
    );
}

#[test]
fn translate_attribute_modifiers_771_remaps_embedded_attribute_id() {
    let old = write_component_stack_with_following_damage(771, "attribute_modifiers", |old| {
        wire::write_varint(old, 1); // one entry
        wire::write_varint(old, 4); // 1.21.6 attack_speed; 26.2 id is 5
        write_utf(old, "minecraft:test_speed");
        old.extend_from_slice(&2.0f64.to_be_bytes());
        wire::write_varint(old, 0); // operation add_value
        wire::write_varint(old, 0); // slot group
        wire::write_varint(old, 0); // display = default
    });
    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(771, old) else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    let modifiers = data
        .component_patch
        .get::<azalea_inventory::components::AttributeModifiers>()
        .expect("attribute modifiers preserved");
    assert_eq!(modifiers.modifiers.len(), 1);
    assert_eq!(
        modifiers.modifiers[0].kind,
        azalea_registry::builtin::Attribute::AttackSpeed
    );
    assert_eq!(
        data.component_patch
            .get::<azalea_inventory::components::Damage>()
            .map(|damage| damage.amount),
        Some(7)
    );
}

#[test]
fn translate_old_custom_model_data_drops_only_that_component() {
    for protocol in [767, 768] {
        let old =
            write_component_stack_with_following_damage(protocol, "custom_model_data", |old| {
                wire::write_varint(old, 123);
            });
        let ClientboundGamePacket::ContainerSetContent(p) =
            translate_decode_and_remap(protocol, old)
        else {
            panic!("wrong packet for protocol {protocol}");
        };
        let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
            panic!("empty stack for protocol {protocol}");
        };
        assert!(
            data.component_patch
                .get::<azalea_inventory::components::CustomModelData>()
                .is_none()
        );
        assert_eq!(
            data.component_patch
                .get::<azalea_inventory::components::Damage>()
                .map(|damage| damage.amount),
            Some(7)
        );
    }
}

#[test]
fn translate_potion_contents_767_drops_only_that_component() {
    let old = write_component_stack_with_following_damage(767, "potion_contents", |old| {
        old.push(0); // potion absent
        old.push(0); // custom color absent
        wire::write_varint(old, 0); // no custom effects
    });
    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(767, old) else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    assert!(
        data.component_patch
            .get::<azalea_inventory::components::PotionContents>()
            .is_none()
    );
    assert_eq!(
        data.component_patch
            .get::<azalea_inventory::components::Damage>()
            .map(|damage| damage.amount),
        Some(7)
    );
}

#[test]
fn translate_equippable_historical_shapes_stay_aligned() {
    for protocol in [768, 770, 771, 774, 775] {
        let old = write_component_stack_with_following_damage(protocol, "equippable", |old| {
            wire::write_varint(old, 0); // mainhand slot
            wire::write_varint(old, 0); // raw equip-sound registry id
            old.push(0); // model/asset absent
            old.push(0); // camera overlay absent
            old.push(0); // allowed entities absent
            old.extend_from_slice(&[0, 0, 0]); // dispensable, swappable, damageOnHurt
            if protocol >= 770 {
                old.push(0); // equipOnInteract
            }
            if protocol >= 771 {
                old.push(0); // canBeSheared
                wire::write_varint(old, 0); // shearing sound registry id
            }
        });
        assert_trailing_damage(protocol, old);
    }
}

#[test]
fn translate_jukebox_769_consumes_resource_key_and_tooltip() {
    let old = write_component_stack_with_following_damage(769, "jukebox_playable", |old| {
        old.push(0); // EitherHolder ResourceKey branch
        write_utf(old, "minecraft:13");
        old.push(0); // showInTooltip
    });
    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(769, old) else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    assert_eq!(
        data.component_patch
            .get::<azalea_inventory::components::Damage>()
            .map(|damage| damage.amount),
        Some(7)
    );
}

fn write_holder_set(out: &mut Vec<u8>, direct: bool, named: &str) {
    if direct {
        wire::write_varint(out, 2); // one direct holder => len + 1
        wire::write_varint(out, 1); // raw registry holder id
    } else {
        wire::write_varint(out, 0);
        write_utf(out, named);
    }
}

fn assert_trailing_damage(protocol: i32, old: Vec<u8>) {
    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(protocol, old)
    else {
        panic!("wrong packet for protocol {protocol}");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack for protocol {protocol}");
    };
    assert_eq!(
        data.component_patch
            .get::<azalea_inventory::components::Damage>()
            .map(|damage| damage.amount),
        Some(7),
        "protocol {protocol}"
    );
}

#[test]
fn translate_blocks_attacks_bypass_tag_keys_stay_aligned() {
    for protocol in [770, 774] {
        let old = write_component_stack_with_following_damage(protocol, "blocks_attacks", |old| {
            old.extend_from_slice(&0.0f32.to_be_bytes());
            old.extend_from_slice(&1.0f32.to_be_bytes());
            wire::write_varint(old, 0); // damage reductions
            old.extend_from_slice(&1.0f32.to_be_bytes());
            old.extend_from_slice(&0.0f32.to_be_bytes());
            old.extend_from_slice(&1.0f32.to_be_bytes());
            old.push(1); // bypassedBy present
            write_utf(old, "minecraft:bypasses_shield"); // historical TagKey
            old.extend_from_slice(&[0, 0]); // no block/disabled sounds
        });
        assert_trailing_damage(protocol, old);
    }
}

#[test]
fn translate_blocks_attacks_reduction_holder_sets_stay_aligned() {
    for protocol in [770, 774] {
        for direct in [false, true] {
            let old =
                write_component_stack_with_following_damage(protocol, "blocks_attacks", |old| {
                    old.extend_from_slice(&0.0f32.to_be_bytes());
                    old.extend_from_slice(&1.0f32.to_be_bytes());
                    wire::write_varint(old, 1); // one damage reduction
                    old.extend_from_slice(&90.0f32.to_be_bytes());
                    old.push(1); // DamageReduction.type present
                    write_holder_set(old, direct, "minecraft:is_projectile");
                    old.extend_from_slice(&0.0f32.to_be_bytes());
                    old.extend_from_slice(&1.0f32.to_be_bytes());
                    old.extend_from_slice(&1.0f32.to_be_bytes());
                    old.extend_from_slice(&0.0f32.to_be_bytes());
                    old.extend_from_slice(&1.0f32.to_be_bytes());
                    old.push(0); // bypassedBy absent
                    old.extend_from_slice(&[0, 0]); // no block/disabled sounds
                });
            assert_trailing_damage(protocol, old);
        }
    }
}

#[test]
fn translate_damage_resistant_tag_keys_stay_aligned() {
    for protocol in [768, 774] {
        let old =
            write_component_stack_with_following_damage(protocol, "damage_resistant", |old| {
                write_utf(old, "minecraft:is_fire");
            });
        assert_trailing_damage(protocol, old);
    }
}

#[test]
fn translate_banner_pattern_tag_keys_stay_aligned() {
    for protocol in [770, 774] {
        let old = write_component_stack_with_following_damage(
            protocol,
            "provides_banner_patterns",
            |old| write_utf(old, "minecraft:pattern_item/flower"),
        );
        assert_trailing_damage(protocol, old);
    }
}

#[test]
fn translate_either_registry_components_stay_aligned() {
    for (protocol, component) in [
        (770, "chicken/variant"),
        (774, "chicken/variant"),
        (774, "damage_type"),
        (774, "zombie_nautilus/variant"),
    ] {
        for registry_arm in [false, true] {
            let old = write_component_stack_with_following_damage(protocol, component, |old| {
                if registry_arm {
                    old.push(1); // EitherHolder holder branch
                    wire::write_varint(old, 1); // raw holderRegistry id
                } else {
                    old.push(0); // resource-key branch
                    write_utf(old, "minecraft:test_variant");
                }
            });
            assert_trailing_damage(protocol, old);
        }
    }
}

#[test]
fn historical_nested_stack_components_remap_nested_component_ids() {
    use azalea_registry::builtin::ItemKind;
    use pomme_protocol::{ClientRegistry, RegistryTable};

    fn assert_lock(stack: &azalea_inventory::ItemStack) {
        let azalea_inventory::ItemStack::Present(data) = stack else {
            panic!("nested stack empty");
        };
        assert_eq!(data.kind, ItemKind::DripstoneBlock);
        assert_eq!(
            data.component_patch
                .get::<azalea_inventory::components::Lock>()
                .map(|lock| lock.key.as_str()),
            Some("pomme:nested_key")
        );
    }

    let protocol = 775;
    let table = RegistryTable::for_protocol(protocol).expect("registry table");
    let dripstone = registry_id(table, ClientRegistry::Item, "dripstone_block");
    let lock = registry_id(table, ClientRegistry::DataComponentType, "lock");
    assert_ne!(
        dripstone,
        registry_id(
            RegistryTable::native(),
            ClientRegistry::Item,
            "dripstone_block"
        )
    );
    for component in [
        "use_remainder",
        "charged_projectiles",
        "bundle_contents",
        "container",
    ] {
        let old = write_component_stack_with_following_damage(protocol, component, |old| {
            if component != "use_remainder" {
                wire::write_varint(old, 1); // one nested template
            }
            if component == "container" {
                old.push(1); // Optional<ItemStackTemplate>::Some
            }
            // Mojang 26.1 ItemStackTemplate: item, count, ordinary component patch.
            wire::write_varint(old, dripstone);
            wire::write_varint(old, 1);
            wire::write_varint(old, 1); // one nested added component
            wire::write_varint(old, 0); // no nested removals
            wire::write_varint(old, lock);
            write_utf(old, "pomme:nested_key");
        });
        let ClientboundGamePacket::ContainerSetContent(p) =
            translate_decode_and_remap(protocol, old)
        else {
            panic!("wrong packet for {component}");
        };
        let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
            panic!("outer stack empty for {component}");
        };
        match component {
            "use_remainder" => assert_lock(
                &data
                    .component_patch
                    .get::<azalea_inventory::components::UseRemainder>()
                    .expect("use_remainder preserved")
                    .convert_into,
            ),
            "charged_projectiles" => assert_lock(
                &data
                    .component_patch
                    .get::<azalea_inventory::components::ChargedProjectiles>()
                    .expect("charged_projectiles preserved")
                    .items[0],
            ),
            "bundle_contents" => assert_lock(
                &data
                    .component_patch
                    .get::<azalea_inventory::components::BundleContents>()
                    .expect("bundle_contents preserved")
                    .items[0],
            ),
            "container" => assert_lock(
                &data
                    .component_patch
                    .get::<azalea_inventory::components::Container>()
                    .expect("container preserved")
                    .items[0],
            ),
            _ => unreachable!(),
        }
        assert_eq!(
            data.component_patch
                .get::<azalea_inventory::components::Damage>()
                .map(|damage| damage.amount),
            Some(7)
        );
    }
}

#[test]
fn translate_container_775_optional_empty_slot_becomes_empty_itemstack() {
    let protocol = 775;
    let old = write_component_stack_with_following_damage(protocol, "container", |old| {
        wire::write_varint(old, 1); // one slot
        old.push(0); // Optional<ItemStackTemplate>::None
    });
    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(protocol, old)
    else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("outer stack empty");
    };
    let container = data
        .component_patch
        .get::<azalea_inventory::components::Container>()
        .expect("container preserved");
    assert_eq!(container.items.len(), 1);
    assert!(matches!(
        container.items[0],
        azalea_inventory::ItemStack::Empty
    ));
    assert_eq!(
        data.component_patch
            .get::<azalea_inventory::components::Damage>()
            .map(|damage| damage.amount),
        Some(7)
    );
}

#[test]
fn translate_tooltip_display_775_remaps_hidden_component_ids() {
    use azalea_registry::builtin::DataComponentKind;
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let table = RegistryTable::for_protocol(775).unwrap();
    let lock = registry_id(table, ClientRegistry::DataComponentType, "lock");
    assert_ne!(
        lock,
        registry_id(
            RegistryTable::native(),
            ClientRegistry::DataComponentType,
            "lock"
        )
    );
    let old = write_component_stack_with_following_damage(775, "tooltip_display", |old| {
        old.push(0); // hideTooltip=false
        wire::write_varint(old, 1); // one hidden component
        wire::write_varint(old, lock);
    });
    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(775, old) else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    let tooltip = data
        .component_patch
        .get::<azalea_inventory::components::TooltipDisplay>()
        .expect("tooltip_display preserved");
    assert!(!tooltip.hide_tooltip);
    assert_eq!(tooltip.hidden_components, vec![DataComponentKind::Lock]);
    assert_eq!(
        data.component_patch
            .get::<azalea_inventory::components::Damage>()
            .map(|damage| damage.amount),
        Some(7)
    );
}

#[test]
fn translate_component_ids_775_before_latest_decode() {
    let old = write_component_stack_with_following_damage(775, "lock", |old| {
        write_utf(old, "pomme:test_key");
    });
    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(775, old) else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    assert_eq!(
        data.component_patch
            .get::<azalea_inventory::components::Lock>()
            .map(|lock| lock.key.as_str()),
        Some("pomme:test_key")
    );
    assert_eq!(
        data.component_patch
            .get::<azalea_inventory::components::Damage>()
            .map(|damage| damage.amount),
        Some(7)
    );
}

#[test]
fn translate_stack_766_does_not_trust_server_private_mining_keys() {
    use pomme_protocol::{ClientRegistry, RegistryTable};
    use simdnbt::owned::{Nbt, NbtCompound};

    let table = RegistryTable::for_protocol(766).expect("1.20.6 registry table");
    let iron_pickaxe = registry_id(table, ClientRegistry::Item, "iron_pickaxe");
    let mut custom = NbtCompound::new();
    custom.insert("pomme:legacy_efficiency", 99i32);
    custom.insert("pomme:legacy_aqua_affinity", 1i8);
    custom.insert("server:real_value", 42i32);

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(766, Direction::Clientbound, "container_set_content"),
    );
    old.push(1);
    wire::write_varint(&mut old, 2);
    wire::write_varint(&mut old, 1);
    wire::write_varint(&mut old, 1);
    wire::write_varint(&mut old, iron_pickaxe);
    wire::write_varint(&mut old, 1);
    wire::write_varint(&mut old, 0);
    wire::write_varint(&mut old, 0); // custom_data
    Nbt::new("".into(), custom).azalea_write(&mut old).unwrap();
    wire::write_varint(&mut old, 0); // carried item empty

    let ClientboundGamePacket::ContainerSetContent(p) = translate_decode_and_remap(766, old) else {
        panic!("wrong packet");
    };
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    let custom = data
        .component_patch
        .get::<azalea_inventory::components::CustomData>()
        .expect("custom_data preserved");
    assert_eq!(custom.nbt.int("server:real_value"), Some(42));
    assert_eq!(custom.nbt.int("pomme:legacy_efficiency"), None);
    assert_eq!(custom.nbt.byte("pomme:legacy_aqua_affinity"), None);
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

/// A 1.20.4 optional item (`bool + item + i8 count + NBT`) keeps its legacy
/// NBT in latest `custom_data` through `container_set_content`; the trailing
/// carried stack survives too.
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
    let custom = data
        .component_patch
        .get::<azalea_inventory::components::CustomData>()
        .expect("legacy NBT preserved as custom_data");
    assert_eq!(custom.nbt.byte("d"), Some(5));
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

    let ClientboundGamePacket::UpdateAttributes(p) = translate_decode_and_remap(765, old) else {
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
    let t = translation_for(765);
    assert!(t.translates_config());
    assert!(!translation_for(766).translates_config());

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

/// The expected 26.2-side component for a JSON literal, parsed with
/// azalea's own JSON path.
fn expect_text(json: &str) -> azalea_chat::FormattedText {
    use serde::de::Deserialize;
    azalea_chat::FormattedText::deserialize(
        &serde_json::from_str::<serde_json::Value>(json).unwrap(),
    )
    .unwrap()
}

/// The 1.20.2 JSON -> NBT component transcode, end to end through
/// `system_chat` (leading component) — nested styling and a mixed `extra`
/// array (bare string beside a compound) normalize into azalea's shape.
#[test]
fn translate_system_chat_764() {
    let json = r#"{"text":"a","color":"red","extra":["b",{"text":"c","bold":true}]}"#;
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(764, Direction::Clientbound, "system_chat"));
    write_json(&mut old, json);
    old.push(0); // not an action bar

    let ClientboundGamePacket::SystemChat(p) = translate_and_decode(764, old) else {
        panic!("wrong packet");
    };
    assert!(!p.overlay);
    assert_eq!(p.content.to_string(), expect_text(json).to_string());
}

/// `tab_list` carries two back-to-back components.
#[test]
fn translate_tab_list_764() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(764, Direction::Clientbound, "tab_list"));
    write_json(&mut old, r#"{"text":"head"}"#);
    write_json(&mut old, r#""foot""#); // bare JSON literal

    let ClientboundGamePacket::TabList(p) = translate_and_decode(764, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.header.to_string(), "head");
    assert_eq!(p.footer.to_string(), "foot");
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

    let ClientboundGamePacket::PlayerCombatKill(p) = translate_and_decode(764, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.player_id, MinecraftEntityId(9));
    assert_eq!(p.message.to_string(), "died");
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

    let ClientboundGamePacket::BossEvent(p) = translate_and_decode(764, old) else {
        panic!("wrong packet");
    };
    let azalea_protocol::packets::game::c_boss_event::Operation::Add(add) = p.operation else {
        panic!("wrong operation");
    };
    assert_eq!(add.name.to_string(), "boss");
    assert_eq!(add.progress, 0.5);
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

    let ClientboundGamePacket::SetPlayerTeam(p) = translate_and_decode(764, old) else {
        panic!("wrong packet");
    };
    let azalea_protocol::packets::game::c_set_player_team::Method::Add((parameters, players)) =
        p.method
    else {
        panic!("wrong method");
    };
    assert_eq!(parameters.display_name.to_string(), "Reds");
    assert_eq!(parameters.player_prefix.to_string(), "[R] ");
    assert_eq!(players, vec!["bob".to_string()]);
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

    let ClientboundGamePacket::PlayerChat(p) = translate_and_decode(764, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.global_index, 0);
    assert_eq!(p.index, 3);
    assert_eq!(p.body.content, "hi");
    assert_eq!(p.unsigned_content.as_ref().unwrap().to_string(), "hey");
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

    let ClientboundGamePacket::DisguisedChat(p) = translate_and_decode(764, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.message.to_string(), "psst");
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

    let ClientboundGamePacket::PlayerInfoUpdate(p) = translate_and_decode(764, old) else {
        panic!("wrong packet");
    };
    assert!(p.actions.add_player);
    assert!(p.actions.update_display_name);
    let entry = &p.entries[0];
    assert_eq!(entry.profile.name, "bob");
    assert_eq!(entry.display_name.as_ref().unwrap().to_string(), "Bob");
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

    let ClientboundGamePacket::CommandSuggestions(p) = translate_and_decode(764, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.id, 5);
    assert_eq!(p.suggestions.list()[0].text(), "/hey");
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

    let ClientboundGamePacket::SetScore(p) = translate_and_decode(764, build(0, b"obj", true))
    else {
        panic!("wrong packet for change");
    };
    assert_eq!(p.owner, "bob");
    assert_eq!(p.objective_name, "obj");
    assert_eq!(p.score, 42);
    assert!(p.display.is_none());
    assert!(p.number_format.is_none());

    let ClientboundGamePacket::ResetScore(p) = translate_and_decode(764, build(1, b"obj", false))
    else {
        panic!("wrong packet for remove");
    };
    assert_eq!(p.owner, "bob");
    assert_eq!(p.objective_name.as_deref(), Some("obj"));

    let ClientboundGamePacket::ResetScore(p) = translate_and_decode(764, build(1, b"", false))
    else {
        panic!("wrong packet for remove-all");
    };
    assert!(p.objective_name.is_none());
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

    let ClientboundGamePacket::SetObjective(p) = translate_and_decode(764, old) else {
        panic!("wrong packet");
    };
    let azalea_protocol::packets::game::c_set_objective::Method::Add {
        display_name,
        number_format,
        ..
    } = p.method
    else {
        panic!("wrong method");
    };
    assert_eq!(display_name.to_string(), "Deaths");
    assert_eq!(number_format, azalea_chat::numbers::NumberFormat::Blank);
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

    let ClientboundGamePacket::ResourcePackPush(p) = translate_and_decode(764, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.id, uuid::Uuid::nil());
    assert_eq!(p.url, "http");
    assert!(p.required);
    assert_eq!(p.prompt.as_ref().unwrap().to_string(), "pls");
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
    use azalea_protocol::packets::config::ClientboundConfigPacket;

    let t = translation_for(764);

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        config_id(764, Direction::Clientbound, "disconnect"),
    );
    write_json(&mut old, r#"{"text":"bye"}"#);
    let frames = t.translate_config_frame(old.into_boxed_slice());
    assert_eq!(frames.len(), 1);
    let packet: ClientboundConfigPacket =
        azalea_protocol::read::deserialize_packet(&mut std::io::Cursor::new(&frames[0])).unwrap();
    let ClientboundConfigPacket::Disconnect(p) = packet else {
        panic!("wrong packet");
    };
    assert_eq!(p.reason.to_string(), "bye");

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        config_id(764, Direction::Clientbound, "resource_pack"),
    );
    old.extend_from_slice(&[1, b'u']); // url
    old.extend_from_slice(&[1, b'h']); // hash
    old.extend_from_slice(&[0, 0]); // not required, no prompt
    let frames = t.translate_config_frame(old.into_boxed_slice());
    let packet: ClientboundConfigPacket =
        azalea_protocol::read::deserialize_packet(&mut std::io::Cursor::new(&frames[0])).unwrap();
    let ClientboundConfigPacket::ResourcePackPush(p) = packet else {
        panic!("wrong packet");
    };
    assert_eq!(p.url, "u");
    assert!(p.prompt.is_none());

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
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(764, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 9); // entity id
    old.extend_from_slice(&[2, 6, 1]); // index 2, optional_component, present
    write_json(&mut old, r#"{"text":"Named"}"#);
    old.extend_from_slice(&[3, 0, 1]); // index 3, byte serializer
    old.push(0xFF);

    let ClientboundGamePacket::SetEntityData(p) = translate_and_decode(764, old) else {
        panic!("wrong packet");
    };
    let azalea_entity::EntityDataValue::OptionalFormattedText(name) = &p.packed_items.0[0].value
    else {
        panic!("wrong serializer");
    };
    assert_eq!(name.as_ref().unwrap().to_string(), "Named");
    assert!(matches!(
        p.packed_items.0[1].value,
        azalea_entity::EntityDataValue::Byte(1)
    ));
}

/// `update_attributes` names its attribute by registry id, and those shift
/// between versions (1.21.2 also dropped the category prefixes), so each is
/// remapped into the native space. Unremapped, an old `max_health` decodes as
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

        let ClientboundGamePacket::UpdateAttributes(p) = translate_decode_and_remap(protocol, old)
        else {
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
    use azalea_buf::AzBuf;
    use simdnbt::owned::NbtTag;

    let mut unnamed = Vec::new();
    NbtTag::Compound(dimension_registry_codec(dimensions))
        .azalea_write(&mut unnamed)
        .unwrap();
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
    use azalea_protocol::packets::config::ClientboundConfigPacket;
    use azalea_registry::DataRegistry;

    let mut old = Vec::new();
    write_login_763(
        &mut old,
        "minecraft:custom_end",
        &[
            ("minecraft:custom_end", 1),
            ("minecraft:custom_overworld", 0),
        ],
    );

    let frames = translation_for(763)
        .split_login_registries(&old)
        .expect("registry codec");
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

    // The same frame then decodes as a game login, its dimension key resolved
    // against the order just captured.
    let ClientboundGamePacket::Login(p) = translate_and_decode(763, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.player_id, MinecraftEntityId(7));
    assert_eq!(p.levels, vec!["minecraft:overworld".into()]);
    assert_eq!(p.max_players, 20);
    assert!(p.show_death_screen);
    assert!(!p.enforces_secure_chat);
    assert_eq!(p.common.dimension_type.protocol_id(), 1);
    assert_eq!(p.common.dimension.to_string(), "minecraft:overworld");
    assert_eq!(p.common.seed, 42);
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

    let ClientboundGamePacket::Respawn(p) = translate_and_decode(763, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.data_to_keep, 3);
    assert_eq!(p.common.dimension.to_string(), "minecraft:the_nether");
    assert_eq!(p.common.seed, 42);
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

    let ClientboundGamePacket::ForgetLevelChunk(p) = translate_and_decode(763, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.pos.x, 5);
    assert_eq!(p.pos.z, -3);
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

    let ClientboundGamePacket::AddEntity(p) = translate_decode_and_remap(763, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.id, MinecraftEntityId(42));
    assert_eq!(p.uuid, uuid);
    assert_eq!(p.entity_type, azalea_registry::builtin::EntityKind::Player);
    assert_eq!(p.position.x, 1.5);
    assert_eq!(p.position.y, 64.0);
    assert_eq!(p.position.z, -2.5);
    assert_eq!((p.x_rot, p.y_rot, p.y_head_rot), (10, 20, 20));
    assert_eq!(p.data, 0);
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

    let ClientboundGamePacket::ContainerSetContent(p) = translate_and_decode(763, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.container_id, 1);
    let azalea_inventory::ItemStack::Present(data) = &p.items[0] else {
        panic!("empty stack");
    };
    assert_eq!(data.kind, azalea_registry::builtin::ItemKind::Stone);
    assert_eq!(data.count, 3);
    assert!(matches!(p.carried_item, azalea_inventory::ItemStack::Empty));
}

/// A 763 chunk names both its heightmap root and every block entity's tag.
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
    let section = [0u8, 1, 0, 5, 0, 0];
    wire::write_varint(&mut old, section.len() as u32);
    old.extend_from_slice(&section);
    wire::write_varint(&mut old, 1); // one block entity
    old.push(0x11); // packed xz
    old.extend_from_slice(&64i16.to_be_bytes()); // y
    wire::write_varint(&mut old, 0); // block entity type
    write_named_nbt_d5(&mut old);
    old.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // empty light masks + lists

    let ClientboundGamePacket::LevelChunkWithLight(p) = translate_and_decode(763, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.x, 3);
    assert_eq!(p.z, -2);
    assert_eq!(p.chunk_data.heightmaps.len(), 1);
    assert_eq!(&*p.chunk_data.heightmaps[0].1, &[7u64]);
    assert_eq!(p.chunk_data.block_entities.len(), 1);
    let be = &p.chunk_data.block_entities[0];
    assert_eq!(be.y, 64);
    assert_eq!(be.data.int("d"), Some(5));
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

    let ClientboundGamePacket::BlockEntityData(p) = translate_and_decode(763, old) else {
        panic!("wrong packet");
    };
    let simdnbt::owned::Nbt::Some(tag) = p.tag else {
        panic!("tag dropped");
    };
    assert_eq!(tag.as_compound().int("d"), Some(5));
}

/// `tag_query` carries the same named root after its transaction id.
#[test]
fn translate_tag_query_763() {
    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(763, Direction::Clientbound, "tag_query"));
    wire::write_varint(&mut old, 7); // transaction id
    write_named_nbt_d5(&mut old);

    let ClientboundGamePacket::TagQuery(p) = translate_and_decode(763, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.transaction_id, 7);
    assert_eq!(p.tag.compound().and_then(|c| c.int("d")), Some(5));
}

/// Every element pomme embeds for `minecraft:core` has to survive azalea's
/// registry holder: an entry it fails to deserialize is logged and skipped
/// (`RegistryType::append_nbt`), which shifts the protocol id of every entry
/// after it. Claiming the pack is only safe while this holds.
#[test]
fn embedded_known_pack_elements_deserialize_through_azalea() {
    for (registry, elements) in pomme_protocol::KnownPackTable::native().registries() {
        let holder = crate::net::known_packs::filled_holder(registry);
        let kept = match registry {
            "dimension_type" => holder.dimension_type.map.len(),
            "enchantment" => holder.enchantment.map.len(),
            _ => holder.extra.values().map(|r| r.map.len()).sum(),
        };
        assert_eq!(kept, elements.count(), "{registry}: entries were dropped");

        // The values have to survive the JSON -> NBT conversion too, not just
        // the deserialize: `dimension_info` reads these two off the holder.
        if registry == "dimension_type" {
            let overworld = &holder.dimension_type.map[&Identifier::new("minecraft:overworld")];
            assert_eq!(overworld.height, 384);
            assert_eq!(overworld.min_y, -64);
        }
    }
}

/// The reply's fields go out as namespace, id, version (`KnownPack`'s stream
/// codec), and pomme claims vanilla's core pack at the native version.
#[test]
fn select_known_packs_reply_is_byte_exact() {
    use azalea_buf::AzBuf;
    use azalea_protocol::packets::config::s_select_known_packs::{
        KnownPack, ServerboundSelectKnownPacks,
    };

    let core_of = |namespace: &str| KnownPack {
        namespace: namespace.to_owned(),
        id: "core".to_owned(),
        version: pomme_protocol::version::NATIVE.name.to_owned(),
    };
    let offered = vec![core_of("minecraft"), core_of("pomme")];
    let known_packs = crate::net::known_packs::select_packs(&offered);
    assert_eq!(known_packs, offered[..1]);

    let mut bytes = Vec::new();
    ServerboundSelectKnownPacks { known_packs }
        .azalea_write(&mut bytes)
        .unwrap();
    let mut expected = vec![1]; // one pack
    for field in ["minecraft", "core", pomme_protocol::version::NATIVE.name] {
        expected.push(field.len() as u8);
        expected.extend_from_slice(field.as_bytes());
    }
    assert_eq!(bytes, expected);
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
    use azalea_core::delta::PositionDelta8;
    use azalea_protocol::packets::game::c_move_entity_pos::ClientboundMoveEntityPos;
    use azalea_protocol::packets::game::c_move_entity_pos_rot::{
        ClientboundMoveEntityPosRot, CompactLookDirection,
    };
    use azalea_protocol::packets::game::c_move_entity_rot::ClientboundMoveEntityRot;

    let entity_id = MinecraftEntityId(7);
    let delta = PositionDelta8 {
        xa: 100,
        ya: -200,
        za: 300,
    };
    let look_direction = CompactLookDirection {
        y_rot: 10,
        x_rot: -20,
    };
    let native = |packet| azalea_protocol::write::serialize_packet(&packet).unwrap();
    let translate = |old: Vec<u8>| {
        translation_for(777)
            .translate_game_frame(old.into_boxed_slice())
            .unwrap()
    };
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

    let pos = native(ClientboundGamePacket::MoveEntityPos(
        ClientboundMoveEntityPos {
            entity_id,
            delta,
            on_ground: true,
        },
    ));
    let mut old = head("move_entity_pos", 1);
    shorts(&mut old, [100, -200, 300]);
    assert_eq!(&translate(old)[..], &pos[..]);

    // Two steps (properties = 2 << 1 | onGround) summing to the same delta.
    let mut old = head("move_entity_pos", (2 << 1) | 1);
    for (ticks, d) in [(1, [40, -50, 100]), (2, [60, -150, 200])] {
        wire::write_varint(&mut old, ticks);
        shorts(&mut old, d);
    }
    assert_eq!(&translate(old)[..], &pos[..]);

    let pos_rot = native(ClientboundGamePacket::MoveEntityPosRot(
        ClientboundMoveEntityPosRot {
            entity_id,
            delta,
            look_direction,
            on_ground: false,
        },
    ));
    let mut old = head("move_entity_pos_rot", 0);
    shorts(&mut old, [100, -200, 300]);
    old.extend_from_slice(&[10, (-20i8) as u8]);
    assert_eq!(&translate(old)[..], &pos_rot[..]);

    let rot = native(ClientboundGamePacket::MoveEntityRot(
        ClientboundMoveEntityRot {
            entity_id,
            look_direction,
            on_ground: true,
        },
    ));
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(777, Direction::Clientbound, "move_entity_rot"),
    );
    wire::write_varint(&mut old, 7);
    old.extend_from_slice(&[1, 10, (-20i8) as u8]);
    assert_eq!(&translate(old)[..], &rot[..]);

    // A summed displacement past the short range saturates.
    let mut old = head("move_entity_pos", 2 << 1);
    for _ in 0..2 {
        wire::write_varint(&mut old, 1);
        shorts(&mut old, [30000, 0, -30000]);
    }
    let ClientboundGamePacket::MoveEntityPos(p) = translate_and_decode(777, old) else {
        panic!("wrong packet");
    };
    assert_eq!(
        (p.delta.xa, p.delta.ya, p.delta.za),
        (i16::MAX, 0, i16::MIN)
    );
}

/// 26.3's `entity_position_sync` carries a `PositionPath` (`PositionPath`
/// in the 26.3 reference: a tagged `Vec3`, or a counted `Vec3` + tick offset
/// list ending at the last step) and no delta.
#[test]
fn translate_entity_position_sync_777() {
    use azalea_core::position::Vec3;
    use azalea_entity::LookDirection;
    use azalea_protocol::common::movements::PositionMoveRotation;
    use azalea_protocol::packets::game::c_entity_position_sync::ClientboundEntityPositionSync;

    let packet = ClientboundGamePacket::EntityPositionSync(ClientboundEntityPositionSync {
        id: MinecraftEntityId(9),
        values: PositionMoveRotation {
            pos: Vec3::new(1.0, 65.0, -2.0),
            delta: Vec3::default(),
            look_direction: LookDirection::new(90.0, -10.0),
        },
        on_ground: true,
    });
    let frame = azalea_protocol::write::serialize_packet(&packet).unwrap();
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
        let translated = translation_for(777)
            .translate_game_frame(old.into_boxed_slice())
            .unwrap();
        assert_eq!(&translated[..], &frame[..]);
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
    use azalea_core::game_type::{GameMode, OptionalGameType};
    use azalea_protocol::packets::common::CommonPlayerSpawnInfo;
    use azalea_protocol::packets::game::c_login::ClientboundLogin;
    use azalea_protocol::packets::game::c_respawn::ClientboundRespawn;
    use azalea_registry::DataRegistry;

    let spawn_info = |previous| CommonPlayerSpawnInfo {
        dimension_type: azalea_registry::data::DimensionKind::new_raw(0),
        dimension: "minecraft:overworld".into(),
        seed: 42,
        game_type: GameMode::Survival,
        previous_game_type: OptionalGameType(previous),
        is_debug: false,
        is_flat: false,
        last_death_location: None,
        portal_cooldown: 0,
        sea_level: 63,
    };
    // (previous game type, its 26.2 byte, its 26.3 byte)
    let cases = [
        (None, 0xFF, 0),
        (Some(GameMode::Survival), 0, 1),
        (Some(GameMode::Creative), 1, 2),
    ];
    for (previous, native_byte, wire_byte) in cases {
        let login = ClientboundGamePacket::Login(ClientboundLogin {
            player_id: MinecraftEntityId(7),
            hardcore: false,
            levels: vec!["minecraft:overworld".into()],
            max_players: 20,
            chunk_radius: 12,
            simulation_distance: 10,
            reduced_debug_info: false,
            show_death_screen: true,
            do_limited_crafting: false,
            common: spawn_info(previous),
            online_mode: false,
            enforces_secure_chat: true,
        });
        let respawn = ClientboundGamePacket::Respawn(ClientboundRespawn {
            common: spawn_info(previous),
            data_to_keep: 3,
        });
        // The byte precedes debug/flat/death/portal/sea level and the
        // packet's own trailer (two bools, or the data-to-keep byte).
        for (packet, name, from_end) in [(login, "login", 8), (respawn, "respawn", 7)] {
            let frame = azalea_protocol::write::serialize_packet(&packet).unwrap();
            let at = frame.len() - from_end;
            assert_eq!(frame[at], native_byte, "{name}");
            let mut old = frame.to_vec();
            old[at] = wire_byte;
            old[0] = old_id(777, Direction::Clientbound, name) as u8;
            let translated = translation_for(777)
                .translate_game_frame(old.into_boxed_slice())
                .unwrap();
            assert_eq!(&translated[..], &frame[..], "{name}");
        }
    }
}

/// 26.3 renumbered `animate` (wake up 0, critical hit 1, magic critical hit
/// 2) and moved the swings to `swing_animation`, rewritten back onto
/// `animate` by hand; a `none` animation drops.
#[test]
fn translate_animate_777() {
    use azalea_protocol::packets::game::c_animate::{AnimationAction, ClientboundAnimate};

    let native = |action| {
        azalea_protocol::write::serialize_packet(&ClientboundGamePacket::Animate(
            ClientboundAnimate {
                id: MinecraftEntityId(7),
                action,
            },
        ))
        .unwrap()
    };
    let t = translation_for(777);
    for (wire, action) in [
        (0, AnimationAction::WakeUp),
        (1, AnimationAction::CriticalHit),
        (2, AnimationAction::MagicCriticalHit),
    ] {
        let mut old = Vec::new();
        wire::write_varint(&mut old, old_id(777, Direction::Clientbound, "animate"));
        wire::write_varint(&mut old, 7);
        old.push(wire);
        let translated = t.translate_game_frame(old.into_boxed_slice()).unwrap();
        assert_eq!(&translated[..], &native(action)[..]);
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
        old.into_boxed_slice()
    };
    for (hand, action) in [
        (0, AnimationAction::SwingMainHand),
        (1, AnimationAction::SwingOffHand),
    ] {
        let translated = t.translate_game_frame(swing(hand, 1)).unwrap();
        assert_eq!(&translated[..], &native(action)[..]);
    }
    assert!(t.translate_game_frame(swing(0, 0)).is_none());
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
    // as the NBT string "hi"; azalea's decoder sizes the value.
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

/// 26.3's `update_advancements` sends each advancement's screen position
/// after the entry rather than inside its `DisplayInfo`, and sends one even
/// without a display.
#[test]
fn translate_update_advancements_777() {
    use azalea_buf::AzBuf;
    use azalea_inventory::{ItemStack, ItemStackData};
    use azalea_protocol::packets::game::c_update_advancements::{
        Advancement, AdvancementHolder, ClientboundUpdateAdvancements, DisplayInfo, FrameType,
    };

    let displayed = AdvancementHolder {
        id: "minecraft:story/root".into(),
        value: Advancement {
            parent_id: None,
            display: Some(Box::new(DisplayInfo {
                title: azalea_chat::FormattedText::default(),
                description: azalea_chat::FormattedText::default(),
                icon: ItemStack::Present(ItemStackData {
                    count: 1,
                    kind: azalea_registry::builtin::ItemKind::Stone,
                    component_patch: Default::default(),
                }),
                frame: FrameType::Task,
                show_toast: true,
                hidden: false,
                background: Some(
                    "minecraft:textures/gui/advancements/backgrounds/stone.png".into(),
                ),
                x: 1.5,
                y: -2.0,
            })),
            requirements: vec![vec!["a".into()]],
            sends_telemetry_event: false,
        },
    };
    let bare = AdvancementHolder {
        id: "minecraft:story/mine_stone".into(),
        value: Advancement {
            parent_id: Some("minecraft:story/root".into()),
            display: None,
            requirements: vec![],
            sends_telemetry_event: true,
        },
    };
    let packet = ClientboundGamePacket::UpdateAdvancements(ClientboundUpdateAdvancements {
        reset: true,
        added: vec![displayed.clone(), bare.clone()],
        removed: vec![],
        progress: Default::default(),
        show_advancements: true,
    });
    let frame = azalea_protocol::write::serialize_packet(&packet).unwrap();

    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(777, Direction::Clientbound, "update_advancements"),
    );
    old.push(1); // reset
    wire::write_varint(&mut old, 2);
    // The displayed holder ends in its one requirement (4 bytes) and the
    // telemetry bool; the position pair precedes those in the native form.
    let mut holder = Vec::new();
    displayed.azalea_write(&mut holder).unwrap();
    let (body, tail) = holder.split_at(holder.len() - 5);
    let (display, position) = body.split_at(body.len() - 8);
    old.extend_from_slice(display);
    old.extend_from_slice(tail);
    old.extend_from_slice(position);
    bare.azalea_write(&mut old).unwrap();
    old.extend_from_slice(&[0; 8]); // a position without a display
    old.extend_from_slice(&[0, 0, 1]); // no removals, no progress, show

    let translated = translation_for(777)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    assert_eq!(&translated[..], &frame[..]);
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
/// `punch`; `player_action` actions after the first shift up by one; and
/// `accept_teleportation` takes the pose of the `move_player_pos_rot`
/// following it, which 26.3 no longer sends.
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
    let ClientboundGamePacket::LightUpdate(p) = translate_and_decode(
        777,
        update(old_id(777, Direction::Clientbound, "light_update"), true),
    ) else {
        panic!("wrong packet");
    };
    assert_eq!(p.light_data.sky_updates.len(), 1);

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
    let ClientboundGamePacket::PlayerChat(_) = translate_and_decode(
        777,
        chat(
            old_id(777, Direction::Clientbound, "player_chat"),
            &[2, 5, 1],
        ),
    ) else {
        panic!("wrong packet");
    };
}

/// A 26.3 component whose value layout changed (`pot_decorations` became
/// four optional item templates) can't be sized with 26.2's codec, so the
/// particle carrying it drops rather than misparsing.
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
    assert!(
        translation_for(777)
            .translate_game_frame(old.into_boxed_slice())
            .is_none()
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
    let ClientboundGamePacket::Commands(p) = translate_and_decode(777, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.entries.len(), 5);
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
        .translate_game_frame(old.clone().into_boxed_slice())
        .unwrap();
    let expected = ghost(
        table_id(Phase::Game, Direction::Clientbound, "place_ghost_recipe"),
        &[slot("composite"), 2, slot("item"), 5, slot("item"), 7],
        &named(&[slot("tag")]),
    );
    assert_eq!(&translated[..], &expected[..]);
    let ClientboundGamePacket::PlaceGhostRecipe(_) = translate_and_decode(777, old) else {
        panic!("wrong packet");
    };
}

/// The level-load gate's `LEVEL_CHUNKS_LOAD_START` (game event 13, unchanged
/// in 26.3's `ClientboundGameEventPacket`) reaches the handler on a 777 wire.
#[test]
fn translate_game_event_777() {
    use azalea_protocol::packets::game::c_game_event::EventType;

    let mut old = Vec::new();
    wire::write_varint(&mut old, old_id(777, Direction::Clientbound, "game_event"));
    old.push(13);
    old.extend_from_slice(&0f32.to_be_bytes());
    let ClientboundGamePacket::GameEvent(p) = translate_and_decode(777, old) else {
        panic!("wrong packet");
    };
    assert_eq!(p.event, EventType::WaitForLevelChunks);
}

/// The dialog packets pomme dispatches on, in both phases.
fn dialog_packet_ids_match_azalea() {
    use azalea_protocol::packets::config::{
        ClientboundConfigPacket, ServerboundConfigPacket, c_clear_dialog as cfg_clear,
        c_server_links as cfg_links, c_show_dialog as cfg_show, s_custom_click_action as cfg_click,
    };
    use azalea_protocol::packets::game::{
        c_clear_dialog, c_server_links, c_show_dialog, s_custom_click_action,
    };

    let id = || Identifier::new("minecraft:test");
    let game = [
        (
            ClientboundGamePacket::ServerLinks(c_server_links::ClientboundServerLinks {
                links: Vec::new(),
            })
            .id(),
            "server_links",
        ),
        (
            ClientboundGamePacket::ClearDialog(c_clear_dialog::ClientboundClearDialog).id(),
            "clear_dialog",
        ),
        (
            ClientboundGamePacket::ShowDialog(c_show_dialog::ClientboundShowDialog {
                dialog: Holder::Direct(simdnbt::owned::Nbt::None),
            })
            .id(),
            "show_dialog",
        ),
    ];
    for (azalea, name) in game {
        assert_eq!(
            azalea,
            table_id(Phase::Game, Direction::Clientbound, name),
            "{name}"
        );
    }
    let click = ServerboundGamePacket::CustomClickAction(
        s_custom_click_action::ServerboundCustomClickAction {
            id: id(),
            payload: simdnbt::owned::Nbt::None,
        },
    );
    assert_eq!(
        click.id(),
        table_id(Phase::Game, Direction::Serverbound, "custom_click_action")
    );

    let config = [
        (
            ClientboundConfigPacket::ServerLinks(cfg_links::ClientboundServerLinks {
                links: Vec::new(),
            })
            .id(),
            "server_links",
        ),
        (
            ClientboundConfigPacket::ClearDialog(cfg_clear::ClientboundClearDialog).id(),
            "clear_dialog",
        ),
        (
            ClientboundConfigPacket::ShowDialog(cfg_show::ClientboundShowDialog {
                dialog: simdnbt::owned::Nbt::None,
            })
            .id(),
            "show_dialog",
        ),
    ];
    for (azalea, name) in config {
        assert_eq!(
            azalea,
            table_id(Phase::Configuration, Direction::Clientbound, name),
            "{name}"
        );
    }
    let click =
        ServerboundConfigPacket::CustomClickAction(cfg_click::ServerboundCustomClickAction {
            id: id(),
            payload: simdnbt::owned::Nbt::None,
        });
    assert_eq!(
        click.id(),
        table_id(
            Phase::Configuration,
            Direction::Serverbound,
            "custom_click_action"
        )
    );
}
