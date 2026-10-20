//! Sound packets, as vanilla 26.2 lays them out
//! (`net/minecraft/network/protocol/game/Clientbound*SoundPacket`).

use crate::codec::{DecodeResult, Reader};

#[derive(Clone, Debug, PartialEq)]
pub enum SoundPacket {
    Sound {
        sound: SoundEvent,
        /// A `SoundSource` ordinal.
        source: u32,
        /// Fixed-point: the block position times 8.
        x: i32,
        y: i32,
        z: i32,
        volume: f32,
        pitch: f32,
        seed: i64,
    },
    Entity {
        sound: SoundEvent,
        source: u32,
        entity: i32,
        volume: f32,
        pitch: f32,
        seed: i64,
    },
    Stop {
        source: Option<u32>,
        name: Option<String>,
    },
}

/// `SoundEvent.STREAM_CODEC` (SoundEvent.java:30-31): a registry holder.
#[derive(Clone, Debug, PartialEq)]
pub enum SoundEvent {
    /// A `sound_event` registry id.
    Registered(u32),
    Direct {
        location: String,
        fixed_range: Option<f32>,
    },
}

/// Decodes packet `name`, or `None` if it isn't a sound packet.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<SoundPacket>> {
    let read: fn(&mut Reader) -> DecodeResult<SoundPacket> = match name {
        // ClientboundSoundPacket.java:41-48
        "sound" => |r| {
            Ok(SoundPacket::Sound {
                sound: sound_event(r)?,
                source: r.varint()?,
                x: r.i32()?,
                y: r.i32()?,
                z: r.i32()?,
                volume: r.f32()?,
                pitch: r.f32()?,
                seed: r.i64()?,
            })
        },
        // ClientboundSoundEntityPacket.java:37-42
        "sound_entity" => |r| {
            Ok(SoundPacket::Entity {
                sound: sound_event(r)?,
                source: r.varint()?,
                entity: r.var_i32()?,
                volume: r.f32()?,
                pitch: r.f32()?,
                seed: r.i64()?,
            })
        },
        // ClientboundStopSoundPacket.java:33-35
        "stop_sound" => |r| {
            let flags = r.u8()?;
            Ok(SoundPacket::Stop {
                source: if flags & 1 != 0 {
                    Some(r.varint()?)
                } else {
                    None
                },
                name: if flags & 2 != 0 {
                    Some(r.identifier()?.to_owned())
                } else {
                    None
                },
            })
        },
        _ => return None,
    };
    Some(read(r))
}

/// `ByteBufCodecs.holder`: 0 for a direct value, else the registry id + 1.
fn sound_event(r: &mut Reader) -> DecodeResult<SoundEvent> {
    Ok(match r.varint()? {
        // SoundEvent.DIRECT_STREAM_CODEC
        0 => SoundEvent::Direct {
            location: r.identifier()?.to_owned(),
            fixed_range: r.optional(Reader::f32)?,
        },
        id => SoundEvent::Registered(id - 1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::write_utf;

    fn decode(name: &str, bytes: &[u8]) -> SoundPacket {
        let mut r = Reader::new(bytes);
        let packet = read(name, &mut r).expect("a sound packet").unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    fn utf(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        write_utf(&mut out, s);
        out
    }

    /// Volume 0.5, pitch 2, seed 7.
    fn tail() -> Vec<u8> {
        [
            &0.5f32.to_be_bytes()[..],
            &2f32.to_be_bytes(),
            &7i64.to_be_bytes(),
        ]
        .concat()
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("block_update", &mut Reader::new(&[])).is_none());
    }

    #[test]
    fn sound_with_a_registered_event_and_the_ui_source() {
        let position = [-8i32, 520, 16].map(i32::to_be_bytes).concat();
        let bytes = [&[42, 10][..], &position, &tail()].concat();
        assert_eq!(
            decode("sound", &bytes),
            SoundPacket::Sound {
                sound: SoundEvent::Registered(41),
                source: 10,
                x: -8,
                y: 520,
                z: 16,
                volume: 0.5,
                pitch: 2.0,
                seed: 7,
            }
        );
    }

    #[test]
    fn entity_sound_with_direct_events() {
        let ranged = [&[0][..], &utf("custom.thing"), &[1], &16f32.to_be_bytes()].concat();
        let bytes = [&ranged[..], &[3, 9], &tail()].concat();
        assert_eq!(
            decode("sound_entity", &bytes),
            SoundPacket::Entity {
                sound: SoundEvent::Direct {
                    location: "custom.thing".into(),
                    fixed_range: Some(16.0),
                },
                source: 3,
                entity: 9,
                volume: 0.5,
                pitch: 2.0,
                seed: 7,
            }
        );
        let unranged = [&[0][..], &utf("a:b"), &[0]].concat();
        let mut r = Reader::new(&unranged);
        assert_eq!(
            sound_event(&mut r),
            Ok(SoundEvent::Direct {
                location: "a:b".into(),
                fixed_range: None,
            })
        );
    }

    #[test]
    fn stop_sound_flags() {
        let stop = |source, name: Option<&str>| SoundPacket::Stop {
            source,
            name: name.map(Into::into),
        };
        assert_eq!(decode("stop_sound", &[0]), stop(None, None));
        assert_eq!(decode("stop_sound", &[1, 10]), stop(Some(10), None));
        let named = [&[2][..], &utf("a:b")].concat();
        assert_eq!(decode("stop_sound", &named), stop(None, Some("a:b")));
        let both = [&[3, 4][..], &utf("a:b")].concat();
        assert_eq!(decode("stop_sound", &both), stop(Some(4), Some("a:b")));
    }
}
