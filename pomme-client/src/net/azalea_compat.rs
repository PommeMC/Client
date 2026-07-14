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

/// The 26.1 -> 26.2 translation under test; 26.1 wire layouts below are
/// hand-built from the decompiled reference codecs
/// (`reference/26.1/decompiled/.../network/protocol/`).
fn translation() -> crate::net::translate::Translation {
    crate::net::translate::Translation::for_protocol(775).expect("26.1 translation data")
}

/// 1.21.11 (774) has embedded tables but no wire translation.
#[test]
fn no_translation_without_coverage() {
    assert!(crate::net::translate::Translation::for_protocol(774).is_none());
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

    let translated = translation().translate_login_frame(old);
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

    let translated = translation()
        .translate_game_frame(old.into_boxed_slice())
        .unwrap();
    assert_eq!(&translated[..], &frame[..]);
}

/// 26.1's team `Parameters` order is `displayName, options, visibility,
/// collision, color, prefix, suffix` with color as a `ChatFormatting`
/// ordinal; 26.2 reordered to `displayName, prefix, suffix, visibility,
/// collision, color, options` with color as `Optional<TeamColor>`
/// (`ClientboundSetPlayerTeamPacket.Parameters` in both references).
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

    let translated = translation()
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
    expected.extend_from_slice(&[1, 5]); // color: Some(TeamColor 5)
    expected.push(3); // options
    assert_eq!(&translated[..], &expected[..]);
}

/// RESET (ChatFormatting ordinal 21) has no TeamColor equivalent and maps to
/// an empty optional; the method-0 player list is copied verbatim.
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

    let translated = translation()
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
    expected.push(0); // color: None
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
    assert!(translation().remap_inbound(&mut packet));
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
        translation().remap_outbound(&mut packet);
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
