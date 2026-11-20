//! `set_entity_data`, decoded with pomme's grammar for every session:
//! translated frames already carry the native layout.

use crossbeam_channel::Sender;
use pomme_item::{ItemStack, ItemStackData};
use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::entity_data::{DataValue, Value, Variant};
use pomme_protocol::item::ItemCodec;
use pomme_registry::Registry;
use pomme_registry::builtin::EntityKind;

use super::handler::{send_ordered, variant_index};
use super::translate::Translation;
use super::{NetworkEvent, item_codec};
use crate::chat_component::ComponentText;
use crate::entity::MetaValue;
use crate::entity::villager::{VillagerKind, VillagerProfession};
use crate::net::registry_holder::RegistryHolder;
use crate::position::BlockPos;

/// `ItemEntity.DATA_ITEM`.
const ITEM_INDEX: u8 = 8;

/// `Pose.CROUCHING`'s id.
const CROUCHING: u32 = 5;

/// Decodes a `set_entity_data`; `None` for any other packet.
pub(super) fn handle_raw_entity_data(
    packet_id: u32,
    payload: &[u8],
    registry_holder: &RegistryHolder,
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    if packet_id != item_codec::item_packet_ids().set_entity_data {
        return None;
    }
    Some(
        read_entity_data(payload, super::translate::active()).map(|(id, values)| {
            for DataValue { index, value } in values {
                match &value {
                    Value::Item(Some(stack)) if index == ITEM_INDEX => {
                        if let ItemStack::Present(stack) = ItemStack::from_wire(stack) {
                            send_item_data(id, &stack, event_tx);
                        }
                    }
                    value => send_data_value(id, index, value, registry_holder, event_tx),
                }
            }
        }),
    )
}

/// `ClientboundSetEntityDataPacket.java:39-44`: the entity id, then values.
/// Where `translation` left an item stack's id in the wire version's space,
/// it becomes the native id, or an empty stack when there is none.
pub(super) fn read_entity_data(
    payload: &[u8],
    translation: Option<&Translation>,
) -> DecodeResult<(i32, Vec<DataValue>)> {
    let mut r = Reader::new(payload);
    let id = r.var_i32()?;
    let mut values = ItemCodec::native().read_entity_data(&mut r)?;
    r.ensure_eof()?;
    if let Some(translation) = translation.filter(|t| t.entity_data_items_in_wire_ids()) {
        for data in &mut values {
            if let Value::Item(stack) = &mut data.value {
                *stack = stack.take().and_then(|mut stack| {
                    stack.item = translation.remap_item_to_native(stack.item)?;
                    Some(stack)
                });
            }
        }
    }
    Ok((id, values))
}

fn send_item_data(id: i32, stack: &ItemStackData, event_tx: &Sender<NetworkEvent>) {
    let damage = stack
        .component::<pomme_item::components::Damage>()
        .map(|component| component.amount)
        .unwrap_or(0);
    send_ordered(
        event_tx,
        NetworkEvent::EntityItemData {
            id,
            item_name: crate::player::inventory::item_resource_name(stack.kind),
            item_id: stack.kind.to_u32(),
            damage,
            count: stack.count,
            stack: stack.clone(),
        },
    );
}

