use std::collections::HashMap;
use std::sync::OnceLock;

use crate::version::{EMBEDDED, LATEST, ProtocolVersion};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Handshake,
    Status,
    Login,
    Configuration,
    Game,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Serverbound,
    Clientbound,
}

/// Packet-id tables for one game version: per phase and direction, the
/// vanilla resource names in registration order (wire id == index). Generated
/// by `tools/protogen` from the decompiled `<Phase>Protocols.java`.
pub struct PacketTable {
    version: ProtocolVersion,
    phases: [PhaseTable; 5],
}

struct PhaseTable {
    serverbound: DirectionTable,
    clientbound: DirectionTable,
}

struct DirectionTable {
    names: Vec<String>,
    ids: HashMap<String, u32>,
}

#[derive(serde::Deserialize)]
struct TableFile {
    version: String,
    protocol: i32,
    handshake: PhaseFile,
    status: PhaseFile,
    login: PhaseFile,
    configuration: PhaseFile,
    game: PhaseFile,
}

#[derive(serde::Deserialize)]
struct PhaseFile {
    serverbound: Vec<String>,
    clientbound: Vec<String>,
}

impl PacketTable {
    /// The table for the version the client speaks internally. Parsed once
    /// from the embedded JSON; panics on malformed data (a generator bug,
    /// caught at first use / in tests rather than emitting wrong ids).
    pub fn latest() -> &'static PacketTable {
        static TABLE: OnceLock<PacketTable> = OnceLock::new();
        TABLE.get_or_init(|| {
            Self::parse(include_str!("data/protocol-26.2.json"), LATEST)
                .expect("embedded 26.2 packet table")
        })
    }

    /// The table for a launchable protocol number, or `None` for versions
    /// without an embedded table.
    pub fn for_protocol(protocol: i32) -> Option<&'static PacketTable> {
        if protocol == LATEST.protocol {
            return Some(Self::latest());
        }
        static TABLES: [OnceLock<PacketTable>; EMBEDDED.len()] =
            [const { OnceLock::new() }; EMBEDDED.len()];
        crate::version::embedded_get(protocol, &TABLES, |e| {
            Self::parse(e.packets, e.version)
                .unwrap_or_else(|err| panic!("embedded {} packet table: {err}", e.version.name))
        })
    }

    fn parse(json: &str, expected: ProtocolVersion) -> Result<Self, String> {
        let file: TableFile = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if file.version != expected.name || file.protocol != expected.protocol {
            return Err(format!(
                "table is {}/{}, expected {}/{}",
                file.version, file.protocol, expected.name, expected.protocol
            ));
        }
        if file.game.serverbound.is_empty() || file.game.clientbound.is_empty() {
            return Err("empty game packet list".into());
        }
        // The game clientbound chain registers the bundle delimiter first;
        // anything else at id 0 means protogen mis-ordered the calls.
        if file.game.clientbound[0] != "bundle_delimiter" {
            return Err(format!(
                "game clientbound id 0 is {}, expected bundle_delimiter",
                file.game.clientbound[0]
            ));
        }
        let phases = [
            file.handshake,
            file.status,
            file.login,
            file.configuration,
            file.game,
        ]
        .map(|p| PhaseTable {
            serverbound: DirectionTable::build(p.serverbound),
            clientbound: DirectionTable::build(p.clientbound),
        });
        for (phase, table) in phases.iter().enumerate() {
            for dir in [&table.serverbound, &table.clientbound] {
                if dir.ids.len() != dir.names.len() {
                    return Err(format!("duplicate packet name in phase {phase}"));
                }
            }
        }
        Ok(Self {
            version: expected,
            phases,
        })
    }

    pub fn version(&self) -> ProtocolVersion {
        self.version
    }

    pub fn id(&self, phase: Phase, dir: Direction, name: &str) -> Option<u32> {
        self.direction(phase, dir).ids.get(name).copied()
    }

    pub fn name_of(&self, phase: Phase, dir: Direction, id: u32) -> Option<&str> {
        self.direction(phase, dir)
            .names
            .get(id as usize)
            .map(String::as_str)
    }

    fn direction(&self, phase: Phase, dir: Direction) -> &DirectionTable {
        let phase = &self.phases[phase as usize];
        match dir {
            Direction::Serverbound => &phase.serverbound,
            Direction::Clientbound => &phase.clientbound,
        }
    }
}

