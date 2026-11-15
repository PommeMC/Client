//! The configuration phase's own packets, as vanilla 26.2 lays them out
//! (`net/minecraft/network/protocol/configuration`). The packets it shares
//! with the game phase (keep-alive, ping, disconnect, tags, resource packs,
//! dialogs) are read and written by those packets' modules.

use crate::codec::{DecodeResult, MAX_STRING_LENGTH, Reader};
use crate::known_packs::KnownPack;
use crate::login::cookie_response;
use crate::packets::{PacketTable, Phase};
use crate::wire::{write_utf, write_varint};

/// `ServerboundSelectKnownPacks`' `ByteBufCodecs.list(64)`; the clientbound
/// list is unbounded.
const MAX_SELECTED_PACKS: usize = 64;

/// What only the configuration phase sends to the server.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerboundConfig {
    /// `ServerboundFinishConfigurationPacket`.
    FinishConfiguration,
    /// `ServerboundSelectKnownPacks`: the offered packs the client has.
    SelectKnownPacks(Vec<KnownPack>),
    /// `ServerboundCookieResponsePacket`.
    CookieResponse {
        key: String,
        payload: Option<Vec<u8>>,
    },
}

impl ServerboundConfig {
    /// The packet's name in the packet table.
    pub fn name(&self) -> &'static str {
        match self {
            Self::FinishConfiguration => "finish_configuration",
            Self::SelectKnownPacks(_) => "select_known_packs",
            Self::CookieResponse { .. } => "cookie_response",
        }
    }

    /// The native frame: the packet id, then the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = PacketTable::native()
            .serverbound_frame(Phase::Configuration, self.name())
            .unwrap_or_else(|| panic!("{} missing from packet table", self.name()));
        match self {
            Self::FinishConfiguration => {}
            Self::SelectKnownPacks(packs) => {
                debug_assert!(packs.len() <= MAX_SELECTED_PACKS);
                write_varint(&mut buf, packs.len() as u32);
                for pack in packs {
                    write_utf(&mut buf, &pack.namespace);
                    write_utf(&mut buf, &pack.id);
                    write_utf(&mut buf, &pack.version);
                }
            }
            Self::CookieResponse { key, payload } => {
                cookie_response(&mut buf, key, payload.as_deref());
            }
        }
        buf
    }
}

/// What only the configuration phase receives.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigPacket {
    RegistryData(RegistryData),
    /// The packs the server can leave out of its registry data.
    SelectKnownPacks(Vec<KnownPack>),
    FinishConfiguration,
    /// The key of the cookie asked for.
    CookieRequest(String),
}

/// `ClientboundRegistryDataPacket`: one synced registry's entries, in
/// protocol-id order.
#[derive(Clone, Debug, PartialEq)]
pub struct RegistryData {
    pub registry: String,
    pub entries: Vec<RegistryEntry>,
}

/// `RegistrySynchronization.PackedRegistryEntry`.
#[derive(Clone, Debug, PartialEq)]
pub struct RegistryEntry {
    pub id: String,
    /// The element as network NBT; absent when a known pack carries it.
    pub data: Option<Box<[u8]>>,
}

/// Decodes configuration packet `name`; `None` if it isn't one this module
/// owns.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<ConfigPacket>> {
    Some(match name {
        "registry_data" => registry_data(r).map(ConfigPacket::RegistryData),
        "select_known_packs" => r
            .list(r.remaining(), known_pack)
            .map(ConfigPacket::SelectKnownPacks),
        "finish_configuration" => Ok(ConfigPacket::FinishConfiguration),
        "cookie_request" => r
            .identifier()
            .map(|key| ConfigPacket::CookieRequest(key.to_owned())),
        _ => return None,
    })
}

fn registry_data(r: &mut Reader) -> DecodeResult<RegistryData> {
    Ok(RegistryData {
        registry: r.identifier()?.to_owned(),
        entries: r.list(r.remaining(), |r| {
            Ok(RegistryEntry {
                id: r.identifier()?.to_owned(),
                data: r.optional(|r| Ok(r.nbt()?.into()))?,
            })
        })?,
    })
}

/// `KnownPack.STREAM_CODEC`.
fn known_pack(r: &mut Reader) -> DecodeResult<KnownPack> {
    Ok(KnownPack {
        namespace: r.utf(MAX_STRING_LENGTH)?.to_owned(),
        id: r.utf(MAX_STRING_LENGTH)?.to_owned(),
        version: r.utf(MAX_STRING_LENGTH)?.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vanilla_fixtures::case;

    fn core() -> KnownPack {
        KnownPack {
            namespace: "minecraft".into(),
            id: "core".into(),
            version: "26.2".into(),
        }
    }

    #[test]
    fn serverbound_packets_encode_as_vanilla_does() {
        let packets = [
            ServerboundConfig::FinishConfiguration,
            ServerboundConfig::SelectKnownPacks(vec![core()]),
            ServerboundConfig::CookieResponse {
                key: "minecraft:session".into(),
                payload: Some(vec![1, 2, 3]),
            },
        ];
        for packet in packets {
            assert_eq!(
                packet.encode()[1..],
                case(&format!("configuration/serverbound/{}", packet.name())),
                "{}",
                packet.name()
            );
        }
    }

    /// Fixture `name` through this module's reader.
    fn read_case(name: &str) -> ConfigPacket {
        let payload = case(&format!("configuration/clientbound/{name}"));
        let mut r = Reader::new(&payload);
        let packet = read(name, &mut r).unwrap().unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    #[test]
    fn clientbound_packets_match_vanilla_encoder() {
        let ConfigPacket::RegistryData(data) = read_case("registry_data") else {
            panic!("not registry data");
        };
        assert_eq!(data.registry, "minecraft:dimension_type");
        let [sent, known] = &data.entries[..] else {
            panic!("{} entries", data.entries.len());
        };
        assert_eq!(sent.id, "minecraft:overworld");
        // A compound: tag 10, its fields, then the end tag.
        let nbt = sent.data.as_deref().unwrap();
        assert_eq!((nbt[0], nbt[nbt.len() - 1]), (10, 0));
        assert_eq!(
            *known,
            RegistryEntry {
                id: "minecraft:the_nether".into(),
                data: None,
            }
        );

        assert_eq!(
            read_case("select_known_packs"),
            ConfigPacket::SelectKnownPacks(vec![core()])
        );
        assert_eq!(
            read_case("finish_configuration"),
            ConfigPacket::FinishConfiguration
        );
        assert_eq!(
            read_case("cookie_request"),
            ConfigPacket::CookieRequest("minecraft:session".into())
        );
    }

    #[test]
    fn other_packets_are_left_to_their_modules() {
        assert!(read("keep_alive", &mut Reader::new(&[])).is_none());
    }
}
