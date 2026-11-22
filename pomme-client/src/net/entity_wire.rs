//! The entity lifecycle, movement and event packets, decoded by pomme for
//! every session: translated frames already carry the native layout, and
//! only `add_entity`'s entity type needs a registry remap.

use crossbeam_channel::Sender;
use pomme_protocol::codec::DecodeResult;
use pomme_protocol::entity::{self, EntityPacket};
use pomme_registry::Registry;
use pomme_registry::builtin::EntityKind;

use super::NetworkEvent;
use super::handler::send_ordered;

/// `ClientboundAnimatePacket.WAKE_UP`.
const WAKE_UP: u8 = 0;

/// Decodes an entity packet; `None` for any other packet.
pub(super) fn handle_raw_entity_packet(
    packet_id: u32,
    payload: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, entity::read)?.map(|packet| {
            if let Some(event) = event(packet) {
                send_ordered(event_tx, event);
            }
        }),
    )
}

/// The client event for a packet, if the client acts on it.
fn event(packet: EntityPacket) -> Option<NetworkEvent> {
    Some(match packet {
        EntityPacket::Add {
            id,
            uuid,
            entity_type,
            position,
            movement,
            x_rot,
            y_rot,
            y_head_rot,
            data,
        } => {
            let Some(entity_type) = native_entity_type(entity_type) else {
                tracing::debug!("Dropping entity {id} of an unknown type {entity_type}");
                return None;
            };
            NetworkEvent::EntitySpawned {
                id,
                uuid: uuid::Uuid::from_u128(uuid),
                entity_type,
                position: position.into(),
                velocity: movement,
                x_rot_deg: x_rot,
                y_rot_deg: y_rot,
                head_y_rot_deg: y_head_rot,
                data,
            }
        }
        EntityPacket::Remove { ids } => NetworkEvent::EntitiesRemoved { ids },
        // TODO: a stepped delta is applied as one move, and a stepped path
        // below jumps to its end; 26.3 walks the entity through the steps
        // (`VecDelta.Stepped.decode`).
        EntityPacket::Move {
            id,
            delta,
            rotation,
            on_ground,
        } => match (delta.map(|delta| delta.total()), rotation) {
            (Some(d), Some((y_rot_deg, x_rot_deg))) => NetworkEvent::EntityMovedRotated {
                id,
                dx: d.x,
                dy: d.y,
                dz: d.z,
                y_rot_deg,
                x_rot_deg,
                on_ground,
            },
            (Some(d), None) => NetworkEvent::EntityMoved {
                id,
                dx: d.x,
                dy: d.y,
                dz: d.z,
                on_ground,
            },
            (None, Some((y_rot_deg, x_rot_deg))) => NetworkEvent::EntityRotated {
                id,
                y_rot_deg,
                x_rot_deg,
                on_ground,
            },
            (None, None) => return None,
        },
        // Unlike a teleport, a position sync doesn't change velocity.
        EntityPacket::PositionSync {
            id,
            path,
            y_rot,
            x_rot,
            on_ground,
        } => NetworkEvent::EntityTeleported {
            id,
            position: path.end()?.into(),
            velocity: None,
            y_rot_deg: y_rot,
            x_rot_deg: x_rot,
            on_ground,
        },
        // TODO: apply `relative` (vanilla `PositionMoveRotation.calculateAbsolute`).
        EntityPacket::Teleport {
            id,
            change,
            relative: _,
            on_ground,
        } => NetworkEvent::EntityTeleported {
            id,
            position: change.position.into(),
            velocity: Some(change.delta),
            y_rot_deg: change.y_rot,
            x_rot_deg: change.x_rot,
            on_ground,
        },
        EntityPacket::Motion { id, velocity } => NetworkEvent::EntityMotion { id, velocity },
        EntityPacket::RotateHead { id, y_head_rot } => NetworkEvent::EntityHeadRotation {
            id,
            head_y_rot_deg: y_head_rot,
        },
        // WAKE_UP is vanilla `handleAnimate`'s `stopSleepInBed(false, false)`.
        // TODO: the two critical hit actions spawn particles.
        EntityPacket::Animate { id, action } => match action {
            WAKE_UP => NetworkEvent::EntityWakeUp { id },
            _ => return None,
        },
        // Either hand's swing drives the zombie attack swing (skeleton aim
        // uses the aggressive flag instead).
        // TODO: the animation's type and length are unused; 26.3 swings
        // through `SwingState.startIfAble` with them.
        EntityPacket::Swing { id, .. } => NetworkEvent::EntitySwing { id },
        EntityPacket::Event { id, event } => entity_event(id, event)?,
        EntityPacket::HurtAnimation { id, yaw } => NetworkEvent::HurtAnimation { id, yaw },
        EntityPacket::Damage { id, .. } => NetworkEvent::EntityDamaged { id },
        EntityPacket::TakeItem {
            item,
            collector,
            amount,
        } => NetworkEvent::ItemPickedUp {
            item_id: item,
            collector_id: collector,
            amount,
        },
        EntityPacket::Passengers {
            vehicle,
            passengers,
        } => NetworkEvent::SetPassengers {
            vehicle,
            passengers,
        },
    })
}

/// `Entity.handleEntityEvent`'s ids the client acts on.
// TODO: event 60 (`makePoofParticles`) when a mob's death clock hits 20.
fn entity_event(id: i32, event: i8) -> Option<NetworkEvent> {
    Some(match event {
        1 => NetworkEvent::RabbitJump { id },
        3 => NetworkEvent::EntityDied { id },
        4 => NetworkEvent::GolemPunch { id },
        8 => NetworkEvent::WolfShaking { id, shaking: true },
        9 => NetworkEvent::FinishUseItem { id },
        10 => NetworkEvent::SheepEatStart { id },
        11 => NetworkEvent::GolemOfferFlower { id, offering: true },
        19 => NetworkEvent::SquidTentacleReset { id },
        32 => NetworkEvent::ArmorStandHit { id },
        24..=28 => NetworkEvent::PermissionLevel {
            id,
            level: (event - 24) as u8,
        },
        34 => NetworkEvent::GolemOfferFlower {
            id,
            offering: false,
        },
        45 => NetworkEvent::FoxEat { id },
        56 => NetworkEvent::WolfShaking { id, shaking: false },
        58 => NetworkEvent::GoatRam { id, lowering: true },
        59 => NetworkEvent::GoatRam {
            id,
            lowering: false,
        },
        63 => NetworkEvent::SnifferDigging { id },
        64 => NetworkEvent::ArmadilloPeek { id },
        _ => return None,
    })
}

/// The wire entity type in the native registry; translated sessions remap
/// it from their version's ids.
fn native_entity_type(id: u32) -> Option<EntityKind> {
    EntityKind::from_u32(super::translate::native_id(
        pomme_protocol::ClientRegistry::EntityType,
        id,
    )?)
}
