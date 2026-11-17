//! Configuration-phase frames, decoded by pomme, and the registries collected
//! from them.

use azalea_core::registry_holder::RegistryHolder;
use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::configuration::{self, ConfigPacket, RegistryData};
use pomme_protocol::lifecycle::{self, LifecyclePacket};
use pomme_protocol::resource_pack::{self, ResourcePackPacket};
use pomme_protocol::tags::{self, Tags};
use pomme_protocol::{Direction, KnownPack, PacketTable, Phase};
use pomme_registry::identifier::Identifier;
use simdnbt::owned::NbtTag;

/// A configuration packet pomme acts on, by the module that reads it.
#[derive(Debug)]
pub(super) enum ConfigFrame {
    Config(ConfigPacket),
    /// Keep-alive, ping and disconnect, laid out as in the game phase.
    Lifecycle(LifecyclePacket),
    Tags(Tags),
    ResourcePack(ResourcePackPacket),
}

// TODO: vanilla shows `code_of_conduct` and answers it, stores and returns
// cookies, and follows `transfer`.
const IGNORED_PACKETS: &[&str] = &[
    "code_of_conduct",
    "custom_payload",
    "custom_report_details",
    "reset_chat",
    "store_cookie",
    "transfer",
    "update_enabled_features",
];

/// Decodes a native configuration frame; `None` for a packet pomme ignores.
/// The dialog packets and `server_links` are read by
/// `dialog::handle_raw_dialog_packet` first.
pub(super) fn read(frame: &[u8]) -> Option<DecodeResult<ConfigFrame>> {
    let mut r = Reader::new(frame);
    let name = match r.varint() {
        Ok(id) => PacketTable::native().name_of(Phase::Configuration, Direction::Clientbound, id),
        Err(e) => return Some(Err(e)),
    };
    let Some(name) = name else {
        return Some(Err(r.error("unknown packet")));
    };
    if IGNORED_PACKETS.contains(&name) {
        return None;
    }
    let r = &mut r;
    let packet = configuration::read(name, r)
        .map(|packet| packet.map(ConfigFrame::Config))
        .or_else(|| lifecycle::read(name, r).map(|packet| packet.map(ConfigFrame::Lifecycle)))
        .or_else(|| tags::read(name, r).map(|packet| packet.map(ConfigFrame::Tags)))
        .or_else(|| {
            resource_pack::read(name, r).map(|packet| packet.map(ConfigFrame::ResourcePack))
        })
        .unwrap_or_else(|| Err(r.error("unknown packet")));
    Some(packet.and_then(|packet| {
        r.ensure_eof()?;
        Ok(packet)
    }))
}

/// [`read`], with a frame that doesn't decode logged and skipped, as the
/// connection treats one.
pub(super) fn read_or_skip(frame: &[u8]) -> Option<ConfigFrame> {
    read(frame)?
        .map_err(|e| tracing::warn!("Skipping malformed packet: {e}"))
        .ok()
}

/// Vanilla `RegistryDataCollector` and `KnownPacksManager`: the registries a
/// server syncs, with the entries it left to a known pack filled in.
#[derive(Default)]
pub(super) struct Registries {
    pub holder: RegistryHolder,
    /// Whether any registry was appended. A reconfiguration that appends
    /// none keeps the registries it had.
    pub received: bool,
    selected_known_packs: bool,
}

impl Registries {
    /// Vanilla `handleSelectKnownPacks`: the offered packs to claim, so the
    /// server can skip their registry data.
    pub fn select_packs(&mut self, protocol: i32, offered: &[KnownPack]) -> Vec<KnownPack> {
        let known = super::known_packs::select_packs(protocol, offered);
        self.selected_known_packs = !known.is_empty();
        known
    }

    /// [`Self::append`] as the connection applies it: a registry the holder
    /// can't take is logged and left out; `Err` is a reason to disconnect.
    pub fn append_or_skip(&mut self, protocol: i32, data: RegistryData) -> Result<(), String> {
        match self.append(protocol, data) {
            Err(RegistryError::Unsupported(why)) => {
                tracing::warn!("Skipping registry: {why}");
                Ok(())
            }
            Err(RegistryError::MissingElement(why)) => Err(why),
            Ok(()) => Ok(()),
        }
    }

    /// Appends one registry. The server omits the data of every entry a pack
    /// we claimed carries, so those are filled in before azalea drops them.
    pub fn append(&mut self, protocol: i32, data: RegistryData) -> Result<(), RegistryError> {
        let registry = Identifier::new(&data.registry);
        let entries: Option<Vec<_>> = data
            .entries
            .into_iter()
            .map(|entry| {
                let data = match entry.data {
                    Some(nbt) => Some(compound(&nbt)?),
                    None => None,
                };
                Some((Identifier::new(&entry.id), data))
            })
            .collect();
        let Some(entries) = entries else {
            return Err(RegistryError::not_a_compound(&registry));
        };
        let entries = if self.selected_known_packs {
            super::known_packs::fill_known_entries(protocol, &registry, entries)?
        } else {
            entries
        };
        self.holder.append(registry, entries);
        self.received = true;
        Ok(())
    }
}

/// Why a registry wasn't appended.
#[derive(Debug, thiserror::Error)]
pub(super) enum RegistryError {
    /// azalea's holder can't hold it.
    #[error("{0}")]
    Unsupported(String),
    /// A claimed pack lacks an element the server left out; vanilla
    /// disconnects.
    #[error("{0}")]
    MissingElement(String),
}

impl RegistryError {
    // TODO: vanilla takes any tag as an element (26.3's `block_transformer`
    // entries are lists); azalea's holder only compounds, so such a registry
    // is left out whole.
    pub fn not_a_compound(registry: &Identifier) -> Self {
        Self::Unsupported(format!("an element of {registry} isn't a compound"))
    }
}

