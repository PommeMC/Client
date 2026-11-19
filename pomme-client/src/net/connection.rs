use crossbeam_channel::Sender;
use pomme_protocol::configuration::{ConfigPacket, ServerboundConfig};
use pomme_protocol::lifecycle::LifecyclePacket;
use pomme_protocol::login::{self, LoginPacket, ServerboundLogin};
use pomme_protocol::serverbound::GamePacket;
use pomme_protocol::status::ClientIntent;
use pomme_protocol::tags::Tags;
use pomme_protocol::{Direction, PacketTable, Phase};
use thiserror::Error;
use tokio::sync::mpsc;

use super::NetworkEvent;
use super::address::ServerAddr;
use super::chat::ChatPacketError;
use super::chat_security::{ChatSender, ProfileKeyPair};
use super::config_wire::{ConfigFrame, Registries};
use super::conn::{Conn, MemoryEnd, RawWriter};
use super::handler::{handle_raw_game_packet, send_ordered};
use super::sender::{Outbound, PacketSender};
use crate::chat_component::ComponentText;
use crate::ui::server_dialog::DialogRegistry;
use crate::world::block::BlockStateTable;

#[derive(Error, Debug)]
pub enum ConnectionError {
    #[error("invalid server address: {0}")]
    InvalidAddress(String),

    #[error("connection failed: {0}")]
    Connect(std::io::Error),

