use azalea_protocol::address::ServerAddr;
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
use super::conn::{Conn, MemoryEnd, RawWriter};
use super::handler::{handle_game_packet, handle_raw_game_packet};
use super::sender::{Outbound, PacketSender};

#[derive(Error, Debug)]
pub enum ConnectionError {
    #[error("invalid server address: {0}")]
    InvalidAddress(String),

    #[error("connection failed: {0}")]
    Connect(std::io::Error),

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

    #[error("joining {0} servers is not supported yet")]
    Unjoinable(String),
}

impl From<super::resolve::ConnectError> for ConnectionError {
    fn from(e: super::resolve::ConnectError) -> Self {
        use super::resolve::ConnectError;
        match e {
            ConnectError::Resolve(e) => Self::InvalidAddress(e.to_string()),
            ConnectError::Io(e) => Self::Connect(e),
        }
    }
}

pub enum Transport {
    Remote {
        server: String,
        /// The server's protocol from an earlier server-list ping, when joining
        /// from the list; saves `negotiate_wire_version` its status probe.
        protocol: Option<i32>,
    },
    /// An integrated server in this process, reached over an in-memory pipe.
    #[allow(
        dead_code,
        reason = "the integrated server constructs these once it lands"
    )]
    Memory(MemoryEnd),
}

pub struct ConnectArgs {
    pub transport: Transport,
    pub username: String,
    pub uuid: uuid::Uuid,
    pub access_token: Option<String>,
    pub view_distance: u8,
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
    mut game_packet_rx: mpsc::UnboundedReceiver<Outbound>,
) -> Result<(), ConnectionError> {
    let ConnectArgs {
        transport,
        username,
        uuid,
        access_token,
        view_distance,
    } = args;

    let mut conn = match transport {
        Transport::Remote { server, protocol } => {
            let server_addr: ServerAddr = server
                .as_str()
                .try_into()
                .map_err(|_| ConnectionError::InvalidAddress(server.clone()))?;
            negotiate_wire_version(&server_addr, protocol).await?;
            super::resolve::connect(&server_addr, ClientIntention::Login).await?
        }
        Transport::Memory(end) => open_integrated(end).await?,
    };

    let hello = ServerboundLoginPacket::Hello(ServerboundHello {
        name: username.clone(),
        profile_id: uuid,
    });
    let frame = serialize_frame(&hello)?;
    let frame = match super::translate::active() {
        Some(t) => t.translate_outbound_login_frame(frame),
        None => frame,
    };
    conn.writer.write(&frame).await?;

    tracing::info!("Sent login hello as {username} ({uuid})");
    if access_token.is_none() {
        tracing::warn!(
            "Connecting offline (no access token). The server keys op/permissions to the \
             authenticated account, so op-only commands like /time may return \"Unknown command\" \
             under this offline identity."
        );
    }

    login_sequence(&mut conn, &uuid, access_token.as_deref()).await?;

    // 1.20.1 and older have no configuration phase: the server enters play as
    // soon as it has sent the profile, and the registries ride in the game
    // login packet rather than in registry_data packets.
    let no_config = super::translate::active().is_some_and(|t| t.no_config_phase());
    if !no_config {
        conn.write_packet(&ServerboundLoginPacket::LoginAcknowledged(
            ServerboundLoginAcknowledged {},
        ))
        .await?;
    }

    let joined = if no_config {
        tracing::info!("Skipping configuration phase");
        read_inline_registries(&mut conn).await?
    } else {
        tracing::info!("Entering configuration phase");
        Joined {
            registries: config_sequence(
                &mut conn,
                view_distance,
                &event_tx,
                &mut game_packet_rx,
                None,
            )
            .await?,
            deferred_login: None,
        }
    };

    tracing::info!("Entering game state");
    let biome_colors = extract_biome_climate(&joined.registries);
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
        joined,
        view_distance,
    )
    .await
}

