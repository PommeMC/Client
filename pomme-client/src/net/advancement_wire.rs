//! Native `update_advancements` decoding, with display icons read through
//! pomme's item grammar. Translated sessions keep azalea's decode.

use std::collections::HashMap;

use crossbeam_channel::Sender;
use pomme_protocol::ClientRegistry;
use pomme_protocol::codec::{DecodeResult, MAX_STRING_LENGTH, Reader};

use super::{NetworkEvent, item_codec};
use crate::chat_component::Component;
use crate::ui::text::format_component_spans;
use crate::ui::toast::{AdvancementData, AdvancementDisplay, AdvancementFrame, AdvancementsUpdate};

/// `DisplayInfo` flag bits.
const HAS_BACKGROUND: i32 = 1;
const SHOW_TOAST: i32 = 2;

/// Decodes a native `update_advancements`; `None` for any other packet or a
/// translated session.
pub(super) fn handle_raw_advancements(
    packet_id: u32,
    payload: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    if packet_id != item_codec::item_packet_ids().update_advancements
        || !item_codec::native_session()
    {
        return None;
    }
    Some(read_update(payload).map(|update| {
        let _ = event_tx.try_send(NetworkEvent::AdvancementsUpdate(Box::new(update)));
    }))
}

/// `ClientboundUpdateAdvancementsPacket.java:42-48`.
pub(super) fn read_update(payload: &[u8]) -> DecodeResult<AdvancementsUpdate> {
    let mut r = Reader::new(payload);
    let reset = r.bool()?;
    let added = r.list(usize::MAX, |r| Ok((string(r)?, read_advancement(r)?)))?;
    let removed = r.list(usize::MAX, string)?;
    let progress = r.list(usize::MAX, |r| Ok((string(r)?, read_progress(r)?)))?;
    let show_advancements = r.bool()?;
    r.ensure_eof()?;
    Ok(AdvancementsUpdate {
        reset,
        added,
        removed,
        progress,
        show_advancements,
    })
}

/// `Advancement.read` (`Advancement.java:85-86`): parent, display,
/// requirements, then the telemetry flag.
fn read_advancement(r: &mut Reader) -> DecodeResult<AdvancementData> {
    if r.bool()? {
        string(r)?;
    }
    let display = if r.bool()? {
        Some(read_display(r)?)
    } else {
        None
    };
    // AdvancementRequirements.java:27-29
    let requirements = r.list(usize::MAX, |r| r.list(usize::MAX, string))?;
    r.bool()?;
    Ok(AdvancementData {
        display,
        requirements,
    })
}

/// `DisplayInfo.fromNetwork` (`DisplayInfo.java:116-131`).
fn read_display(r: &mut Reader) -> DecodeResult<AdvancementDisplay> {
    let title = read_text(r)?;
    read_text(r)?;
    let icon = item_codec::native().read_template(r)?;
    let frame = match r.varint()? {
        0 => AdvancementFrame::Task,
        1 => AdvancementFrame::Challenge,
        2 => AdvancementFrame::Goal,
        _ => return Err(r.error("unknown advancement type")),
    };
    let flags = r.i32()?;
    if flags & HAS_BACKGROUND != 0 {
        string(r)?;
    }
    r.f32()?;
    r.f32()?;
    let icon_item = item_codec::native()
        .registries()
        .name_of(ClientRegistry::Item, icon.item)
        .map(str::to_owned);
    Ok(AdvancementDisplay {
        title: format_component_spans(&title, [1.0; 4]),
        frame,
        show_toast: flags & SHOW_TOAST != 0,
        icon_item,
    })
}

/// `ComponentSerialization.TRUSTED_STREAM_CODEC`: a text component as NBT.
fn read_text(r: &mut Reader) -> DecodeResult<Component> {
    let nbt = r.nbt()?;
    super::chat::read_component(nbt, &mut 0).map_err(|_| r.error("invalid text component"))
}

/// `AdvancementProgress.fromNetwork`: each criterion and whether it's
/// obtained (`CriterionProgress.java:46-50`, a nullable instant).
fn read_progress(r: &mut Reader) -> DecodeResult<HashMap<String, bool>> {
    let criteria = r.list(usize::MAX, |r| {
        let name = string(r)?;
        let obtained = r.bool()?;
        if obtained {
            r.i64()?;
        }
        Ok((name, obtained))
    })?;
    Ok(criteria.into_iter().collect())
}

/// `FriendlyByteBuf.readUtf` / `readIdentifier`, both at the default limit.
fn string(r: &mut Reader) -> DecodeResult<String> {
    Ok(r.utf(MAX_STRING_LENGTH)?.to_owned())
}

#[cfg(test)]
mod tests {
    use pomme_protocol::wire::{write_utf, write_varint};

    use super::*;

    /// One advancement with a display of `frame` and `flags`, no background.
    fn frame(frame: u8, flags: i32) -> Vec<u8> {
        let stick = item_codec::native()
            .registries()
            .id_of(ClientRegistry::Item, "stick")
            .unwrap();
        let mut p = vec![0, 1];
        write_utf(&mut p, "minecraft:a");
        p.extend([0, 1, 8, 0, 1, b't', 8, 0, 0]);
        write_varint(&mut p, stick);
        p.extend([1, 0, 0, frame]);
        p.extend(flags.to_be_bytes());
        p.extend([0; 8]);
        // No requirements, no telemetry; nothing removed or progressed.
        p.extend([0, 0, 0, 0, 1]);
        p
    }

    #[test]
    fn hidden_displays_without_toasts() {
        let update = read_update(&frame(1, 4)).unwrap();
        let display = update.added[0].1.display.as_ref().unwrap();
        assert!(!display.show_toast);
        assert!(matches!(display.frame, AdvancementFrame::Challenge));
        assert_eq!(display.icon_item.as_deref(), Some("stick"));
        assert!(update.show_advancements);
    }

    #[test]
    fn rejects_unknown_frames_and_trailing_bytes() {
        assert_eq!(
            read_update(&frame(3, 0)).map_err(|e| e.what).err(),
            Some("unknown advancement type")
        );
        let mut long = frame(0, 0);
        long.push(0);
        assert_eq!(
            read_update(&long).map_err(|e| e.what).err(),
            Some("trailing bytes")
        );
    }
}
