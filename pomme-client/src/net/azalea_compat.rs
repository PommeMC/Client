//! Cross-checks of pomme-protocol's vanilla-derived table and encoders
//! against azalea (kept here so pomme-protocol stays azalea-free). On a
//! disagreement the table generated from the decompiled reference is
//! authoritative — azalea's own tables can lag (its 26.2 `Particle` enum is
//! out of sync, see `handler::handle_raw_game_packet`) — so a failure means
//! "investigate which side is wrong", with in-game behavior as tiebreaker.

use azalea_core::entity_id::MinecraftEntityId;
use azalea_core::sound::CustomSound;
use azalea_protocol::packets::ProtocolPacket;
use azalea_protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket};
use azalea_registry::Holder;
use azalea_registry::builtin::SoundEvent;
use azalea_registry::identifier::Identifier;
use glam::DVec3;
use pomme_protocol::chunk::{self, ChunkPacket};
use pomme_protocol::container::{self, ContainerPacket};
use pomme_protocol::hud::{self, BossFlags, BossOp, HudPacket};
use pomme_protocol::lifecycle::{self, LifecyclePacket};
use pomme_protocol::packets::{Direction, PacketTable, Phase};
use pomme_protocol::player_info::{self, PlayerInfoPacket};
use pomme_protocol::scoreboard::{self, NumberFormat, ObjectiveMethod, ScoreboardPacket};
use pomme_protocol::sound::{self, SoundPacket};
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