    #[error("packet read error: {0}")]
    Read(#[from] super::frame::FrameError),

    #[error("packet read error: {0}")]
    Decode(#[from] pomme_protocol::codec::DecodeError),

    #[error("packet write error: {0}")]
    Write(#[from] std::io::Error),

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("disconnected by server: {0}")]
    Disconnected(String),

    #[error("encryption failed: {0}")]
    Encryption(String),

    /// A client-side disconnect with a vanilla translation key.
    #[error("{}", crate::lang::translate(.0).unwrap_or(.0))]
    ClientDisconnect(&'static str),

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
    Memory {
        end: MemoryEnd,
        /// The protocol that server speaks.
        protocol: i32,
    },
}

pub struct ConnectArgs {
    pub transport: Transport,
    pub username: String,
    pub uuid: uuid::Uuid,
    pub access_token: Option<String>,
    pub view_distance: u8,
    pub chat_options: crate::ui::chat::ChatOptions,
    pub skin_options: crate::ui::menu::SkinOptions,
}

pub struct ConnectionHandle {
    pub event_rx: crossbeam_channel::Receiver<NetworkEvent>,
    pub packet_tx: PacketSender,
    pub task: tokio::task::JoinHandle<()>,
}

impl Drop for ConnectionHandle {
    fn drop(&mut self) {
        self.task.abort();
        // The session is over: restore the launched version's wire protocol.
        crate::version::clear_session_protocol();
    }
}

pub fn spawn_connection(rt: &tokio::runtime::Runtime, args: ConnectArgs) -> ConnectionHandle {
    // Chunk events carry their section and light bytes (up to ~150 KB each),
    // so the queue stays short; a full queue blocks the network task until the
    // game thread drains it, which it does every frame.
    let (event_tx, event_rx) = crossbeam_channel::bounded(256);
    let (packet_tx, packet_rx) = mpsc::unbounded_channel::<Outbound>();
    let game_packet_tx = packet_tx.clone();
    let packet_tx = PacketSender::new(packet_tx);
    let task = rt.spawn(async move {
        if let Err(e) = connect_to_server(args, event_tx.clone(), game_packet_tx, packet_rx).await {
            tracing::error!("Network error: {e}");
            let reason = friendly_error_reason(&e);
            send_ordered(&event_tx, NetworkEvent::Disconnected { reason });
        }
    });
    ConnectionHandle {
        event_rx,
        packet_tx,
        task,
    }
}

pub async fn connect_to_server(
    args: ConnectArgs,
    event_tx: Sender<NetworkEvent>,
    game_packet_tx: mpsc::UnboundedSender<Outbound>,
    mut game_packet_rx: mpsc::UnboundedReceiver<Outbound>,
) -> Result<(), ConnectionError> {
    let ConnectArgs {
        transport,
        username,
        uuid,
        access_token,
        view_distance,
        chat_options,
        skin_options,
    } = args;

    let (mut conn, wire) = match transport {
        Transport::Remote { server, protocol } => {
            let server_addr = ServerAddr::parse(&server)
                .ok_or_else(|| ConnectionError::InvalidAddress(server.clone()))?;
            let wire = negotiate_wire_version(&server_addr, protocol).await?;
            select_block_protocol(&event_tx, wire);
            (
                super::resolve::connect(&server_addr, ClientIntent::Login).await?,
                wire,
            )
        }
        Transport::Memory { end, protocol } => {
            // The integrated server's protocol is known, so there is nothing
            // to probe; one other than native is translated like a remote
            // server's.
            adopt_wire_protocol(protocol);
            select_block_protocol(&event_tx, protocol);
            let mut conn = Conn::from_memory(end);
            super::resolve::send_intention(&mut conn, "localhost", 0, ClientIntent::Login).await?;
            (conn, protocol)
        }
    };

    conn.translation =
        super::translate::TranslationSession::for_protocol(wire).map(std::sync::Arc::new);

    let frame = ServerboundLogin::Hello {
        name: username.clone(),
        profile_id: uuid.as_u128(),
    }
    .encode();
    let frame = match conn.translation.as_deref() {
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

    let profile_id = login_sequence(&mut conn, &uuid, access_token.as_deref()).await?;

    // 1.20.1 and older have no configuration phase: the server enters play as
    // soon as it has sent the profile, and the registries ride in the game
    // login packet rather than in registry_data packets.
    let no_config = conn
        .translation
        .as_deref()
        .is_some_and(|t| t.no_config_phase());
    if !no_config {
        send_login(&mut conn, ServerboundLogin::LoginAcknowledged).await?;
    }
    send_ordered(&event_tx, NetworkEvent::LoggedIn);

    let joined = if no_config {
        tracing::info!("Skipping configuration phase");
        read_inline_registries(&mut conn).await?
    } else {
        tracing::info!("Entering configuration phase");
        Joined {
            configured: config_sequence(
                &mut conn,
                view_distance,
                chat_options,
                skin_options,
                &event_tx,
                &mut game_packet_rx,
                None,
            )
            .await?,
            deferred_login: None,
        }
    };

    tracing::info!("Entering game state");
    let (key_pair_tx, key_pair_rx) = mpsc::unbounded_channel();
    let biome_colors = extract_biome_climate(&joined.configured.registries);
    send_ordered(
        &event_tx,
        NetworkEvent::BiomeColors {
            colors: biome_colors,
        },
    );
    send_ordered(&event_tx, NetworkEvent::Connected);

    game_loop(
        conn,
        &event_tx,
        GameLoopArgs {
            block_state_table: BlockStateTable::for_protocol(wire),
            outbound_tx: game_packet_tx,
            outbound_rx: game_packet_rx,
            joined,
            view_distance,
            chat_options,
            skin_options,
            chat: ChatSender::new(profile_id, uuid, access_token, key_pair_tx),
            key_pair_rx,
        },
    )
    .await
}

/// What the phases before the game loop produced.
struct Joined {
    configured: Configured,
    /// The game `login` frame, when it was already read off the wire: a server
    /// with no configuration phase sends it before the game loop starts, which
    /// then replays it as its first packet.
    deferred_login: Option<Box<[u8]>>,
}

/// What a configuration phase leaves the session with.
struct Configured {
    registries: std::sync::Arc<azalea_core::registry_holder::RegistryHolder>,
    dialogs: std::sync::Arc<DialogRegistry>,
}

/// Reads the registries a pre-configuration-phase server ships inside its game
/// `login` packet, returning them with the untranslated login frame for the
/// game loop to replay. That frame is the first the server sends after the
/// profile (`PlayerList.placeNewPlayer`), with no acknowledgement in between.
async fn read_inline_registries(conn: &mut Conn) -> Result<Joined, ConnectionError> {
    let login = conn.reader.read().await?;
    super::capture::record(Phase::Game, &login);
    let translation = conn
        .translation
        .as_deref()
        .expect("translation for a config-less version");
    let Some(frames) = translation.split_login_registries(&login) else {
        // A server that turns the join away here does it with a play-phase
        // disconnect, the login phase having already ended.
        if let Some(raw) = translation.translate_game_frame(login)
            && let Some(reason) = super::lifecycle_wire::read_disconnect(&raw)
        {
            return Err(ConnectionError::Disconnected(reason));
        }
        return Err(ConnectionError::Disconnected(
            "could not read the registries from the login packet".into(),
        ));
    };

    let mut registries = Registries::default();
    for frame in frames {
        if let Some(ConfigFrame::Config(ConfigPacket::RegistryData(data))) =
            super::config_wire::read_or_skip(&frame)
        {
            registries
                .append_or_skip(crate::version::session_protocol(), data)
                .map_err(ConnectionError::Disconnected)?;
        }
    }
    Ok(Joined {
        configured: Configured {
            registries: std::sync::Arc::new(registries.holder),
            dialogs: Default::default(),
        },
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
) -> Result<i32, ConnectionError> {
    let selected = crate::version::selected_protocol();
    let probed = match known {
        Some(p) => Some(p),
        None => {
            let probe = async {
                let (status, _) = super::resolve::request_status(server_addr).await.ok()?;
                Some(status.version?.protocol)
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
    Ok(wire)
}

/// Publish the adopted wire identity before either transport starts login.
fn select_block_protocol(event_tx: &Sender<NetworkEvent>, protocol: i32) {
    // Builds the table here, so the game thread only switches to it.
    let _ = crate::world::block::BlockStateTable::for_protocol(protocol);
    send_ordered(event_tx, NetworkEvent::ProtocolSelected { protocol });
}

/// Speaks `wire` for the rest of the session through the translation layer.
/// Block metadata is selected separately with the same negotiated protocol.
fn adopt_wire_protocol(wire: i32) {
    crate::version::set_session_protocol(wire);
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

/// Returns the game profile id the server logged us in as.
async fn login_sequence(
    conn: &mut Conn,
    uuid: &uuid::Uuid,
    access_token: Option<&str>,
) -> Result<uuid::Uuid, ConnectionError> {
    loop {
        // Older-version layouts are rewritten before the decode (26.1's
        // login_finished lacks the trailing session id).
        let raw = conn.reader.read().await?;
        super::capture::record(Phase::Login, &raw);
        let raw = match conn.translation.as_deref() {
            Some(t) => t.translate_login_frame(raw),
            None => raw,
        };
        let packet = login::read_frame(&raw)?;
        tracing::info!("Login packet: {:?}", std::mem::discriminant(&packet));
        match packet {
            LoginPacket::Hello(hello) => {
                handle_encryption(conn, &hello, uuid, access_token).await?;
            }
            LoginPacket::Compression(threshold) => {
                conn.set_compression_threshold(threshold);
                tracing::info!("Compression enabled (threshold: {threshold})");
            }
            LoginPacket::Finished(finished) => {
                let id = uuid::Uuid::from_u128(finished.profile.id);
                tracing::info!("Login success: {} ({id})", finished.profile.name);
                return Ok(id);
            }
            LoginPacket::Disconnect(reason) => {
                let reason = crate::ui::text::component_from_json(&reason).plain_text();
                return Err(ConnectionError::Disconnected(
                    crate::ui::text::strip_formatting(&reason),
                ));
            }
            // TODO: vanilla answers from the cookies an earlier transfer stored.
            LoginPacket::CookieRequest(key) => {
                send_login(
                    conn,
                    ServerboundLogin::CookieResponse { key, payload: None },
                )
                .await?;
            }
            // Vanilla `handleCustomQuery`: every query is answered as not
            // understood, which a server waiting on one needs to carry on.
            LoginPacket::CustomQuery { transaction_id } => {
                send_login(conn, ServerboundLogin::CustomQueryAnswer { transaction_id }).await?;
            }
        }
    }
}

async fn send_login(conn: &mut Conn, packet: ServerboundLogin) -> std::io::Result<()> {
    conn.writer.write(&packet.encode()).await
}

async fn handle_encryption(
    conn: &mut Conn,
    hello: &login::Hello,
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
        let hash = super::session::server_hash(&hello.server_id, &e.secret_key, &hello.public_key);
        super::session::join(access_token, uuid, &hash)
            .await
            .map_err(ConnectionError::Auth)?;
        tracing::info!("Session server authentication successful");
    } else {
        tracing::info!("Server does not require authentication");
    }

    let key = ServerboundLogin::Key {
        key: e.encrypted_public_key,
        encrypted_challenge: e.encrypted_challenge,
    };
    send_login(conn, key).await?;

    conn.set_encryption_key(e.secret_key);
    tracing::info!("Encryption enabled");
    Ok(())
}

async fn config_sequence(
    conn: &mut Conn,
    view_distance: u8,
    chat_options: crate::ui::chat::ChatOptions,
    skin_options: crate::ui::menu::SkinOptions,
    event_tx: &Sender<NetworkEvent>,
    outbound_rx: &mut mpsc::UnboundedReceiver<Outbound>,
    // `Some` on a mid-session reconfiguration: the previous registries,
    // kept when the server re-sends nothing (vanilla's RegistryDataCollector
    // returns the original registries unchanged in that case).
    previous: Option<&Configured>,
) -> Result<Configured, ConnectionError> {
    let protocol = crate::version::session_protocol();
    let mut registries = Registries::default();
    let mut received_dialog_tags = None;
    let mut received_timeline_tags = None;

    // Vanilla sends brand and client information once, from the login
    // listener; a reconfiguration sends neither.
    if previous.is_none() {
        // Some servers key off the brand.
        let brand = GamePacket::CustomPayload {
            id: "minecraft:brand".into(),
            data: super::brand_payload(),
        };
        write_config_packet(conn, &brand).await?;
        let information = super::client_information(view_distance, chat_options, skin_options);
        write_config_packet(conn, &GamePacket::ClientInformation(information)).await?;
    }

    // Older wire versions translate each frame first (765's registry_data
    // fans out into several, hence the queue).
    let mut pending = std::collections::VecDeque::new();
    loop {
        let packet = if let Some(packet) = pending.pop_front() {
            packet
        } else {
            tokio::select! {
                raw = conn.reader.read() => {
                    let raw = raw?;
                    super::capture::record(Phase::Configuration, &raw);
                    let frames = match conn.translation.as_deref() {
                        Some(t) => t.translate_config_frame(raw),
                        None => vec![raw],
                    };
                    for frame in frames {
                        if super::dialog::handle_raw_dialog_packet(
                            Phase::Configuration,
                            &frame,
                            event_tx,
                        ) {
                            continue;
                        }
                        pending.extend(super::config_wire::read_or_skip(&frame));
                    }
                    continue;
                }
                // `Some(..)` disables the branch when the channel closes instead
                // of busy-looping on a closed receiver.
                Some(outbound) = outbound_rx.recv() => {
                    if let Outbound::CustomClick { id, payload } = &outbound {
                        if let Some(frame) =
                            custom_click_frame(Phase::Configuration, id, payload.as_ref())
                        {
                            write_config_frame(conn, frame).await?;
                        }
                    } else if let Outbound::Packet(packet) = outbound
                        && matches!(*packet, GamePacket::ResourcePack { .. })
                    {
                        write_config_packet(conn, &packet).await?;
                    }
                    // Anything else is discarded: vanilla defers its outbound
                    // queue, but pomme's game keeps ticking through a
                    // reconfiguration, so stale movement/actions are best dropped.
                    continue;
                }
            }
        };
        match packet {
            ConfigFrame::Config(ConfigPacket::RegistryData(data)) => registries
                .append_or_skip(protocol, data)
                .map_err(ConnectionError::Disconnected)?,
            ConfigFrame::Tags(tags) => {
                // A later packet replaces an earlier one's tags per registry.
                if let Some(tags) = registry_tags(&tags, DIALOG_REGISTRY) {
                    received_dialog_tags = Some(tags);
                }
                if let Some(tags) = registry_tags(&tags, super::environment_wire::TIMELINE_REGISTRY)
                {
                    received_timeline_tags = Some(tags);
                }
                forward_tags(event_tx, &tags);
            }
            ConfigFrame::Config(ConfigPacket::SelectKnownPacks(offered)) => {
                let known = registries.select_packs(protocol, &offered);
                write_config_frame(conn, ServerboundConfig::SelectKnownPacks(known).encode())
                    .await?;
            }
            ConfigFrame::Lifecycle(LifecyclePacket::KeepAlive(id)) => {
                write_config_packet(conn, &GamePacket::KeepAlive { id }).await?;
            }
            ConfigFrame::Lifecycle(LifecyclePacket::Ping(id)) => {
                write_config_packet(conn, &GamePacket::Pong { id }).await?;
            }
            ConfigFrame::Config(ConfigPacket::FinishConfiguration) => {
                write_config_frame(conn, ServerboundConfig::FinishConfiguration.encode()).await?;
                return Ok(match previous {
                    Some(previous) if !registries.received => Configured {
                        registries: previous.registries.clone(),
                        dialogs: match received_dialog_tags {
                            Some(tags) => std::sync::Arc::new(previous.dialogs.with_tags(tags)),
                            None => previous.dialogs.clone(),
                        },
                    },
                    // TODO: timeline tags resent without registry data
                    // keep the previous resolution.
                    _ => {
                        super::environment_wire::resolve_timeline_tags(
                            &mut registries.holder,
                            &received_timeline_tags.unwrap_or_default(),
                        );
                        Configured {
                            dialogs: std::sync::Arc::new(dialog_registry(
                                &registries.holder,
                                received_dialog_tags.unwrap_or_default(),
                            )),
                            registries: std::sync::Arc::new(registries.holder),
                        }
                    }
                });
            }
            ConfigFrame::Lifecycle(LifecyclePacket::Disconnect(reason)) => {
                return Err(ConnectionError::Disconnected(
                    super::lifecycle_wire::disconnect_reason(&reason),
                ));
            }
            // TODO: vanilla answers from the cookies an earlier transfer stored.
            ConfigFrame::Config(ConfigPacket::CookieRequest(key)) => {
                let response = ServerboundConfig::CookieResponse { key, payload: None };
                write_config_frame(conn, response.encode()).await?;
            }
            ConfigFrame::ResourcePack(packet) => {
                if let Some(reply) =
                    super::resource_pack_wire::handle_resource_pack_packet(packet, event_tx)
                {
                    write_config_packet(conn, &reply).await?;
                }
            }
            // The other lifecycle packets belong to the game phase.
            ConfigFrame::Lifecycle(_) => {}
        }
    }
}

/// The synced biome registry, in protocol-id order.
pub(crate) fn biome_registry(
    holder: &azalea_core::registry_holder::RegistryHolder,
) -> Option<&azalea_core::registry_holder::RegistryType<simdnbt::owned::NbtCompound>> {
    holder
        .extra
        .get(&pomme_registry::identifier::Identifier::new(
            "minecraft:worldgen/biome",
        ))
}

/// How many biomes chunk sections can name.
pub(crate) fn biome_count(holder: &azalea_core::registry_holder::RegistryHolder) -> usize {
    biome_registry(holder).map_or(0, |registry| registry.map.len())
}

fn extract_biome_climate(
    holder: &azalea_core::registry_holder::RegistryHolder,
) -> std::collections::HashMap<u32, crate::renderer::chunk::mesher::BiomeClimate> {
    use crate::renderer::chunk::mesher::{BiomeClimate, GrassColorModifier, int_to_rgb};

    let mut result = std::collections::HashMap::new();
    if let Some(registry) = biome_registry(holder) {
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

            let water_color = effects
                .and_then(|e| nbt_color_from_compound(e, "water_color"))
                .map(int_to_rgb)
                .unwrap_or_else(|| int_to_rgb(0x3F76E4));

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
                    water_color,
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

pub(super) fn chat_types_from_registry_holder(
    holder: &azalea_core::registry_holder::RegistryHolder,
) -> super::chat::ChatTypeRegistry {
    let key: pomme_registry::identifier::Identifier = "minecraft:chat_type".into();
    let entries = holder
        .extra
        .get(&key)
        .map(|registry| registry.map.values().cloned().collect())
        .unwrap_or_default();
    super::chat::ChatTypeRegistry::from_entries(entries)
}

/// The registry the dialog glyphs and holders resolve against.
const DIALOG_REGISTRY: &str = "minecraft:dialog";

fn dialog_registry_key() -> pomme_registry::identifier::Identifier {
    DIALOG_REGISTRY.into()
}

fn dialog_registry(
    holder: &azalea_core::registry_holder::RegistryHolder,
    tags: std::collections::HashMap<String, Vec<usize>>,
) -> DialogRegistry {
    let entries = holder
        .extra
        .get(&dialog_registry_key())
        .map(|registry| {
            registry
                .map
                .iter()
                .map(|(id, nbt)| (id.to_string(), nbt.clone()))
                .collect()
        })
        .unwrap_or_default();
    DialogRegistry::new(entries, tags)
}

/// One registry's tags from an `update_tags` packet, if it has any.
fn registry_tags(
    tags: &Tags,
    registry: &str,
) -> Option<std::collections::HashMap<String, Vec<usize>>> {
    Some(
        tags.get(registry)?
            .iter()
            .map(|(name, elements)| {
                let entries = elements
                    .iter()
                    .filter_map(|&id| usize::try_from(id).ok())
                    .collect();
                (name.clone(), entries)
            })
            .collect(),
    )
}

/// Forwards the item and `minecraft:block` tags of an `update_tags` packet,
/// and the placeable paintings.
fn forward_tags(event_tx: &Sender<NetworkEvent>, tags: &Tags) {
    send_ordered(
        event_tx,
        NetworkEvent::ItemTags(super::recipe_wire::item_tags(tags)),
    );
    if let Some(paintings) = tags.get(super::PAINTING_VARIANT_REGISTRY) {
        let placeable = paintings
            .iter()
            .find(|(name, _)| name.as_str() == "minecraft:placeable")
            .map(|(_, elements)| elements.clone())
            .unwrap_or_default();
        send_ordered(event_tx, NetworkEvent::PlaceablePaintings(placeable));
    }
    let Some(tags) = tags.get("minecraft:block") else {
        return;
    };
    let tags = tags
        .iter()
        .map(|(name, elements)| (name.clone(), elements.clone()))
        .collect();
    send_ordered(event_tx, NetworkEvent::BlockTags { tags });
}

fn nbt_string_from_compound(compound: &simdnbt::owned::NbtCompound, key: &str) -> Option<String> {
    compound.get(key).and_then(|v| match v {
        simdnbt::owned::NbtTag::String(s) => Some(s.to_string()),
        _ => None,
    })
}

struct GameLoopArgs {
    block_state_table: BlockStateTable,
    outbound_tx: mpsc::UnboundedSender<Outbound>,
    outbound_rx: mpsc::UnboundedReceiver<Outbound>,
    joined: Joined,
    view_distance: u8,
    chat_options: crate::ui::chat::ChatOptions,
    skin_options: crate::ui::menu::SkinOptions,
    chat: ChatSender,
    key_pair_rx: mpsc::UnboundedReceiver<Option<std::sync::Arc<ProfileKeyPair>>>,
}

async fn game_loop(
    mut conn: Conn,
    event_tx: &Sender<NetworkEvent>,
    args: GameLoopArgs,
) -> Result<(), ConnectionError> {
    let GameLoopArgs {
        block_state_table,
        outbound_tx,
        mut outbound_rx,
        joined,
        view_distance,
        chat_options,
        skin_options,
        mut chat,
        mut key_pair_rx,
    } = args;
    let Joined {
        mut configured,
        mut deferred_login,
    } = joined;
    let mut chat_types = chat_types_from_registry_holder(&configured.registries);
    let sender = PacketSender::new(outbound_tx);
    let mut batch_size_calculator = super::chunk_batch::ChunkBatchSizeCalculator::default();
    let shared_tree: crate::net::commands::SharedCommandTree =
        std::sync::Arc::new(parking_lot::Mutex::new(None));

    // Share the registries with the game loop for hashing predicted container
    // clicks.
    send_ordered(
        event_tx,
        NetworkEvent::Registries(configured.registries.clone()),
    );
    send_ordered(
        event_tx,
        NetworkEvent::DialogRegistry(configured.dialogs.clone()),
    );

    let translation_session = conn.translation.clone();
    let translation = translation_session.as_deref();
    let mut inbound_chat =
        super::chat::InboundChat::for_protocol(crate::version::session_protocol());
    let mut chat_tick = tokio::time::interval(std::time::Duration::from_millis(50));
    chat_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    if deferred_login.is_some() {
        // 1.20.1 sends both from its login handler, where later versions send
        // them in the configuration phase (ClientPacketListener.handleLogin).
        let info = GamePacket::ClientInformation(super::client_information(
            view_distance,
            chat_options,
            skin_options,
        ));
        write_game_frame(&mut conn.writer, translation, info.encode()).await?;

        let brand = GamePacket::CustomPayload {
            id: "minecraft:brand".into(),
            data: super::brand_payload(),
        };
        write_game_frame(&mut conn.writer, translation, brand.encode()).await?;
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
                    if let Some(frame) = outbound_frame(out, translation, &mut chat, &shared_tree)? {
                        write_game_frame(&mut conn.writer, translation, frame).await?;
                    }
                    continue;
                }
                Some(key_pair) = key_pair_rx.recv() => {
                    if let Some(frame) = chat.key_pair_ready(key_pair) {
                        write_game_frame(&mut conn.writer, translation, frame).await?;
                    }
                    continue;
                }
                _ = chat_tick.tick() => {
                    chat.tick();
                    continue;
                }
                raw = conn.reader.read() => raw,
            }
        };
        let raw = raw?;
        super::capture::record(Phase::Game, &raw);
        if let Some(flag) = translation.and_then(|t| t.server_data_secure_chat(&raw)) {
            super::lifecycle_wire::send_secure_chat(flag, event_tx);
        }
        let raw = match translation {
            Some(t) => match t.translate_game_frame(raw) {
                Some(raw) => raw,
                None => continue,
            },
            None => raw,
        };
        match super::chat::handle_raw_chat_packet(&raw, event_tx, &chat_types, &mut inbound_chat) {
            Some(Err(ChatPacketError::Malformed(error))) => {
                tracing::warn!("Skipping malformed chat packet: {error}");
                continue;
            }
            Some(Err(ChatPacketError::Disconnect(key))) => {
                return Err(ConnectionError::ClientDisconnect(key));
            }
            Some(Ok(())) => continue,
            None => {}
        }
        if let Some(login) = super::lifecycle_wire::read_login(&raw) {
            match login {
                Ok(mut login) => {
                    inbound_chat.reset();
                    if crate::version::session_protocol() < 776 {
                        login.online_mode = conn.is_encrypted();
                    }
                    super::lifecycle_wire::login_events(&login, &configured.registries, event_tx);
                }
                Err(e) => tracing::warn!("Skipping malformed login packet: {e}"),
            }
            continue;
        }
        if super::dialog::handle_raw_dialog_packet(Phase::Game, &raw, event_tx)
            || handle_raw_game_packet(
                &raw,
                block_state_table,
                &configured.registries,
                &sender,
                &shared_tree,
                &mut batch_size_calculator,
                event_tx,
            )
        {
            continue;
        }
        match super::read_raw_frame(&raw, START_CONFIGURATION) {
            Some(Ok(())) => {
                // Vanilla clears the client level before acknowledging
                // (ClientPacketListener.handleConfigurationStart); chat
                // survives the transition, block tags don't. Whatever the
                // game queued goes first, then the pending chat
                // acknowledgement.
                // TODO: chat events still in flight to the game thread
                // miss this ack; the next login resets the tracker anyway.
                send_ordered(event_tx, NetworkEvent::Reconfiguring);
                while let Ok(out) = outbound_rx.try_recv() {
                    if let Some(frame) = outbound_frame(out, translation, &mut chat, &shared_tree)?
                    {
                        write_game_frame(&mut conn.writer, translation, frame).await?;
                    }
                }
                if let Some(frame) = chat.flush_ack() {
                    write_game_frame(&mut conn.writer, translation, frame).await?;
                }
                let ack = GamePacket::ConfigurationAcknowledged.encode();
                write_game_frame(&mut conn.writer, translation, ack).await?;
                let next = config_sequence(
                    &mut conn,
                    view_distance,
                    chat_options,
                    skin_options,
                    event_tx,
                    &mut outbound_rx,
                    Some(&configured),
                )
                .await?;
                if !std::sync::Arc::ptr_eq(&next.registries, &configured.registries) {
                    chat_types = chat_types_from_registry_holder(&next.registries);
                    send_ordered(event_tx, NetworkEvent::Registries(next.registries.clone()));
                    send_ordered(
                        event_tx,
                        NetworkEvent::BiomeColors {
                            colors: extract_biome_climate(&next.registries),
                        },
                    );
                }
                if !std::sync::Arc::ptr_eq(&next.dialogs, &configured.dialogs) {
                    send_ordered(event_tx, NetworkEvent::DialogRegistry(next.dialogs.clone()));
                }
                configured = next;
                continue;
            }
            Some(Err(e)) => {
                tracing::warn!("Skipping malformed start_configuration packet: {e}");
                continue;
            }
            None => {}
        }
        if let Some(tags) = super::read_raw_frame(&raw, pomme_protocol::tags::read) {
            match tags {
                Ok(tags) => {
                    if let Some(tags) = registry_tags(&tags, DIALOG_REGISTRY) {
                        configured.dialogs =
                            std::sync::Arc::new(configured.dialogs.with_tags(tags));
                        send_ordered(
                            event_tx,
                            NetworkEvent::DialogRegistry(configured.dialogs.clone()),
                        );
                    }
                    forward_tags(event_tx, &tags);
                }
                Err(e) => tracing::warn!("Skipping malformed update_tags packet: {e}"),
            }
        }
    }
}

/// `start_configuration`, which carries nothing.
pub(super) const START_CONFIGURATION: super::RawRead<()> =
    |name, _| (name == "start_configuration").then_some(Ok(()));

/// The frame one queued outbound item writes, if any.
fn outbound_frame(
    out: Outbound,
    translation: Option<&super::translate::TranslationSession>,
    chat: &mut ChatSender,
    tree: &crate::net::commands::SharedCommandTree,
) -> Result<Option<Vec<u8>>, ConnectionError> {
    Ok(match out {
        Outbound::Packet(mut packet) => {
            if let Some(t) = translation {
                t.remap_outbound(&mut packet);
            }
            Some(packet.encode())
        }
        Outbound::Raw(bytes) => Some(bytes),
        Outbound::ChatInput(input) => match chat.encode_input(&input, tree.lock().as_deref()) {
            Ok(frame) => Some(frame),
            Err(error) => {
                tracing::warn!("Not sending invalid chat input: {error}");
                None
            }
        },
        Outbound::ChatLogin { online_mode } => {
            chat.login(online_mode);
            None
        }
        Outbound::ChatMark(mark) => chat.mark(*mark),
        Outbound::CustomClick { id, payload } => {
            custom_click_frame(Phase::Game, &id, payload.as_ref())
        }
    })
}

/// A custom click action for `phase`, unless the wire version predates the
/// packet (the id translation passes appended packets through unmapped) or
/// the action doesn't encode.
fn custom_click_frame(
    phase: Phase,
    id: &str,
    payload: Option<&simdnbt::owned::NbtTag>,
) -> Option<Vec<u8>> {
    let wire = PacketTable::for_protocol(crate::version::session_protocol());
    if wire.is_none_or(|t| {
        t.id(phase, Direction::Serverbound, "custom_click_action")
            .is_none()
    }) {
        tracing::debug!("Not sending custom click action {id:?}: the wire version lacks it");
        return None;
    }
    super::chat::encode_outbound_custom_click_action(phase, id, payload)
        .map_err(|e| tracing::warn!("Could not encode custom click action {id:?}: {e}"))
        .ok()
}

/// Writes one native-layout frame, translating it for other wire versions.
async fn write_game_frame(
    writer: &mut RawWriter,
    translation: Option<&super::translate::TranslationSession>,
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

/// Writes a packet configuration shares with the game phase.
async fn write_config_packet(conn: &mut Conn, packet: &GamePacket) -> Result<(), ConnectionError> {
    let frame = packet
        .encode_in(Phase::Configuration)
        .unwrap_or_else(|| panic!("{} isn't a configuration packet", packet.name()));
    write_config_frame(conn, frame).await
}

/// Writes one native-layout configuration frame, translating it for other
/// wire versions (767 down: id remaps, suppression of packets the wire version
/// lacks, and the few layout rewrites in `translate_outbound_config_frame`).
async fn write_config_frame(conn: &mut Conn, frame: Vec<u8>) -> Result<(), ConnectionError> {
    let frame = match conn
        .translation
        .as_deref()
        .filter(|t| t.translates_config())
    {
        Some(t) => match t.translate_outbound_config_frame(frame) {
            Some(frame) => frame,
            None => return Ok(()),
        },
        None => frame,
    };
    Ok(conn.writer.write(&frame).await?)
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
    use pomme_protocol::version::NATIVE;

    use super::*;

    /// Game-phase `update_tags` and `start_configuration` read raw.
    #[test]
    fn game_tags_and_start_configuration_read_raw() {
        use azalea_protocol::common::tags::{TagMap, Tags as AzaleaTags};
        use azalea_protocol::packets::game::{
            ClientboundGamePacket, c_start_configuration, c_update_tags,
        };

        let tag = |name: &str, elements: Vec<i32>| AzaleaTags {
            name: name.into(),
            elements,
        };
        let map = TagMap(
            [
                (
                    "minecraft:block".into(),
                    vec![tag("minecraft:logs", vec![1, 300])],
                ),
                (
                    DIALOG_REGISTRY.into(),
                    vec![tag("minecraft:pause_screen_additions", vec![2])],
                ),
            ]
            .into_iter()
            .collect(),
        );
        let frame = azalea_protocol::write::serialize_packet(&ClientboundGamePacket::UpdateTags(
            c_update_tags::ClientboundUpdateTags { tags: map },
        ))
        .unwrap();
        let tags = crate::net::read_raw_frame(&frame, pomme_protocol::tags::read)
            .unwrap()
            .unwrap();
        let expected: Tags = [
            (
                "minecraft:block".to_owned(),
                [("minecraft:logs".to_owned(), vec![1, 300])].into(),
            ),
            (
                DIALOG_REGISTRY.to_owned(),
                [("minecraft:pause_screen_additions".to_owned(), vec![2])].into(),
            ),
        ]
        .into();
        assert_eq!(tags, expected);
        assert!(crate::net::read_raw_frame(&frame, START_CONFIGURATION).is_none());

        assert_eq!(
            registry_tags(&tags, DIALOG_REGISTRY),
            Some([("minecraft:pause_screen_additions".to_owned(), vec![2])].into())
        );
        let (tx, rx) = crossbeam_channel::unbounded();
        forward_tags(&tx, &tags);
        assert!(rx.try_iter().any(|event| matches!(
            event,
            NetworkEvent::BlockTags { tags } if tags == [("minecraft:logs".to_owned(), vec![1, 300])]
        )));

        let frame =
            azalea_protocol::write::serialize_packet(&ClientboundGamePacket::StartConfiguration(
                c_start_configuration::ClientboundStartConfiguration,
            ))
            .unwrap();
        assert!(matches!(
            crate::net::read_raw_frame(&frame, START_CONFIGURATION),
            Some(Ok(()))
        ));
    }

    /// A server that accepts pomme's known-pack claim sends the biomes as ids
    /// alone; the climate the mesher colours with then comes entirely from the
    /// embedded elements.
    #[test]
    fn filled_biome_entries_carry_their_climate() {
        use pomme_registry::identifier::Identifier;

        let holder = crate::net::known_packs::filled_holder(NATIVE.protocol, "worldgen/biome")
            .expect("biomes fill");
        let plains_id = holder.extra[&Identifier::new("minecraft:worldgen/biome")]
            .map
            .get_index_of(&Identifier::new("minecraft:plains"))
            .expect("plains biome") as u32;

        let plains = &extract_biome_climate(&holder)[&plains_id];
        assert_eq!(plains.temperature, 0.8);
        assert_eq!(plains.downfall, 0.4);
    }

    #[test]
    fn integrated_server_is_joinable() {
        assert!(crate::net::translate::joinable(
            crate::singleplayer::PROTOCOL
        ));
    }

    /// 762 (1.19.4) is not a supported version at all, so it never gains a
    /// wire translation; 775 has one.
    #[test]
    fn resolve_wire_gates_unjoinable_versions() {
        let native = NATIVE.protocol;
        // Launched as the native version.
        assert_eq!(resolve_wire(Some(775), native), Ok(775));
        assert_eq!(resolve_wire(Some(762), native), Ok(native));
        assert_eq!(resolve_wire(None, native), Ok(native));
        // Launched as the newest listed version, which need not be native.
        let latest = pomme_protocol::version::LATEST.protocol;
        assert_eq!(resolve_wire(Some(native), latest), Ok(native));
        if crate::net::translate::joinable(latest) {
            assert_eq!(resolve_wire(None, latest), Ok(latest));
        }
        // An untranslated launched version is refused whatever the probe
        // yielded, unless the server itself speaks a joinable protocol.
        assert_eq!(resolve_wire(None, 762), Err(762));
        assert_eq!(resolve_wire(Some(762), 762), Err(762));
        assert_eq!(resolve_wire(Some(native), 762), Ok(native));
        // A staged version (tables embedded, not yet in TRANSLATED) is
        // refused when launched and adopted around when the server is
        // joinable.
        for v in pomme_protocol::version::VERSIONS {
            if pomme_protocol::PacketTable::for_protocol(v.protocol).is_some()
                && !crate::net::translate::joinable(v.protocol)
            {
                assert_eq!(
                    resolve_wire(Some(v.protocol), native),
                    Ok(native),
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

    /// A remote join selects the block protocol before it opens the
    /// connection, so configuration-phase tags already use that table. The
    /// port is closed, so a selection after connecting never happens.
    #[tokio::test]
    async fn remote_join_selects_the_protocol_before_connecting() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = listener.local_addr().unwrap().to_string();
        drop(listener);
        let (event_tx, event_rx) = crossbeam_channel::bounded(4096);
        let (packet_tx, packet_rx) = mpsc::unbounded_channel();
        let joined = connect_to_server(
            ConnectArgs {
                transport: Transport::Remote {
                    server,
                    protocol: Some(NATIVE.protocol),
                },
                username: "Steve".to_owned(),
                uuid: uuid::Uuid::nil(),
                access_token: None,
                view_distance: 8,
                chat_options: crate::ui::chat::ChatOptions::default(),
                skin_options: crate::ui::menu::SkinOptions::default(),
            },
            event_tx,
            packet_tx,
            packet_rx,
        )
        .await;

        assert!(joined.is_err());
        assert!(matches!(
            event_rx.try_recv(),
            Ok(NetworkEvent::ProtocolSelected { protocol }) if protocol == NATIVE.protocol
        ));
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
        use azalea_protocol::packets::login::c_login_finished::ClientboundLoginFinished;
        use azalea_protocol::packets::login::{ClientboundLoginPacket, ServerboundLoginPacket};
        use azalea_protocol::read::deserialize_packet;
        use uuid::Uuid;

        use crate::net::conn::memory_pipes;

        /// The next packet the client sent, in whichever phase the caller
        /// names.
        async fn sent<P: azalea_protocol::packets::ProtocolPacket + std::fmt::Debug>(
            peer: &mut Conn,
        ) -> P {
            deserialize_packet(&mut std::io::Cursor::new(
                &peer.reader.read().await.unwrap(),
            ))
            .unwrap()
        }

        async fn send<P: azalea_protocol::packets::ProtocolPacket + std::fmt::Debug>(
            peer: &mut Conn,
            packet: &P,
        ) {
            let frame = azalea_protocol::write::serialize_packet(packet).unwrap();
            peer.writer.write(&frame).await.unwrap();
        }

        let (client_end, server_end) = memory_pipes();
        let mut peer = Conn::from_memory(server_end);

        let (event_tx, event_rx) = crossbeam_channel::bounded(4096);
        let (packet_tx, packet_rx) = mpsc::unbounded_channel();

        let client = tokio::spawn(connect_to_server(
            ConnectArgs {
                transport: Transport::Memory {
                    end: client_end,
                    protocol: NATIVE.protocol,
                },
                username: "Steve".to_owned(),
                uuid: Uuid::nil(),
                access_token: None,
                view_distance: 8,
                chat_options: crate::ui::chat::ChatOptions::default(),
                skin_options: crate::ui::menu::SkinOptions::default(),
            },
            event_tx,
            packet_tx,
            packet_rx,
        ));

        assert!(matches!(
            sent(&mut peer).await,
            ServerboundHandshakePacket::Intention(p) if p.protocol_version == NATIVE.protocol
        ));
        assert!(matches!(
            sent(&mut peer).await,
            ServerboundLoginPacket::Hello(p) if p.name == "Steve"
        ));

        let finished = ClientboundLoginPacket::LoginFinished(ClientboundLoginFinished {
            game_profile: GameProfile::new(Uuid::nil(), "Steve".to_owned()),
            session_id: Uuid::nil(),
        });
        send(&mut peer, &finished).await;

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

        let finish = ClientboundConfigPacket::FinishConfiguration(ClientboundFinishConfiguration);
        send(&mut peer, &finish).await;

        assert!(matches!(
            sent(&mut peer).await,
            ServerboundConfigPacket::FinishConfiguration(_)
        ));

        // Protocol selection and the end of login precede configuration; the
        // remaining events arrive as soon as configuration ends, before any
        // game packet.
        let events: Vec<_> = std::iter::from_fn(|| event_rx.recv().ok())
            .take(4)
            .collect();
        assert!(
            matches!(events[0], NetworkEvent::ProtocolSelected { protocol } if protocol == NATIVE.protocol)
        );
        assert!(matches!(events[1], NetworkEvent::LoggedIn));
        assert!(matches!(events[2], NetworkEvent::BiomeColors { .. }));
        assert!(matches!(events[3], NetworkEvent::Connected));

        client.abort();
    }
}
