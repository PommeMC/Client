//! Connection lifecycle packets (join, respawn, keep-alive, teleports, time),
//! as vanilla 26.2 lays them out
//! (`net/minecraft/network/protocol/{game,common}/Clientbound*Packet`).

use crate::codec::{DecodeResult, Reader};
use crate::entity::{PositionMoveRotation, Relative, position_move_rotation};

#[derive(Clone, Debug, PartialEq)]
pub enum LifecyclePacket {
    Login(Login),
    Respawn {
        spawn: SpawnInfo,
        /// `ClientboundRespawnPacket.KEEP_*` bits.
        data_to_keep: u8,
    },
    KeepAlive(i64),
    Ping(i32),
    /// The reason, a text component as NBT.
    Disconnect(Box<[u8]>),
    PlayerPosition {
        id: u32,
        change: PositionMoveRotation,
        relative: Relative,
    },
    PlayerRotation {
        y_rot: f32,
        relative_y: bool,
        x_rot: f32,
        relative_x: bool,
    },
    SetTime {
        game_time: i64,
        /// `world_clock` registry ids and their states.
        clocks: Vec<(u32, ClockState)>,
    },
    SimulationDistance(u32),
}

/// `ClientboundLoginPacket`.
#[derive(Clone, Debug, PartialEq)]
pub struct Login {
    pub player_id: i32,
    pub hardcore: bool,
    pub levels: Vec<String>,
    pub max_players: u32,
    pub chunk_radius: u32,
    pub simulation_distance: u32,
    pub reduced_debug_info: bool,
    pub show_death_screen: bool,
    pub do_limited_crafting: bool,
    pub spawn: SpawnInfo,
    pub online_mode: bool,
    pub enforces_secure_chat: bool,
}

/// `CommonPlayerSpawnInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnInfo {
    /// A `dimension_type` registry id.
    pub dimension_type: u32,
    pub dimension: String,
    pub seed: i64,
    /// `GameType` ids; the previous one is `None` for -1
    /// (`GameType.byNullableId`).
    pub game_type: u8,
    pub previous_game_type: Option<u8>,
    pub is_debug: bool,
    pub is_flat: bool,
    /// The dimension and block position of the last death.
    pub last_death: Option<(String, (i32, i32, i32))>,
    pub portal_cooldown: u32,
    pub sea_level: i32,
}

/// `ClockNetworkState`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockState {
    pub total_ticks: i64,
    pub partial_tick: f32,
    pub rate: f32,
}

/// Decodes packet `name`, or `None` if it isn't a lifecycle packet.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<LifecyclePacket>> {
    let read: fn(&mut Reader) -> DecodeResult<LifecyclePacket> = match name {
        "login" => |r| Ok(LifecyclePacket::Login(login(r)?)),
        // ClientboundRespawnPacket.java:22
        "respawn" => |r| {
            Ok(LifecyclePacket::Respawn {
                spawn: spawn_info(r)?,
                data_to_keep: r.u8()?,
            })
        },
        // ClientboundKeepAlivePacket.java:23
        "keep_alive" => |r| Ok(LifecyclePacket::KeepAlive(r.i64()?)),
        // ClientboundPingPacket.java:23
        "ping" => |r| Ok(LifecyclePacket::Ping(r.i32()?)),
        // ClientboundDisconnectPacket.java:20
        "disconnect" => |r| Ok(LifecyclePacket::Disconnect(r.nbt()?.into())),
        // ClientboundPlayerPositionPacket.java:19
        "player_position" => |r| {
            Ok(LifecyclePacket::PlayerPosition {
                id: r.varint()?,
                change: position_move_rotation(r)?,
                relative: Relative::from_bits(r.i32()?),
            })
        },
        // ClientboundPlayerRotationPacket.java:16
        "player_rotation" => |r| {
            Ok(LifecyclePacket::PlayerRotation {
                y_rot: r.f32()?,
                relative_y: r.bool()?,
                x_rot: r.f32()?,
                relative_x: r.bool()?,
            })
        },
        // ClientboundSetTimePacket.java:21, ClockNetworkState.java:14
        "set_time" => |r| {
            Ok(LifecyclePacket::SetTime {
                game_time: r.i64()?,
                clocks: r.list(usize::MAX, |r| {
                    Ok((
                        r.varint()?,
                        ClockState {
                            total_ticks: r.varlong()? as i64,
                            partial_tick: r.f32()?,
                            rate: r.f32()?,
                        },
                    ))
                })?,
            })
        },
        // ClientboundSetSimulationDistancePacket.java:18
        "set_simulation_distance" => |r| Ok(LifecyclePacket::SimulationDistance(r.varint()?)),
        _ => return None,
    };
    Some(read(r))
}

