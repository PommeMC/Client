//! The local player's state packets, decoded by pomme for every session:
//! translated frames already carry the native layout and registry ids.

use std::collections::HashSet;

use crossbeam_channel::Sender;
use pomme_protocol::codec::DecodeResult;
use pomme_protocol::player::{self, PlayerPacket};

use super::handler::send_ordered;
use super::{NetworkEvent, Weather};
use crate::attribute::{
    AttributeKind, AttributeModifier, AttributeModifierOperation, AttributeSnapshot,
};

/// `ClientboundGameEventPacket.Type` ids the client acts on.
const START_RAINING: u8 = 1;
const STOP_RAINING: u8 = 2;
const CHANGE_GAME_MODE: u8 = 3;
const RAIN_LEVEL_CHANGE: u8 = 7;
const THUNDER_LEVEL_CHANGE: u8 = 8;
const LEVEL_CHUNKS_LOAD_START: u8 = 13;

/// Decodes a player-state packet; `None` for any other packet.
pub(super) fn handle_raw_player_packet(
    packet_id: u32,
    payload: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, player::read)?.map(|packet| {
            if let Some(event) = event(packet) {
                send_ordered(event_tx, event);
            }
        }),
    )
}

/// The client event for a packet, if the client acts on it.
fn event(packet: PlayerPacket) -> Option<NetworkEvent> {
    Some(match packet {
        PlayerPacket::Health {
            health,
            food,
            saturation,
        } => NetworkEvent::PlayerHealth {
            health,
            food: u32::try_from(food).unwrap_or(0),
            saturation,
        },
        PlayerPacket::Experience {
            progress, level, ..
        } => NetworkEvent::PlayerExperience { progress, level },
        PlayerPacket::Attributes { entity, snapshots } => NetworkEvent::EntityAttributesUpdate {
            entity_id: entity,
            snapshots: attribute_snapshots(snapshots)?,
        },
        PlayerPacket::UpdateEffect {
            entity,
            effect,
            amplifier,
            duration,
            flags,
        } => NetworkEvent::UpdateMobEffect {
            entity_id: entity,
            effect: crate::mob_effect::MobEffectInstance {
                effect_id: effect,
                amplifier,
                duration,
                ambient: flags & player::EFFECT_AMBIENT != 0,
                show_icon: flags & player::EFFECT_SHOW_ICON != 0,
            },
        },
        PlayerPacket::RemoveEffect { entity, effect } => NetworkEvent::RemoveMobEffect {
            entity_id: entity,
            effect_id: effect,
        },
        PlayerPacket::HeldSlot { slot } => NetworkEvent::HeldSlot {
            slot: u8::try_from(slot).ok().filter(|s| *s < 9)?,
        },
        // TODO: invulnerable and instant_break flags
        PlayerPacket::Abilities {
            flags,
            flying_speed,
            walking_speed,
        } => NetworkEvent::PlayerAbilitiesChanged {
            flying: flags & player::ABILITY_FLYING != 0,
            can_fly: flags & player::ABILITY_CAN_FLY != 0,
            flying_speed,
            walking_speed,
        },
        PlayerPacket::GameEvent { event, param } => match event {
            CHANGE_GAME_MODE => NetworkEvent::GameModeChanged {
                game_mode: param as u8,
                previous: None,
            },
            LEVEL_CHUNKS_LOAD_START => NetworkEvent::LevelChunksLoadStart,
            START_RAINING => NetworkEvent::WeatherUpdate(Weather::StartRaining),
            STOP_RAINING => NetworkEvent::WeatherUpdate(Weather::StopRaining),
            RAIN_LEVEL_CHANGE => NetworkEvent::WeatherUpdate(Weather::RainLevel(param)),
            THUNDER_LEVEL_CHANGE => NetworkEvent::WeatherUpdate(Weather::ThunderLevel(param)),
            _ => return None,
        },
        PlayerPacket::CombatKill { player, message } => {
            let message = match super::chat::read_component(&message, &mut 0) {
                Ok(message) => message.plain_text(),
                Err(e) => {
                    tracing::debug!("Unreadable death message: {e}");
                    String::new()
                }
            };
            tracing::info!("Player died: {message}");
            NetworkEvent::PlayerDied {
                player_id: player,
                message,
            }
        }
    })
}

/// The client's snapshots; `None` drops the packet.
// TODO: a duplicate modifier id makes vanilla's `addModifier` throw after
// earlier snapshots applied, and `onPacketError` disconnects; Pomme has no
// packet-error disconnect yet, so it drops the packet.
fn attribute_snapshots(
    snapshots: Vec<player::AttributeSnapshot>,
) -> Option<Vec<AttributeSnapshot>> {
    snapshots
        .into_iter()
        .filter_map(|snapshot| {
            let Some(attribute) = AttributeKind::from_id(snapshot.attribute) else {
                tracing::debug!("Skipping unknown attribute {}", snapshot.attribute);
                return None;
            };
            Some(attribute_snapshot(attribute, snapshot))
        })
        .collect()
}

fn attribute_snapshot(
    attribute: AttributeKind,
    snapshot: player::AttributeSnapshot,
) -> Option<AttributeSnapshot> {
    let mut seen = HashSet::with_capacity(snapshot.modifiers.len());
    let mut modifiers = Vec::with_capacity(snapshot.modifiers.len());
    for modifier in snapshot.modifiers {
        if !seen.insert(modifier.id.clone()) {
            tracing::warn!(
                "Ignoring malformed {attribute:?} snapshot with duplicate modifier {}",
                modifier.id
            );
            return None;
        }
        modifiers.push(AttributeModifier {
            id: modifier.id,
            amount: modifier.amount,
            operation: AttributeModifierOperation::from_id(modifier.operation),
        });
    }
    Some(AttributeSnapshot {
        attribute,
        base: snapshot.base,
        modifiers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(attribute: u32, ids: &[&str]) -> player::AttributeSnapshot {
        player::AttributeSnapshot {
            attribute,
            base: 4.0,
            modifiers: ids
                .iter()
                .map(|id| player::AttributeModifier {
                    id: (*id).into(),
                    amount: 1.0,
                    operation: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn duplicate_modifier_ids_drop_the_packet() {
        let attack_speed = AttributeKind::ALL
            .iter()
            .position(|k| *k == AttributeKind::AttackSpeed)
            .unwrap() as u32;
        let ok = attribute_snapshots(vec![snapshot(
            attack_speed,
            &["minecraft:a", "minecraft:b"],
        )]);
        assert_eq!(ok.unwrap()[0].attribute, AttributeKind::AttackSpeed);
        let dup = vec![snapshot(attack_speed, &["minecraft:a", "minecraft:a"])];
        assert!(attribute_snapshots(dup).is_none());
    }

    #[test]
    fn unknown_attributes_are_skipped() {
        let snapshots = attribute_snapshots(vec![snapshot(999, &[])]).unwrap();
        assert!(snapshots.is_empty());
    }
}