/// Opens the connection to an integrated server. It speaks the latest protocol
/// by definition, so there is nothing to probe and translation stays inert for
/// the session.
async fn open_integrated(end: MemoryEnd) -> Result<Conn, ConnectionError> {
    adopt_wire_protocol(pomme_protocol::version::LATEST.protocol);

    let mut conn = Conn::from_memory(end);
    super::resolve::send_intention(&mut conn, "localhost", 0, ClientIntention::Login).await?;
    Ok(conn)
}

/// What the phases before the game loop produced.
struct Joined {
    registries: std::sync::Arc<azalea_core::registry_holder::RegistryHolder>,
    /// The game `login` frame, when it was already read off the wire: a server
    /// with no configuration phase sends it before the game loop starts, which
    /// then replays it as its first packet.
    deferred_login: Option<Box<[u8]>>,
}

/// Reads the registries a pre-configuration-phase server ships inside its game
/// `login` packet, returning them with the untranslated login frame for the
/// game loop to replay. That frame is the first the server sends after the
/// profile (`PlayerList.placeNewPlayer`), with no acknowledgement in between.
async fn read_inline_registries(conn: &mut Conn) -> Result<Joined, ConnectionError> {
    use azalea_core::registry_holder::RegistryHolder;

    let login = conn.reader.read().await?;
    let translation = super::translate::active().expect("translation for a config-less version");
    let Some(frames) = translation.split_login_registries(&login) else {
        // A server that turns the join away here does it with a play-phase
        // disconnect, the login phase having already ended.
        if let Some(raw) = translation.translate_game_frame(login)
            && let Ok(ClientboundGamePacket::Disconnect(p)) =
                deserialize_packet::<ClientboundGamePacket>(&mut std::io::Cursor::new(&raw))
        {
            return Err(ConnectionError::Disconnected(format!("{}", p.reason)));
        }
        return Err(ConnectionError::Disconnected(
            "could not read the registries from the login packet".into(),
        ));
    };

    let mut registry_holder = RegistryHolder::default();
    for frame in frames {
        match deserialize_packet::<ClientboundConfigPacket>(&mut std::io::Cursor::new(&frame)) {
            Ok(ClientboundConfigPacket::RegistryData(p)) => {
                registry_holder.append(p.registry_id, p.entries);
            }
            Ok(_) => {}
            Err(e) => skip_malformed_packet(e)?,
        }
    }
    Ok(Joined {
        registries: std::sync::Arc::new(registry_holder),
        deferred_login: Some(login),
    })
}

/// Adopts the server's protocol as the wire version when translation data
/// for it exists, so one client joins any supported server version;
/// otherwise the launched version is kept (and the server shows its own
/// mismatch message, as before). A launched version without translation
/// data (listed for pings, not yet joinable) can never complete a join —
/// the handshake either gets the server's mismatch rejection or succeeds
/// and breaks mid-connect on untranslated packets — so it is refused up
/// front on every path, a failed probe or a stale server-list `known`
/// protocol included. The protocol comes from `known` (a server-list ping)
/// or a status probe. Sets the session protocol and the matching
/// block-state table, so it must run before the login handshake and before
/// any world state loads.
async fn negotiate_wire_version(
    server_addr: &ServerAddr,
    known: Option<i32>,
) -> Result<(), ConnectionError> {
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
    let wire = resolve_wire(probed, selected).map_err(|p| {
        let name = pomme_protocol::ProtocolVersion::from_protocol(p)
            .map(|v| v.name.to_string())
            .unwrap_or_else(|| format!("protocol {p}"));
        ConnectionError::Unjoinable(name)
    })?;
    tracing::info!("Negotiated wire protocol {wire}");
    adopt_wire_protocol(wire);
    Ok(())
}

/// Speaks `wire` for the rest of the session. The translation layer and the
/// block-state tables both key off it, so they always move together.
fn adopt_wire_protocol(wire: i32) {
    crate::version::set_session_protocol(wire);
    crate::world::block::set_active_protocol(wire);
}

