//! `set_entity_data` values into the client's entity events. Native sessions
//! decode the packet with pomme's grammar; translated ones map azalea's
//! decode onto the same values.

use azalea_buf::AzBuf;
use azalea_core::registry_holder::RegistryHolder;
use azalea_entity::EntityDataValue;
use azalea_inventory::{ItemStack, ItemStackData};
use azalea_protocol::packets::game::c_set_entity_data::ClientboundSetEntityData;
use azalea_registry::builtin::EntityKind;
use azalea_registry::{DataRegistry, Registry};
use crossbeam_channel::Sender;
use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::entity_data::{DataValue, Value, Variant};
use pomme_protocol::item::ItemCodec;

use super::handler::{send_ordered, variant_index};
use super::{NetworkEvent, item_codec};
use crate::entity::MetaValue;
use crate::entity::villager::{VillagerKind, VillagerProfession};
use crate::position::BlockPos;

/// `ItemEntity.DATA_ITEM`.
const ITEM_INDEX: u8 = 8;

/// `Pose.CROUCHING`'s id.
const CROUCHING: u32 = 5;

/// Decodes a native `set_entity_data`; `None` for any other packet or a
/// translated session.
pub(super) fn handle_raw_entity_data(
    packet_id: u32,
    payload: &[u8],
    registry_holder: &RegistryHolder,
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    if packet_id != item_codec::item_packet_ids().set_entity_data || !item_codec::native_session() {
        return None;
    }
    Some(read_entity_data(payload).map(|(id, values)| {
        for data in values {
            match &data.value {
                Value::Item(Some(stack)) if data.index == ITEM_INDEX => {
                    if let ItemStack::Present(stack) = item_codec::to_azalea(stack) {
                        send_item_data(id, &stack, event_tx);
                    }
                }
                value => send_data_value(id, data.index, value, registry_holder, event_tx),
            }
        }
    }))
}

/// `ClientboundSetEntityDataPacket.java:39-44`: the entity id, then values.
fn read_entity_data(payload: &[u8]) -> DecodeResult<(i32, Vec<DataValue>)> {
    let mut r = Reader::new(payload);
    let id = r.var_i32()?;
    let values = ItemCodec::native().read_entity_data(&mut r)?;
    r.ensure_eof()?;
    Ok((id, values))
}

/// A translated session's `set_entity_data`, decoded by azalea.
// TODO: drop once translated sessions decode it raw too.
pub(super) fn handle_typed_entity_data(
    p: &ClientboundSetEntityData,
    registry_holder: &RegistryHolder,
    event_tx: &Sender<NetworkEvent>,
) {
    for item in p.packed_items.iter() {
        match &item.value {
            EntityDataValue::ItemStack(ItemStack::Present(stack)) if item.index == ITEM_INDEX => {
                send_item_data(p.id.0, stack, event_tx);
            }
            value => {
                if let Some(value) = from_azalea(value) {
                    send_data_value(p.id.0, item.index, &value, registry_holder, event_tx);
                }
            }
        }
    }
}

fn send_item_data(id: i32, stack: &ItemStackData, event_tx: &Sender<NetworkEvent>) {
    let damage = stack
        .get_component::<azalea_inventory::components::Damage>()
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
            let name = match name {
                Some(nbt) => match super::chat::read_component(nbt, &mut 0) {
                    Ok(name) => Some(name.plain_text()),
                    Err(e) => {
                        tracing::debug!("Skipping unreadable custom name: {e}");
                        return;
                    }
                },
                None => None,
            };
            NetworkEvent::EntityCustomName { id, name }
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

/// The values [`send_data_value`] reads, from azalea's decode.
pub(super) fn from_azalea(value: &EntityDataValue) -> Option<Value> {
    Some(match value {
        EntityDataValue::Byte(v) => Value::Byte(*v),
        EntityDataValue::Int(v) => Value::Int(*v),
        EntityDataValue::Long(v) => Value::Long(*v),
        EntityDataValue::Float(v) => Value::Float(*v),
        EntityDataValue::Boolean(v) => Value::Bool(*v),
        EntityDataValue::OptionalFormattedText(text) => Value::OptionalComponent(match text {
            Some(text) => {
                let mut nbt = Vec::new();
                text.azalea_write(&mut nbt).ok()?;
                Some(nbt.into())
            }
            None => None,
        }),
        EntityDataValue::OptionalBlockPos(pos) => {
            Value::OptionalBlockPos(pos.map(|pos| (pos.x, pos.y, pos.z)))
        }
        EntityDataValue::Pose(pose) => Value::Pose(*pose as u32),
        EntityDataValue::VillagerData(data) => Value::VillagerData {
            kind: data.kind.to_u32(),
            profession: data.profession.to_u32(),
            level: data.level,
        },
        EntityDataValue::CatVariant(v) => Value::Variant(Variant::Cat, v.protocol_id()),
        EntityDataValue::CowVariant(v) => Value::Variant(Variant::Cow, v.protocol_id()),
        EntityDataValue::WolfVariant(v) => Value::Variant(Variant::Wolf, v.protocol_id()),
        EntityDataValue::ChickenVariant(v) => Value::Variant(Variant::Chicken, v.protocol_id()),
        EntityDataValue::SnifferState(state) => Value::Int(*state as i32),
        _ => return None,
    })
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