impl DirectionTable {
    fn build(names: Vec<String>) -> Self {
        let ids = names
            .iter()
            .enumerate()
            .map(|(id, name)| (name.clone(), id as u32))
            .collect();
        Self { names, ids }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHASES: [Phase; 5] = [
        Phase::Handshake,
        Phase::Status,
        Phase::Login,
        Phase::Configuration,
        Phase::Game,
    ];
    const DIRECTIONS: [Direction; 2] = [Direction::Serverbound, Direction::Clientbound];

    /// A packet resource name: `/`-separated lowercase segments, as in
    /// `debug/block_value`.
    fn is_resource_name(name: &str) -> bool {
        !name.is_empty()
            && name.split('/').all(|seg| {
                !seg.is_empty()
                    && seg
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
    }

    /// Visits every `(phase, direction, id, name)` the table defines.
    fn for_each_name(t: &PacketTable, mut f: impl FnMut(Phase, Direction, u32, &str)) {
        for phase in PHASES {
            for dir in DIRECTIONS {
                let mut id = 0;
                while let Some(name) = t.name_of(phase, dir, id) {
                    f(phase, dir, id, name);
                    id += 1;
                }
            }
        }
    }

    /// Asserts every id `t` has in the phase/direction resolves to the same
    /// name in `other` (a prefix check; pass `equal` to also require `other`
    /// to end at the same id).
    fn assert_prefix(
        t: &PacketTable,
        other: &PacketTable,
        phase: Phase,
        dir: Direction,
        equal: bool,
    ) {
        let mut id = 0;
        while let Some(name) = t.name_of(phase, dir, id) {
            assert_eq!(
                Some(name),
                other.name_of(phase, dir, id),
                "{phase:?} {dir:?} {id}"
            );
            id += 1;
        }
        if equal {
            assert_eq!(
                other.name_of(phase, dir, id),
                None,
                "{phase:?} {dir:?} {id}"
            );
        }
    }

    /// Registration-order anchors, spot-checked by hand against
    /// `reference/26.2/decompiled/.../GameProtocols.java`.
    #[test]
    fn anchors_26_2() {
        let t = PacketTable::latest();
        assert_eq!(t.version().protocol, 776);
        assert_eq!(t.id(Phase::Game, Direction::Serverbound, "attack"), Some(1));
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "interact"),
            Some(0x1A)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "level_particles"),
            Some(47)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "remove_mob_effect"),
            Some(0x4E)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "update_mob_effect"),
            Some(0x84)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "boss_event"),
            Some(9)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "recipe_book_add"),
            Some(74)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "update_advancements"),
            Some(130)
        );
        assert_eq!(
            t.name_of(Phase::Game, Direction::Clientbound, 0),
            Some("bundle_delimiter")
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "clear_titles"),
            Some(14)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "set_subtitle_text"),
            Some(112)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "set_title_text"),
            Some(114)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "set_titles_animation"),
            Some(115)
        );
        assert_eq!(
            t.id(Phase::Handshake, Direction::Serverbound, "intention"),
            Some(0)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "teleport_to_entity"),
            Some(0x40)
        );
        assert_eq!(t.id(Phase::Game, Direction::Serverbound, "no_such"), None);
    }

    /// Registration-order anchors for 26.1, spot-checked by hand against
    /// `reference/26.1/decompiled/.../GameProtocols.java`. Ids match 26.2
    /// everywhere; the serverbound slot 62 packet was renamed in 26.2
    /// (`spectate_entity` -> `spectator_action`).
    #[test]
    fn anchors_26_1() {
        let t = PacketTable::for_protocol(775).unwrap();
        assert_eq!(t.version().protocol, 775);
        assert_eq!(t.version().name, "26.1");
        assert_eq!(t.id(Phase::Game, Direction::Serverbound, "attack"), Some(1));
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "interact"),
            Some(0x1A)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "level_particles"),
            Some(47)
        );
        assert_eq!(
            t.name_of(Phase::Game, Direction::Serverbound, 62),
            Some("spectate_entity")
        );
        assert_eq!(
            t.id(Phase::Login, Direction::Clientbound, "login_finished"),
            Some(2)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "login"),
            PacketTable::latest().id(Phase::Game, Direction::Clientbound, "login")
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "set_player_team"),
            PacketTable::latest().id(Phase::Game, Direction::Clientbound, "set_player_team")
        );
    }

    /// Registration-order anchors for 1.21.11, spot-checked by hand against
    /// `reference/1.21.11/decompiled/.../GameProtocols.java` and cross-checked
    /// in full against Mojang's `generated/reports/packets.json`. Unlike
    /// 26.x, game ids diverge broadly from 26.2 (100 clientbound, 65
    /// serverbound), and `attack`/`spectator_action`/`set_game_rule` don't
    /// exist yet (attacking is `interact` with an ATTACK action).
    #[test]
    fn anchors_1_21_11() {
        let t = PacketTable::for_protocol(774).unwrap();
        assert_eq!(t.version().protocol, 774);
        assert_eq!(t.version().name, "1.21.11");
        assert_eq!(t.id(Phase::Game, Direction::Serverbound, "attack"), None);
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "interact"),
            Some(25)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "container_click"),
            Some(17)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "level_particles"),
            Some(46)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "set_entity_data"),
            Some(97)
        );
        assert_eq!(
            t.name_of(Phase::Game, Direction::Clientbound, 0),
            Some("bundle_delimiter")
        );
        assert_eq!(
            t.id(Phase::Login, Direction::Clientbound, "login_finished"),
            Some(2)
        );
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 65).is_some());
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 66).is_none());
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 138)
                .is_some()
        );
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 139)
                .is_none()
        );
    }

    /// Registration-order anchors for 1.21.10, cross-checked in full against
    /// Mojang's `generated/reports/packets.json`. Its game tables match
    /// 1.21.11 exactly except clientbound 40, which 1.21.11 renamed
    /// (`horse_screen_open` -> `mount_screen_open`, identical fields).
    #[test]
    fn anchors_1_21_10() {
        let t = PacketTable::for_protocol(773).unwrap();
        assert_eq!(t.version().protocol, 773);
        assert_eq!(t.version().name, "1.21.10");
        assert_eq!(t.id(Phase::Game, Direction::Serverbound, "attack"), None);
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "interact"),
            Some(25)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "container_click"),
            Some(17)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "level_particles"),
            Some(46)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "set_entity_data"),
            Some(97)
        );
        assert_eq!(
            t.name_of(Phase::Game, Direction::Clientbound, 40),
            Some("horse_screen_open")
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "mount_screen_open"),
            None
        );
        assert_eq!(
            t.id(Phase::Login, Direction::Clientbound, "login_finished"),
            Some(2)
        );
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 65).is_some());
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 66).is_none());
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 138)
                .is_some()
        );
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 139)
                .is_none()
        );
    }

    /// Registration-order anchors for 1.21.8, cross-checked in full against
    /// Mojang's `generated/reports/packets.json`. 1.21.9 inserted the
    /// clientbound debug/game-test packets (shifting everything from
    /// `debug_sample` up) and renamed serverbound 22
    /// (`debug_sample_subscription` -> `debug_subscription_request`);
    /// serverbound ids are otherwise identical to 1.21.10.
    #[test]
    fn anchors_1_21_8() {
        let t = PacketTable::for_protocol(772).unwrap();
        assert_eq!(t.version().protocol, 772);
        assert_eq!(t.version().name, "1.21.8");
        assert_eq!(t.id(Phase::Game, Direction::Serverbound, "attack"), None);
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "interact"),
            Some(25)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "container_click"),
            Some(17)
        );
        assert_eq!(
            t.name_of(Phase::Game, Direction::Serverbound, 22),
            Some("debug_sample_subscription")
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "level_particles"),
            Some(41)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "set_entity_data"),
            Some(92)
        );
        assert_eq!(
            t.name_of(Phase::Game, Direction::Clientbound, 35),
            Some("horse_screen_open")
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "explode"),
            Some(32)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "add_entity"),
            Some(1)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "set_entity_motion"),
            Some(94)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "player_rotation"),
            Some(66)
        );
        assert_eq!(
            t.id(
                Phase::Game,
                Direction::Clientbound,
                "set_default_spawn_position"
            ),
            Some(90)
        );
        assert_eq!(
            t.id(Phase::Login, Direction::Clientbound, "login_finished"),
            Some(2)
        );
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 65).is_some());
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 66).is_none());
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 133)
                .is_some()
        );
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 134)
                .is_none()
        );
    }

    /// 1.21.6's packet tables match 1.21.8's in every phase and direction
    /// (1.21.7 only added registry content — the lava_chicken music disc and
    /// its sound), verified by diffing the two `generated/reports/
    /// packets.json`; asserted in full here.
    #[test]
    fn anchors_1_21_6() {
        let t = PacketTable::for_protocol(771).unwrap();
        assert_eq!(t.version().protocol, 771);
        assert_eq!(t.version().name, "1.21.6");
        let t772 = PacketTable::for_protocol(772).unwrap();
        for phase in PHASES {
            for dir in DIRECTIONS {
                assert_prefix(t, t772, phase, dir, true);
            }
        }
    }

    /// Registration-order anchors for 1.21.5, cross-checked in full against
    /// Mojang's `generated/reports/packets.json`. 1.21.6 appended its
    /// dialog/waypoint clientbound packets (the 1.21.5 clientbound list is a
    /// strict prefix of 1.21.6's) but inserted `change_game_mode` and
    /// `custom_click_action` serverbound, shifting most serverbound ids.
    #[test]
    fn anchors_1_21_5() {
        let t = PacketTable::for_protocol(770).unwrap();
        assert_eq!(t.version().protocol, 770);
        assert_eq!(t.version().name, "1.21.5");
        assert_eq!(t.id(Phase::Game, Direction::Serverbound, "attack"), None);
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "change_game_mode"),
            None
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "interact"),
            Some(24)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "container_click"),
            Some(16)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "player_command"),
            Some(40)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "level_particles"),
            Some(41)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "set_entity_data"),
            Some(92)
        );
        assert_eq!(t.id(Phase::Game, Direction::Clientbound, "waypoint"), None);
        assert_eq!(
            t.id(Phase::Login, Direction::Clientbound, "login_finished"),
            Some(2)
        );
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 63).is_some());
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 64).is_none());
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 130)
                .is_some()
        );
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 131)
                .is_none()
        );
    }

    /// Registration-order anchors for 1.21.4, cross-checked in full against
    /// Mojang's `generated/reports/packets.json`. 1.21.5 removed
    /// `add_experience_orb` (clientbound 2) and inserted
    /// `test_instance_block_status` at 119, so only the clientbound ids
    /// between them shift down one, and inserted `set_test_block` (57) and
    /// `test_instance_block_action` (61) serverbound.
    #[test]
    fn anchors_1_21_4() {
        let t = PacketTable::for_protocol(769).unwrap();
        assert_eq!(t.version().protocol, 769);
        assert_eq!(t.version().name, "1.21.4");
        assert_eq!(t.id(Phase::Game, Direction::Serverbound, "attack"), None);
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "interact"),
            Some(24)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "container_click"),
            Some(16)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "player_command"),
            Some(40)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "add_experience_orb"),
            Some(2)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "player_chat"),
            Some(59)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "level_particles"),
            Some(42)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "set_entity_data"),
            Some(93)
        );
        assert_eq!(
            t.id(Phase::Login, Direction::Clientbound, "login_finished"),
            Some(2)
        );
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 61).is_some());
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 62).is_none());
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 130)
                .is_some()
        );
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 131)
                .is_none()
        );
    }

    /// Registration-order anchors for 1.21.3, cross-checked in full against
    /// Mojang's `generated/reports/packets.json`. Clientbound tables match
    /// 1.21.4's exactly (asserted in full); serverbound, 1.21.4 replaced
    /// `pick_item` with the from_block/from_entity split and added
    /// `player_loaded`, shifting later ids.
    #[test]
    fn anchors_1_21_3() {
        let t = PacketTable::for_protocol(768).unwrap();
        assert_eq!(t.version().protocol, 768);
        assert_eq!(t.version().name, "1.21.3");
        let t769 = PacketTable::for_protocol(769).unwrap();
        assert_prefix(t, t769, Phase::Game, Direction::Clientbound, true);
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "interact"),
            Some(24)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "container_click"),
            Some(16)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "player_command"),
            Some(39)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "pick_item"),
            Some(34)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "move_vehicle"),
            Some(32)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "player_loaded"),
            None
        );
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 59).is_some());
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 60).is_none());
    }

    /// Registration-order anchors for 1.21.1, cross-checked in full against
    /// Mojang's `generated/reports/packets.json`. 1.21.2 rebuilt the recipe
    /// book (dropping `recipe`), renamed clientbound `set_carried_item` to
    /// `set_held_slot` and login `game_profile` to `login_finished`, and
    /// added the position-sync/minecart/inventory packets, shifting most
    /// ids.
    #[test]
    fn anchors_1_21_1() {
        let t = PacketTable::for_protocol(767).unwrap();
        assert_eq!(t.version().protocol, 767);
        assert_eq!(t.version().name, "1.21.1");
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "interact"),
            Some(22)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "container_click"),
            Some(14)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "player_input"),
            Some(38)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "client_tick_end"),
            None
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "player_position"),
            Some(64)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "teleport_entity"),
            Some(112)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "set_carried_item"),
            Some(83)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "recipe"),
            Some(65)
        );
        assert_eq!(
            t.id(Phase::Login, Direction::Clientbound, "game_profile"),
            Some(2)
        );
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 57).is_some());
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 58).is_none());
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 123)
                .is_some()
        );
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 124)
                .is_none()
        );
    }

    /// 1.20.6's tables are a strict prefix of 1.21.1's in every phase and
    /// direction — 1.21 only appended `custom_report_details` and
    /// `server_links` (config + game clientbound) — verified against the
    /// decompiled `<Phase>Protocols.java` registrations and asserted here.
    #[test]
    fn anchors_1_20_6() {
        let t = PacketTable::for_protocol(766).unwrap();
        assert_eq!(t.version().protocol, 766);
        assert_eq!(t.version().name, "1.20.6");
        let t767 = PacketTable::for_protocol(767).unwrap();
        for phase in PHASES {
            for dir in DIRECTIONS {
                assert_prefix(t, t767, phase, dir, false);
            }
        }
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "server_links"),
            None
        );
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 121)
                .is_some()
        );
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 122)
                .is_none()
        );
    }

    /// Registration-order anchors for 1.20.2, spot-checked by hand against
    /// the decompiled `ConnectionProtocol.java` registrations (the last
    /// version before 1.20.3 split resource_pack into pop/push and added
    /// reset_score/ticking packets and the crafter serverbound).
    #[test]
    fn anchors_1_20_2() {
        let t = PacketTable::for_protocol(764).unwrap();
        assert_eq!(t.version().protocol, 764);
        assert_eq!(t.version().name, "1.20.2");
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "interact"),
            Some(18)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "container_click"),
            Some(13)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "resource_pack"),
            Some(39)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "resource_pack"),
            Some(66)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "set_score"),
            Some(93)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "system_chat"),
            Some(103)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "player_info_update"),
            Some(60)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "reset_score"),
            None
        );
        assert_eq!(
            t.id(Phase::Login, Direction::Clientbound, "game_profile"),
            Some(2)
        );
        assert_eq!(
            t.id(
                Phase::Configuration,
                Direction::Clientbound,
                "resource_pack"
            ),
            Some(6)
        );
        assert_eq!(
            t.id(
                Phase::Configuration,
                Direction::Clientbound,
                "registry_data"
            ),
            Some(5)
        );
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 53).is_some());
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 54).is_none());
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 112)
                .is_some()
        );
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 113)
                .is_none()
        );
    }

    /// Registration-order anchors for 1.20.4, spot-checked by hand against
    /// the decompiled `ConnectionProtocol.java` registrations (this version
    /// predates packet resource names and the per-phase Protocols files;
    /// protogen derives the names from the packet class names, which is how
    /// 1.20.5 named them).
    #[test]
    fn anchors_1_20_4() {
        let t = PacketTable::for_protocol(765).unwrap();
        assert_eq!(t.version().protocol, 765);
        assert_eq!(t.version().name, "1.20.4");
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "interact"),
            Some(19)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "container_click"),
            Some(13)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Serverbound, "player_command"),
            Some(34)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "player_position"),
            Some(62)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "container_set_slot"),
            Some(21)
        );
        assert_eq!(
            t.id(Phase::Game, Direction::Clientbound, "update_attributes"),
            Some(113)
        );
        assert_eq!(
            t.id(Phase::Login, Direction::Clientbound, "game_profile"),
            Some(2)
        );
        assert_eq!(
            t.id(
                Phase::Configuration,
                Direction::Clientbound,
                "registry_data"
            ),
            Some(5)
        );
        assert_eq!(
            t.id(
                Phase::Configuration,
                Direction::Serverbound,
                "select_known_packs"
            ),
            None
        );
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 54).is_some());
        assert!(t.name_of(Phase::Game, Direction::Serverbound, 55).is_none());
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 116)
                .is_some()
        );
        assert!(
            t.name_of(Phase::Game, Direction::Clientbound, 117)
                .is_none()
        );
    }

    /// A malformed name silently stops matching across versions: the
    /// pre-1.20.5 tables derive names from packet class names, where a nested
    /// `MovePlayerPacket.Pos` once produced `move_player_packet._pos`.
    #[test]
    fn embedded_names_are_well_formed() {
        for embedded in &EMBEDDED {
            let t = PacketTable::for_protocol(embedded.version.protocol).unwrap();
            for_each_name(t, |phase, dir, id, name| {
                assert!(
                    is_resource_name(name),
                    "{} {phase:?} {dir:?} {id}: malformed name '{name}'",
                    embedded.version.name
                );
            });
        }
    }

    /// `(legacy, named, renames)`: a pre-1.20.5 table whose names protogen
    /// derives from class names, the first version that names those packets
    /// itself, and the names that version changed.
    const LEGACY_NAMES: &[(i32, i32, &[&str])] = &[
        (765, 766, &[]),
        // 1.20.3 split resource_pack into resource_pack_push/_pop and added a
        // UUID, so the join layer needs a rewrite here, not just a rename.
        (764, 765, &["resource_pack"]),
    ];

    /// Every derived name must appear in the version that named the packets
    /// itself, bar the renames, which is what checks the derivation itself;
    /// `embedded_names_are_well_formed` only checks its shape.
    #[test]
    fn legacy_names_match_the_named_version() {
        for &(legacy, named, renames) in LEGACY_NAMES {
            let named_table = PacketTable::for_protocol(named).unwrap();
            for_each_name(
                PacketTable::for_protocol(legacy).unwrap(),
                |phase, dir, id, name| {
                    assert!(
                        named_table.id(phase, dir, name).is_some() || renames.contains(&name),
                        "{legacy} {phase:?} {dir:?} {id}: '{name}' has no {named} equivalent"
                    );
                },
            );
        }
    }

    /// The latest protocol resolves to the shared latest table, every
    /// embedded version to its own table, and anything else to nothing.
    #[test]
    fn for_protocol_lookups() {
        assert!(std::ptr::eq(
            PacketTable::for_protocol(LATEST.protocol).unwrap(),
            PacketTable::latest()
        ));
        for e in &EMBEDDED {
            let protocol = e.version.protocol;
            assert_eq!(
                PacketTable::for_protocol(protocol)
                    .unwrap()
                    .version()
                    .protocol,
                protocol,
                "{}",
                e.version.name
            );
        }
        assert!(PacketTable::for_protocol(0).is_none());
    }

    /// Per-phase counts from the 26.2 registration lists; a regenerated table
    /// that changes these means the game version moved.
    #[test]
    fn counts_26_2() {
        let t = PacketTable::latest();
        let count = |phase, dir| {
            (0..)
                .take_while(|&i| t.name_of(phase, dir, i).is_some())
                .count()
        };
        use Direction::{Clientbound, Serverbound};
        assert_eq!(count(Phase::Handshake, Serverbound), 1);
        assert_eq!(count(Phase::Handshake, Clientbound), 0);
        assert_eq!(count(Phase::Status, Serverbound), 2);
        assert_eq!(count(Phase::Status, Clientbound), 2);
        assert_eq!(count(Phase::Login, Serverbound), 5);
        assert_eq!(count(Phase::Login, Clientbound), 6);
        assert_eq!(count(Phase::Configuration, Serverbound), 10);
        assert_eq!(count(Phase::Configuration, Clientbound), 20);
        assert_eq!(count(Phase::Game, Serverbound), 69);
        assert_eq!(count(Phase::Game, Clientbound), 141);
    }
}
