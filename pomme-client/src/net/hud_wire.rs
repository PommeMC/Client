//! Title, tab list and boss bar packets, decoded by pomme for every session:
//! translated frames already carry native NBT text.

use crossbeam_channel::Sender;
use pomme_protocol::codec::DecodeResult;
use pomme_protocol::hud::{self, BossOp, HudPacket};

use super::NetworkEvent;
use super::chat::component_spans;
use crate::ui::boss_bar::BossBarOp;

/// Decodes a HUD packet; `None` for any other packet.
pub(super) fn handle_raw_hud_packet(
    packet_id: u32,
    payload: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, hud::read)?.map(|packet| {
            if let Some(event) = event(packet) {
                let _ = event_tx.try_send(event);
            }
        }),
    )
}

/// The client event for a packet; `None` when its text is unreadable.
fn event(packet: HudPacket) -> Option<NetworkEvent> {
    Some(match packet {
        HudPacket::Title(text) => NetworkEvent::TitleText {
            spans: component_spans(&text)?,
        },
        HudPacket::Subtitle(text) => NetworkEvent::SubtitleText {
            spans: component_spans(&text)?,
        },
        HudPacket::TitlesAnimation {
            fade_in,
            stay,
            fade_out,
        } => NetworkEvent::TitlesAnimation {
            fade_in,
            stay,
            fade_out,
        },
        HudPacket::ClearTitles { reset_times } => NetworkEvent::ClearTitles { reset_times },
        HudPacket::TabList { header, footer } => NetworkEvent::TabListHeaderFooter {
            header: component_spans(&header)?,
            footer: component_spans(&footer)?,
        },
        HudPacket::BossEvent { id, op } => NetworkEvent::BossBarUpdate {
            id: uuid::Uuid::from_u128(id),
            op: match op {
                BossOp::Add {
                    name,
                    progress,
                    color,
                    overlay,
                    flags,
                } => BossBarOp::Add {
                    name: component_spans(&name)?,
                    progress,
                    color: color as u8,
                    overlay: overlay as u8,
                    flags,
                },
                BossOp::Remove => BossBarOp::Remove,
                BossOp::Progress(progress) => BossBarOp::UpdateProgress(progress),
                BossOp::Name(name) => BossBarOp::UpdateName(component_spans(&name)?),
                BossOp::Style { color, overlay } => BossBarOp::UpdateStyle {
                    color: color as u8,
                    overlay: overlay as u8,
                },
                BossOp::Properties(flags) => BossBarOp::UpdateProperties(flags),
            },
        },
    })
}
