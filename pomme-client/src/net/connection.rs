use azalea_protocol::address::ServerAddr;
use azalea_protocol::connect::{Connection, ReadConnection, WriteConnection};
use azalea_protocol::packets::ClientIntention;
use azalea_protocol::packets::config::{ClientboundConfigPacket, ServerboundConfigPacket};
use azalea_protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket};
use azalea_protocol::packets::login::c_hello::ClientboundHello;
use azalea_protocol::packets::login::s_hello::ServerboundHello;
use azalea_protocol::packets::login::s_key::ServerboundKey;
use azalea_protocol::packets::login::s_login_acknowledged::ServerboundLoginAcknowledged;
use azalea_protocol::packets::login::{ClientboundLoginPacket, ServerboundLoginPacket};
use azalea_protocol::read::{ReadPacketError, deserialize_packet};
use crossbeam_channel::Sender;
use thiserror::Error;
use tokio::sync::mpsc;

use super::NetworkEvent;
use super::handler::{handle_game_packet, handle_raw_game_packet};
use super::sender::{Outbound, PacketSender};

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

impl From<super::resolve::ConnectError> for ConnectionError {
    fn from(e: super::resolve::ConnectError) -> Self {
        use super::resolve::ConnectError;
        match e {
            ConnectError::Resolve(e) => Self::InvalidAddress(e.to_string()),
            ConnectError::Unreachable(e) => Self::Connect(e.into()),
            ConnectError::Handshake(e) => Self::Connect(e),
        }
    }
}

pub struct ConnectArgs {
    pub server: String,
    pub username: String,
    pub uuid: uuid::Uuid,
    pub access_token: Option<String>,
    pub view_distance: u8,
    /// The server's protocol from an earlier server-list ping, when joining
    /// from the list; saves `negotiate_wire_version` its status probe.
    pub protocol: Option<i32>,
}

pub struct ConnectionHandle {
    pub event_rx: crossbeam_channel::Receiver<NetworkEvent>,
    pub chat_tx: crossbeam_channel::Sender<String>,
    pub packet_tx: PacketSender,
    pub task: tokio::task::JoinHandle<()>,
}

impl Drop for ConnectionHandle {
    fn drop(&mut self) {
        self.task.abort();
        // The session is over: restore the launched version's wire protocol
        // and block table so nothing stale leaks into the next one.
        crate::version::clear_session_protocol();
        crate::world::block::set_active_protocol(crate::version::selected_protocol());
    }
}

pub fn spawn_connection(rt: &tokio::runtime::Runtime, args: ConnectArgs) -> ConnectionHandle {
    let (event_tx, event_rx) = crossbeam_channel::bounded(4096);
    let (chat_tx, chat_rx) = crossbeam_channel::bounded::<String>(64);
    let (packet_tx, packet_rx) = mpsc::unbounded_channel::<Outbound>();
    let game_packet_tx = packet_tx.clone();
    let packet_tx = PacketSender::new(packet_tx);
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
        event_rx,
        chat_tx,
        packet_tx,
        task,
    }
}

