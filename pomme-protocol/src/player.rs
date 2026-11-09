//! The local player's state packets, as vanilla 26.2 lays them out
//! (`net/minecraft/network/protocol/game/Clientbound*Packet`).

use crate::codec::{DecodeResult, MAX_STRING_LENGTH, Reader};

/// `ClientboundUpdateMobEffectPacket` flag bits.
pub const EFFECT_AMBIENT: u8 = 0x01;
pub const EFFECT_SHOW_ICON: u8 = 0x04;

/// `ClientboundPlayerAbilitiesPacket` flag bits.
pub const ABILITY_FLYING: u8 = 0x02;
pub const ABILITY_CAN_FLY: u8 = 0x04;

/// `ClientboundUpdateAttributesPacket.AttributeSnapshot`.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributeSnapshot {
    /// An `attribute` registry id.
    pub attribute: u32,
    pub base: f64,
    pub modifiers: Vec<AttributeModifier>,
}

/// `AttributeModifier.STREAM_CODEC`.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributeModifier {
    pub id: String,
    pub amount: f64,
    /// `AttributeModifier.Operation` id: add value, add multiplied base,
    /// add multiplied total.
    pub operation: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlayerPacket {
    Cooldown {
        group: String,
        duration: i32,
    },
    Health {
        health: f32,
        food: i32,
        saturation: f32,
    },
    Experience {
        progress: f32,
        level: i32,
        total: i32,
    },
    Attributes {
        entity: i32,
        snapshots: Vec<AttributeSnapshot>,
    },
    UpdateEffect {
        entity: i32,
        /// A `mob_effect` registry id.
        effect: u32,
        amplifier: i32,
        duration: i32,
        flags: u8,
    },
    RemoveEffect {
        entity: i32,
        effect: u32,
    },
    HeldSlot {
        slot: i32,
    },
    Abilities {
        flags: u8,
        flying_speed: f32,
        walking_speed: f32,
    },
    /// `ClientboundGameEventPacket.Type` id, then its parameter.
    GameEvent {
        event: u8,
        param: f32,
    },
    CombatKill {
        player: i32,
        /// The death message, a text component as NBT.
        message: Box<[u8]>,
    },
}

/// Decodes packet `name`, or `None` if it isn't a player-state packet.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<PlayerPacket>> {
    let read: fn(&mut Reader) -> DecodeResult<PlayerPacket> = match name {
        // ClientboundCooldownPacket: Identifier, signed VAR_INT.
        "cooldown" => |r| {
            Ok(PlayerPacket::Cooldown {
                group: r.identifier()?.to_owned(),
                duration: r.var_i32()?,
            })
        },
        // ClientboundSetHealthPacket.java:26-36
        "set_health" => |r| {
            Ok(PlayerPacket::Health {
                health: r.f32()?,
                food: r.var_i32()?,
                saturation: r.f32()?,
            })
        },
        // ClientboundSetExperiencePacket.java:26-36: level before total.
        "set_experience" => |r| {
            Ok(PlayerPacket::Experience {
                progress: r.f32()?,
                level: r.var_i32()?,
                total: r.var_i32()?,
            })
        },
        // ClientboundUpdateAttributesPacket.java:30, 66-67
        "update_attributes" => |r| {
            Ok(PlayerPacket::Attributes {
                entity: r.var_i32()?,
                snapshots: r.list(128, attribute_snapshot)?,
            })
        },
        // ClientboundUpdateMobEffectPacket.java:50-64
        "update_mob_effect" => |r| {
            Ok(PlayerPacket::UpdateEffect {
                entity: r.var_i32()?,
                effect: r.varint()?,
                amplifier: r.var_i32()?,
                duration: r.var_i32()?,
                flags: r.u8()?,
            })
        },
        // ClientboundRemoveMobEffectPacket.java:24
        "remove_mob_effect" => |r| {
            Ok(PlayerPacket::RemoveEffect {
                entity: r.var_i32()?,
                effect: r.varint()?,
            })
        },
        // ClientboundSetHeldSlotPacket.java:19
        "set_held_slot" => |r| Ok(PlayerPacket::HeldSlot { slot: r.var_i32()? }),
        // ClientboundPlayerAbilitiesPacket.java:37-64
        "player_abilities" => |r| {
            Ok(PlayerPacket::Abilities {
                flags: r.u8()?,
                flying_speed: r.f32()?,
                walking_speed: r.f32()?,
            })
        },
        // ClientboundGameEventPacket.java:49-57
        "game_event" => |r| {
            Ok(PlayerPacket::GameEvent {
                event: r.u8()?,
                param: r.f32()?,
            })
        },
        // ClientboundPlayerCombatKillPacket.java:18
        "player_combat_kill" => |r| {
            Ok(PlayerPacket::CombatKill {
                player: r.var_i32()?,
                message: r.nbt()?.into(),
            })
        },
        _ => return None,
    };
    Some(read(r))
}