fn send_data_value(
    id: i32,
    index: u8,
    value: &Value,
    registry_holder: &RegistryHolder,
    event_tx: &Sender<NetworkEvent>,
) {
    // Avatar's absorption/score sit at 17/18 since 1.21.9 (773); 15/16 on
    // older wire versions (main hand moved to 15, pushing them up).
    let (absorption_idx, score_idx) = if crate::version::session_protocol() <= 772 {
        (15, 16)
    } else {
        (17, 18)
    };
    let variant = |kind, protocol_id| NetworkEvent::EntityVariant {
        id,
        kind,
        variant: variant_index(registry_holder, kind, protocol_id),
    };
    let event = match (index, value) {
        // Entity.DATA_POSE
        (6, Value::Pose(pose)) => NetworkEvent::EntityPose {
            id,
            is_crouching: *pose == CROUCHING,
        },
        // LivingEntity.SLEEPING_POS_ID
        (14, Value::OptionalBlockPos(pos)) => NetworkEvent::EntitySleepingPos {
            id,
            pos: pos.map(|(x, y, z)| BlockPos::new(x, y, z)),
        },
        // Player score and absorption, by wire version as above. Kind-blind;
        // the consumer applies them only to the local player.
        (i, Value::Int(score)) if i == score_idx => {
            send_scalar(id, index, value, event_tx);
            NetworkEvent::PlayerScore {
                entity_id: id,
                score: *score,
            }
        }
        (i, Value::Float(absorption)) if i == absorption_idx => {
            send_scalar(id, index, value, event_tx);
            NetworkEvent::PlayerAbsorption {
                entity_id: id,
                absorption: *absorption,
            }
        }
        // Entity.DATA_CUSTOM_NAME; needed for jeb_ sheep detection.
        (2, Value::OptionalComponent(name)) => {
            let (name, spans) = match name {
                Some(nbt) => match super::chat::read_component(nbt, &mut 0) {
                    Ok(component) => {
                        let plain = component.plain_text();
                        let spans = crate::ui::text::format_component_spans(
                            &component,
                            [1.0, 1.0, 1.0, 1.0],
                        );
                        let spans = crate::ui::text::legacy_format_spans(&spans, true);
                        (Some(plain), Some(spans))
                    }
                    Err(e) => {
                        tracing::debug!("Skipping unreadable custom name: {e}");
                        return;
                    }
                },
                None => (None, None),
            };
            NetworkEvent::EntityCustomName { id, name, spans }
        }
        (18, Value::Variant(Variant::Cow, v)) => variant(EntityKind::Cow, *v),
        (18, Value::Variant(Variant::Chicken, v)) => variant(EntityKind::Chicken, *v),
        // Cat / wolf variants: 20 / 23 on 26.x, one lower on 1.21.9-1.21.11
        // (no AgeableMob age-locked slot).
        (19 | 20, Value::Variant(Variant::Cat, v)) => variant(EntityKind::Cat, *v),
        (22 | 23, Value::Variant(Variant::Wolf, v)) => variant(EntityKind::Wolf, *v),
        // Villagers at 19 (18 on 1.21.9-1.21.11), zombie villagers at 20.
        (
            18..=20,
            Value::VillagerData {
                kind,
                profession,
                level,
            },
        ) => {
            let (Some(kind), Some(profession)) = (
                VillagerKind::from_id(*kind),
                VillagerProfession::from_id(*profession),
            ) else {
                return;
            };
            NetworkEvent::VillagerData {
                id,
                kind,
                profession,
                level: *level,
            }
        }
        _ => {
            send_scalar(id, index, value, event_tx);
            return;
        }
    };
    send_ordered(event_tx, event);
}

/// Scalar values are forwarded raw; the store resolves their meaning per
/// (kind, index) like vanilla `onSyncedDataUpdated`
/// (`EntityStore::apply_entity_data`).
fn send_scalar(id: i32, index: u8, value: &Value, event_tx: &Sender<NetworkEvent>) {
    let value = match value {
        Value::Bool(v) => MetaValue::Bool(*v),
        Value::Int(v) => MetaValue::Int(*v),
        Value::Byte(v) => MetaValue::Byte(*v),
        Value::Float(v) => MetaValue::Float(*v),
        Value::Long(v) => MetaValue::Long(*v),
        _ => return,
    };
    send_ordered(event_tx, NetworkEvent::EntityData { id, index, value });
}

#[cfg(test)]
mod tests {
    use pomme_protocol::wire::write_varint;

    use super::*;

    #[test]
    fn native_entity_data_becomes_events() {
        let stone = ItemCodec::native()
            .registries()
            .id_of(pomme_protocol::ClientRegistry::Item, "stone")
            .unwrap();
        let mut payload = vec![7];
        payload.extend([ITEM_INDEX, 7, 2]);
        write_varint(&mut payload, stone);
        payload.extend([0, 0]);
        // Pose, then a byte the store resolves itself.
        payload.extend([6, 20, CROUCHING as u8, 0, 0, 0x02, 0xFF]);
        let (tx, rx) = crossbeam_channel::unbounded();
        handle_raw_entity_data(
            item_codec::item_packet_ids().set_entity_data,
            &payload,
            &RegistryHolder::default(),
            &tx,
        )
        .expect("set_entity_data")
        .expect("valid frame");
        let events: Vec<_> = rx.try_iter().collect();
        assert!(matches!(
            &events[..],
            [
                NetworkEvent::EntityItemData { id: 7, count: 2, item_name, .. },
                NetworkEvent::EntityPose { id: 7, is_crouching: true },
                NetworkEvent::EntityData { id: 7, index: 0, value: MetaValue::Byte(2) },
            ] if item_name == "stone"
        ));
    }

    #[test]
    fn other_packets_pass_through() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let other = item_codec::item_packet_ids().set_entity_data + 1;
        assert!(handle_raw_entity_data(other, &[], &RegistryHolder::default(), &tx).is_none());
    }
}
