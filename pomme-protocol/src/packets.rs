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
            t.name_of(Phase::Game, Direction::Clientbound, 0),
            Some("bundle_delimiter")
        );
        assert_eq!(
            t.id(Phase::Handshake, Direction::Serverbound, "intention"),
            Some(0)
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

    #[test]
    fn for_protocol_lookups() {
        assert!(std::ptr::eq(
            PacketTable::for_protocol(776).unwrap(),
            PacketTable::latest()
        ));
        assert!(PacketTable::for_protocol(772).is_none());
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
