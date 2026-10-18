//! Sound packets, decoded by pomme for every session: translated frames
//! already carry the native layout, and sound event ids are remapped from
//! the wire version's.

use crossbeam_channel::Sender;
use pomme_protocol::codec::DecodeResult;
use pomme_protocol::sound::{self, SoundEvent, SoundPacket};
use pomme_protocol::{ClientRegistry, RegistryTable};

use super::NetworkEvent;
use super::translate::native_id;
use crate::audio::SoundRef;
use crate::entity::components::Position;

/// Decodes a sound packet; `None` for any other packet.
pub(super) fn handle_raw_sound_packet(
    packet_id: u32,
    payload: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, sound::read)?.map(|packet| {
            if let Some(event) = event(packet) {
                let _ = event_tx.try_send(event);
            }
        }),
    )
}

/// The client event for a packet; `None` for a sound with no native event.
fn event(packet: SoundPacket) -> Option<NetworkEvent> {
    Some(match packet {
        SoundPacket::Sound {
            sound,
            source,
            x,
            y,
            z,
            volume,
            pitch,
            seed,
        } => NetworkEvent::PlaySound {
            sound: sound_ref(sound)?,
            category: source as u8,
            pos: Position::new(x as f64 / 8.0, y as f64 / 8.0, z as f64 / 8.0),
            volume,
            pitch,
            seed: seed as u64,
        },
        SoundPacket::Entity {
            sound,
            source,
            entity,
            volume,
            pitch,
            seed,
        } => NetworkEvent::PlayEntitySound {
            sound: sound_ref(sound)?,
            category: source as u8,
            entity_id: entity,
            volume,
            pitch,
            seed: seed as u64,
        },
        SoundPacket::Stop { source, name } => NetworkEvent::StopSound {
            sound_id: name,
            category: source.map(|source| source as u8),
        },
    })
}

/// A registered event by its native name, its id remapped from the wire
/// version's, or a direct event by its location.
fn sound_ref(sound: SoundEvent) -> Option<SoundRef> {
    match sound {
        SoundEvent::Registered(id) => {
            let id = native_id(ClientRegistry::SoundEvent, id)?;
            let name = RegistryTable::native().name_of(ClientRegistry::SoundEvent, id)?;
            Some(SoundRef::from_id(name))
        }
        SoundEvent::Direct { location, .. } => Some(SoundRef::from_id(&location)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(sound: SoundEvent, source: u32) -> Option<(String, u8)> {
        match event(SoundPacket::Sound {
            sound,
            source,
            x: 8,
            y: -16,
            z: 0,
            volume: 1.0,
            pitch: 1.0,
            seed: 0,
        })? {
            NetworkEvent::PlaySound {
                sound,
                category,
                pos,
                ..
            } => {
                assert_eq!((pos.x, pos.y, pos.z), (1.0, -2.0, 0.0));
                Some((sound.event_name().to_owned(), category))
            }
            _ => panic!("expected PlaySound"),
        }
    }

    #[test]
    fn registered_and_direct_events_resolve_by_name() {
        let pling = RegistryTable::native()
            .id_of(ClientRegistry::SoundEvent, "block.note_block.pling")
            .unwrap();
        assert_eq!(
            play(SoundEvent::Registered(pling), 10),
            Some(("block.note_block.pling".into(), 10))
        );
        let direct = SoundEvent::Direct {
            location: "minecraft:custom.thing".into(),
            fixed_range: None,
        };
        assert_eq!(play(direct, 0), Some(("custom.thing".into(), 0)));
        assert_eq!(play(SoundEvent::Registered(u32::MAX), 0), None);
    }
}