/// `ClientboundLoginPacket(RegistryFriendlyByteBuf)`
/// (ClientboundLoginPacket.java:28).
fn login(r: &mut Reader) -> DecodeResult<Login> {
    Ok(Login {
        player_id: r.i32()?,
        hardcore: r.bool()?,
        levels: r.list(usize::MAX, |r| Ok(r.identifier()?.to_owned()))?,
        max_players: r.varint()?,
        chunk_radius: r.varint()?,
        simulation_distance: r.varint()?,
        reduced_debug_info: r.bool()?,
        show_death_screen: r.bool()?,
        do_limited_crafting: r.bool()?,
        spawn: spawn_info(r)?,
        online_mode: r.bool()?,
        enforces_secure_chat: r.bool()?,
    })
}

/// `CommonPlayerSpawnInfo(RegistryFriendlyByteBuf)`
/// (CommonPlayerSpawnInfo.java:23).
fn spawn_info(r: &mut Reader) -> DecodeResult<SpawnInfo> {
    Ok(SpawnInfo {
        dimension_type: r.varint()?,
        dimension: r.identifier()?.to_owned(),
        seed: r.i64()?,
        game_type: r.u8()?,
        previous_game_type: match r.i8()? {
            -1 => None,
            id => Some(id as u8),
        },
        is_debug: r.bool()?,
        is_flat: r.bool()?,
        last_death: r.optional(|r| Ok((r.identifier()?.to_owned(), r.block_pos()?)))?,
        portal_cooldown: r.varint()?,
        sea_level: r.var_i32()?,
    })
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::*;
    use crate::wire::{write_utf, write_varint};

    fn decode(name: &str, bytes: &[u8]) -> LifecyclePacket {
        let mut r = Reader::new(bytes);
        let packet = read(name, &mut r).expect("a lifecycle packet").unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    fn utf(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        write_utf(&mut out, s);
        out
    }

    fn var(v: u32) -> Vec<u8> {
        let mut out = Vec::new();
        write_varint(&mut out, v);
        out
    }

    /// Dimension type 2 in the overworld, creative after no previous mode,
    /// with `last_death` and sea level 63.
    fn spawn_bytes(last_death: bool) -> Vec<u8> {
        let mut out = [
            var(2),
            utf("minecraft:overworld"),
            9i64.to_be_bytes().to_vec(),
        ]
        .concat();
        out.extend([1, 0xFF, 0, 1]);
        if last_death {
            out.push(1);
            out.extend(utf("minecraft:the_nether"));
            out.extend(((5i64 << 38) | (-3i64 & 0x3FF_FFFF) << 12 | 70).to_be_bytes());
        } else {
            out.push(0);
        }
        out.extend([var(4), var(63)].concat());
        out
    }

    fn spawn(last_death: Option<(String, (i32, i32, i32))>) -> SpawnInfo {
        SpawnInfo {
            dimension_type: 2,
            dimension: "minecraft:overworld".into(),
            seed: 9,
            game_type: 1,
            previous_game_type: None,
            is_debug: false,
            is_flat: true,
            last_death,
            portal_cooldown: 4,
            sea_level: 63,
        }
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("block_update", &mut Reader::new(&[])).is_none());
    }

    #[test]
    fn login() {
        let bytes = [
            &7i32.to_be_bytes()[..],
            &[1],
            &var(2),
            &utf("minecraft:overworld"),
            &utf("minecraft:the_nether"),
            &[20, 10, 8, 0, 1, 0],
            &spawn_bytes(true),
            &[1, 0],
        ]
        .concat();
        assert_eq!(
            decode("login", &bytes),
            LifecyclePacket::Login(Login {
                player_id: 7,
                hardcore: true,
                levels: vec!["minecraft:overworld".into(), "minecraft:the_nether".into()],
                max_players: 20,
                chunk_radius: 10,
                simulation_distance: 8,
                reduced_debug_info: false,
                show_death_screen: true,
                do_limited_crafting: false,
                spawn: spawn(Some(("minecraft:the_nether".into(), (5, 70, -3)))),
                online_mode: true,
                enforces_secure_chat: false,
            })
        );
    }

    #[test]
    fn respawn_with_no_previous_game_type() {
        let bytes = [spawn_bytes(false), vec![3]].concat();
        assert_eq!(
            decode("respawn", &bytes),
            LifecyclePacket::Respawn {
                spawn: spawn(None),
                data_to_keep: 3,
            }
        );
    }

    #[test]
    fn keep_alive_ping_and_disconnect() {
        assert_eq!(
            decode("keep_alive", &(-2i64).to_be_bytes()),
            LifecyclePacket::KeepAlive(-2)
        );
        assert_eq!(
            decode("ping", &5i32.to_be_bytes()),
            LifecyclePacket::Ping(5)
        );
        let reason = [8, 0, 3, b'b', b'y', b'e'];
        assert_eq!(
            decode("disconnect", &reason),
            LifecyclePacket::Disconnect(reason.into())
        );
    }

    #[test]
    fn player_position_and_rotation() {
        let mut bytes = var(3);
        for v in [1.5f64, 64.0, -2.0, 0.0, 0.5, 0.0] {
            bytes.extend(v.to_be_bytes());
        }
        bytes.extend(90f32.to_be_bytes());
        bytes.extend(10f32.to_be_bytes());
        // Y, X_ROT and ROTATE_DELTA relative.
        bytes.extend((0b1_0001_0010i32).to_be_bytes());
        assert_eq!(
            decode("player_position", &bytes),
            LifecyclePacket::PlayerPosition {
                id: 3,
                change: PositionMoveRotation {
                    position: DVec3::new(1.5, 64.0, -2.0),
                    delta: DVec3::new(0.0, 0.5, 0.0),
                    y_rot: 90.0,
                    x_rot: 10.0,
                },
                relative: Relative {
                    y: true,
                    x_rot: true,
                    rotate_delta: true,
                    ..Default::default()
                },
            }
        );
        let rotation = [
            &45f32.to_be_bytes()[..],
            &[1],
            &(-30f32).to_be_bytes(),
            &[0],
        ]
        .concat();
        assert_eq!(
            decode("player_rotation", &rotation),
            LifecyclePacket::PlayerRotation {
                y_rot: 45.0,
                relative_y: true,
                x_rot: -30.0,
                relative_x: false,
            }
        );
    }

    #[test]
    fn set_time_with_two_clocks() {
        let mut bytes = 100i64.to_be_bytes().to_vec();
        bytes.extend(var(2));
        for (id, ticks) in [(0u32, 6000u32), (1, 300)] {
            bytes.extend(var(id));
            bytes.extend(var(ticks));
            bytes.extend(0.5f32.to_be_bytes());
            bytes.extend(1f32.to_be_bytes());
        }
        let state = |total_ticks| ClockState {
            total_ticks,
            partial_tick: 0.5,
            rate: 1.0,
        };
        assert_eq!(
            decode("set_time", &bytes),
            LifecyclePacket::SetTime {
                game_time: 100,
                clocks: vec![(0, state(6000)), (1, state(300))],
            }
        );
        assert_eq!(
            decode("set_simulation_distance", &[12]),
            LifecyclePacket::SimulationDistance(12)
        );
    }
}