pub async fn connect_to_server(
    args: ConnectArgs,
    event_tx: Sender<NetworkEvent>,
    chat_rx: crossbeam_channel::Receiver<String>,
    game_packet_tx: mpsc::UnboundedSender<Outbound>,
    game_packet_rx: mpsc::UnboundedReceiver<Outbound>,
) -> Result<(), ConnectionError> {
    let server_addr: ServerAddr = args
        .server
        .as_str()
        .try_into()
        .map_err(|_| ConnectionError::InvalidAddress(args.server.clone()))?;
    negotiate_wire_version(&server_addr, args.protocol).await;
    let conn = super::resolve::connect(&server_addr, ClientIntention::Login).await?;
    let mut conn = conn.login();

    conn.write(ServerboundHello {
        name: args.username.clone(),
        profile_id: args.uuid,
    })
    .await?;

    tracing::info!("Sent login hello as {} ({})", args.username, args.uuid);
    if args.access_token.is_none() {
        tracing::warn!(
            "Connecting offline (no access token). The server keys op/permissions to the \
             authenticated account, so op-only commands like /time may return \"Unknown command\" \
             under this offline identity."
        );
    }

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

/// Adopts the server's protocol as the wire version when translation data
/// for it exists, so one client joins any supported server version;
/// otherwise the launched version is kept (and the server shows its own
/// mismatch message, as before). The protocol comes from `known` (a
/// server-list ping; a stale one just means the server rejects the handshake
/// with its mismatch message, same as before) or a status probe. Sets the
/// session protocol and the matching block-state table, so it must run
/// before the login handshake and before any world state loads.
async fn negotiate_wire_version(server_addr: &ServerAddr, known: Option<i32>) {
    let selected = crate::version::selected_protocol();
    let probed = match known {
        Some(p) => Some(p),
        None => {
            let probe = async {
                let (status, _) = super::resolve::request_status(server_addr).await.ok()?;
                Some(status.version.protocol)
            };
            tokio::time::timeout(std::time::Duration::from_secs(5), probe)
                .await
                .ok()
                .flatten()
        }
    };
    let wire = match probed {
        Some(p) if super::translate::joinable(p) => p,
        Some(p) => {
            tracing::warn!("Server speaks unsupported protocol {p}; connecting as {selected}");
            selected
        }
        None => {
            tracing::warn!("Server protocol probe failed; connecting as {selected}");
            selected
        }
    };
    tracing::info!("Negotiated wire protocol {wire}");
    crate::version::set_session_protocol(wire);
    crate::world::block::set_active_protocol(wire);
}

async fn login_sequence(
    conn: &mut Connection<ClientboundLoginPacket, ServerboundLoginPacket>,
    args: &ConnectArgs,
) -> Result<(), ConnectionError> {
    loop {
        // Read the raw frame ourselves so older-version layouts can be
        // rewritten before the typed decode (26.1's login_finished lacks the
        // trailing session id).
        let raw = conn.reader.raw.read().await?;
        let raw = match super::translate::active() {
            Some(t) => t.translate_login_frame(raw),
            None => raw,
        };
        let packet: ClientboundLoginPacket = deserialize_packet(&mut std::io::Cursor::new(&raw))?;
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

            let dry_foliage_color_override = effects
                .and_then(|e| nbt_color_from_compound(e, "dry_foliage_color"))
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
                    dry_foliage_color_override,
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
    outbound_tx: mpsc::UnboundedSender<Outbound>,
    mut outbound_rx: mpsc::UnboundedReceiver<Outbound>,
    registry_holder: azalea_core::registry_holder::RegistryHolder,
) -> Result<(), ConnectionError> {
    let (mut reader, mut writer): (
        ReadConnection<ClientboundGamePacket>,
        WriteConnection<ServerboundGamePacket>,
    ) = conn.into_split();

    let sender = PacketSender::new(outbound_tx.clone());

    let shared_tree: crate::net::commands::SharedCommandTree =
        std::sync::Arc::new(parking_lot::Mutex::new(None));

    tokio::spawn(async move {
        let translation = super::translate::active();
        'writer: while let Some(out) = outbound_rx.recv().await {
            // Frames are in the latest layout (azalea's serializer and
            // `wire`'s encoders both emit it); older wire versions get them
            // translated before framing.
            let frame = match out {
                Outbound::Packet(mut packet) => {
                    if let Some(t) = translation {
                        t.remap_outbound(&mut packet);
                    }
                    match azalea_protocol::write::serialize_packet(&*packet) {
                        Ok(frame) => Vec::from(frame),
                        Err(e) => {
                            tracing::error!("Failed to serialize packet: {e}");
                            break;
                        }
                    }
                }
                Outbound::Raw(bytes) => bytes,
            };
            let frames = match translation {
                Some(t) if t.translates_outbound() => t.translate_outbound_game_frame(frame),
                _ => vec![frame],
            };
            for frame in frames {
                if let Err(e) = writer.raw.write(&frame).await {
                    tracing::error!("Failed to write packet: {e}");
                    break 'writer;
                }
            }
        }
    });

    let chat_outbound_tx = outbound_tx;
    let chat_tree = shared_tree.clone();
    tokio::spawn(async move {
        // TODO: secure chat session + signing for enforce-secure-profile=true servers.
        // When access_token is set, fetch profile certs
        // (azalea_auth::certs::fetch_certificates),
        // send ServerboundChatSessionUpdate, then sign chat and signable-arg commands
        // (ServerboundChatCommandSigned) with azalea_crypto signing (needs the
        // "signing" feature). Everything is sent unsigned atm, which only
        // works on enforce-secure-profile=false.
        while let Ok(msg) = tokio::task::block_in_place(|| chat_rx.recv()) {
            let packet = if let Some(command) = msg.strip_prefix('/') {
                tracing::info!("Sending command: {command:?}");
                let signable = chat_tree
                    .lock()
                    .as_ref()
                    .map(|tree| tree.has_signable_args(command))
                    .unwrap_or(false);
                if signable {
                    tracing::warn!(
                        "Command has signable arguments but chat signing is not implemented; sending unsigned"
                    );
                }
                ServerboundGamePacket::ChatCommand(
                    azalea_protocol::packets::game::s_chat_command::ServerboundChatCommand {
                        command: command.to_string(),
                    },
                )
            } else {
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
            if chat_outbound_tx
                .send(Outbound::Packet(Box::new(packet)))
                .is_err()
            {
                break;
            }
        }
    });

    // Share the registries with the game loop for hashing predicted container
    // clicks.
    let registry_holder = std::sync::Arc::new(registry_holder);
    let _ = event_tx.try_send(NetworkEvent::Registries(registry_holder.clone()));

    let translation = super::translate::active();
    loop {
        let raw = match reader.raw.read().await {
            Ok(raw) => raw,
            Err(e) => {
                skip_malformed_packet(e)?;
                continue;
            }
        };
        let raw = match translation {
            Some(t) => match t.translate_game_frame(raw) {
                Some(raw) => raw,
                None => continue,
            },
            None => raw,
        };
        if handle_raw_game_packet(&raw, event_tx) {
            continue;
        }
        match deserialize_packet::<ClientboundGamePacket>(&mut std::io::Cursor::new(&raw)) {
            Ok(mut packet) => {
                if let Some(t) = translation
                    && !t.remap_inbound(&mut packet)
                {
                    continue;
                }
                handle_game_packet(&packet, &sender, event_tx, &registry_holder, &shared_tree)
            }
            Err(e) => skip_malformed_packet(e)?,
        }
    }
}

/// Recoverable decode errors skip the packet; anything else tears down the
/// connection.
fn skip_malformed_packet(err: Box<ReadPacketError>) -> Result<(), ConnectionError> {
    match &*err {
        ReadPacketError::Parse { .. }
        | ReadPacketError::UnknownPacketId { .. }
        | ReadPacketError::LeftoverData { .. } => {
            tracing::warn!("Skipping malformed packet: {err}");
            Ok(())
        }
        _ => Err(err.into()),
    }
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
