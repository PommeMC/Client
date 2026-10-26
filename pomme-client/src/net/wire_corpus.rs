//! Replays what each tested version's vanilla server sent during
//! `world/join` (`wire-fixtures/<version>/world/join.bin`, recorded by
//! `just wire-fixtures`) through the translation and the readers the
//! connection uses, in `connection.rs`'s order, so every version is checked
//! against bytes vanilla produced rather than hand-built frames.

use std::collections::BTreeSet;
use std::io::Read;

use azalea_core::registry_holder::RegistryHolder;
use azalea_protocol::packets::ProtocolPacket;
use azalea_protocol::packets::config::ClientboundConfigPacket;
use azalea_protocol::packets::game::ClientboundGamePacket;
use azalea_protocol::packets::login::ClientboundLoginPacket;
use azalea_protocol::read::deserialize_packet;
use pomme_protocol::chunk::ChunkPacket;
use pomme_protocol::section::Section;
use pomme_protocol::version::{ProtocolVersion, tested_versions};
use pomme_protocol::{Direction, PacketTable, Phase};

use super::chat::{ChatPacketError, InboundChat};
use super::chunk_batch::ChunkBatchSizeCalculator;
use super::sender::PacketSender;
use super::translate::Translation;
use crate::world::block::BlockStateTable;

/// Packets a version's join drops or misreads: the translation's deliberate
/// drops, and open bugs marked `TODO:`. A gap must still fail on every version
/// it names, so a fix forces its entry out.
const KNOWN_GAPS: &[Gap] = &[
    // TODO: parser ids pass through unremapped, where 26.2 inserted parsers.
    gap("1.20.1", "1.21.5", "commands", "parser ids"),
    // TODO: 1.20.4's trailing `enforcesSecureChat` bool isn't stripped.
    gap("1.20.4", "1.20.4", "server_data", "trailing bool"),
    // TODO: one 26.3 registry element doesn't decode; `update_recipes`
    // is dropped (see `translate_update_recipes_777`).
    gap("26.3", "26.3", "registry_data", "an element"),
    gap("26.3", "26.3", "update_recipes", "dropped"),
    gap("1.20.1", "1.21.1", "recipe", "no native equivalent"),
    gap(
        "1.20.1",
        "1.21.1",
        "update_recipes",
        "no mechanical mapping",
    ),
    gap("1.20.1", "1.20.4", "update_advancements", "old-form icons"),
    gap("1.20.1", "1.20.2", "server_data", "never read"),
    gap("1.20.1", "1.20.1", "update_enabled_features", "never read"),
    gap("26.3", "26.3", "post_effects", "no native equivalent"),
];

struct Gap {
    /// Oldest and newest version it covers.
    versions: (&'static str, &'static str),
    /// The packet's name on the wire version.
    packet: &'static str,
    why: &'static str,
}

const fn gap(
    oldest: &'static str,
    newest: &'static str,
    packet: &'static str,
    why: &'static str,
) -> Gap {
    Gap {
        versions: (oldest, newest),
        packet,
        why,
    }
}

impl Gap {
    fn covers(&self, version: &ProtocolVersion, packet: &str) -> bool {
        let protocol = |name| ProtocolVersion::from_name(name).unwrap().protocol;
        let (oldest, newest) = (protocol(self.versions.0), protocol(self.versions.1));
        self.packet == packet && (oldest..=newest).contains(&version.protocol)
    }
}

#[test]
fn every_version_reads_its_join_traffic() {
    let mut unexpected = Vec::new();
    let mut gaps_hit = BTreeSet::new();
    for version in tested_versions() {
        for (packet, error) in replay(&version) {
            match KNOWN_GAPS.iter().position(|g| g.covers(&version, &packet)) {
                Some(gap) => {
                    gaps_hit.insert((gap, version.name));
                }
                None => unexpected.push(format!("{} {packet}: {error}", version.name)),
            }
        }
    }
    for (i, gap) in KNOWN_GAPS.iter().enumerate() {
        for version in tested_versions() {
            if gap.covers(&version, gap.packet) && !gaps_hit.contains(&(i, version.name)) {
                unexpected.push(format!(
                    "{} {} ({}): reads now; drop it from KNOWN_GAPS",
                    version.name, gap.packet, gap.why
                ));
            }
        }
    }
    assert!(unexpected.is_empty(), "\n{}", unexpected.join("\n"));
}