fn attribute_snapshot(r: &mut Reader) -> DecodeResult<AttributeSnapshot> {
    Ok(AttributeSnapshot {
        attribute: r.varint()?,
        base: r.f64()?,
        modifiers: r.list(usize::MAX, |r| {
            Ok(AttributeModifier {
                id: r.utf(MAX_STRING_LENGTH)?.to_owned(),
                amount: r.f64()?,
                operation: r.varint()?,
            })
        })?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{write_utf, write_varint};

    fn decode(name: &str, bytes: &[u8]) -> PlayerPacket {
        let mut r = Reader::new(bytes);
        let packet = read(name, &mut r).expect("a player packet").unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("add_entity", &mut Reader::new(&[])).is_none());
    }

    fn decode_native_cooldown(duration: i32) {
        let mut bytes = Vec::new();
        write_utf(&mut bytes, "example:shared_igniters");
        write_varint(&mut bytes, duration as u32);
        let mut r = Reader::new(&bytes);
        let result = read("cooldown", &mut r)
            .expect("native Identifier cooldown must be owned by the player decoder");
        assert!(
            result.is_ok(),
            "valid signed cooldown {duration}: {result:?}"
        );
        assert_eq!(
            result.unwrap(),
            PlayerPacket::Cooldown {
                group: "example:shared_igniters".into(),
                duration,
            }
        );
        r.ensure_eof().expect("consume group and signed duration");
    }

    #[test]
    fn native_cooldown_group_is_owned() {
        decode_native_cooldown(100);
    }

    #[test]
    fn native_cooldown_negative_duration_is_owned() {
        decode_native_cooldown(-1);
    }

    #[test]
    fn native_cooldown_zero_duration_is_owned() {
        decode_native_cooldown(0);
    }

    #[test]
    fn native_cooldown_malformed_payload_is_rejected() {
        let mut group_only = Vec::new();
        write_utf(&mut group_only, "minecraft:flint_and_steel");
        let mut truncated_duration = group_only.clone();
        truncated_duration.push(0x80);
        for bytes in [vec![5, b'a'], group_only, truncated_duration] {
            let result = read("cooldown", &mut Reader::new(&bytes))
                .expect("malformed cooldown must remain owned, not fall back to Azalea");
            assert!(
                result.is_err(),
                "malformed cooldown was accepted: {result:?}"
            );
        }
    }

    #[test]
    fn health_and_experience() {
        let health = [&15.5f32.to_be_bytes()[..], &[18], &2.0f32.to_be_bytes()].concat();
        assert_eq!(
            decode("set_health", &health),
            PlayerPacket::Health {
                health: 15.5,
                food: 18,
                saturation: 2.0
            }
        );
        let experience = [&0.25f32.to_be_bytes()[..], &[30, 0xAC, 0x02]].concat();
        assert_eq!(
            decode("set_experience", &experience),
            PlayerPacket::Experience {
                progress: 0.25,
                level: 30,
                total: 300
            }
        );
    }

    #[test]
    fn attributes_with_modifiers() {
        let mut bytes = vec![9, 1, 24];
        bytes.extend(20.0f64.to_be_bytes());
        write_varint(&mut bytes, 1);
        write_utf(&mut bytes, "minecraft:test");
        bytes.extend(4.0f64.to_be_bytes());
        bytes.push(2);
        assert_eq!(
            decode("update_attributes", &bytes),
            PlayerPacket::Attributes {
                entity: 9,
                snapshots: vec![AttributeSnapshot {
                    attribute: 24,
                    base: 20.0,
                    modifiers: vec![AttributeModifier {
                        id: "minecraft:test".into(),
                        amount: 4.0,
                        operation: 2,
                    }],
                }],
            }
        );
    }

    #[test]
    fn effects_and_abilities_keep_their_flags() {
        assert_eq!(
            decode("update_mob_effect", &[1, 3, 1, 0xC8, 0x01, 0x0F]),
            PlayerPacket::UpdateEffect {
                entity: 1,
                effect: 3,
                amplifier: 1,
                duration: 200,
                flags: 0x0F
            }
        );
        assert_eq!(
            decode("remove_mob_effect", &[1, 3]),
            PlayerPacket::RemoveEffect {
                entity: 1,
                effect: 3
            }
        );
        let abilities = [&[0x06][..], &0.05f32.to_be_bytes(), &0.1f32.to_be_bytes()].concat();
        assert_eq!(
            decode("player_abilities", &abilities),
            PlayerPacket::Abilities {
                flags: ABILITY_FLYING | ABILITY_CAN_FLY,
                flying_speed: 0.05,
                walking_speed: 0.1
            }
        );
    }

    #[test]
    fn small_packets() {
        assert_eq!(
            decode("set_held_slot", &[4]),
            PlayerPacket::HeldSlot { slot: 4 }
        );
        let game_event = [&[3][..], &1.0f32.to_be_bytes()].concat();
        assert_eq!(
            decode("game_event", &game_event),
            PlayerPacket::GameEvent {
                event: 3,
                param: 1.0
            }
        );
        let message = [8, 0, 2, b'h', b'i'];
        let kill = [&[1][..], &message].concat();
        assert_eq!(
            decode("player_combat_kill", &kill),
            PlayerPacket::CombatKill {
                player: 1,
                message: message.into()
            }
        );
    }
}