/// pomme's `EntityTypes` size table against azalea's, for every entity type.
/// azalea rounds its generated sizes to five decimals (the donkey's vanilla
/// `1.3964844f` is its `1.39648`), so they agree to that.
#[test]
fn entity_dimensions_match_azalea() {
    use azalea_entity::dimensions::EntityDimensions as AzaleaDimensions;
    use azalea_registry::Registry;
    use azalea_registry::builtin::EntityKind;

    use crate::entity::dimensions::EntityDimensions;

    let mut checked = 0;
    for kind in (0..).map_while(EntityKind::from_u32) {
        let pomme = EntityDimensions::of(kind);
        let azalea = AzaleaDimensions::from(kind);
        assert!(
            (pomme.width - azalea.width).abs() < 1e-5
                && (pomme.height - azalea.height).abs() < 1e-5,
            "{kind}: {pomme:?} vs {} x {}",
            azalea.width,
            azalea.height
        );
        checked += 1;
    }
    assert!(checked > 150, "{checked} entity types");
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

    let bundle_selected = ServerboundGamePacket::BundleItemSelected(
        azalea_protocol::packets::game::s_bundle_item_selected::ServerboundBundleItemSelected {
            slot_id: 0,
            selected_item_index: 0,
        },
    );
    assert_eq!(
        bundle_selected.id(),
        table_id(Phase::Game, Direction::Serverbound, "bundle_item_selected")
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

    let team = ClientboundGamePacket::SetPlayerTeam(
        azalea_protocol::packets::game::c_set_player_team::ClientboundSetPlayerTeam {
            name: String::new(),
            method: azalea_protocol::packets::game::c_set_player_team::Method::Remove,
        },
    );
    assert_eq!(
        team.id(),
        table_id(Phase::Game, Direction::Clientbound, "set_player_team")
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

    recipe_packet_ids_match_azalea();
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

/// Translates a hand-built old-version game frame and decodes the result
/// with pomme's `read` for packet `name`, as the raw chain does.
fn translate_and_read<P>(protocol: i32, old: Vec<u8>, name: &str, read: RawRead<P>) -> P {
    let translated = translation_for(protocol)
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    read_native_frame(&translated, name, read)
}

/// A pomme-protocol decoder (`chunk::read`, `block::read`, …).
type RawRead<P> =
    fn(&str, &mut pomme_protocol::codec::Reader) -> Option<pomme_protocol::codec::DecodeResult<P>>;

/// A native 26.2 game frame for packet `name`, decoded with `read`.
fn read_native_frame<P>(frame: &[u8], name: &str, read: RawRead<P>) -> P {
    let mut r = pomme_protocol::codec::Reader::new(frame);
    assert_eq!(
        r.varint(),
        Ok(table_id(Phase::Game, Direction::Clientbound, name))
    );
    let decoded = read(name, &mut r).unwrap().unwrap();
    r.ensure_eof().unwrap();
    decoded
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
/// collision, color, options` and changed color to `Optional<TeamColor>`.
/// Pomme owns the native team decoder, so translation targets the real 26.2
/// layout rather than Azalea's stale packet type.
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

/// A 26.1 `add_entity`'s type id comes out as the wrong kind in 26.2's
/// numbering (ids past the sulfur_cube insertion shift by one); the remap
/// the raw path applies fixes it.
#[test]
fn remap_add_entity_26_1() {
    use azalea_registry::Registry;
    use azalea_registry::builtin::EntityKind;

    // 26.1 tadpole is id 130, which 26.2 numbers as sulfur_cube.
    assert_eq!(EntityKind::from_u32(130), Some(EntityKind::SulfurCube));
    let native =
        translation_for(775).remap_to_native(pomme_protocol::ClientRegistry::EntityType, 130);
    assert_eq!(
        native.and_then(EntityKind::from_u32),
        Some(EntityKind::Tadpole)
    );
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

    let ChunkPacket::Level { x, z, sections, .. } =
        translate_and_read(774, old, "level_chunk_with_light", chunk::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!((x, z), (3, -2));
    assert_eq!(sections[..], [0, 1, 0, 0, 0, 5, 0, 0]);
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

/// Pins the azalea bug the raw explode path in
/// `handler::handle_raw_game_packet` works around: a native 26.2 explosion with
/// vanilla's default block particles (`Level.
/// DEFAULT_EXPLOSION_BLOCK_PARTICLES`) doesn't decode faithfully through
/// azalea's typed codec. When it does, the raw path can go.
#[test]
fn azalea_explode_still_misdecodes_native_particles() {
    use azalea_entity::particle::Particle;
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let table = RegistryTable::native();
    let particle = |name| registry_id(table, ClientRegistry::ParticleType, name);

    let mut raw = Vec::new();
    wire::write_varint(
        &mut raw,
        table_id(Phase::Game, Direction::Clientbound, "explode"),
    );
    for c in [1.0f64, 65.0, -2.0] {
        raw.extend_from_slice(&c.to_be_bytes());
    }
    raw.extend_from_slice(&4.0f32.to_be_bytes()); // radius
    raw.extend_from_slice(&7i32.to_be_bytes()); // block count
    raw.push(0); // no knockback
    wire::write_varint(&mut raw, particle("explosion_emitter"));
    wire::write_varint(
        &mut raw,
        registry_id(table, ClientRegistry::SoundEvent, "entity.generic.explode") + 1,
    );
    wire::write_varint(&mut raw, 2);
    for (name, scaling) in [("poof", 0.5f32), ("smoke", 1.0)] {
        wire::write_varint(&mut raw, particle(name));
        raw.extend_from_slice(&scaling.to_be_bytes());
        raw.extend_from_slice(&1.0f32.to_be_bytes()); // speed
        wire::write_varint(&mut raw, 1); // weight
    }

    let decoded = azalea_protocol::read::deserialize_packet::<ClientboundGamePacket>(
        &mut std::io::Cursor::new(&raw[..]),
    );
    let faithful = matches!(
        &decoded,
        Ok(ClientboundGamePacket::Explode(p))
            if p.explosion_particle == Particle::ExplosionEmitter
                && p.block_particles.len() == 2
                && p.block_particles[0].value.particle == Particle::Poof
                && p.block_particles[1].value.particle == Particle::Smoke
    );
    assert!(
        !faithful,
        "azalea now decodes native explosions: {decoded:?}"
    );
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

    let ChunkPacket::Level {
        x,
        z,
        heightmaps,
        sections,
        ..
    } = translate_and_read(769, old, "level_chunk_with_light", chunk::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!((x, z), (3, -2));
    assert_eq!(heightmaps.len(), 1);
    assert_eq!(&*heightmaps[0].1, &[7u64]);
    assert_eq!(sections[..], [0, 1, 0, 0, 0, 5, 0, 0]);
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
            items: vec![pomme_stack(azalea_registry::builtin::ItemKind::Stone, 3)],
            carried: None,
        }
    );
}

/// `count` of `kind` with no components, as pomme decodes it.
fn pomme_stack(
    kind: azalea_registry::builtin::ItemKind,
    count: i32,
) -> Option<pomme_protocol::item::ItemStack> {
    use azalea_registry::Registry;

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
fn native_item(protocol: i32, id: u32) -> Option<azalea_registry::builtin::ItemKind> {
    use azalea_registry::Registry;

    let native =
        translation_for(protocol).remap_to_native(pomme_protocol::ClientRegistry::Item, id);
    native.and_then(azalea_registry::builtin::ItemKind::from_u32)
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
    use azalea_registry::Registry;
    use azalea_registry::builtin::MenuKind;
    use pomme_protocol::{ClientRegistry, RegistryTable};

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

/// Translates a `set_entity_data` frame for entity 9 carrying `values` (the
/// entry list without its terminator) and returns the decoded entries.
fn entity_data_entries(protocol: i32, values: &[u8]) -> Vec<azalea_entity::EntityDataItem> {
    let mut old = Vec::new();
    wire::write_varint(
        &mut old,
        old_id(protocol, Direction::Clientbound, "set_entity_data"),
    );
    wire::write_varint(&mut old, 9); // entity id
    old.extend_from_slice(values);
    old.push(0xFF);
    let ClientboundGamePacket::SetEntityData(p) = translate_decode_and_remap(protocol, old) else {
        panic!("wrong packet");
    };
    p.packed_items.0
}

fn entity_data_stack(entry: &azalea_entity::EntityDataItem) -> &azalea_inventory::ItemStack {
    let azalea_entity::EntityDataValue::ItemStack(stack) = &entry.value else {
        panic!("wrong value");
    };
    stack
}

fn entity_data_item(entry: &azalea_entity::EntityDataItem) -> azalea_registry::builtin::ItemKind {
    let azalea_inventory::ItemStack::Present(data) = entity_data_stack(entry) else {
        panic!("empty stack");
    };
    data.kind
}

/// `translate_entity_data` already remaps item stacks, so `remap_inbound` must
/// not remap them again: 26.3 `oak_log` (163) lands as native `oak_log` (161).
#[test]
fn entity_data_item_remaps_once_777() {
    // index 8, item_stack serializer, count 1, 26.3 oak_log (varint 163),
    // empty patch
    let entries = entity_data_entries(777, &[8, 7, 1, 0xA3, 1, 0, 0]);
    assert_eq!(
        entity_data_item(&entries[0]),
        azalea_registry::builtin::ItemKind::OakLog
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
        azalea_registry::builtin::ItemKind::OakLog
    );
}

/// An item 26.2 lacks (26.3 `poplar_log`, 172) empties the stack; a walkable
/// one keeps the entries after it.
#[test]
fn entity_data_unmapped_item_empties_777() {
    // poplar_log (varint 172), empty patch; then index 9, int serializer, 7
    let entries = entity_data_entries(777, &[8, 7, 1, 0xAC, 1, 0, 0, 9, 1, 7]);
    assert!(matches!(
        entity_data_stack(&entries[0]),
        azalea_inventory::ItemStack::Empty
    ));
    assert_eq!(entries.len(), 2);
}

/// On the verbatim-tail fallback an unmapped item can't be skipped, so the
/// stack empties and the list ends there.
#[test]
fn entity_data_unmapped_item_with_components_ends_list_777() {
    let entries = entity_data_entries(777, &[8, 7, 1, 0xAC, 1, 1, 0, 3, 5, 9, 1, 7]);
    assert!(matches!(
        entity_data_stack(&entries[0]),
        azalea_inventory::ItemStack::Empty
    ));
    assert_eq!(entries.len(), 1);
}

/// `translate_item_765` leaves the wire item id for the typed pass, so
/// `remap_inbound` must still remap `set_entity_data` stacks on 765 wires.
#[test]
fn entity_data_item_remaps_765() {
    // index 8, item_stack serializer, present, 765 item id, count 1, empty NBT
    let entries = entity_data_entries(765, &[8, 7, 1, SHIFTED_ITEM, 1, 0x0A, 0x00]);
    assert_eq!(entity_data_item(&entries[0]), SHIFTED_ITEM_KIND);
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
            item: pomme_stack(azalea_registry::builtin::ItemKind::Stone, 2)
        }
    );
    assert_eq!(
        read("set_player_inventory", build(-2, 7)),
        ContainerPacket::PlayerInventory {
            slot: 7,
            item: pomme_stack(azalea_registry::builtin::ItemKind::Stone, 2)
        }
    );
    assert_eq!(
        read("container_set_slot", build(3, 1)),
        ContainerPacket::Slot {
            container_id: 3,
            state_id: 2,
            slot: 1,
            item: pomme_stack(azalea_registry::builtin::ItemKind::Stone, 2)
        }
    );
}

/// 765's single-NBT `registry_data` splits into per-registry frames whose
/// entries reorder by their explicit ids (wire order is the id space); the
/// captured dimension-type order then keys the 765 spawn-info rewrites,
/// whose dimension type is a resource key string.
#[test]
fn translate_config_registry_data_765() {
    use azalea_buf::AzBuf;
    use azalea_protocol::packets::config::ClientboundConfigPacket;
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

    let LifecyclePacket::Respawn {
        spawn,
        data_to_keep,
    } = translate_and_read(765, old, "respawn", lifecycle::read)
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
    assert!(crate::net::handler::handle_raw_for_test(&translated, &tx).0);
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
    let LifecyclePacket::Login(p) = translate_and_read(763, old, "login", lifecycle::read) else {
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
        native.and_then(
            <azalea_registry::builtin::EntityKind as azalea_registry::Registry>::from_u32
        ),
        Some(azalea_registry::builtin::EntityKind::Player)
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
            items: vec![pomme_stack(azalea_registry::builtin::ItemKind::Stone, 3)],
            carried: None,
        }
    );
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

    let ChunkPacket::Level {
        x,
        z,
        heightmaps,
        block_entities,
        ..
    } = translate_and_read(763, old, "level_chunk_with_light", chunk::read)
    else {
        panic!("wrong packet");
    };
    assert_eq!((x, z), (3, -2));
    assert_eq!(heightmaps.len(), 1);
    assert_eq!(&*heightmaps[0].1, &[7u64]);
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

/// A survival overworld spawn (dimension type 0, seed 42, sea level 63) after
/// `previous`, as azalea encodes it.
fn azalea_spawn_info(
    previous: Option<azalea_core::game_type::GameMode>,
) -> azalea_protocol::packets::common::CommonPlayerSpawnInfo {
    use azalea_registry::DataRegistry;

    azalea_protocol::packets::common::CommonPlayerSpawnInfo {
        dimension_type: azalea_registry::data::DimensionKind::new_raw(0),
        dimension: "minecraft:overworld".into(),
        seed: 42,
        game_type: azalea_core::game_type::GameMode::Survival,
        previous_game_type: azalea_core::game_type::OptionalGameType(previous),
        is_debug: false,
        is_flat: false,
        last_death_location: None,
        portal_cooldown: 0,
        sea_level: 63,
    }
}

/// 26.3's `CommonPlayerSpawnInfo` sends `previousGameType` as an optional
/// varint (0 = none, else id + 1) where 26.2 reads a signed byte; it sits
/// a fixed distance from the end of `login` and `respawn`.
#[test]
fn translate_spawn_info_777() {
    use azalea_core::game_type::GameMode;
    use azalea_protocol::packets::game::c_login::ClientboundLogin;
    use azalea_protocol::packets::game::c_respawn::ClientboundRespawn;

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
            common: azalea_spawn_info(previous),
            online_mode: false,
            enforces_secure_chat: true,
        });
        let respawn = ClientboundGamePacket::Respawn(ClientboundRespawn {
            common: azalea_spawn_info(previous),
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
/// `net::recipe_wire` and `wire.rs` decode and encode these themselves; only
/// the ids are shared. azalea's place-recipe and seen-recipe packets still
/// carry a resource location where 26.2 sends a `RecipeDisplayId`, so only the
/// settings encoder round-trips through azalea.
fn recipe_packet_ids_match_azalea() {
    use azalea_buf::{AzBuf, AzBufVar};
    use azalea_protocol::common::recipe::{
        RecipeDisplayData, SlotDisplayData, StonecutterRecipeDisplay,
    };
    use azalea_protocol::packets::game::{
        c_place_ghost_recipe, c_recipe_book_remove, c_recipe_book_settings, c_update_recipes,
        s_place_recipe, s_recipe_book_change_settings, s_recipe_book_seen_recipe,
    };

    let clientbound = [
        (
            ClientboundGamePacket::RecipeBookRemove(
                c_recipe_book_remove::ClientboundRecipeBookRemove {
                    recipes: Vec::new(),
                },
            ),
            "recipe_book_remove",
        ),
        (
            ClientboundGamePacket::RecipeBookSettings(
                c_recipe_book_settings::ClientboundRecipeBookSettings {
                    book_settings: c_recipe_book_settings::RecipeBookSettings {
                        gui_open: false,
                        filtering_craftable: false,
                        furnace_gui_open: false,
                        furnace_filtering_craftable: false,
                        blast_furnace_gui_open: false,
                        blast_furnace_filtering_craftable: false,
                        smoker_gui_open: false,
                        smoker_filtering_craftable: false,
                    },
                },
            ),
            "recipe_book_settings",
        ),
        (
            ClientboundGamePacket::UpdateRecipes(c_update_recipes::ClientboundUpdateRecipes {
                item_sets: Default::default(),
                stonecutter_recipes: Vec::new(),
            }),
            "update_recipes",
        ),
        (
            ClientboundGamePacket::PlaceGhostRecipe(
                c_place_ghost_recipe::ClientboundPlaceGhostRecipe {
                    container_id: 0,
                    recipe: RecipeDisplayData::Stonecutter(StonecutterRecipeDisplay {
                        input: SlotDisplayData::Empty,
                        result: SlotDisplayData::Empty,
                        crafting_station: SlotDisplayData::Empty,
                    }),
                },
            ),
            "place_ghost_recipe",
        ),
    ];
    for (packet, name) in clientbound {
        assert_eq!(
            packet.id(),
            table_id(Phase::Game, Direction::Clientbound, name),
            "{name}"
        );
    }

    let recipe: Identifier = "minecraft:stick".into();
    let place = ServerboundGamePacket::PlaceRecipe(s_place_recipe::ServerboundPlaceRecipe {
        container_id: 0,
        recipe: recipe.clone(),
        shift_down: false,
    });
    assert_eq!(
        place.id(),
        table_id(Phase::Game, Direction::Serverbound, "place_recipe")
    );
    let seen = ServerboundGamePacket::RecipeBookSeenRecipe(
        s_recipe_book_seen_recipe::ServerboundRecipeBookSeenRecipe { recipe },
    );
    assert_eq!(
        seen.id(),
        table_id(
            Phase::Game,
            Direction::Serverbound,
            "recipe_book_seen_recipe"
        )
    );

    let encoded = pomme_protocol::wire::encode_recipe_book_change_settings(
        pomme_protocol::wire::RecipeBookType::BlastFurnace,
        true,
        false,
    );
    let mut cur = std::io::Cursor::new(encoded.as_slice());
    assert_eq!(
        u32::azalea_read_var(&mut cur).unwrap(),
        table_id(
            Phase::Game,
            Direction::Serverbound,
            "recipe_book_change_settings"
        )
    );
    let decoded =
        s_recipe_book_change_settings::ServerboundRecipeBookChangeSettings::azalea_read(&mut cur)
            .unwrap();
    assert_eq!(
        decoded,
        s_recipe_book_change_settings::ServerboundRecipeBookChangeSettings {
            book_type: s_recipe_book_change_settings::RecipeBookType::BlastFurnace,
            is_open: true,
            is_filtering: false,
        }
    );
    assert_eq!(cur.position() as usize, encoded.len());
}

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

/// A sample value of `shape`: non-empty lists, present optionals (to a
/// depth), holders inline or by reference, and the first dispatch arm.
fn sample_component(
    shape: &pomme_protocol::item::Shape,
    inline: bool,
    depth: u32,
    out: &mut Vec<u8>,
) {
    use pomme_protocol::item::Shape::*;

    let deeper = |shape, out: &mut Vec<u8>| sample_component(shape, inline, depth + 1, out);
    match shape {
        Unit => {}
        Bool => out.push(1),
        Int | Float => out.extend(1i32.to_be_bytes()),
        Long | Double => out.extend(1i64.to_be_bytes()),
        VarInt => out.push(1),
        Utf(_) => out.extend([1, b'a']),
        Uuid => out.extend([0; 16]),
        Nbt => out.extend([8, 0, 1, b'a']),
        Compound => out.extend([10, 0]),
        Optional(inner) => {
            let present = depth < 3;
            out.push(present as u8);
            if present {
                deeper(inner, out);
            }
        }
        List(_, inner) => {
            out.push(1);
            deeper(inner, out);
        }
        Map(key, value) => {
            out.push(1);
            deeper(key, out);
            deeper(value, out);
        }
        Either(left, _) => {
            out.push(1);
            deeper(left, out);
        }
        Struct(fields) => fields.iter().for_each(|f| deeper(f, out)),
        Ref(inner) => deeper(inner, out),
        Holder(direct) => {
            if inline {
                out.push(0);
                deeper(direct, out);
            } else {
                out.push(2);
            }
        }
        HolderSet => out.extend([2, 1]),
        ById(arms) => {
            out.push(arms.len() as u8 - 1);
            deeper(&arms[arms.len() - 1], out);
        }
        ByRegistry(registry, arms) | ByRegistryOr(registry, arms, _) => {
            let id = PacketTable::native().version().protocol;
            let table = pomme_protocol::RegistryTable::for_protocol(id).unwrap();
            let (name, arm) = &arms[0];
            out.push(table.id_of(*registry, name).unwrap() as u8);
            deeper(arm, out);
        }
        // Stone x1 with an empty patch.
        ItemTemplate => out.extend([1, 1, 0, 0]),
        // Damage 1.
        TypedComponent => out.extend([3, 1]),
    }
}

/// Every component pomme hands to azalea must read back to exactly its
/// vanilla span; a shared-layout claim that's wrong fails here.
#[test]
fn azalea_reads_every_kept_component_exactly() {
    use pomme_protocol::ClientRegistry;
    use pomme_protocol::codec::Reader;
    use pomme_protocol::item::ItemCodec;

    use super::item_codec::{DROPPED, transcode_to_azalea};

    // Holders azalea keeps as plain ids, so inline values can't cross.
    const PLAIN_HOLDERS: &[&str] = &["equippable", "consumable", "death_protection", "trim"];
    // Dropped for wrong values rather than a wrong layout.
    const SAME_BYTES: &[&str] = &[
        "block_entity_data",
        "tropical_fish/pattern",
        "rabbit/variant",
    ];

    let codec = ItemCodec::native();
    let names = codec.registries().names(ClientRegistry::DataComponentType);
    for (id, name) in names.iter().enumerate() {
        let name = name.as_str();
        let shape = codec.component_shape(id as u32).unwrap();
        let mut reads = Vec::new();
        for inline in [false, true] {
            let mut value = Vec::new();
            sample_component(shape, inline, 0, &mut value);
            let mut r = Reader::new(&value);
            codec
                .read_component(&mut r, id as u32)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(r.remaining(), 0, "{name} sample");
            if !(inline && PLAIN_HOLDERS.contains(&name)) {
                reads.push(transcode_to_azalea(id as u32, &value).is_some());
            }
        }
        if !DROPPED.contains(&name) {
            assert!(
                reads.iter().all(|&r| r),
                "{name} doesn't read through azalea"
            );
        } else if !SAME_BYTES.contains(&name) {
            assert!(
                !reads.iter().all(|&r| r),
                "{name} reads fine; no need to drop it"
            );
        }
    }
}

/// A game frame: the native packet id, then `payload`.
fn game_frame(name: &str, payload: &[u8]) -> Vec<u8> {
    let mut raw = Vec::new();
    wire::write_varint(
        &mut raw,
        table_id(Phase::Game, Direction::Clientbound, name),
    );
    raw.extend_from_slice(payload);
    raw
}

/// pomme's entity data grammar and azalea's read the same frame identically,
/// for every serializer but the particle ones (azalea's 26.2 `Particle` wire
/// enum is out of sync).
#[test]
fn entity_data_matches_azalea() {
    use azalea_entity::EntityDataValue;
    use pomme_protocol::entity_data::Value;
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let stone = registry_id(RegistryTable::native(), ClientRegistry::Item, "stone") as u8;
    let text: &[u8] = &[8, 0, 2, b'h', b'i'];
    let mut global_pos = vec![1];
    wire::write_utf(&mut global_pos, "minecraft:overworld");
    global_pos.extend([0; 8]);
    let entries: [(u32, Vec<u8>); 41] = [
        (0, vec![5]),
        (1, vec![0xAC, 0x02]),
        (2, vec![0x7F]),
        (3, 1.5f32.to_be_bytes().into()),
        (4, vec![2, b'h', b'i']),
        (5, text.into()),
        (6, [&[1], text].concat()),
        (7, vec![2, stone, 0, 0]),
        (8, vec![1]),
        (9, vec![0; 12]),
        (10, vec![0, 0, 0, 0, 0, 0, 0x10, 0x05]),
        (11, vec![1, 0, 0, 0, 0, 0, 0, 0x10, 0x05]),
        (12, vec![2]),
        (13, [&[1u8][..], &[7; 16]].concat()),
        (14, vec![1]),
        (15, vec![0]),
        (18, vec![1, 2, 3]),
        (19, vec![4]),
        (20, vec![5]),
        (21, vec![0]),
        (22, vec![0]),
        (23, vec![1]),
        (24, vec![0]),
        (25, vec![1]),
        (26, vec![0]),
        (27, vec![0]),
        (28, vec![0]),
        (29, vec![0]),
        (30, vec![1]),
        (31, vec![0]),
        (32, vec![0]),
        (33, global_pos),
        (34, vec![1]),
        (35, vec![1]),
        (36, vec![1]),
        (37, vec![1]),
        (38, vec![1]),
        (39, vec![0; 12]),
        (40, vec![0; 16]),
        (41, vec![0; 8]),
        (42, vec![1]),
    ];
    let mut payload = vec![42];
    for (index, (serializer, value)) in entries.iter().enumerate() {
        payload.push(index as u8);
        wire::write_varint(&mut payload, *serializer);
        payload.extend(value);
    }
    payload.push(0xFF);

    let raw = game_frame("set_entity_data", &payload);
    let mut cur = std::io::Cursor::new(&raw[..]);
    let Ok(ClientboundGamePacket::SetEntityData(azalea)) =
        azalea_protocol::read::deserialize_packet::<ClientboundGamePacket>(&mut cur)
    else {
        panic!("azalea reads the frame");
    };
    assert_eq!(cur.position() as usize, raw.len());

    let mut r = pomme_protocol::codec::Reader::new(&payload[1..]);
    let pomme = pomme_protocol::item::ItemCodec::native()
        .read_entity_data(&mut r)
        .unwrap();
    r.ensure_eof().unwrap();

    assert_eq!(pomme.len(), azalea.packed_items.len());
    for (ours, theirs) in pomme.iter().zip(azalea.packed_items.iter()) {
        assert_eq!(ours.index, theirs.index);
        match (&ours.value, &theirs.value) {
            (Value::Item(Some(stack)), EntityDataValue::ItemStack(az)) => {
                assert_eq!(&crate::net::item_codec::to_azalea(stack), az);
            }
            // azalea re-encodes text as a compound; compare what it says.
            (Value::OptionalComponent(Some(nbt)), EntityDataValue::OptionalFormattedText(az)) => {
                assert_eq!(plain(nbt), az.as_ref().unwrap().to_string());
            }
            (ours, theirs) => assert_eq!(
                Some(ours),
                super::entity_data::from_azalea(theirs)
                    .as_ref()
                    .or(Some(&Value::Other)),
                "{theirs:?}"
            ),
        }
    }
}

/// pomme's `update_advancements` decode agrees with azalea's on every field
/// the client reads.
#[test]
fn advancements_match_azalea() {
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let stone = registry_id(RegistryTable::native(), ClientRegistry::Item, "stone");
    let mut p = vec![1, 1];
    wire::write_utf(&mut p, "minecraft:story/root");
    p.extend([0, 1]);
    p.extend([8, 0, 2, b'h', b'i', 8, 0, 0]);
    wire::write_varint(&mut p, stone);
    p.extend([1, 0, 0, 2]);
    p.extend(3i32.to_be_bytes());
    wire::write_utf(&mut p, "minecraft:block/stone");
    p.extend([0; 8]);
    p.extend([1, 1]);
    wire::write_utf(&mut p, "a");
    p.extend([0, 1]);
    wire::write_utf(&mut p, "minecraft:old");
    p.push(1);
    wire::write_utf(&mut p, "minecraft:story/root");
    p.push(2);
    wire::write_utf(&mut p, "a");
    p.push(1);
    p.extend(5i64.to_be_bytes());
    wire::write_utf(&mut p, "b");
    p.extend([0, 1]);

    let raw = game_frame("update_advancements", &p);
    let Ok(ClientboundGamePacket::UpdateAdvancements(azalea)) =
        azalea_protocol::read::deserialize_packet::<ClientboundGamePacket>(
            &mut std::io::Cursor::new(&raw[..]),
        )
    else {
        panic!("azalea reads the frame");
    };
    let ours = super::advancement_wire::read_update(&p).unwrap();

    assert_eq!(ours.reset, azalea.reset);
    assert_eq!(ours.show_advancements, azalea.show_advancements);
    assert_eq!(ours.removed, ["minecraft:old"]);
    assert_eq!(azalea.removed[0].to_string(), "minecraft:old");
    let (id, data) = &ours.added[0];
    let theirs = &azalea.added[0];
    assert_eq!(*id, theirs.id.to_string());
    assert_eq!(data.requirements, theirs.value.requirements);
    let display = data.display.as_ref().unwrap();
    let their_display = theirs.value.display.as_ref().unwrap();
    assert_eq!(display.show_toast, their_display.show_toast);
    assert!(matches!(
        display.frame,
        crate::ui::toast::AdvancementFrame::Goal
    ));
    assert!(matches!(
        their_display.frame,
        azalea_protocol::packets::game::c_update_advancements::FrameType::Goal
    ));
    let azalea_inventory::ItemStack::Present(icon) = &their_display.icon else {
        panic!("azalea reads the icon");
    };
    assert_eq!(
        display.icon_item.as_deref(),
        Some(&*crate::player::inventory::item_resource_name(icon.kind))
    );
    let (progress_id, criteria) = &ours.progress[0];
    let their_criteria = &azalea.progress[0];
    assert_eq!(
        *progress_id,
        azalea.progress.keys().next().unwrap().to_string()
    );
    for (name, obtained) in criteria {
        assert_eq!(*obtained, their_criteria[name].date.is_some(), "{name}");
    }
}

/// Every entity packet azalea encodes reads back through pomme's layouts
/// with the same values, so the raw path changes nothing for valid input.
#[test]
fn entity_packets_match_azalea() {
    use azalea_core::delta::{LpVec3, PositionDelta8};
    use azalea_core::entity_id::OptionalEntityId;
    use azalea_core::position::Vec3;
    use azalea_entity::LookDirection;
    use azalea_protocol::common::movements::{PositionMoveRotation, RelativeMovements};
    use azalea_protocol::packets::game::c_animate::AnimationAction;
    use azalea_protocol::packets::game::c_move_entity_pos_rot::CompactLookDirection;
    use azalea_protocol::packets::game::*;
    use pomme_protocol::entity::{self, EntityPacket};

    let decode = |name, packet| decode_azalea(name, packet, entity::read);
    let id = MinecraftEntityId(7);
    let angle = entity::unpack_degrees;
    let dvec = |v: Vec3| DVec3::new(v.x, v.y, v.z);
    let position = Vec3::new(1.5, -60.0, 3.25);
    let movement = Vec3::new(0.5, 0.25, -0.75);
    let change = PositionMoveRotation {
        pos: position,
        delta: movement,
        look_direction: LookDirection::new(45.0, -10.0),
    };

    let EntityPacket::Add {
        id: 7,
        uuid,
        entity_type,
        position: p,
        movement: m,
        x_rot,
        y_rot,
        y_head_rot,
        data: 5,
    } = decode(
        "add_entity",
        ClientboundGamePacket::AddEntity(c_add_entity::ClientboundAddEntity {
            id,
            uuid: uuid::Uuid::from_u128(42),
            entity_type: azalea_registry::builtin::EntityKind::Zombie,
            position,
            movement: LpVec3::from(movement),
            x_rot: -64,
            y_rot: 32,
            y_head_rot: 16,
            data: 5,
        }),
    )
    else {
        panic!("add_entity");
    };
    assert_eq!(uuid, 42);
    assert_eq!(
        <azalea_registry::builtin::EntityKind as azalea_registry::Registry>::from_u32(entity_type),
        Some(azalea_registry::builtin::EntityKind::Zombie)
    );
    assert_eq!(p, dvec(position));
    assert!((m - dvec(movement)).abs().max_element() < 1e-3);
    assert_eq!(
        (x_rot, y_rot, y_head_rot),
        (angle(-64), angle(32), angle(16))
    );

    let delta = PositionDelta8 {
        xa: 4096,
        ya: -2048,
        za: 1,
    };
    let look = CompactLookDirection {
        y_rot: 64,
        x_rot: -32,
    };
    let delta_vec = Some(DVec3::new(1.0, -0.5, 1.0 / 4096.0));
    let rotation = Some((angle(64), angle(-32)));
    let cases = [
        (
            "move_entity_pos",
            ClientboundGamePacket::MoveEntityPos(c_move_entity_pos::ClientboundMoveEntityPos {
                entity_id: id,
                delta,
                on_ground: true,
            }),
            EntityPacket::Move {
                id: 7,
                delta: delta_vec,
                rotation: None,
                on_ground: true,
            },
        ),
        (
            "move_entity_pos_rot",
            ClientboundGamePacket::MoveEntityPosRot(
                c_move_entity_pos_rot::ClientboundMoveEntityPosRot {
                    entity_id: id,
                    delta,
                    look_direction: look,
                    on_ground: false,
                },
            ),
            EntityPacket::Move {
                id: 7,
                delta: delta_vec,
                rotation,
                on_ground: false,
            },
        ),
        (
            "move_entity_rot",
            ClientboundGamePacket::MoveEntityRot(c_move_entity_rot::ClientboundMoveEntityRot {
                entity_id: id,
                look_direction: look,
                on_ground: true,
            }),
            EntityPacket::Move {
                id: 7,
                delta: None,
                rotation,
                on_ground: true,
            },
        ),
        (
            "entity_position_sync",
            ClientboundGamePacket::EntityPositionSync(
                c_entity_position_sync::ClientboundEntityPositionSync {
                    id,
                    values: change.clone(),
                    on_ground: true,
                },
            ),
            EntityPacket::PositionSync {
                id: 7,
                values: entity::PositionMoveRotation {
                    position: dvec(position),
                    delta: dvec(movement),
                    y_rot: 45.0,
                    x_rot: -10.0,
                },
                on_ground: true,
            },
        ),
        (
            "teleport_entity",
            ClientboundGamePacket::TeleportEntity(c_teleport_entity::ClientboundTeleportEntity {
                id,
                change,
                // azalea writes the flags byte-swapped; see the test below.
                relative: RelativeMovements::default(),
                on_ground: false,
            }),
            EntityPacket::Teleport {
                id: 7,
                change: entity::PositionMoveRotation {
                    position: dvec(position),
                    delta: dvec(movement),
                    y_rot: 45.0,
                    x_rot: -10.0,
                },
                relative: entity::Relative::default(),
                on_ground: false,
            },
        ),
        (
            "rotate_head",
            ClientboundGamePacket::RotateHead(c_rotate_head::ClientboundRotateHead {
                entity_id: id,
                y_head_rot: -96,
            }),
            EntityPacket::RotateHead {
                id: 7,
                y_head_rot: angle(-96),
            },
        ),
        (
            "animate",
            ClientboundGamePacket::Animate(c_animate::ClientboundAnimate {
                id,
                action: AnimationAction::WakeUp,
            }),
            EntityPacket::Animate { id: 7, action: 2 },
        ),
        (
            "entity_event",
            ClientboundGamePacket::EntityEvent(c_entity_event::ClientboundEntityEvent {
                entity_id: MinecraftEntityId(300),
                event_id: 34,
            }),
            EntityPacket::Event { id: 300, event: 34 },
        ),
        (
            "hurt_animation",
            ClientboundGamePacket::HurtAnimation(c_hurt_animation::ClientboundHurtAnimation {
                id,
                yaw: 135.0,
            }),
            EntityPacket::HurtAnimation { id: 7, yaw: 135.0 },
        ),
        (
            "damage_event",
            ClientboundGamePacket::DamageEvent(c_damage_event::ClientboundDamageEvent {
                entity_id: id,
                source_type_id: 12,
                source_cause_id: OptionalEntityId(Some(MinecraftEntityId(3))),
                source_direct_id: OptionalEntityId(None),
                source_position: Some(position),
            }),
            EntityPacket::Damage {
                id: 7,
                damage_type: 12,
                cause: Some(3),
                direct: None,
                position: Some(dvec(position)),
            },
        ),
        (
            "take_item_entity",
            ClientboundGamePacket::TakeItemEntity(c_take_item_entity::ClientboundTakeItemEntity {
                item_id: 9,
                player_id: MinecraftEntityId(1),
                amount: 3,
            }),
            EntityPacket::TakeItem {
                item: 9,
                collector: 1,
                amount: 3,
            },
        ),
        (
            "set_passengers",
            ClientboundGamePacket::SetPassengers(c_set_passengers::ClientboundSetPassengers {
                vehicle: id,
                passengers: vec![MinecraftEntityId(8), MinecraftEntityId(9)],
            }),
            EntityPacket::Passengers {
                vehicle: 7,
                passengers: vec![8, 9],
            },
        ),
        (
            "remove_entities",
            ClientboundGamePacket::RemoveEntities(c_remove_entities::ClientboundRemoveEntities {
                entity_ids: vec![MinecraftEntityId(5), id],
            }),
            EntityPacket::Remove { ids: vec![5, 7] },
        ),
    ];
    for (name, packet, expected) in cases {
        assert_eq!(decode(name, packet), expected, "{name}");
    }

    let EntityPacket::Motion { id: 7, velocity } = decode(
        "set_entity_motion",
        ClientboundGamePacket::SetEntityMotion(c_set_entity_motion::ClientboundSetEntityMotion {
            id,
            delta: LpVec3::from(Vec3::new(6.0, -1.0, 0.0)),
        }),
    ) else {
        panic!("set_entity_motion");
    };
    assert!((velocity - DVec3::new(6.0, -1.0, 0.0)).abs().max_element() < 1e-3);
}

/// Teleport relative flags are `ByteBufCodecs.INT` (big-endian) with each
/// `Relative` ordinal as a bit. azalea reads them that way but writes them
/// byte-swapped, so this builds the vanilla bytes by hand: pomme and azalea's
/// reader agree on X and Y_ROT.
#[test]
fn teleport_relative_flags_are_a_big_endian_int() {
    use pomme_protocol::entity::{self, EntityPacket};

    let mut raw = Vec::new();
    wire::write_varint(
        &mut raw,
        table_id(Phase::Game, Direction::Clientbound, "teleport_entity"),
    );
    raw.push(7);
    raw.extend([0u8; 56]);
    raw.extend(0b1001i32.to_be_bytes());
    raw.push(0);

    let Ok(ClientboundGamePacket::TeleportEntity(azalea)) =
        azalea_protocol::read::deserialize_packet::<ClientboundGamePacket>(
            &mut std::io::Cursor::new(&raw[..]),
        )
    else {
        panic!("azalea reads the frame");
    };
    assert!(azalea.relative.x && azalea.relative.y_rot && !azalea.relative.y);

    let mut r = pomme_protocol::codec::Reader::new(&raw[1..]);
    let Some(Ok(EntityPacket::Teleport { relative, .. })) = entity::read("teleport_entity", &mut r)
    else {
        panic!("pomme reads the frame");
    };
    assert_eq!(
        relative,
        entity::Relative {
            x: true,
            y_rot: true,
            ..Default::default()
        }
    );
}

/// Every player-state packet azalea encodes reads back through pomme's
/// layouts with the same values.
#[test]
fn player_packets_match_azalea() {
    use azalea_core::attribute_modifier_operation::AttributeModifierOperation;
    use azalea_inventory::components::AttributeModifier;
    use azalea_protocol::packets::game::c_game_event::EventType;
    use azalea_protocol::packets::game::c_player_abilities::PlayerAbilitiesFlags;
    use azalea_protocol::packets::game::*;
    use azalea_registry::Registry;
    use azalea_registry::builtin::{Attribute, MobEffect};
    use pomme_protocol::player::{self, AttributeSnapshot, PlayerPacket};

    let decode = |name, packet| decode_azalea(name, packet, player::read);
    let cases = [
        (
            "set_health",
            ClientboundGamePacket::SetHealth(c_set_health::ClientboundSetHealth {
                health: 15.5,
                food: 18,
                saturation: 2.0,
            }),
            PlayerPacket::Health {
                health: 15.5,
                food: 18,
                saturation: 2.0,
            },
        ),
        (
            "set_experience",
            ClientboundGamePacket::SetExperience(c_set_experience::ClientboundSetExperience {
                experience_progress: 0.25,
                experience_level: 30,
                total_experience: 1395,
            }),
            PlayerPacket::Experience {
                progress: 0.25,
                level: 30,
                total: 1395,
            },
        ),
        (
            "update_attributes",
            ClientboundGamePacket::UpdateAttributes(
                c_update_attributes::ClientboundUpdateAttributes {
                    entity_id: MinecraftEntityId(9),
                    values: vec![c_update_attributes::AttributeSnapshot {
                        attribute: Attribute::MaxHealth,
                        base: 20.0,
                        modifiers: vec![AttributeModifier {
                            id: Identifier::new("minecraft:test"),
                            amount: 4.0,
                            operation: AttributeModifierOperation::AddMultipliedTotal,
                        }],
                    }],
                },
            ),
            PlayerPacket::Attributes {
                entity: 9,
                snapshots: vec![AttributeSnapshot {
                    attribute: Attribute::MaxHealth.to_u32(),
                    base: 20.0,
                    modifiers: vec![player::AttributeModifier {
                        id: "minecraft:test".into(),
                        amount: 4.0,
                        operation: 2,
                    }],
                }],
            },
        ),
        (
            "remove_mob_effect",
            ClientboundGamePacket::RemoveMobEffect(
                c_remove_mob_effect::ClientboundRemoveMobEffect {
                    entity_id: MinecraftEntityId(1),
                    effect: MobEffect::Speed,
                },
            ),
            PlayerPacket::RemoveEffect {
                entity: 1,
                effect: MobEffect::Speed.to_u32(),
            },
        ),
        (
            "set_held_slot",
            ClientboundGamePacket::SetHeldSlot(c_set_held_slot::ClientboundSetHeldSlot { slot: 4 }),
            PlayerPacket::HeldSlot { slot: 4 },
        ),
        (
            "player_abilities",
            ClientboundGamePacket::PlayerAbilities(
                c_player_abilities::ClientboundPlayerAbilities {
                    flags: PlayerAbilitiesFlags {
                        invulnerable: false,
                        flying: true,
                        can_fly: true,
                        instant_break: false,
                    },
                    flying_speed: 0.05,
                    walking_speed: 0.1,
                },
            ),
            PlayerPacket::Abilities {
                flags: player::ABILITY_FLYING | player::ABILITY_CAN_FLY,
                flying_speed: 0.05,
                walking_speed: 0.1,
            },
        ),
        (
            "game_event",
            ClientboundGamePacket::GameEvent(c_game_event::ClientboundGameEvent {
                event: EventType::ThunderLevelChange,
                param: 0.5,
            }),
            PlayerPacket::GameEvent {
                event: 8,
                param: 0.5,
            },
        ),
    ];
    for (name, packet, expected) in cases {
        assert_eq!(decode(name, packet), expected, "{name}");
    }

    // azalea keeps its effect flags type private, so read the same vanilla
    // bytes with both: amplifier 1, 200 ticks, ambient + icon.
    let mut raw = Vec::new();
    wire::write_varint(
        &mut raw,
        table_id(Phase::Game, Direction::Clientbound, "update_mob_effect"),
    );
    wire::write_varint(&mut raw, 1);
    wire::write_varint(&mut raw, MobEffect::Speed.to_u32());
    raw.extend([
        1,
        0xC8,
        0x01,
        player::EFFECT_AMBIENT | player::EFFECT_SHOW_ICON,
    ]);
    let Ok(ClientboundGamePacket::UpdateMobEffect(azalea)) =
        azalea_protocol::read::deserialize_packet::<ClientboundGamePacket>(
            &mut std::io::Cursor::new(&raw[..]),
        )
    else {
        panic!("azalea reads update_mob_effect");
    };
    let mut r = pomme_protocol::codec::Reader::new(&raw);
    r.varint().unwrap();
    let Some(Ok(PlayerPacket::UpdateEffect {
        entity,
        effect,
        amplifier,
        duration,
        flags,
    })) = player::read("update_mob_effect", &mut r)
    else {
        panic!("pomme reads update_mob_effect");
    };
    assert_eq!(entity, azalea.entity_id.0);
    assert_eq!(effect, azalea.mob_effect.to_u32());
    assert_eq!(
        (amplifier, duration),
        (azalea.data.amplifier, azalea.data.duration)
    );
    assert_eq!(
        flags & player::EFFECT_AMBIENT != 0,
        azalea.data.flags.ambient
    );
    assert_eq!(
        flags & player::EFFECT_SHOW_ICON != 0,
        azalea.data.flags.show_icon
    );

    let PlayerPacket::CombatKill { player: 3, message } = decode(
        "player_combat_kill",
        ClientboundGamePacket::PlayerCombatKill(
            c_player_combat_kill::ClientboundPlayerCombatKill {
                player_id: MinecraftEntityId(3),
                message: "Tester fell".into(),
            },
        ),
    ) else {
        panic!("player_combat_kill");
    };
    assert_eq!(plain(&message), "Tester fell");
}

/// `packet` as azalea encodes it, decoded by pomme's `read` for `name`.
fn decode_azalea<P>(name: &str, packet: ClientboundGamePacket, read: RawRead<P>) -> P {
    read_native_frame(
        &azalea_protocol::write::serialize_packet(&packet).unwrap(),
        name,
        read,
    )
}

#[test]
fn block_packets_match_azalea() {
    use azalea_block::BlockState;
    use azalea_core::position::{BlockPos, ChunkSectionBlockPos, ChunkSectionPos};
    use azalea_protocol::packets::game::*;
    use azalea_registry::Registry;
    use azalea_registry::builtin::{BlockEntityKind, BlockKind};
    use pomme_protocol::block::{self, BlockPacket};

    let decode = |name, packet| decode_azalea(name, packet, block::read);
    let pos = BlockPos::new(-3, -60, 4);
    let state = BlockState::try_from(129u32).unwrap();
    let mut compound = simdnbt::owned::NbtCompound::new();
    compound.insert("d", 5);
    let cases = [
        (
            "block_update",
            ClientboundGamePacket::BlockUpdate(c_block_update::ClientboundBlockUpdate {
                pos,
                block_state: state,
            }),
            BlockPacket::Update {
                pos: (-3, -60, 4),
                state: 129,
            },
        ),
        (
            "section_blocks_update",
            ClientboundGamePacket::SectionBlocksUpdate(
                c_section_blocks_update::ClientboundSectionBlocksUpdate {
                    section_pos: ChunkSectionPos::new(-1, -4, 2),
                    states: vec![c_section_blocks_update::BlockStateWithPosition {
                        pos: ChunkSectionBlockPos::new(15, 3, 0),
                        state,
                    }],
                },
            ),
            BlockPacket::SectionUpdate {
                updates: vec![((-1, -61, 32), 129)],
            },
        ),
        (
            "block_changed_ack",
            ClientboundGamePacket::BlockChangedAck(
                c_block_changed_ack::ClientboundBlockChangedAck { seq: 17 },
            ),
            BlockPacket::ChangedAck { seq: 17 },
        ),
        (
            "block_event",
            ClientboundGamePacket::BlockEvent(c_block_event::ClientboundBlockEvent {
                pos,
                action_id: 1,
                action_parameter: 2,
                block: BlockKind::Chest,
            }),
            BlockPacket::Event {
                pos: (-3, -60, 4),
                action: 1,
                param: 2,
                block: BlockKind::Chest.to_u32(),
            },
        ),
        (
            "block_entity_data",
            ClientboundGamePacket::BlockEntityData(
                c_block_entity_data::ClientboundBlockEntityData {
                    pos,
                    block_entity_type: BlockEntityKind::Sign,
                    tag: simdnbt::owned::Nbt::Some(simdnbt::owned::BaseNbt::new("", compound)),
                },
            ),
            BlockPacket::EntityData {
                pos: (-3, -60, 4),
                kind: BlockEntityKind::Sign.to_u32(),
                tag: [10, 3, 0, 1, b'd', 0, 0, 0, 5, 0].into(),
            },
        ),
        (
            "level_event",
            ClientboundGamePacket::LevelEvent(c_level_event::ClientboundLevelEvent {
                event_type: 2001,
                pos,
                data: 129,
                global_event: true,
            }),
            BlockPacket::LevelEvent {
                event: 2001,
                pos: (-3, -60, 4),
                data: 129,
                global: true,
            },
        ),
    ];
    for (name, packet, expected) in cases {
        assert_eq!(decode(name, packet), expected, "{name}");
    }
}

#[test]
fn chunk_packets_match_azalea() {
    use std::sync::Arc;

    use azalea_core::bitset::BitSet;
    use azalea_core::heightmap_kind::HeightmapKind;
    use azalea_core::position::ChunkPos;
    use azalea_protocol::packets::game::*;
    use azalea_registry::Registry;
    use azalea_registry::builtin::BlockEntityKind;
    use pomme_protocol::chunk::{BlockEntityInfo, LightData};

    let decode = |name, packet| decode_azalea(name, packet, chunk::read);
    let light_data = || c_light_update::ClientboundLightUpdatePacketData {
        sky_y_mask: BitSet::from(vec![0b101u64, 1]),
        block_y_mask: BitSet::from(vec![0b10u64]),
        empty_sky_y_mask: BitSet::from(Vec::<u64>::new()),
        empty_block_y_mask: BitSet::from(vec![1u64 << 40]),
        sky_updates: Arc::new(vec![vec![7; 2048].into_boxed_slice()].into()),
        block_updates: Arc::new(Box::new([])),
    };
    let light = LightData {
        sky_y_mask: vec![0b101, 1],
        block_y_mask: vec![0b10],
        empty_sky_y_mask: vec![],
        empty_block_y_mask: vec![1 << 40],
        sky_updates: vec![vec![7; 2048].into()],
        block_updates: vec![],
    };
    let mut compound = simdnbt::owned::NbtCompound::new();
    compound.insert("d", 5);
    let cases = [
        (
            "level_chunk_with_light",
            ClientboundGamePacket::LevelChunkWithLight(
                c_level_chunk_with_light::ClientboundLevelChunkWithLight {
                    x: 3,
                    z: -2,
                    chunk_data: c_level_chunk_with_light::ClientboundLevelChunkPacketData {
                        heightmaps: vec![
                            (HeightmapKind::WorldSurface, vec![7].into()),
                            (HeightmapKind::MotionBlocking, vec![8, 9].into()),
                        ],
                        data: Arc::new(vec![1, 2, 3].into()),
                        block_entities: vec![c_level_chunk_with_light::BlockEntity {
                            packed_xz: 0x59,
                            y: (-64i16) as u16,
                            kind: BlockEntityKind::Sign,
                            data: simdnbt::owned::Nbt::Some(simdnbt::owned::BaseNbt::new(
                                "", compound,
                            )),
                        }],
                    },
                    light_data: light_data(),
                },
            ),
            ChunkPacket::Level {
                x: 3,
                z: -2,
                heightmaps: vec![(1, vec![7].into()), (4, vec![8, 9].into())],
                sections: vec![1, 2, 3].into(),
                block_entities: vec![BlockEntityInfo {
                    packed_xz: 0x59,
                    y: -64,
                    kind: BlockEntityKind::Sign.to_u32(),
                    tag: Some([10, 3, 0, 1, b'd', 0, 0, 0, 5, 0].into()),
                }],
                light: light.clone(),
            },
        ),
        (
            "light_update",
            ClientboundGamePacket::LightUpdate(c_light_update::ClientboundLightUpdate {
                x: -5,
                z: 6,
                light_data: light_data(),
            }),
            ChunkPacket::Light { x: -5, z: 6, light },
        ),
        (
            "forget_level_chunk",
            ClientboundGamePacket::ForgetLevelChunk(
                // A positive x: azalea's `ChunkPos` writer sign-extends x
                // over the z half (vanilla masks it, `ChunkPos.pack`); its
                // reader is right, and pomme's own fixture covers negatives.
                c_forget_level_chunk::ClientboundForgetLevelChunk {
                    pos: ChunkPos::new(3, -7),
                },
            ),
            ChunkPacket::Forget { x: 3, z: -7 },
        ),
        (
            "chunk_batch_start",
            ClientboundGamePacket::ChunkBatchStart(c_chunk_batch_start::ClientboundChunkBatchStart),
            ChunkPacket::BatchStart,
        ),
        (
            "chunk_batch_finished",
            ClientboundGamePacket::ChunkBatchFinished(
                c_chunk_batch_finished::ClientboundChunkBatchFinished { batch_size: 64 },
            ),
            ChunkPacket::BatchFinished { size: 64 },
        ),
        (
            "set_chunk_cache_center",
            ClientboundGamePacket::SetChunkCacheCenter(
                c_set_chunk_cache_center::ClientboundSetChunkCacheCenter { x: -1, z: 9 },
            ),
            ChunkPacket::CacheCenter { x: -1, z: 9 },
        ),
        (
            "set_chunk_cache_radius",
            ClientboundGamePacket::SetChunkCacheRadius(
                c_set_chunk_cache_radius::ClientboundSetChunkCacheRadius { radius: 12 },
            ),
            ChunkPacket::CacheRadius { radius: 12 },
        ),
    ];
    for (name, packet, expected) in cases {
        assert_eq!(decode(name, packet), expected, "{name}");
    }
}

#[test]
fn container_packets_match_azalea() {
    use azalea_inventory::components::EquipmentSlot;
    use azalea_inventory::{ItemStack, ItemStackData};
    use azalea_protocol::packets::game::*;
    use azalea_registry::Registry;
    use azalea_registry::builtin::{ItemKind, MenuKind};

    let decode = |name, packet| decode_azalea(name, packet, container::read);
    let azalea_stack = |kind, count| {
        ItemStack::Present(ItemStackData {
            kind,
            count,
            component_patch: Default::default(),
        })
    };
    let cases = [
        (
            "container_set_content",
            ClientboundGamePacket::ContainerSetContent(
                c_container_set_content::ClientboundContainerSetContent {
                    container_id: 2,
                    state_id: 7,
                    items: vec![azalea_stack(ItemKind::Diamond, 3), ItemStack::Empty],
                    carried_item: azalea_stack(ItemKind::Stone, 1),
                },
            ),
            ContainerPacket::Content {
                container_id: 2,
                state_id: 7,
                items: vec![pomme_stack(ItemKind::Diamond, 3), None],
                carried: pomme_stack(ItemKind::Stone, 1),
            },
        ),
        (
            "container_set_slot",
            ClientboundGamePacket::ContainerSetSlot(
                c_container_set_slot::ClientboundContainerSetSlot {
                    container_id: 0,
                    state_id: 4,
                    slot: 36,
                    item_stack: azalea_stack(ItemKind::Bread, 2),
                },
            ),
            ContainerPacket::Slot {
                container_id: 0,
                state_id: 4,
                slot: 36,
                item: pomme_stack(ItemKind::Bread, 2),
            },
        ),
        (
            "container_set_data",
            ClientboundGamePacket::ContainerSetData(
                c_container_set_data::ClientboundContainerSetData {
                    container_id: 3,
                    id: 1,
                    value: 200,
                },
            ),
            ContainerPacket::Data {
                container_id: 3,
                id: 1,
                value: 200,
            },
        ),
        (
            "container_close",
            ClientboundGamePacket::ContainerClose(c_container_close::ClientboundContainerClose {
                container_id: 3,
            }),
            ContainerPacket::Close { container_id: 3 },
        ),
        (
            "set_cursor_item",
            ClientboundGamePacket::SetCursorItem(c_set_cursor_item::ClientboundSetCursorItem {
                contents: azalea_stack(ItemKind::Stone, 64),
            }),
            ContainerPacket::Cursor {
                item: pomme_stack(ItemKind::Stone, 64),
            },
        ),
        (
            "set_player_inventory",
            ClientboundGamePacket::SetPlayerInventory(
                c_set_player_inventory::ClientboundSetPlayerInventory {
                    slot: 40,
                    contents: azalea_stack(ItemKind::Shield, 1),
                },
            ),
            ContainerPacket::PlayerInventory {
                slot: 40,
                item: pomme_stack(ItemKind::Shield, 1),
            },
        ),
        (
            "set_equipment",
            ClientboundGamePacket::SetEquipment(c_set_equipment::ClientboundSetEquipment {
                entity_id: MinecraftEntityId(9),
                slots: c_set_equipment::EquipmentSlots {
                    slots: vec![
                        (
                            EquipmentSlot::Mainhand,
                            azalea_stack(ItemKind::StoneSword, 1),
                        ),
                        (EquipmentSlot::Head, ItemStack::Empty),
                    ],
                },
            }),
            ContainerPacket::Equipment {
                entity: 9,
                slots: vec![(0, pomme_stack(ItemKind::StoneSword, 1)), (5, None)],
            },
        ),
    ];
    for (name, packet, expected) in cases {
        assert_eq!(decode(name, packet), expected, "{name}");
    }

    let ContainerPacket::OpenScreen {
        container_id,
        menu,
        title,
    } = decode(
        "open_screen",
        ClientboundGamePacket::OpenScreen(c_open_screen::ClientboundOpenScreen {
            container_id: 3,
            menu_type: MenuKind::Generic9x3,
            title: azalea_chat::FormattedText::from("Chest"),
        }),
    )
    else {
        panic!("open_screen");
    };
    assert_eq!((container_id, menu), (3, MenuKind::Generic9x3.to_u32()));
    assert_eq!(plain(&title), "Chest");
}

#[test]
fn hud_packets_match_azalea() {
    use azalea_protocol::packets::game::c_boss_event::{
        AddOperation, BossBarColor, BossBarOverlay, ClientboundBossEvent, Operation, Properties,
        Style,
    };
    use azalea_protocol::packets::game::*;

    let decode = |name, packet| decode_azalea(name, packet, hud::read);
    let boss = |operation| {
        ClientboundGamePacket::BossEvent(ClientboundBossEvent {
            id: uuid::Uuid::from_u128(7),
            operation,
        })
    };
    let style = Style {
        color: BossBarColor::Red,
        overlay: BossBarOverlay::Notched10,
    };
    let properties = Properties {
        darken_screen: true,
        play_music: false,
        create_world_fog: true,
    };
    let flags = BossFlags {
        darken_screen: true,
        play_music: false,
        create_world_fog: true,
    };

    let cases = [
        (
            "set_titles_animation",
            ClientboundGamePacket::SetTitlesAnimation(
                c_set_titles_animation::ClientboundSetTitlesAnimation {
                    fade_in: 10,
                    stay: 70,
                    fade_out: 20,
                },
            ),
            HudPacket::TitlesAnimation {
                fade_in: 10,
                stay: 70,
                fade_out: 20,
            },
        ),
        (
            "clear_titles",
            ClientboundGamePacket::ClearTitles(c_clear_titles::ClientboundClearTitles {
                reset_times: true,
            }),
            HudPacket::ClearTitles { reset_times: true },
        ),
        (
            "boss_event",
            boss(Operation::Remove),
            HudPacket::BossEvent {
                id: 7,
                op: BossOp::Remove,
            },
        ),
        (
            "boss_event",
            boss(Operation::UpdateProgress(0.25)),
            HudPacket::BossEvent {
                id: 7,
                op: BossOp::Progress(0.25),
            },
        ),
        (
            "boss_event",
            boss(Operation::UpdateStyle(style.clone())),
            HudPacket::BossEvent {
                id: 7,
                op: BossOp::Style {
                    color: 2,
                    overlay: 2,
                },
            },
        ),
        (
            "boss_event",
            boss(Operation::UpdateProperties(properties.clone())),
            HudPacket::BossEvent {
                id: 7,
                op: BossOp::Properties(flags),
            },
        ),
    ];
    for (name, packet, expected) in cases {
        assert_eq!(decode(name, packet), expected, "{name}");
    }

    let HudPacket::Title(title) = decode(
        "set_title_text",
        ClientboundGamePacket::SetTitleText(c_set_title_text::ClientboundSetTitleText {
            text: "Hello".into(),
        }),
    ) else {
        panic!("set_title_text");
    };
    assert_eq!(plain(&title), "Hello");
    let HudPacket::Subtitle(subtitle) = decode(
        "set_subtitle_text",
        ClientboundGamePacket::SetSubtitleText(c_set_subtitle_text::ClientboundSetSubtitleText {
            text: "sub".into(),
        }),
    ) else {
        panic!("set_subtitle_text");
    };
    assert_eq!(plain(&subtitle), "sub");
    let HudPacket::TabList { header, footer } = decode(
        "tab_list",
        ClientboundGamePacket::TabList(c_tab_list::ClientboundTabList {
            header: "head".into(),
            footer: "foot".into(),
        }),
    ) else {
        panic!("tab_list");
    };
    assert_eq!(
        (plain(&header), plain(&footer)),
        ("head".into(), "foot".into())
    );
    let HudPacket::BossEvent {
        id: 7,
        op:
            BossOp::Add {
                name,
                progress: 0.5,
                color: 2,
                overlay: 2,
                flags: added,
            },
    } = decode(
        "boss_event",
        boss(Operation::Add(AddOperation {
            name: "Boss".into(),
            progress: 0.5,
            style,
            properties,
        })),
    )
    else {
        panic!("boss_event add");
    };
    assert_eq!((plain(&name), added), ("Boss".into(), flags));
    let HudPacket::BossEvent {
        op: BossOp::Name(name),
        ..
    } = decode("boss_event", boss(Operation::UpdateName("Renamed".into())))
    else {
        panic!("boss_event name");
    };
    assert_eq!(plain(&name), "Renamed");
}

/// azalea writes `set_objective`'s add and change without the number
/// format's presence bool, so only its remove is compared here; azalea's
/// team packet predates 26.2's `Optional<TeamColor>`.
#[test]
fn scoreboard_packets_match_azalea() {
    use azalea_chat::numbers::NumberFormat as AzaleaFormat;
    use azalea_protocol::packets::game::c_set_display_objective::DisplaySlot;
    use azalea_protocol::packets::game::*;

    let decode = |name, packet| decode_azalea(name, packet, scoreboard::read);
    let score = |number_format| {
        ClientboundGamePacket::SetScore(c_set_score::ClientboundSetScore {
            owner: "Bob".into(),
            objective_name: "kills".into(),
            score: 9,
            display: None,
            number_format,
        })
    };
    let pomme_score = |number_format| ScoreboardPacket::Score {
        owner: "Bob".into(),
        objective: "kills".into(),
        score: 9,
        display: None,
        number_format,
    };

    let cases = [
        (
            "set_objective",
            ClientboundGamePacket::SetObjective(c_set_objective::ClientboundSetObjective {
                objective_name: "kills".into(),
                method: c_set_objective::Method::Remove,
            }),
            ScoreboardPacket::Objective {
                name: "kills".into(),
                method: ObjectiveMethod::Remove,
            },
        ),
        (
            "set_display_objective",
            ClientboundGamePacket::SetDisplayObjective(
                c_set_display_objective::ClientboundSetDisplayObjective {
                    slot: DisplaySlot::Sidebar,
                    objective_name: "kills".into(),
                },
            ),
            ScoreboardPacket::Display {
                slot: 1,
                objective: "kills".into(),
            },
        ),
        ("set_score", score(None), pomme_score(None)),
        (
            "set_score",
            score(Some(AzaleaFormat::Blank)),
            pomme_score(Some(NumberFormat::Blank)),
        ),
        (
            "reset_score",
            ClientboundGamePacket::ResetScore(c_reset_score::ClientboundResetScore {
                owner: "Bob".into(),
                objective_name: None,
            }),
            ScoreboardPacket::ResetScore {
                owner: "Bob".into(),
                objective: None,
            },
        ),
    ];
    for (name, packet, expected) in cases {
        assert_eq!(decode(name, packet), expected, "{name}");
    }

    let ScoreboardPacket::Score {
        display: Some(display),
        number_format: Some(NumberFormat::Fixed(fixed)),
        ..
    } = decode(
        "set_score",
        ClientboundGamePacket::SetScore(c_set_score::ClientboundSetScore {
            owner: "Bob".into(),
            objective_name: "kills".into(),
            score: 5,
            display: Some("Hero".into()),
            number_format: Some(AzaleaFormat::Fixed { value: "x".into() }),
        }),
    )
    else {
        panic!("set_score with display");
    };
    assert_eq!(
        (plain(&display), plain(&fixed)),
        ("Hero".into(), "x".into())
    );
}

/// azalea's writer sets the `update_list_order` and `update_hat` bits but
/// never writes their payloads, so those two are covered by
/// `player_info.rs`'s fixtures instead.
#[test]
fn player_info_packets_match_azalea() {
    use std::sync::Arc;

    use azalea_auth::game_profile::{GameProfile, GameProfileProperties, ProfilePropertyValue};
    use azalea_core::game_type::GameMode;
    use azalea_protocol::packets::game::c_player_info_remove::ClientboundPlayerInfoRemove;
    use azalea_protocol::packets::game::c_player_info_update::{
        ActionEnumSet, ClientboundPlayerInfoUpdate, PlayerInfoEntry,
    };
    use azalea_protocol::packets::game::s_chat_session_update::{
        ProfilePublicKeyData, RemoteChatSessionData,
    };
    use pomme_protocol::player_info::{Actions, ChatSession, Property};
    use uuid::Uuid;

    let decode = |name, packet| decode_azalea(name, packet, player_info::read);
    let mut profile = GameProfile::new(Uuid::from_u128(9), "bob".into());
    let mut properties = GameProfileProperties::default();
    properties.map.insert(
        "textures".into(),
        ProfilePropertyValue {
            value: "abc".into(),
            signature: Some("sig".into()),
        },
    );
    profile.properties = Arc::new(properties);
    let sent = ActionEnumSet {
        add_player: true,
        initialize_chat: true,
        update_game_mode: true,
        update_listed: true,
        update_latency: true,
        update_display_name: true,
        update_list_order: false,
        update_hat: false,
    };
    let PlayerInfoPacket::Update { actions, entries } = decode(
        "player_info_update",
        ClientboundGamePacket::PlayerInfoUpdate(ClientboundPlayerInfoUpdate {
            actions: sent,
            entries: vec![PlayerInfoEntry {
                profile,
                chat_session: Some(RemoteChatSessionData {
                    session_id: Uuid::from_u128(7),
                    profile_public_key: ProfilePublicKeyData {
                        expires_at: 1000,
                        key: vec![0xAA, 0xBB],
                        key_signature: vec![0xCC],
                    },
                }),
                game_mode: GameMode::Creative,
                listed: true,
                latency: 42,
                display_name: Some(Box::new("Bob".into())),
                ..Default::default()
            }],
        }),
    ) else {
        panic!("player_info_update");
    };
    assert_eq!(
        actions,
        Actions {
            add_player: true,
            initialize_chat: true,
            update_game_mode: true,
            update_listed: true,
            update_latency: true,
            update_display_name: true,
            update_list_order: false,
            update_hat: false,
        }
    );
    let [entry] = &entries[..] else {
        panic!("{entries:?}");
    };
    assert_eq!(entry.profile_id, 9);
    assert_eq!(entry.name, "bob");
    assert_eq!(
        entry.properties,
        [Property {
            name: "textures".into(),
            value: "abc".into(),
            signature: Some("sig".into()),
        }]
    );
    assert_eq!(
        entry.chat_session,
        Some(ChatSession {
            session_id: 7,
            expires_at: 1000,
            key: [0xAA, 0xBB].into(),
            key_signature: [0xCC].into(),
        })
    );
    assert_eq!(
        (entry.game_mode, entry.listed, entry.latency),
        (1, true, 42)
    );
    assert_eq!(plain(entry.display_name.as_deref().unwrap()), "Bob");

    assert_eq!(
        decode(
            "player_info_remove",
            ClientboundGamePacket::PlayerInfoRemove(ClientboundPlayerInfoRemove {
                profile_ids: vec![Uuid::from_u128(1), Uuid::from_u128(2)],
            }),
        ),
        PlayerInfoPacket::Remove(vec![1, 2])
    );
}

#[test]
fn sound_packets_match_azalea() {
    use azalea_protocol::packets::game::c_sound::SoundSource;
    use azalea_protocol::packets::game::{c_sound, c_sound_entity, c_stop_sound};
    use pomme_protocol::sound::SoundEvent as PommeSound;
    use pomme_protocol::{ClientRegistry, RegistryTable};

    let decode = |name, packet| decode_azalea(name, packet, sound::read);
    let explode = registry_id(
        RegistryTable::native(),
        ClientRegistry::SoundEvent,
        "entity.generic.explode",
    );
    let cases = [
        (
            "sound",
            ClientboundGamePacket::Sound(c_sound::ClientboundSound {
                sound: Holder::Reference(SoundEvent::EntityGenericExplode),
                source: SoundSource::Blocks,
                x: -8,
                y: 520,
                z: 16,
                volume: 0.5,
                pitch: 2.0,
                seed: 7,
            }),
            SoundPacket::Sound {
                sound: PommeSound::Registered(explode),
                source: 4,
                x: -8,
                y: 520,
                z: 16,
                volume: 0.5,
                pitch: 2.0,
                seed: 7,
            },
        ),
        (
            "sound_entity",
            ClientboundGamePacket::SoundEntity(c_sound_entity::ClientboundSoundEntity {
                sound: sound_holder(),
                source: SoundSource::Voice,
                id: MinecraftEntityId(9),
                volume: 1.0,
                pitch: 1.0,
                seed: 3,
            }),
            SoundPacket::Entity {
                sound: PommeSound::Direct {
                    location: "minecraft:test.sound".into(),
                    fixed_range: None,
                },
                source: 9,
                entity: 9,
                volume: 1.0,
                pitch: 1.0,
                seed: 3,
            },
        ),
        (
            "stop_sound",
            ClientboundGamePacket::StopSound(c_stop_sound::ClientboundStopSound {
                source: Some(SoundSource::Music),
                name: Some(Identifier::new("minecraft:music.game")),
            }),
            SoundPacket::Stop {
                source: Some(1),
                name: Some("minecraft:music.game".into()),
            },
        ),
    ];
    for (name, packet, expected) in cases {
        assert_eq!(decode(name, packet), expected, "{name}");
    }
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

/// azalea writes `Relative` as a little-endian bitset where vanilla writes a
/// big-endian int, so its teleports here are all absolute; `lifecycle.rs`'s
/// fixtures cover mixed bits.
#[test]
fn lifecycle_packets_match_azalea() {
    use azalea_core::game_type::GameMode;
    use azalea_protocol::common::movements::{PositionMoveRotation, RelativeMovements};
    use azalea_protocol::packets::game::*;
    use azalea_registry::DataRegistry;
    use pomme_protocol::lifecycle::{ClockState, Login, SpawnInfo};

    let decode = |name, packet| decode_azalea(name, packet, lifecycle::read);
    let spawn = SpawnInfo {
        dimension_type: 0,
        dimension: "minecraft:overworld".into(),
        seed: 42,
        game_type: 0,
        previous_game_type: Some(1),
        is_debug: false,
        is_flat: false,
        last_death: None,
        portal_cooldown: 0,
        sea_level: 63,
    };
    let cases = [
        (
            "login",
            ClientboundGamePacket::Login(c_login::ClientboundLogin {
                player_id: MinecraftEntityId(7),
                hardcore: true,
                levels: vec!["minecraft:overworld".into()],
                max_players: 20,
                chunk_radius: 12,
                simulation_distance: 10,
                reduced_debug_info: false,
                show_death_screen: true,
                do_limited_crafting: false,
                common: azalea_spawn_info(Some(GameMode::Creative)),
                online_mode: true,
                enforces_secure_chat: false,
            }),
            LifecyclePacket::Login(Login {
                player_id: 7,
                hardcore: true,
                levels: vec!["minecraft:overworld".into()],
                max_players: 20,
                chunk_radius: 12,
                simulation_distance: 10,
                reduced_debug_info: false,
                show_death_screen: true,
                do_limited_crafting: false,
                spawn: spawn.clone(),
                online_mode: true,
                enforces_secure_chat: false,
            }),
        ),
        (
            "respawn",
            ClientboundGamePacket::Respawn(c_respawn::ClientboundRespawn {
                common: azalea_spawn_info(None),
                data_to_keep: 3,
            }),
            LifecyclePacket::Respawn {
                spawn: SpawnInfo {
                    previous_game_type: None,
                    ..spawn.clone()
                },
                data_to_keep: 3,
            },
        ),
        (
            "keep_alive",
            ClientboundGamePacket::KeepAlive(c_keep_alive::ClientboundKeepAlive { id: 99 }),
            LifecyclePacket::KeepAlive(99),
        ),
        (
            "ping",
            ClientboundGamePacket::Ping(c_ping::ClientboundPing { id: 5 }),
            LifecyclePacket::Ping(5),
        ),
        (
            "player_position",
            ClientboundGamePacket::PlayerPosition(c_player_position::ClientboundPlayerPosition {
                id: 3,
                change: PositionMoveRotation {
                    pos: azalea_core::position::Vec3::new(1.5, 64.0, -2.0),
                    delta: azalea_core::position::Vec3::new(0.0, 0.5, 0.0),
                    look_direction: azalea_entity::LookDirection::new(90.0, 10.0),
                },
                relative: RelativeMovements::default(),
            }),
            LifecyclePacket::PlayerPosition {
                id: 3,
                change: pomme_protocol::entity::PositionMoveRotation {
                    position: DVec3::new(1.5, 64.0, -2.0),
                    delta: DVec3::new(0.0, 0.5, 0.0),
                    y_rot: 90.0,
                    x_rot: 10.0,
                },
                relative: pomme_protocol::entity::Relative::default(),
            },
        ),
        (
            "player_rotation",
            ClientboundGamePacket::PlayerRotation(c_player_rotation::ClientboundPlayerRotation {
                y_rot: 45.0,
                relative_y: true,
                x_rot: -30.0,
                relative_x: false,
            }),
            LifecyclePacket::PlayerRotation {
                y_rot: 45.0,
                relative_y: true,
                x_rot: -30.0,
                relative_x: false,
            },
        ),
        (
            "set_time",
            ClientboundGamePacket::SetTime(c_set_time::ClientboundSetTime {
                game_time: 100,
                clock_updates: [(
                    azalea_registry::data::WorldClock::new_raw(1),
                    c_set_time::ClockState {
                        total_ticks: 6000,
                        partial_tick: 0.5,
                        rate: 1.0,
                    },
                )]
                .into_iter()
                .collect(),
            }),
            LifecyclePacket::SetTime {
                game_time: 100,
                clocks: vec![(
                    1,
                    ClockState {
                        total_ticks: 6000,
                        partial_tick: 0.5,
                        rate: 1.0,
                    },
                )],
            },
        ),
        (
            "set_simulation_distance",
            ClientboundGamePacket::SetSimulationDistance(
                c_set_simulation_distance::ClientboundSetSimulationDistance {
                    simulation_distance: 8,
                },
            ),
            LifecyclePacket::SimulationDistance(8),
        ),
    ];
    for (name, packet, expected) in cases {
        assert_eq!(decode(name, packet), expected, "{name}");
    }

    let LifecyclePacket::Disconnect(reason) = decode(
        "disconnect",
        ClientboundGamePacket::Disconnect(c_disconnect::ClientboundDisconnect {
            reason: "bye".into(),
        }),
    ) else {
        panic!("disconnect");
    };
    assert_eq!(plain(&reason), "bye");
}
