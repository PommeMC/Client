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
    use super::*;

    fn decode(name: &str, bytes: &[u8]) -> LifecyclePacket {
        let mut r = Reader::new(bytes);
        let packet = read(name, &mut r).expect("a lifecycle packet").unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("block_update", &mut Reader::new(&[])).is_none());
    }

    /// Each case decodes vanilla's own encoding of the packet the oracle
    /// built (`CodecOracle.lifecycle`) to exactly its values.
    #[test]
    fn matches_vanilla_encoder() {
        use crate::vanilla_fixtures::{case, registry_id};

        let vanilla = |name: &str| {
            let packet = name.split('/').next().unwrap();
            decode(packet, &case(name))
        };
        let spawn = SpawnInfo {
            dimension_type: registry_id("dimension_type", "minecraft:overworld"),
            dimension: "minecraft:overworld".into(),
            seed: 42,
            game_type: 1,
            previous_game_type: None,
            is_debug: false,
            is_flat: true,
            last_death: None,
            portal_cooldown: 4,
            sea_level: 63,
        };
        let died = SpawnInfo {
            dimension_type: registry_id("dimension_type", "minecraft:the_nether"),
            dimension: "minecraft:the_nether".into(),
            seed: -7,
            game_type: 0,
            previous_game_type: Some(2),
            is_debug: true,
            is_flat: false,
            last_death: Some(("minecraft:the_nether".into(), (5, 70, -3))),
            portal_cooldown: 0,
            sea_level: 32,
        };
        assert_eq!(
            vanilla("login/fresh"),
            LifecyclePacket::Login(Login {
                player_id: 7,
                hardcore: false,
                levels: vec![
                    "minecraft:overworld".into(),
                    "minecraft:the_nether".into(),
                    "minecraft:the_end".into(),
                ],
                max_players: 20,
                chunk_radius: 10,
                simulation_distance: 8,
                reduced_debug_info: false,
                show_death_screen: true,
                do_limited_crafting: false,
                spawn,
                online_mode: true,
                enforces_secure_chat: false,
            })
        );
        assert_eq!(
            vanilla("login/after_death"),
            LifecyclePacket::Login(Login {
                player_id: -1,
                hardcore: true,
                levels: vec!["minecraft:the_nether".into()],
                max_players: 0,
                chunk_radius: 2,
                simulation_distance: 2,
                reduced_debug_info: true,
                show_death_screen: false,
                do_limited_crafting: true,
                spawn: died.clone(),
                online_mode: false,
                enforces_secure_chat: true,
            })
        );
        assert_eq!(
            vanilla("respawn/keep_all"),
            LifecyclePacket::Respawn {
                spawn: died,
                data_to_keep: 3,
            }
        );
        assert_eq!(
            vanilla("keep_alive/negative"),
            LifecyclePacket::KeepAlive(-2)
        );
        assert_eq!(vanilla("ping/negative"), LifecyclePacket::Ping(-5));
        assert_eq!(
            vanilla("disconnect/styled"),
            LifecyclePacket::Disconnect(case("disconnect/styled").into())
        );
        let (change, relative) = crate::vanilla_fixtures::teleport();
        assert_eq!(
            vanilla("player_position/mixed_relative"),
            LifecyclePacket::PlayerPosition {
                id: 3,
                change,
                relative,
            }
        );
        assert_eq!(
            vanilla("player_rotation/relative_yaw"),
            LifecyclePacket::PlayerRotation {
                y_rot: 45.0,
                relative_y: true,
                x_rot: -30.0,
                relative_x: false,
            }
        );
        assert_eq!(
            vanilla("set_time/two_clocks"),
            LifecyclePacket::SetTime {
                game_time: 12000,
                clocks: vec![
                    (
                        registry_id("world_clock", "minecraft:overworld"),
                        ClockState {
                            total_ticks: 6000,
                            partial_tick: 0.5,
                            rate: 1.0,
                        },
                    ),
                    (
                        registry_id("world_clock", "minecraft:the_end"),
                        ClockState {
                            total_ticks: 300,
                            partial_tick: 0.0,
                            rate: 0.0,
                        },
                    ),
                ],
            }
        );
        assert_eq!(
            vanilla("set_simulation_distance/twelve"),
            LifecyclePacket::SimulationDistance(12)
        );
    }
}
