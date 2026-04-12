use azalea_protocol::address::ServerAddr;
use azalea_protocol::connect::{Connection, ReadConnection, WriteConnection};
use azalea_protocol::packets::config::{ClientboundConfigPacket, ServerboundConfigPacket};
use azalea_protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket};
use azalea_protocol::packets::handshake::s_intention::ServerboundIntention;
use azalea_protocol::packets::login::c_hello::ClientboundHello;
use azalea_protocol::packets::login::s_hello::ServerboundHello;
use azalea_protocol::packets::login::s_key::ServerboundKey;
use azalea_protocol::packets::login::s_login_acknowledged::ServerboundLoginAcknowledged;
use azalea_protocol::packets::login::{ClientboundLoginPacket, ServerboundLoginPacket};
use azalea_protocol::packets::{ClientIntention, PROTOCOL_VERSION};
use azalea_protocol::read::ReadPacketError;
use crossbeam_channel::Sender;
use thiserror::Error;
use tokio::sync::mpsc;

use super::NetworkEvent;
use super::handler::handle_game_packet;
use super::sender::PacketSender;

#[derive(Error, Debug)]
pub enum ConnectionError {
    #[error("invalid server address: {0}")]
    InvalidAddress(String),

    #[error("connection failed: {0}")]
    Connect(#[from] azalea_protocol::connect::ConnectionError),

    #[error("packet read error: {0}")]
    Read(#[from] Box<ReadPacketError>),

    #[error("packet write error: {0}")]
    Write(#[from] std::io::Error),

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("disconnected by server: {0}")]
    Disconnected(String),

    #[error("encryption failed: {0}")]
    Encryption(String),
}

pub struct ConnectArgs {
    pub server: String,
    pub username: String,
    pub uuid: uuid::Uuid,
    pub access_token: Option<String>,
    pub view_distance: u8,
}

pub struct ConnectionHandle {
    pub events: crossbeam_channel::Receiver<NetworkEvent>,
    pub chat_tx: crossbeam_channel::Sender<String>,
    pub packet_tx: mpsc::UnboundedSender<ServerboundGamePacket>,
    pub task: tokio::task::JoinHandle<()>,
}

pub fn spawn_connection(rt: &tokio::runtime::Runtime, args: ConnectArgs) -> ConnectionHandle {
    let (event_tx, event_rx) = crossbeam_channel::bounded(4096);
    let (chat_tx, chat_rx) = crossbeam_channel::bounded::<String>(64);
    let (packet_tx, packet_rx) = mpsc::unbounded_channel::<ServerboundGamePacket>();
    let game_packet_tx = packet_tx.clone();
    let task = rt.spawn(async move {
        if let Err(e) =
            connect_to_server(args, event_tx.clone(), chat_rx, game_packet_tx, packet_rx).await
        {
            tracing::error!("Network error: {e}");
            let reason = friendly_error_reason(&e);
            let _ = event_tx.try_send(NetworkEvent::Disconnected { reason });
        }
    });
    ConnectionHandle {
        events: event_rx,
        chat_tx,
        packet_tx,
        task,
    }
}

pub async fn connect_to_server(
    args: ConnectArgs,
    event_tx: Sender<NetworkEvent>,
    chat_rx: crossbeam_channel::Receiver<String>,
    game_packet_tx: mpsc::UnboundedSender<ServerboundGamePacket>,
    game_packet_rx: mpsc::UnboundedReceiver<ServerboundGamePacket>,
) -> Result<(), ConnectionError> {
    let server_addr: ServerAddr = args
        .server
        .as_str()
        .try_into()
        .map_err(|_| ConnectionError::InvalidAddress(args.server.clone()))?;
    let addr = azalea_protocol::resolve::resolve_address(&server_addr)
        .await
        .map_err(|e| ConnectionError::InvalidAddress(format!("{}: {e}", args.server)))?;
    tracing::info!("Connecting to {} (resolved: {addr})...", args.server);

    let mut conn: Connection<_, _> = Connection::new(&addr).await?;

    conn.write(ServerboundIntention {
        protocol_version: PROTOCOL_VERSION,
        hostname: server_addr.host.clone(),
        port: server_addr.port,
        intention: ClientIntention::Login,
    })
    .await?;

    let mut conn = conn.login();

    conn.write(ServerboundHello {
        name: args.username.clone(),
        profile_id: args.uuid,
    })
    .await?;

    tracing::info!("Sent login hello as {}", args.username);

    login_sequence(&mut conn, &args).await?;

    conn.write(ServerboundLoginAcknowledged {}).await?;
    let mut conn = conn.config();

    tracing::info!("Entering configuration phase");
    let registry_holder = config_sequence(&mut conn, args.view_distance, &event_tx).await?;

    let conn = conn.game();
    tracing::info!("Entering game state");
    let biome_colors = extract_biome_climate(&registry_holder);
    let _ = event_tx.try_send(NetworkEvent::BiomeColors {
        colors: biome_colors,
    });
    let _ = event_tx.try_send(NetworkEvent::Connected);

    game_loop(
        conn,
        &event_tx,
        chat_rx,
        game_packet_tx,
        game_packet_rx,
        registry_holder,
    )
    .await
}

async fn login_sequence(
    conn: &mut Connection<ClientboundLoginPacket, ServerboundLoginPacket>,
    args: &ConnectArgs,
) -> Result<(), ConnectionError> {
    loop {
        let packet: ClientboundLoginPacket = conn.read().await?;
        tracing::info!("Login packet: {:?}", std::mem::discriminant(&packet));
        match packet {
            ClientboundLoginPacket::Hello(p) => {
                handle_encryption(conn, &p, args).await?;
            }
            ClientboundLoginPacket::LoginCompression(p) => {
                conn.set_compression_threshold(p.compression_threshold);
                tracing::info!(
                    "Compression enabled (threshold: {})",
                    p.compression_threshold
                );
            }
            ClientboundLoginPacket::LoginFinished(p) => {
                tracing::info!(
                    "Login success: {} ({})",
                    p.game_profile.name,
                    p.game_profile.uuid
                );
                return Ok(());
            }
            ClientboundLoginPacket::LoginDisconnect(p) => {
                return Err(ConnectionError::Disconnected(format!("{}", p.reason)));
            }
            ClientboundLoginPacket::CookieRequest(p) => {
                conn.write(
                    azalea_protocol::packets::login::s_cookie_response::ServerboundCookieResponse {
                        key: p.key,
                        payload: None,
                    },
                )
                .await?;
            }
            _ => {
                tracing::debug!("Login packet: {:?}", std::mem::discriminant(&packet));
            }
        }
    }
}

async fn handle_encryption(
    conn: &mut Connection<ClientboundLoginPacket, ServerboundLoginPacket>,
    hello: &ClientboundHello,
    args: &ConnectArgs,
) -> Result<(), ConnectionError> {
    let e = azalea_crypto::encrypt(&hello.public_key, &hello.challenge)
        .map_err(ConnectionError::Encryption)?;

    if hello.should_authenticate {
        let access_token = args.access_token.as_deref().ok_or_else(|| {
            ConnectionError::Auth(
                "server requires authentication but no access token provided".into(),
            )
        })?;

        tracing::info!("Authenticating with session server (uuid: {})", args.uuid);
        conn.authenticate(access_token, &args.uuid, e.secret_key, hello, None)
            .await
            .map_err(|e: azalea_auth::sessionserver::ClientSessionServerError| {
                ConnectionError::Auth(e.to_string())
            })?;
        tracing::info!("Session server authentication successful");
    } else {
        tracing::info!("Server does not require authentication");
    }

    conn.write(ServerboundKey {
        key_bytes: e.encrypted_public_key,
        encrypted_challenge: e.encrypted_challenge,
    })
    .await?;

    conn.set_encryption_key(e.secret_key);
    tracing::info!("Encryption enabled");
    Ok(())
}

async fn config_sequence(
    conn: &mut Connection<ClientboundConfigPacket, ServerboundConfigPacket>,
    view_distance: u8,
    event_tx: &Sender<NetworkEvent>,
) -> Result<azalea_core::registry_holder::RegistryHolder, ConnectionError> {
    use azalea_core::delta::AzBuf;
    use azalea_core::registry_holder::RegistryHolder;
    use azalea_entity::HumanoidArm;
    use azalea_protocol::common::client_information::*;
    use azalea_protocol::packets::config::*;

    let mut registry_holder = RegistryHolder::default();

    // Vanilla announces its brand in the config phase; some servers key off it.
    let mut brand_payload = Vec::new();
    String::from("pomme")
        .azalea_write(&mut brand_payload)
        .unwrap();
    conn.write(ServerboundConfigPacket::CustomPayload(
        s_custom_payload::ServerboundCustomPayload {
            identifier: "minecraft:brand".into(),
            data: brand_payload.into(),
        },
    ))
    .await?;

    conn.write(ServerboundConfigPacket::ClientInformation(
        s_client_information::ServerboundClientInformation {
            information: ClientInformation {
                language: "en_us".into(),
                view_distance,
                chat_visibility: ChatVisibility::Full,
                chat_colors: true,
                model_customization: ModelCustomization {
                    cape: true,
                    jacket: true,
                    left_sleeve: true,
                    right_sleeve: true,
                    left_pants: true,
                    right_pants: true,
                    hat: true,
                },
                main_hand: HumanoidArm::Right,
                text_filtering_enabled: false,
                allows_listing: true,
                particle_status: ParticleStatus::All,
            },
        },
    ))
    .await?;

    loop {
        let packet: ClientboundConfigPacket = conn.read().await?;
        match packet {
            ClientboundConfigPacket::RegistryData(p) => {
                registry_holder.append(p.registry_id, p.entries);
            }
            ClientboundConfigPacket::UpdateTags(_) => {
                tracing::debug!("Received tags");
            }
            ClientboundConfigPacket::SelectKnownPacks(_) => {
                conn.write(ServerboundConfigPacket::SelectKnownPacks(
                    s_select_known_packs::ServerboundSelectKnownPacks {
                        known_packs: vec![],
                    },
                ))
                .await?;
            }
            ClientboundConfigPacket::KeepAlive(p) => {
                conn.write(ServerboundConfigPacket::KeepAlive(
                    s_keep_alive::ServerboundKeepAlive { id: p.id },
                ))
                .await?;
            }
            ClientboundConfigPacket::FinishConfiguration(_) => {
                conn.write(ServerboundConfigPacket::FinishConfiguration(
                    s_finish_configuration::ServerboundFinishConfiguration {},
                ))
                .await?;
                return Ok(registry_holder);
            }
            ClientboundConfigPacket::Disconnect(p) => {
                return Err(ConnectionError::Disconnected(format!("{}", p.reason)));
            }
            ClientboundConfigPacket::CookieRequest(p) => {
                conn.write(ServerboundConfigPacket::CookieResponse(
                    s_cookie_response::ServerboundCookieResponse {
                        key: p.key,
                        payload: None,
                    },
                ))
                .await?;
            }
            ClientboundConfigPacket::ResourcePackPush(p) => {
                tracing::info!(
                    "Server pushing resource pack {} (required: {})",
                    p.id,
                    p.required
                );
                let _ = event_tx.try_send(NetworkEvent::ResourcePackPush {
                    id: p.id,
                    url: p.url.clone(),
                    hash: p.hash.clone(),
                    required: p.required,
                });
                conn.write(ServerboundConfigPacket::ResourcePack(
                    s_resource_pack::ServerboundResourcePack {
                        id: p.id,
                        action: s_resource_pack::Action::Accepted,
                    },
                ))
                .await?;
            }
            ClientboundConfigPacket::ResourcePackPop(p) => {
                tracing::info!("Server popping resource pack {:?}", p.id);
                let _ = event_tx.try_send(NetworkEvent::ResourcePackPop { id: p.id });
            }
            _ => {
                tracing::debug!("Config packet: {:?}", std::mem::discriminant(&packet));
            }
        }
    }
}

fn extract_biome_climate(
    holder: &azalea_core::registry_holder::RegistryHolder,
) -> std::collections::HashMap<u32, crate::renderer::chunk::mesher::BiomeClimate> {
    use crate::renderer::chunk::mesher::{BiomeClimate, GrassColorModifier, int_to_rgb};

    let mut result = std::collections::HashMap::new();
    let biome_key: azalea_registry::identifier::Identifier = "minecraft:worldgen/biome".into();
    if let Some(registry) = holder.extra.get(&biome_key) {
        for (id, (_, nbt)) in registry.map.iter().enumerate() {
            let temp = nbt_float(nbt, "temperature").unwrap_or(0.8);
            let downfall = nbt_float(nbt, "downfall").unwrap_or(0.4);

            let effects = nbt.get("effects").and_then(|v| match v {
                simdnbt::owned::NbtTag::Compound(c) => Some(c),
                _ => None,
            });

            let grass_color_override = effects
                .and_then(|e| nbt_color_from_compound(e, "grass_color"))
                .map(int_to_rgb);

            let foliage_color_override = effects
                .and_then(|e| nbt_color_from_compound(e, "foliage_color"))
                .map(int_to_rgb);

            let grass_color_modifier = effects
                .and_then(|e| nbt_string_from_compound(e, "grass_color_modifier"))
                .map(|s| match s.as_str() {
                    "dark_forest" => GrassColorModifier::DarkForest,
                    "swamp" => GrassColorModifier::Swamp,
                    _ => GrassColorModifier::None,
                })
                .unwrap_or(GrassColorModifier::None);

            result.insert(
                id as u32,
                BiomeClimate {
                    temperature: temp,
                    downfall,
                    grass_color_override,
                    grass_color_modifier,
                    foliage_color_override,
                },
            );
        }
    }
    tracing::info!("Extracted {} biome climate entries", result.len());
    result
}

fn nbt_float(nbt: &simdnbt::owned::NbtCompound, key: &str) -> Option<f32> {
    nbt.get(key).and_then(|v| match v {
        simdnbt::owned::NbtTag::Float(f) => Some(*f),
        simdnbt::owned::NbtTag::Double(d) => Some(*d as f32),
        _ => None,
    })
}

fn nbt_color_from_compound(compound: &simdnbt::owned::NbtCompound, key: &str) -> Option<i32> {
    compound.get(key).and_then(|v| match v {
        simdnbt::owned::NbtTag::Int(i) => Some(*i),
        simdnbt::owned::NbtTag::Long(l) => Some(*l as i32),
        simdnbt::owned::NbtTag::String(s) => {
            let s = s.to_string();
            let hex = s.strip_prefix('#').unwrap_or(&s);
            i32::from_str_radix(hex, 16).ok()
        }
        _ => None,
    })
}

fn nbt_string_from_compound(compound: &simdnbt::owned::NbtCompound, key: &str) -> Option<String> {
    compound.get(key).and_then(|v| match v {
        simdnbt::owned::NbtTag::String(s) => Some(s.to_string()),
        _ => None,
    })
}

async fn game_loop(
    conn: Connection<ClientboundGamePacket, ServerboundGamePacket>,
    event_tx: &Sender<NetworkEvent>,
    chat_rx: crossbeam_channel::Receiver<String>,
    outbound_tx: mpsc::UnboundedSender<ServerboundGamePacket>,
    mut outbound_rx: mpsc::UnboundedReceiver<ServerboundGamePacket>,
    registry_holder: azalea_core::registry_holder::RegistryHolder,
) -> Result<(), ConnectionError> {
    let (mut reader, mut writer): (
        ReadConnection<ClientboundGamePacket>,
        WriteConnection<ServerboundGamePacket>,
    ) = conn.into_split();

    let sender = PacketSender::new(outbound_tx.clone());

    tokio::spawn(async move {
        while let Some(packet) = outbound_rx.recv().await {
            if let Err(e) = writer.write(packet).await {
                tracing::error!("Failed to write packet: {e}");
                break;
            }
        }
    });

    let chat_outbound_tx = outbound_tx;
    tokio::spawn(async move {
        while let Ok(msg) = tokio::task::block_in_place(|| chat_rx.recv()) {
            let packet = if let Some(command) = msg.strip_prefix('/') {
                ServerboundGamePacket::ChatCommand(
                    azalea_protocol::packets::game::s_chat_command::ServerboundChatCommand {
                        command: command.to_string(),
                    },
                )
            } else {
                // TODO: implement chat signing - requires enforce-secure-profile=false for now
                ServerboundGamePacket::Chat(
                    azalea_protocol::packets::game::s_chat::ServerboundChat {
                        message: msg,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                        salt: 0,
                        signature: None,
                        last_seen_messages: Default::default(),
                    },
                )
            };
            if chat_outbound_tx.send(packet).is_err() {
                break;
            }
        }
    });

    loop {
        match reader.read().await {
            Ok(packet) => handle_game_packet(&packet, &sender, event_tx, &registry_holder),
            Err(e) if is_recoverable_read_error(&e) => {
                tracing::warn!("Skipping malformed packet: {e}");
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn is_recoverable_read_error(err: &ReadPacketError) -> bool {
    matches!(
        err,
        ReadPacketError::Parse { .. }
            | ReadPacketError::UnknownPacketId { .. }
            | ReadPacketError::LeftoverData { .. }
    )
}

fn friendly_error_reason(err: &ConnectionError) -> String {
    let msg = err.to_string();
    if msg.contains("connection refused") || msg.contains("Connection refused") {
        "Connection refused".to_string()
    } else if msg.contains("Connection closed")
        || msg.contains("connection reset")
        || msg.contains("broken pipe")
    {
        "Server closed".to_string()
    } else if msg.contains("timed out") || msg.contains("Timed out") {
        "Connection timed out".to_string()
    } else if msg.contains("no addresses found") || msg.contains("failed to lookup") {
        "Unknown host".to_string()
    } else {
        msg
    }
}