/// Every frame of `version`'s fixture that the client would drop or misread,
/// as `(wire packet name, why)`.
fn replay(version: &ProtocolVersion) -> Vec<(String, String)> {
    let path = format!(
        "{}/src/net/wire-fixtures/{}/world/join.bin",
        env!("CARGO_MANIFEST_DIR"),
        version.name
    );
    let Ok(deflated) = std::fs::read(&path) else {
        let fix = format!("no fixture; run `just wire-fixtures {}`", version.name);
        return vec![("-".into(), fix)];
    };
    let mut bytes = Vec::new();
    flate2::read::DeflateDecoder::new(&deflated[..])
        .read_to_end(&mut bytes)
        .expect("fixture inflates");
    let records = pomme_protocol::capture::records(&bytes).expect("fixture records");

    let protocol = version.protocol;
    let wire = PacketTable::for_protocol(protocol).expect("wire table");
    let translation = Translation::for_protocol(protocol);
    let mut session = Session::new(protocol);
    let mut failures = Vec::new();
    for (phase, frame) in records {
        let id = pomme_protocol::codec::Reader::new(frame).varint().unwrap();
        let packet = wire
            .name_of(phase, Direction::Clientbound, id)
            .unwrap_or("?")
            .to_owned();
        let read = match phase {
            Phase::Login => session.login(translation.as_ref(), frame),
            Phase::Configuration => session.config(translation.as_ref(), frame),
            _ => session.game(translation.as_ref(), frame),
        };
        if let Err(error) = read {
            failures.push((packet, error));
        }
    }
    failures
}

/// azalea's typed decode, as the connection falls back to.
fn decode<P: ProtocolPacket + std::fmt::Debug>(frame: &[u8]) -> Result<P, String> {
    deserialize_packet(&mut std::io::Cursor::new(frame)).map_err(|e| e.to_string())
}

/// The connection state the readers consult, built up as the frames arrive.
struct Session {
    protocol: i32,
    registries: RegistryHolder,
    known_packs: bool,
    inbound_chat: InboundChat,
    config_less_login: bool,
    sender: PacketSender,
    batches: ChunkBatchSizeCalculator,
    events: crossbeam_channel::Sender<super::NetworkEvent>,
    events_rx: crossbeam_channel::Receiver<super::NetworkEvent>,
    /// The login dimension's section count, which every chunk must carry.
    sections: Option<u32>,
    // Kept so the readers' sends succeed.
    _outbound: tokio::sync::mpsc::UnboundedReceiver<super::sender::Outbound>,
}

impl Session {
    fn new(protocol: i32) -> Self {
        let (out_tx, outbound) = tokio::sync::mpsc::unbounded_channel();
        let (events, events_rx) = crossbeam_channel::unbounded();
        Self {
            protocol,
            registries: RegistryHolder::default(),
            known_packs: false,
            inbound_chat: InboundChat::for_protocol(protocol),
            config_less_login: true,
            sender: PacketSender::new(out_tx),
            batches: ChunkBatchSizeCalculator::default(),
            events,
            events_rx,
            sections: None,
            _outbound: outbound,
        }
    }

    fn login(&mut self, translation: Option<&Translation>, frame: &[u8]) -> Result<(), String> {
        let frame = match translation {
            Some(t) => t.translate_login_frame(frame.into()),
            None => frame.into(),
        };
        decode::<ClientboundLoginPacket>(&frame).map(drop)
    }

    fn config(&mut self, translation: Option<&Translation>, frame: &[u8]) -> Result<(), String> {
        let frames = match translation {
            Some(t) => t.translate_config_frame(frame.into()),
            None => vec![frame.into()],
        };
        for frame in frames {
            if !super::dialog::handle_raw_dialog_packet(Phase::Configuration, &frame, &self.events)
            {
                self.configure(&frame)?;
            }
        }
        Ok(())
    }

