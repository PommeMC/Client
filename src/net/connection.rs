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

use super::handler::handle_game_packet;
use super::sender::PacketSender;
use super::NetworkEvent;

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
}

pub fn spawn_connection(rt: &tokio::runtime::Runtime, args: ConnectArgs) -> ConnectionHandle {
    let (event_tx, event_rx) = crossbeam_channel::bounded(4096);
    let (chat_tx, chat_rx) = crossbeam_channel::bounded::<String>(64);
    let (packet_tx, packet_rx) = mpsc::unbounded_channel::<ServerboundGamePacket>();
    let game_packet_tx = packet_tx.clone();
    rt.spawn(async move {
        if let Err(e) =
            connect_to_server(args, event_tx.clone(), chat_rx, game_packet_tx, packet_rx).await
        {
            log::error!("Network error: {e}");
            let reason = friendly_error_reason(&e);
            let _ = event_tx.try_send(NetworkEvent::Disconnected { reason });
        }
    });
    ConnectionHandle {
        events: event_rx,
        chat_tx,
        packet_tx,
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
    log::info!("Connecting to {} (resolved: {addr})...", args.server);

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

    log::info!("Sent login hello as {}", args.username);

    login_sequence(&mut conn, &args).await?;

    conn.write(ServerboundLoginAcknowledged {}).await?;
    let mut conn = conn.config();

    log::info!("Entering configuration phase");
    let registry_holder = config_sequence(&mut conn, args.view_distance).await?;

    let conn = conn.game();
    log::info!("Entering game state");
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
        log::info!("Login packet: {:?}", std::mem::discriminant(&packet));
        match packet {
            ClientboundLoginPacket::Hello(p) => {
                handle_encryption(conn, &p, args).await?;
            }
            ClientboundLoginPacket::LoginCompression(p) => {
                conn.set_compression_threshold(p.compression_threshold);
                log::info!(
                    "Compression enabled (threshold: {})",
                    p.compression_threshold
                );
            }
            ClientboundLoginPacket::LoginFinished(p) => {
                log::info!(
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
                log::debug!("Login packet: {:?}", std::mem::discriminant(&packet));
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

        log::info!("Authenticating with session server (uuid: {})", args.uuid);
        conn.authenticate(access_token, &args.uuid, e.secret_key, hello, None)
            .await
            .map_err(|e: azalea_auth::sessionserver::ClientSessionServerError| {
                ConnectionError::Auth(e.to_string())
            })?;
        log::info!("Session server authentication successful");
    } else {
        log::info!("Server does not require authentication");
    }

    conn.write(ServerboundKey {
        key_bytes: e.encrypted_public_key,
        encrypted_challenge: e.encrypted_challenge,
    })
    .await?;

    conn.set_encryption_key(e.secret_key);
    log::info!("Encryption enabled");
    Ok(())
}

async fn config_sequence(
    conn: &mut Connection<ClientboundConfigPacket, ServerboundConfigPacket>,
    view_distance: u8,
) -> Result<azalea_core::registry_holder::RegistryHolder, ConnectionError> {
    use azalea_core::registry_holder::RegistryHolder;
    use azalea_entity::HumanoidArm;
    use azalea_protocol::common::client_information::*;
    use azalea_protocol::packets::config::*;

    let mut registry_holder = RegistryHolder::default();

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
                log::debug!("Received tags");
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
            _ => {
                log::debug!("Config packet: {:?}", std::mem::discriminant(&packet));
            }
        }
    }
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
                log::error!("Failed to write packet: {e}");
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
                log::warn!("Skipping malformed packet: {e}");
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