/// A registry element's network NBT, when it is a compound.
fn compound(nbt: &[u8]) -> Option<simdnbt::owned::NbtCompound> {
    match simdnbt::owned::read_tag(&mut std::io::Cursor::new(nbt)) {
        Ok(NbtTag::Compound(compound)) => Some(compound),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use pomme_protocol::version::NATIVE;
    use pomme_protocol::wire::{write_utf, write_varint};

    use super::*;

    fn frame(name: &str, body: &[u8]) -> Vec<u8> {
        let id = PacketTable::native()
            .id(Phase::Configuration, Direction::Clientbound, name)
            .unwrap();
        let mut frame = Vec::new();
        write_varint(&mut frame, id);
        frame.extend_from_slice(body);
        frame
    }

    #[test]
    fn each_packet_goes_to_the_module_that_reads_it() {
        assert!(matches!(
            read(&frame("finish_configuration", &[])),
            Some(Ok(ConfigFrame::Config(ConfigPacket::FinishConfiguration)))
        ));
        assert!(matches!(
            read(&frame("keep_alive", &7i64.to_be_bytes())),
            Some(Ok(ConfigFrame::Lifecycle(LifecyclePacket::KeepAlive(7))))
        ));
        assert!(matches!(
            read(&frame("update_tags", &[0])),
            Some(Ok(ConfigFrame::Tags(tags))) if tags.is_empty()
        ));
        assert!(matches!(
            read(&frame("resource_pack_pop", &[0])),
            Some(Ok(ConfigFrame::ResourcePack(ResourcePackPacket::Pop {
                id: None
            })))
        ));
    }

    #[test]
    fn ignored_packets_are_skipped_and_bad_ones_are_errors() {
        assert!(read(&frame("store_cookie", &[1, 2, 3])).is_none());
        // Bytes left over, a truncated body, and an id past the table.
        assert!(matches!(
            read(&frame("finish_configuration", &[0])),
            Some(Err(_))
        ));
        assert!(matches!(read(&frame("keep_alive", &[0])), Some(Err(_))));
        assert!(matches!(read(&[0x7f]), Some(Err(_))));
    }

    /// One registry frame: `plains` with data, `desert` without.
    fn biomes() -> RegistryData {
        let mut body = Vec::new();
        write_utf(&mut body, "minecraft:worldgen/biome");
        write_varint(&mut body, 2);
        write_utf(&mut body, "minecraft:plains");
        // Present, then a compound holding the byte `has_precipitation`.
        body.extend([1, 10, 1, 0, 17]);
        body.extend(b"has_precipitation");
        body.extend([1, 0]);
        write_utf(&mut body, "minecraft:desert");
        body.push(0);
        match read(&frame("registry_data", &body)) {
            Some(Ok(ConfigFrame::Config(ConfigPacket::RegistryData(data)))) => data,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_claimed_pack_fills_the_entries_sent_without_data() {
        let core = KnownPack {
            namespace: "minecraft".into(),
            id: "core".into(),
            version: NATIVE.name.into(),
        };
        let mut registries = Registries::default();
        assert_eq!(
            registries.select_packs(NATIVE.protocol, std::slice::from_ref(&core)),
            [core]
        );
        registries.append(NATIVE.protocol, biomes()).unwrap();
        assert!(registries.received);
        let biomes = crate::net::connection::biome_registry(&registries.holder).unwrap();
        let ids: Vec<String> = biomes.map.keys().map(ToString::to_string).collect();
        assert_eq!(ids, ["minecraft:plains", "minecraft:desert"]);
    }

    /// 26.3 has its own core pack, and its `block_transformer` elements are
    /// lists, which the holder can't take from the pack either.
    #[test]
    fn a_translated_version_claims_and_fills_from_its_own_pack() {
        let core = KnownPack {
            namespace: "minecraft".into(),
            id: "core".into(),
            version: "26.3".into(),
        };
        let mut registries = Registries::default();
        assert!(registries.select_packs(777, &[]).is_empty());
        assert_eq!(
            registries.select_packs(777, std::slice::from_ref(&core)),
            [core]
        );
        registries.append(777, biomes()).unwrap();
        assert!(crate::net::connection::biome_registry(&registries.holder).is_some());

        let mut transformers = biomes();
        transformers.registry = "minecraft:block_transformer".into();
        transformers.entries.truncate(1);
        transformers.entries[0].id = "minecraft:axe".into();
        transformers.entries[0].data = None;
        assert!(matches!(
            registries.append(777, transformers.clone()),
            Err(RegistryError::Unsupported(_))
        ));
        assert_eq!(registries.append_or_skip(777, transformers), Ok(()));
    }

    #[test]
    fn a_registry_with_a_non_compound_element_is_left_out() {
        let mut data = biomes();
        // A list of no elements.
        data.entries[0].data = Some([9, 0, 0, 0, 0, 0].into());
        let mut registries = Registries::default();
        assert!(matches!(
            registries.append(NATIVE.protocol, data.clone()),
            Err(RegistryError::Unsupported(_))
        ));
        assert_eq!(registries.append_or_skip(NATIVE.protocol, data), Ok(()));
        assert!(!registries.received);
        assert_eq!(crate::net::connection::biome_count(&registries.holder), 0);
    }

    #[test]
    fn an_unclaimed_registry_is_appended_as_sent() {
        let mut registries = Registries::default();
        assert!(registries.select_packs(NATIVE.protocol, &[]).is_empty());
        registries.append(NATIVE.protocol, biomes()).unwrap();
        // azalea drops the entry that came without data.
        assert_eq!(crate::net::connection::biome_count(&registries.holder), 1);
    }
}
