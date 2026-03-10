use std::net::SocketAddr;

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
}

pub struct ConnectionHandle {
    pub events: crossbeam_channel::Receiver<NetworkEvent>,
    pub chat_tx: crossbeam_channel::Sender<String>,
    pub packet_tx: mpsc::UnboundedSender<ServerboundGamePacket>,
}

pub fn spawn_connection(
    rt: &tokio::runtime::Runtime,
    args: ConnectArgs,
) -> ConnectionHandle {
    let (event_tx, event_rx) = crossbeam_channel::bounded(256);
    let (chat_tx, chat_rx) = crossbeam_channel::bounded::<String>(64);
    let (packet_tx, packet_rx) = mpsc::unbounded_channel::<ServerboundGamePacket>();
    let game_packet_tx = packet_tx.clone();
    rt.spawn(async move {
        if let Err(e) = connect_to_server(args, event_tx.clone(), chat_rx, game_packet_tx, packet_rx).await {
            log::error!("Network error: {e}");
            let reason = if is_connection_closed(&e) {
                "Server closed".to_string()
            } else {
                e.to_string()
            };
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
    let addr = resolve_address(&args.server)?;
    log::info!("Connecting to {addr}...");

    let mut conn: Connection<_, _> = Connection::new(&addr).await?;

    conn.write(ServerboundIntention {
        protocol_version: PROTOCOL_VERSION,
        hostname: args.server.clone(),
        port: addr.port(),
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
    config_sequence(&mut conn).await?;

    let conn = conn.game();
    log::info!("Entering game state");
    let _ = event_tx.try_send(NetworkEvent::Connected);

    game_loop(conn, &event_tx, chat_rx, game_packet_tx, game_packet_rx).await
}

async fn login_sequence(
    conn: &mut Connection<ClientboundLoginPacket, ServerboundLoginPacket>,
    args: &ConnectArgs,
) -> Result<(), ConnectionError> {
    loop {
        let packet: ClientboundLoginPacket = conn.read().await?;
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

        conn.authenticate(access_token, &args.uuid, e.secret_key, hello, None)
            .await
            .map_err(|e: azalea_auth::sessionserver::ClientSessionServerError| {
                ConnectionError::Auth(e.to_string())
            })?;
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
) -> Result<(), ConnectionError> {
    use azalea_entity::HumanoidArm;
    use azalea_protocol::common::client_information::*;
    use azalea_protocol::packets::config::*;

    conn.write(ServerboundConfigPacket::ClientInformation(
        s_client_information::ServerboundClientInformation {
            information: ClientInformation {
                language: "en_us".into(),
                view_distance: 8,
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
            ClientboundConfigPacket::RegistryData(_) => {
                log::debug!("Received registry data");
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
                return Ok(());
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
                // TODO: implement chat signing — requires enforce-secure-profile=false for now
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
        let packet: ClientboundGamePacket = reader.read().await?;
        handle_game_packet(&packet, &sender, event_tx);
    }
}

fn resolve_address(server: &str) -> Result<SocketAddr, ConnectionError> {
    use std::net::ToSocketAddrs;

    let addr = if server.contains(':') {
        server.to_string()
    } else {
        format!("{server}:25565")
    };

    addr.to_socket_addrs()
        .map_err(|e| ConnectionError::InvalidAddress(format!("{addr}: {e}")))?
        .next()
        .ok_or_else(|| ConnectionError::InvalidAddress(format!("{addr}: no addresses found")))
}

fn is_connection_closed(err: &ConnectionError) -> bool {
    let msg = err.to_string();
    msg.contains("Connection closed")
        || msg.contains("connection reset")
        || msg.contains("broken pipe")
}
