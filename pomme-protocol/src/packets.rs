use std::collections::HashMap;
use std::sync::OnceLock;

use crate::version::{LATEST, ProtocolVersion};

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