/// The wire protocol to speak given the probed server protocol and the
/// launched (`selected`) one; `Err` carries an unjoinable outcome (see
/// [`negotiate_wire_version`]). Inert while `selected` is joinable — every
/// arm then yields a joinable wire.
fn resolve_wire(probed: Option<i32>, selected: i32) -> Result<i32, i32> {
    let wire = match probed {
        Some(p) if super::translate::joinable(p) => p,
        Some(p) => {
            tracing::warn!("Server speaks unsupported protocol {p}; falling back to {selected}");
            selected
        }
        None => {
            tracing::warn!("Server protocol probe failed; falling back to {selected}");
            selected
        }
    };
    if super::translate::joinable(wire) {
        Ok(wire)
    } else {
        Err(wire)
    }
}

async fn login_sequence(
    conn: &mut Conn,
    uuid: &uuid::Uuid,
    access_token: Option<&str>,
) -> Result<(), ConnectionError> {
    loop {
        // Read the raw frame ourselves so older-version layouts can be
        // rewritten before the typed decode (26.1's login_finished lacks the
        // trailing session id).
        let raw = conn.reader.read().await?;
        let raw = match super::translate::active() {
            Some(t) => t.translate_login_frame(raw),
            None => raw,
        };
        let packet: ClientboundLoginPacket = deserialize_packet(&mut std::io::Cursor::new(&raw))?;
        tracing::info!("Login packet: {:?}", std::mem::discriminant(&packet));
        match packet {
            ClientboundLoginPacket::Hello(p) => {
                handle_encryption(conn, &p, uuid, access_token).await?;
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
                conn.write_packet(&ServerboundLoginPacket::CookieResponse(
                    azalea_protocol::packets::login::s_cookie_response::ServerboundCookieResponse {
                        key: p.key,
                        payload: None,
                    },
                ))
                .await?;
            }
            _ => {
                tracing::debug!("Login packet: {:?}", std::mem::discriminant(&packet));
            }
        }
    }
}

async fn handle_encryption(
    conn: &mut Conn,
    hello: &ClientboundHello,
    uuid: &uuid::Uuid,
    access_token: Option<&str>,
) -> Result<(), ConnectionError> {
    let e = azalea_crypto::encrypt(&hello.public_key, &hello.challenge)
        .map_err(ConnectionError::Encryption)?;

    if hello.should_authenticate {
        let access_token = access_token.ok_or_else(|| {
            ConnectionError::Auth(
                "server requires authentication but no access token provided".into(),
            )
        })?;

        tracing::info!("Authenticating with session server (uuid: {uuid})");
        join_session_server(access_token, uuid, e.secret_key, hello)
            .await
            .map_err(|e| ConnectionError::Auth(e.to_string()))?;
        tracing::info!("Session server authentication successful");
    } else {
        tracing::info!("Server does not require authentication");
    }

    conn.write_packet(&ServerboundLoginPacket::Key(ServerboundKey {
        key_bytes: e.encrypted_public_key,
        encrypted_challenge: e.encrypted_challenge,
    }))
    .await?;

    conn.set_encryption_key(e.secret_key);
    tracing::info!("Encryption enabled");
    Ok(())
}

/// Proves ownership of the account to Mojang so the server can verify the join.
/// Not a method on [`Conn`]: it is an HTTPS call and touches no connection
/// state.
async fn join_session_server(
    access_token: &str,
    uuid: &uuid::Uuid,
    private_key: [u8; 16],
    hello: &ClientboundHello,
) -> Result<(), azalea_auth::sessionserver::ClientSessionServerError> {
    azalea_auth::sessionserver::join(azalea_auth::sessionserver::SessionServerJoinOpts {
        access_token,
        public_key: &hello.public_key,
        private_key: &private_key,
        uuid,
        server_id: &hello.server_id,
        proxy: None,
    })
    .await
}

