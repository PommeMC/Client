//! `resource_pack_push` and `resource_pack_pop` as vanilla 26.2 lays them
//! out (`net/minecraft/network/protocol/common`).

use crate::codec::{DecodeResult, MAX_STRING_LENGTH, Reader};

#[derive(Clone, Debug, PartialEq)]
pub enum ResourcePackPacket {
    Push {
        id: u128,
        url: String,
        /// SHA-1 hex, at most 40 characters; empty when the server gives none.
        hash: String,
        required: bool,
        /// The prompt's text component, as NBT.
        prompt: Option<Box<[u8]>>,
    },
    /// `None` removes every server pack.
    Pop { id: Option<u128> },
}

/// Decodes packet `name`, or `None` if it isn't a resource pack packet.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<ResourcePackPacket>> {
    let read: fn(&mut Reader) -> DecodeResult<ResourcePackPacket> = match name {
        // ClientboundResourcePackPushPacket.java:25
        "resource_pack_push" => |r| {
            Ok(ResourcePackPacket::Push {
                id: r.uuid()?,
                url: r.utf(MAX_STRING_LENGTH)?.to_owned(),
                hash: r.utf(40)?.to_owned(),
                required: r.bool()?,
                prompt: if r.bool()? {
                    Some(r.nbt()?.into())
                } else {
                    None
                },
            })
        },
        // ClientboundResourcePackPopPacket.java:20-22
        "resource_pack_pop" => |r| {
            Ok(ResourcePackPacket::Pop {
                id: if r.bool()? { Some(r.uuid()?) } else { None },
            })
        },
        _ => return None,
    };
    Some(read(r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vanilla_fixtures::case;

    fn decode(name: &str, bytes: &[u8]) -> ResourcePackPacket {
        let mut r = Reader::new(bytes);
        let packet = read(name, &mut r).expect("a resource pack packet").unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    #[test]
    fn push_and_pop_read_as_vanilla_writes_them() {
        let ResourcePackPacket::Push {
            id,
            url,
            hash,
            required,
            prompt,
        } = decode("resource_pack_push", &case("resource_pack_push/prompt"))
        else {
            panic!("expected a push");
        };
        assert_eq!(id, 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210);
        assert_eq!(url, "https://example.com/pack.zip");
        assert_eq!(hash, "0123456789abcdef0123456789abcdef01234567");
        assert!(required);
        assert!(prompt.is_some());
        assert_eq!(
            decode("resource_pack_pop", &case("resource_pack_pop/all")),
            ResourcePackPacket::Pop { id: None }
        );
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("waypoint", &mut Reader::new(&[])).is_none());
    }
}