    /// The registry state `connection.rs`'s config loop keeps.
    fn configure(&mut self, frame: &[u8]) -> Result<(), String> {
        match decode::<ClientboundConfigPacket>(frame)? {
            ClientboundConfigPacket::RegistryData(p) => {
                let entries = if self.known_packs {
                    super::known_packs::fill_known_entries(
                        self.protocol,
                        &p.registry_id,
                        p.entries,
                    )?
                } else {
                    p.entries
                };
                self.registries.append(p.registry_id, entries);
            }
            ClientboundConfigPacket::SelectKnownPacks(p) => {
                self.known_packs =
                    !super::known_packs::select_packs(self.protocol, &p.known_packs).is_empty();
            }
            _ => {}
        }
        Ok(())
    }

    fn game(&mut self, translation: Option<&Translation>, frame: &[u8]) -> Result<(), String> {
        // 1.20.1 carries the registries in its first game frame, `login`.
        if let Some(t) = translation.filter(|t| t.no_config_phase())
            && std::mem::take(&mut self.config_less_login)
        {
            let frames = t
                .split_login_registries(frame)
                .ok_or("no registries in the login frame")?;
            for frame in frames {
                self.configure(&frame)?;
            }
        }
        let raw = match translation {
            Some(t) => t
                .translate_game_frame(frame.into())
                .ok_or("dropped by the translation")?,
            None => frame.into(),
        };

        let chat_types = super::connection::chat_types_from_registry_holder(&self.registries);
        match super::chat::handle_raw_chat_packet(
            &raw,
            &self.events,
            &chat_types,
            &mut self.inbound_chat,
        ) {
            Some(Err(ChatPacketError::Malformed(e))) => return Err(e.to_string()),
            Some(Err(ChatPacketError::Disconnect(key))) => return Err(key.to_string()),
            Some(Ok(())) => return Ok(()),
            None => {}
        }
        if let Some(login) = super::lifecycle_wire::read_login(&raw) {
            self.inbound_chat.reset();
            let login = login.map_err(|e| e.to_string())?;
            super::lifecycle_wire::login_events(&login, &self.registries, &self.events);
            self.sections = self.events_rx.try_iter().find_map(|event| match event {
                super::NetworkEvent::DimensionInfo(dimension) => Some(dimension.height / 16),
                _ => None,
            });
            return Ok(());
        }
        if super::dialog::handle_raw_dialog_packet(Phase::Game, &raw, &self.events) {
            return Ok(());
        }
        let blocks = BlockStateTable::for_protocol(self.protocol);
        if let Some((_, read)) = super::handler::decode_raw_game_packet(
            &raw,
            blocks,
            &self.registries,
            &self.sender,
            &mut self.batches,
            &self.events,
        ) {
            read?;
            return self.chunk_sections(&raw, blocks);
        }
        decode::<ClientboundGamePacket>(&raw).map(drop)
    }

    /// A chunk's sections as `Chunk::read` parses them once the world loads
    /// the column: exactly the login dimension's count, using up the buffer.
    /// A translation that misframes the sections still reads, just as garbage
    /// and more or fewer of them.
    fn chunk_sections(&self, raw: &[u8], blocks: BlockStateTable) -> Result<(), String> {
        let mut r = pomme_protocol::codec::Reader::new(raw);
        let id = r.varint().map_err(|e| e.to_string())?;
        let name = PacketTable::native()
            .name_of(Phase::Game, Direction::Clientbound, id)
            .unwrap_or_default();
        let Some(Ok(ChunkPacket::Level { sections, .. })) =
            pomme_protocol::chunk::read(name, &mut r)
        else {
            return Ok(());
        };
        let biomes = super::connection::biome_count(&self.registries);
        let expected = self
            .sections
            .ok_or("a chunk before the login's dimension")?;
        let mut r = pomme_protocol::codec::Reader::new(&sections);
        let mut count = 0;
        while r.remaining() > 0 {
            Section::read(&mut r, blocks.state_count(), biomes).map_err(|e| e.to_string())?;
            count += 1;
        }
        if count != expected {
            return Err(format!("{count} sections, the dimension has {expected}"));
        }
        Ok(())
    }
}