async fn config_sequence(
    conn: &mut Conn,
    view_distance: u8,
    event_tx: &Sender<NetworkEvent>,
    outbound_rx: &mut mpsc::UnboundedReceiver<Outbound>,
    // `Some` on a mid-session reconfiguration: the previous registries,
    // kept when the server re-sends nothing (vanilla's RegistryDataCollector
    // returns the original registries unchanged in that case).
    previous_registries: Option<&std::sync::Arc<azalea_core::registry_holder::RegistryHolder>>,
) -> Result<std::sync::Arc<azalea_core::registry_holder::RegistryHolder>, ConnectionError> {
    use azalea_core::registry_holder::RegistryHolder;
    use azalea_protocol::packets::config::*;

    let mut registry_holder = RegistryHolder::default();
    let mut received_registry_data = false;

    // Vanilla sends brand and client information once, from the login
    // listener; a reconfiguration sends neither.
    if previous_registries.is_none() {
        // Some servers key off the brand.
        write_config_packet(
            conn,
            ServerboundConfigPacket::CustomPayload(s_custom_payload::ServerboundCustomPayload {
                identifier: "minecraft:brand".into(),
                data: super::brand_payload().into(),
            }),
        )
        .await?;

        write_config_packet(
            conn,
            ServerboundConfigPacket::ClientInformation(
                s_client_information::ServerboundClientInformation {
                    information: super::client_information(view_distance),
                },
            ),
        )
        .await?;
    }

    // Config frames are read raw so older wire versions translate (765's
    // registry_data fans out into several frames, hence the queue).
    let mut pending = std::collections::VecDeque::new();
    loop {
        let packet = if let Some(packet) = pending.pop_front() {
            packet
        } else {
            tokio::select! {
                raw = conn.reader.read() => {
                    let raw = match raw {
                        Ok(raw) => raw,
                        Err(e) => {
                            skip_malformed_packet(e)?;
                            continue;
                        }
                    };
                    let frames = match super::translate::active() {
                        Some(t) => t.translate_config_frame(raw),
                        None => vec![raw],
                    };
                    for frame in frames {
                        match deserialize_packet::<ClientboundConfigPacket>(
                            &mut std::io::Cursor::new(&frame),
                        ) {
                            Ok(packet) => pending.push_back(packet),
                            Err(e) => skip_malformed_packet(e)?,
                        }
                    }
                    continue;
                }
                // `Some(..)` disables the branch when the channel closes instead
                // of busy-looping on a closed receiver.
                Some(outbound) = outbound_rx.recv() => {
                    if let Outbound::Packet(packet) = outbound
                        && let ServerboundGamePacket::ResourcePack(p) = *packet
                    {
                        use azalea_protocol::packets::config::s_resource_pack as config_pack;
                        use azalea_protocol::packets::game::s_resource_pack as game_pack;
                        let action = match p.action {
                            game_pack::Action::SuccessfullyLoaded => config_pack::Action::SuccessfullyLoaded,
                            game_pack::Action::Declined => config_pack::Action::Declined,
                            game_pack::Action::FailedDownload => config_pack::Action::FailedDownload,
                            game_pack::Action::Accepted => config_pack::Action::Accepted,
                            game_pack::Action::InvalidUrl => config_pack::Action::InvalidUrl,
                            game_pack::Action::FailedReload => config_pack::Action::FailedReload,
                            game_pack::Action::Discarded => config_pack::Action::Discarded,
                        };
                        write_config_packet(conn, ServerboundConfigPacket::ResourcePack(
                            config_pack::ServerboundResourcePack { id: p.id, action },
                        )).await?;
                    }
                    // Anything else is discarded: vanilla defers its outbound
                    // queue, but pomme's game keeps ticking through a
                    // reconfiguration, so stale movement/actions are best dropped.
                    continue;
                }
            }
        };
        match packet {
            ClientboundConfigPacket::RegistryData(p) => {
                received_registry_data = true;
                registry_holder.append(p.registry_id, p.entries);
            }
            ClientboundConfigPacket::UpdateTags(_) => {
                tracing::debug!("Received tags");
            }
            ClientboundConfigPacket::SelectKnownPacks(_) => {
                // Claiming no known packs forces the server to send NBT for
                // every registry entry; `variant_index` (handler.rs) relies on
                // that to equate registry-map position with protocol id.
                write_config_packet(
                    conn,
                    ServerboundConfigPacket::SelectKnownPacks(
                        s_select_known_packs::ServerboundSelectKnownPacks {
                            known_packs: vec![],
                        },
                    ),
                )
                .await?;
            }
            ClientboundConfigPacket::KeepAlive(p) => {
                write_config_packet(
                    conn,
                    ServerboundConfigPacket::KeepAlive(s_keep_alive::ServerboundKeepAlive {
                        id: p.id,
                    }),
                )
                .await?;
            }
            ClientboundConfigPacket::FinishConfiguration(_) => {
                write_config_packet(
                    conn,
                    ServerboundConfigPacket::FinishConfiguration(
                        s_finish_configuration::ServerboundFinishConfiguration {},
                    ),
                )
                .await?;
                return Ok(match previous_registries {
                    Some(previous) if !received_registry_data => previous.clone(),
                    _ => std::sync::Arc::new(registry_holder),
                });
            }
            ClientboundConfigPacket::Disconnect(p) => {
                return Err(ConnectionError::Disconnected(format!("{}", p.reason)));
            }
            ClientboundConfigPacket::CookieRequest(p) => {
                write_config_packet(
                    conn,
                    ServerboundConfigPacket::CookieResponse(
                        s_cookie_response::ServerboundCookieResponse {
                            key: p.key,
                            payload: None,
                        },
                    ),
                )
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
                write_config_packet(
                    conn,
                    ServerboundConfigPacket::ResourcePack(
                        s_resource_pack::ServerboundResourcePack {
                            id: p.id,
                            action: s_resource_pack::Action::Accepted,
                        },
                    ),
                )
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
    mut conn: Conn,
    event_tx: &Sender<NetworkEvent>,
    chat_rx: crossbeam_channel::Receiver<String>,
    outbound_tx: mpsc::UnboundedSender<Outbound>,
    mut outbound_rx: mpsc::UnboundedReceiver<Outbound>,
    joined: Joined,
    view_distance: u8,
) -> Result<(), ConnectionError> {
    let Joined {
        registries: mut registry_holder,
        mut deferred_login,
    } = joined;
    let sender = PacketSender::new(outbound_tx.clone());

    let shared_tree: crate::net::commands::SharedCommandTree =
        std::sync::Arc::new(parking_lot::Mutex::new(None));

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
    let _ = event_tx.try_send(NetworkEvent::Registries(registry_holder.clone()));

    let translation = super::translate::active();
    if deferred_login.is_some() {
        // 1.20.1 sends both from its login handler, where later versions send
        // them in the configuration phase (ClientPacketListener.handleLogin).
        let info = ServerboundGamePacket::ClientInformation(
            azalea_protocol::packets::game::s_client_information::ServerboundClientInformation {
                client_information: super::client_information(view_distance),
            },
        );
        write_game_frame(&mut conn.writer, translation, serialize_frame(&info)?).await?;

        let brand = ServerboundGamePacket::CustomPayload(
            azalea_protocol::packets::game::s_custom_payload::ServerboundCustomPayload {
                identifier: "minecraft:brand".into(),
                data: super::brand_payload().into(),
            },
        );
        write_game_frame(&mut conn.writer, translation, serialize_frame(&brand)?).await?;
    }
    loop {
        let raw = if let Some(raw) = deferred_login.take() {
            Ok(raw)
        } else {
            // TODO: reads and writes share this task, so a blocked write stops
            // the client reading. Harmless against a socket, but an integrated
            // server on a bounded pipe can deadlock; split the writer out.
            tokio::select! {
                Some(out) = outbound_rx.recv() => {
                    let frame = match out {
                        Outbound::Packet(mut packet) => {
                            if let Some(t) = translation {
                                t.remap_outbound(&mut packet);
                            }
                            serialize_frame(&*packet)?
                        }
                        Outbound::Raw(bytes) => bytes,
                    };
                    write_game_frame(&mut conn.writer, translation, frame).await?;
                    continue;
                }
                raw = conn.reader.read() => raw,
            }
        };
        let raw = match raw {
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
                if matches!(packet, ClientboundGamePacket::StartConfiguration(_)) {
                    // Vanilla clears the client level before acknowledging
                    // (ClientPacketListener.handleConfigurationStart); chat
                    // survives the transition.
                    let _ = event_tx.try_send(NetworkEvent::Reconfiguring);
                    let ack = ServerboundGamePacket::ConfigurationAcknowledged(
                        azalea_protocol::packets::game::s_configuration_acknowledged::ServerboundConfigurationAcknowledged,
                    );
                    write_game_frame(&mut conn.writer, translation, serialize_frame(&ack)?).await?;
                    let holder = config_sequence(
                        &mut conn,
                        view_distance,
                        event_tx,
                        &mut outbound_rx,
                        Some(&registry_holder),
                    )
                    .await?;
                    if !std::sync::Arc::ptr_eq(&holder, &registry_holder) {
                        registry_holder = holder;
                        let _ =
                            event_tx.try_send(NetworkEvent::Registries(registry_holder.clone()));
                        let _ = event_tx.try_send(NetworkEvent::BiomeColors {
                            colors: extract_biome_climate(&registry_holder),
                        });
                    }
                    continue;
                }
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

fn serialize_frame<P: azalea_protocol::packets::ProtocolPacket + std::fmt::Debug>(
    packet: &P,
) -> Result<Vec<u8>, ConnectionError> {
    azalea_protocol::write::serialize_packet(packet)
        .map(Vec::from)
        .map_err(|e| ConnectionError::Write(std::io::Error::other(e)))
}

/// Writes one latest-layout frame, translating it for older wire versions.
async fn write_game_frame(
    writer: &mut RawWriter,
    translation: Option<&super::translate::Translation>,
    frame: Vec<u8>,
) -> Result<(), ConnectionError> {
    let frames = match translation {
        Some(t) if t.translates_outbound() => t.translate_outbound_game_frame(frame),
        _ => vec![frame],
    };
    for frame in frames {
        writer.write(&frame).await?;
    }
    Ok(())
}

/// Writes one latest-layout configuration packet, translating it for older
/// wire versions (765 down: id remap plus suppression of packets the wire
/// version lacks).
async fn write_config_packet(
    conn: &mut Conn,
    packet: ServerboundConfigPacket,
) -> Result<(), ConnectionError> {
    let Some(t) = super::translate::active().filter(|t| t.translates_config()) else {
        return Ok(conn.write_packet(&packet).await?);
    };
    if let Some(frame) = t.translate_outbound_config_frame(serialize_frame(&packet)?) {
        conn.writer.write(&frame).await?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use pomme_protocol::version::LATEST;

    use super::*;

    /// 762 (1.19.4) is not a supported version at all, so it never gains a
    /// wire translation; 775 has one.
    #[test]
    fn resolve_wire_gates_unjoinable_versions() {
        let latest = LATEST.protocol;
        assert_eq!(resolve_wire(Some(775), latest), Ok(775));
        assert_eq!(resolve_wire(Some(762), latest), Ok(latest));
        assert_eq!(resolve_wire(None, latest), Ok(latest));
        // An untranslated launched version is refused whatever the probe
        // yielded, unless the server itself speaks a joinable protocol.
        assert_eq!(resolve_wire(None, 762), Err(762));
        assert_eq!(resolve_wire(Some(762), 762), Err(762));
        assert_eq!(resolve_wire(Some(latest), 762), Ok(latest));
        // A staged version (tables embedded, not yet in TRANSLATED) is
        // refused when launched and adopted around when the server is
        // joinable.
        for v in pomme_protocol::version::VERSIONS {
            if pomme_protocol::PacketTable::for_protocol(v.protocol).is_some()
                && !crate::net::translate::joinable(v.protocol)
            {
                assert_eq!(
                    resolve_wire(Some(v.protocol), latest),
                    Ok(latest),
                    "{}",
                    v.name
                );
                assert_eq!(
                    resolve_wire(None, v.protocol),
                    Err(v.protocol),
                    "{}",
                    v.name
                );
            }
        }
    }

    /// The whole join over an in-memory pipe, against a peer that sends only
    /// the two frames the client actually requires: login finished, then
    /// finish configuration. No registry data, no compression, no encryption.
    #[tokio::test]
    async fn joins_an_integrated_server_over_the_pipe() {
        use azalea_auth::game_profile::GameProfile;
        use azalea_protocol::packets::config::c_finish_configuration::ClientboundFinishConfiguration;
        use azalea_protocol::packets::config::{ClientboundConfigPacket, ServerboundConfigPacket};
        use azalea_protocol::packets::handshake::ServerboundHandshakePacket;
        use azalea_protocol::packets::login::ClientboundLoginPacket;
        use azalea_protocol::packets::login::c_login_finished::ClientboundLoginFinished;
        use uuid::Uuid;

        use crate::net::conn::{Conn, memory_pipes};

        /// The next packet the client sent, in whichever phase the caller
        /// names.
        async fn sent<P: azalea_protocol::packets::ProtocolPacket + std::fmt::Debug>(
            peer: &mut Conn,
        ) -> P {
            peer.read_packet().await.unwrap()
        }

        let (client_end, server_end) = memory_pipes();
        let mut peer = Conn::from_memory(server_end);

        let (event_tx, event_rx) = crossbeam_channel::bounded(4096);
        let (_chat_tx, chat_rx) = crossbeam_channel::bounded(64);
        let (packet_tx, packet_rx) = mpsc::unbounded_channel();

        let client = tokio::spawn(connect_to_server(
            ConnectArgs {
                transport: Transport::Memory(client_end),
                username: "Steve".to_owned(),
                uuid: Uuid::nil(),
                access_token: None,
                view_distance: 8,
            },
            event_tx,
            chat_rx,
            packet_tx,
            packet_rx,
        ));

        assert!(matches!(
            sent(&mut peer).await,
            ServerboundHandshakePacket::Intention(p) if p.protocol_version == LATEST.protocol
        ));
        assert!(matches!(
            sent(&mut peer).await,
            ServerboundLoginPacket::Hello(p) if p.name == "Steve"
        ));

        peer.write_packet(&ClientboundLoginPacket::LoginFinished(
            ClientboundLoginFinished {
                game_profile: GameProfile::new(Uuid::nil(), "Steve".to_owned()),
                session_id: Uuid::nil(),
            },
        ))
        .await
        .unwrap();

        assert!(matches!(
            sent(&mut peer).await,
            ServerboundLoginPacket::LoginAcknowledged(_)
        ));
        assert!(matches!(
            sent(&mut peer).await,
            ServerboundConfigPacket::CustomPayload(p) if p.identifier.to_string() == "minecraft:brand"
        ));
        assert!(matches!(
            sent(&mut peer).await,
            ServerboundConfigPacket::ClientInformation(p) if p.information.view_distance == 8
        ));

        peer.write_packet(&ClientboundConfigPacket::FinishConfiguration(
            ClientboundFinishConfiguration,
        ))
        .await
        .unwrap();

        assert!(matches!(
            sent(&mut peer).await,
            ServerboundConfigPacket::FinishConfiguration(_)
        ));

        // Emitted as soon as configuration ends, before any game packet.
        let events: Vec<_> = std::iter::from_fn(|| event_rx.recv().ok())
            .take(2)
            .collect();
        assert!(matches!(events[0], NetworkEvent::BiomeColors { .. }));
        assert!(matches!(events[1], NetworkEvent::Connected));

        client.abort();
    }
}
