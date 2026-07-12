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
