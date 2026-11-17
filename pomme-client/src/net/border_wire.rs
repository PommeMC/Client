use crossbeam_channel::Sender;
use pomme_protocol::codec::{DecodeResult, Reader};

use super::NetworkEvent;
use super::handler::send_ordered;

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
    match name {
        "initialize_border" => Some(read_initialize(r)),
        "set_border_center" => Some(read_center(r)),
        "set_border_lerp_size" => Some(read_lerp_size(r)),
        "set_border_size" => Some(read_size(r)),
        "set_border_warning_delay" => Some(read_warning_delay(r)),
        "set_border_warning_distance" => Some(read_warning_distance(r)),
        _ => None,
    }
}

fn read_initialize(r: &mut Reader) -> DecodeResult<NetworkEvent> {
    Ok(NetworkEvent::InitializeBorder {
        center_x: r.f64()?,
        center_z: r.f64()?,
        old_size: r.f64()?,
        new_size: r.f64()?,
        lerp_time_ms: r.varlong()?,
        max_size: r.varint()?,
        warning_blocks: r.varint()?,
        warning_time: r.varint()?,
    })
}

fn read_center(r: &mut Reader) -> DecodeResult<NetworkEvent> {
    Ok(NetworkEvent::SetBorderCenter {
        center_x: r.f64()?,
        center_z: r.f64()?,
    })
}

fn read_lerp_size(r: &mut Reader) -> DecodeResult<NetworkEvent> {
    Ok(NetworkEvent::SetBorderLerpSize {
        old_size: r.f64()?,
        new_size: r.f64()?,
        lerp_time_ms: r.varlong()?,
    })
}

fn read_size(r: &mut Reader) -> DecodeResult<NetworkEvent> {
    Ok(NetworkEvent::SetBorderSize { size: r.f64()? })
}

fn read_warning_delay(r: &mut Reader) -> DecodeResult<NetworkEvent> {
    Ok(NetworkEvent::SetBorderWarningDelay {
        warning_delay: r.varint()?,
    })
}

fn read_warning_distance(r: &mut Reader) -> DecodeResult<NetworkEvent> {
    Ok(NetworkEvent::SetBorderWarningDistance {
        warning_blocks: r.varint()?,
    })
}
