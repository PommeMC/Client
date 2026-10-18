//! Container, inventory and equipment packets, decoded by pomme for every
//! session: translated frames already carry the native layout, and item and
//! menu ids are remapped from the wire version's.

use azalea_inventory::ItemStack;
use azalea_inventory::components::EquipmentSlot;
use azalea_registry::Registry;
use azalea_registry::builtin::MenuKind;
use crossbeam_channel::Sender;
use pomme_protocol::codec::DecodeResult;
use pomme_protocol::container::{self, ContainerPacket};
use pomme_protocol::{ClientRegistry, item};

use super::NetworkEvent;
use super::translate::native_id;

/// Decodes a container packet; `None` for any other packet.
pub(super) fn handle_raw_container_packet(
    packet_id: u32,
    payload: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, container::read)?.map(|packet| {
            if let Some(event) = event(packet) {
                let _ = event_tx.try_send(event);
            }
        }),
    )
}

/// The client event for a packet, if the client acts on it.
fn event(packet: ContainerPacket) -> Option<NetworkEvent> {
    Some(match packet {
        ContainerPacket::OpenScreen {
            container_id,
            menu,
            title,
        } => {
            let Some(menu_type) =
                native_id(ClientRegistry::Menu, menu).and_then(MenuKind::from_u32)
            else {
                tracing::debug!("Ignoring a screen with unknown menu {menu}");
                return None;
            };
            let title = match super::chat::read_component(&title, &mut 0) {
                Ok(title) => title.plain_text(),
                Err(e) => {
                    tracing::debug!("Unreadable screen title: {e}");
                    String::new()
                }
            };
            NetworkEvent::OpenScreen {
                container_id,
                menu_type,
                title,
            }
        }
        ContainerPacket::Content {
            container_id,
            state_id,
            items,
            carried,
        } => NetworkEvent::ContainerContent {
            container_id,
            items: items.into_iter().map(stack).collect(),
            carried: stack(carried),
            state_id,
        },
        ContainerPacket::Slot {
            container_id,
            state_id,
            slot,
            item,
        } => NetworkEvent::ContainerSlot {
            container_id,
            index: slot as u16,
            item: stack(item),
            state_id,
        },
        ContainerPacket::Data {
            container_id,
            id,
            value,
        } => NetworkEvent::ContainerData {
            container_id,
            id: id as u16,
            value: value as u16,
        },
        ContainerPacket::Close { .. } => NetworkEvent::ContainerClosed,
        ContainerPacket::Cursor { item } => NetworkEvent::CursorItem { item: stack(item) },
        ContainerPacket::PlayerInventory { slot, item } => NetworkEvent::PlayerInventorySlot {
            index: slot,
            item: stack(item),
        },
        ContainerPacket::Equipment { entity, slots } => NetworkEvent::EntityEquipment {
            entity_id: entity,
            slots: slots
                .into_iter()
                .map(|(slot, item)| Some((EquipmentSlot::from_byte(slot)?, stack(item))))
                .collect::<Option<_>>()?,
        },
    })
}

/// A wire stack as the client's: its item remapped from the wire version's
/// ids, and empty when that item has no native counterpart.
fn stack(stack: Option<item::ItemStack>) -> ItemStack {
    stack
        .and_then(|mut stack| {
            stack.item = native_id(ClientRegistry::Item, stack.item)?;
            Some(super::item_codec::to_azalea(&stack))
        })
        .unwrap_or(ItemStack::Empty)
}
