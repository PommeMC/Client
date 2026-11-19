//! The world border's six clientbound packets, each ported from its
//! `STREAM_CODEC` in `net.minecraft.network.protocol.game`.

use crossbeam_channel::Sender;
use pomme_protocol::codec::{DecodeResult, Reader};

use super::NetworkEvent;
use super::handler::send_ordered;
use crate::world::border::WorldBorderInit;

pub(super) fn handle_raw_border_packet(
    packet_id: u32,
    payload: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, read_border_event)?.map(|event| {
            send_ordered(event_tx, event);
        }),
    )
}

fn read_border_event(name: &str, r: &mut Reader) -> Option<DecodeResult<NetworkEvent>> {
    Some(match name {
        "initialize_border" => read_initialize(r),
        "set_border_center" => read_center(r),
        "set_border_lerp_size" => read_lerp_size(r),
        "set_border_size" => r.f64().map(|size| NetworkEvent::SetBorderSize { size }),
        "set_border_warning_delay" => r
            .var_i32()
            .map(|warning_time| NetworkEvent::SetBorderWarningDelay { warning_time }),
        "set_border_warning_distance" => r
            .var_i32()
            .map(|warning_blocks| NetworkEvent::SetBorderWarningDistance { warning_blocks }),
        _ => return None,
    })
}

/// `ClientboundInitializeBorderPacket.write`.
fn read_initialize(r: &mut Reader) -> DecodeResult<NetworkEvent> {
    Ok(NetworkEvent::InitializeBorder(WorldBorderInit {
        center_x: r.f64()?,
        center_z: r.f64()?,
        old_size: r.f64()?,
        new_size: r.f64()?,
        lerp_ticks: r.varlong()? as i64,
        absolute_max_size: r.var_i32()?,
        warning_blocks: r.var_i32()?,
        warning_time: r.var_i32()?,
    }))
}

fn read_center(r: &mut Reader) -> DecodeResult<NetworkEvent> {
    Ok(NetworkEvent::SetBorderCenter {
        center_x: r.f64()?,
        center_z: r.f64()?,
    })
}

/// `ClientboundSetBorderLerpSizePacket.write`.
fn read_lerp_size(r: &mut Reader) -> DecodeResult<NetworkEvent> {
    Ok(NetworkEvent::SetBorderLerpSize {
        old_size: r.f64()?,
        new_size: r.f64()?,
        lerp_ticks: r.varlong()? as i64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The packet `name` reads from `doubles`, big-endian, then `tail`.
    fn read(name: &str, doubles: &[f64], tail: &[u8]) -> NetworkEvent {
        let mut bytes: Vec<u8> = doubles.iter().flat_map(|d| d.to_be_bytes()).collect();
        bytes.extend_from_slice(tail);
        let mut r = Reader::new(&bytes);
        let event = read_border_event(name, &mut r)
            .expect("a border packet")
            .expect("a whole packet");
        r.ensure_eof().expect("nothing left over");
        event
    }

    #[test]
    fn initialize_reads_four_doubles_a_varlong_and_three_varints() {
        // 300 ticks is the two-byte varlong AC 02; 29999984 is F0 86 A7 0E.
        let tail = [0xAC, 0x02, 0xF0, 0x86, 0xA7, 0x0E, 5, 15];
        let event = read("initialize_border", &[1.5, -2.5, 60.0, 20.0], &tail);
        let NetworkEvent::InitializeBorder(init) = event else {
            panic!("expected InitializeBorder");
        };
        assert_eq!(
            init,
            WorldBorderInit {
                center_x: 1.5,
                center_z: -2.5,
                old_size: 60.0,
                new_size: 20.0,
                lerp_ticks: 300,
                absolute_max_size: 29999984,
                warning_blocks: 5,
                warning_time: 15,
            }
        );
    }

    #[test]
    fn the_size_packets_read_their_doubles_and_ticks() {
        assert!(matches!(
            read("set_border_center", &[8.0, -8.0], &[]),
            NetworkEvent::SetBorderCenter { center_x, center_z } if (center_x, center_z) == (8.0, -8.0)
        ));
        assert!(matches!(
            read("set_border_size", &[25.0], &[]),
            NetworkEvent::SetBorderSize { size } if size == 25.0
        ));
        assert!(matches!(
            read("set_border_lerp_size", &[45.0, 25.0], &[0xC8, 0x01]),
            NetworkEvent::SetBorderLerpSize { old_size, new_size, lerp_ticks: 200 }
                if (old_size, new_size) == (45.0, 25.0)
        ));
    }

    #[test]
    fn the_warning_packets_read_one_varint() {
        assert!(matches!(
            read("set_border_warning_delay", &[], &[0xAC, 0x02]),
            NetworkEvent::SetBorderWarningDelay { warning_time: 300 }
        ));
        assert!(matches!(
            read("set_border_warning_distance", &[], &[7]),
            NetworkEvent::SetBorderWarningDistance { warning_blocks: 7 }
        ));
    }

    #[test]
    fn other_packets_are_left_alone() {
        assert!(read_border_event("set_time", &mut Reader::new(&[])).is_none());
    }
}
