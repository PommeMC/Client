use azalea_core::registry_holder::RegistryHolder;
use azalea_protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket};
use azalea_registry::Registry;
use azalea_registry::builtin::EntityKind;
use crossbeam_channel::{Sender, TrySendError};
use pomme_protocol::codec::{DecodeResult, Reader};

use super::NetworkEvent;
use super::commands::{CommandTree, SharedCommandTree};
use super::sender::PacketSender;
use crate::net::chunk_batch::ChunkBatchSizeCalculator;
use crate::player::inventory::item_resource_name;
use crate::renderer::pipelines::entity_renderer::{
    CAT_VARIANT_ORDER, CHICKEN_VARIANT_ORDER, COW_VARIANT_ORDER, WOLF_VARIANT_ORDER,
};
use crate::ui::server_dialog::DialogReference;
use crate::ui::text::format_text_spans;

fn dialog_holder_reference(
    holder: &azalea_registry::Holder<azalea_registry::data::Dialog, simdnbt::owned::Nbt>,
) -> DialogReference {
    match holder {
        azalea_registry::Holder::Reference(dialog) => DialogReference::ProtocolId(dialog.to_u32()),
        azalea_registry::Holder::Direct(nbt) => DialogReference::inline(nbt),
    }
}

/// Queues an event the game thread must not lose, blocking while the queue is
/// full. Blocking stalls this connection's reads and writes until the game
/// thread drains, so on a multi-thread runtime the worker's other tasks are
/// handed off first.
pub(super) fn send_ordered(event_tx: &Sender<NetworkEvent>, event: NetworkEvent) {
    if let Err(TrySendError::Full(event)) = event_tx.try_send(event) {
        let block = || {
            let _ = event_tx.send(event);
        };
        match tokio::runtime::Handle::try_current() {
            Ok(rt) if rt.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(block)
            }
            _ => block(),
        }
    }
}

pub fn handle_game_packet(
    packet: &ClientboundGamePacket,
    sender: &PacketSender,
    event_tx: &Sender<NetworkEvent>,
    registry_holder: &RegistryHolder,
    shared_tree: &SharedCommandTree,
) {
    match packet {
        ClientboundGamePacket::Waypoint(p) => {
            let _ = event_tx.try_send(NetworkEvent::Waypoint {
                operation: p.operation,
                waypoint: p.waypoint.clone(),
            });
        }
        ClientboundGamePacket::UpdateAdvancements(p) => {
            use azalea_protocol::packets::game::c_update_advancements::FrameType;

            use crate::ui::toast;
            let added = p
                .added
                .iter()
                .map(|holder| {
                    (
                        holder.id.to_string(),
                        toast::AdvancementData {
                            display: holder.value.display.as_deref().map(|d| {
                                toast::AdvancementDisplay {
                                    title: format_text_spans(&d.title, [1.0; 4]),
                                    frame: match d.frame {
                                        FrameType::Task => toast::AdvancementFrame::Task,
                                        FrameType::Challenge => toast::AdvancementFrame::Challenge,
                                        FrameType::Goal => toast::AdvancementFrame::Goal,
                                    },
                                    show_toast: d.show_toast,
                                    icon_item: match &d.icon {
                                        azalea_inventory::ItemStack::Present(data) => {
                                            Some(item_resource_name(data.kind))
                                        }
                                        azalea_inventory::ItemStack::Empty => None,
                                    },
                                }
                            }),
                            requirements: holder.value.requirements.clone(),
                        },
                    )
                })
                .collect();
            let progress = p
                .progress
                .iter()
                .map(|(id, criteria)| {
                    (
                        id.to_string(),
                        criteria
                            .iter()
                            .map(|(name, c)| (name.clone(), c.date.is_some()))
                            .collect(),
                    )
                })
                .collect();
            let _ = event_tx.try_send(NetworkEvent::AdvancementsUpdate(Box::new(
                toast::AdvancementsUpdate {
                    reset: p.reset,
                    added,
                    removed: p.removed.iter().map(|id| id.to_string()).collect(),
                    progress,
                    show_advancements: p.show_advancements,
                },
            )));
        }
        ClientboundGamePacket::SetEntityData(p) => {
            super::entity_data::handle_typed_entity_data(p, registry_holder, event_tx);
        }
        ClientboundGamePacket::ResourcePackPush(p) => {
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
            sender.send(ServerboundGamePacket::ResourcePack(
                azalea_protocol::packets::game::s_resource_pack::ServerboundResourcePack {
                    id: p.id,
                    action: azalea_protocol::packets::game::s_resource_pack::Action::Accepted,
                },
            ));
        }
        ClientboundGamePacket::ResourcePackPop(p) => {
            tracing::info!("Server popping resource pack {:?}", p.id);
            let _ = event_tx.try_send(NetworkEvent::ResourcePackPop { id: p.id });
        }
        ClientboundGamePacket::Commands(p) => {
            let tree = std::sync::Arc::new(CommandTree::from_packet(p));
            tracing::info!(
                "Command tree received: {} nodes, root commands = {:?}",
                p.entries.len(),
                tree.root_child_names()
            );
            *shared_tree.lock() = Some(tree.clone());
            let _ = event_tx.try_send(NetworkEvent::CommandTree { tree });
        }
        ClientboundGamePacket::ShowDialog(p) => {
            let _ = event_tx.try_send(NetworkEvent::ShowDialog {
                dialog: dialog_holder_reference(&p.dialog),
            });
        }
        ClientboundGamePacket::ClearDialog(_) => {
            let _ = event_tx.try_send(NetworkEvent::ClearDialog);
        }
        ClientboundGamePacket::CustomChatCompletions(p) => {
            tracing::debug!(
                "Custom chat completions: {:?} ({} entries)",
                p.action,
                p.entries.len()
            );
        }
        _other => {}
    }
}

/// Resolves a variant registry holder id to the mob's renderer pool slot.
/// Matches vanilla, which reads the entry's synced NBT and never the registry
/// id: a known `asset_id` picks the exact slot, else the `model` field picks
/// the mesh (its values name slots; absent means "normal" = slot 0), else the
/// registry path as a last resort for entries synced without NBT.
pub(super) fn variant_index(
    registry_holder: &RegistryHolder,
    kind: EntityKind,
    protocol_id: u32,
) -> u32 {
    // (registry, pool order, asset prefix, vanilla's default texture variant).
    let (registry, order, asset_prefix, default) = match kind {
        EntityKind::Cow => (
            "minecraft:cow_variant",
            COW_VARIANT_ORDER,
            "entity/cow/cow_",
            "temperate",
        ),
        EntityKind::Chicken => (
            "minecraft:chicken_variant",
            CHICKEN_VARIANT_ORDER,
            "entity/chicken/chicken_",
            "temperate",
        ),
        EntityKind::Cat => (
            "minecraft:cat_variant",
            CAT_VARIANT_ORDER,
            "entity/cat/cat_",
            "tabby",
        ),
        // TODO: wolf NBT nests its textures (assets.wild, no flat
        // asset_id/model), so datapack wolves only resolve by registry path.
        EntityKind::Wolf => (
            "minecraft:wolf_variant",
            WOLF_VARIANT_ORDER,
            "entity/wolf/wolf_",
            "pale",
        ),
        _ => return 0,
    };
    let order_pos = |name: &str| order.iter().position(|p| *p == name).map(|i| i as u32);
    let fallback = order_pos(default).unwrap_or(0);
    // Position == protocol id only holds while every entry carries NBT
    // (azalea shift_removes NBT-less ones). Entries the server skips for a
    // pack pomme claimed are filled in first (`net::known_packs`).
    let Some((ident, nbt)) = registry_holder
        .extra
        .get(&azalea_registry::identifier::Identifier::new(registry))
        .and_then(|r| r.map.get_index(protocol_id as usize))
    else {
        return fallback;
    };
    if let Some(asset) = nbt.string("asset_id").map(|s| s.to_str())
        && let Some(suffix) = asset
            .strip_prefix("minecraft:")
            .unwrap_or(&asset)
            .strip_prefix(asset_prefix)
        && let Some(i) = order_pos(suffix)
    {
        return i;
    }
    if let Some(model) = nbt.string("model").map(|s| s.to_str())
        && let Some(i) = order_pos(&model)
    {
        return i;
    }
    order_pos(ident.path()).unwrap_or(fallback)
}

/// Consume packets that azalea's 26.2 codecs cannot represent correctly
/// before the typed decode runs. Returns whether the packet was consumed.
pub fn handle_raw_game_packet(
    raw: &[u8],
    registry_holder: &RegistryHolder,
    sender: &PacketSender,
    batch_size_calculator: &mut ChunkBatchSizeCalculator,
    event_tx: &Sender<NetworkEvent>,
) -> bool {
    let mut cur = std::io::Cursor::new(raw);
    let Ok(packet_id) = Reader::on_cursor(&mut cur, Reader::varint) else {
        return false;
    };

    if took(
        super::recipe_wire::handle_raw_recipe_packet(packet_id, &mut cur, event_tx),
        "recipe",
    ) {
        return true;
    }

    let pos = cur.position() as usize;
    let payload = &raw[pos..];
    if took(
        super::container_wire::handle_raw_container_packet(packet_id, payload, event_tx),
        "container",
    ) || took(
        super::hud_wire::handle_raw_hud_packet(packet_id, payload, event_tx),
        "hud",
    ) || took(
        super::scoreboard_wire::handle_raw_scoreboard_packet(packet_id, payload, event_tx),
        "scoreboard",
    ) || took(
        super::player_info_wire::handle_raw_player_info_packet(packet_id, payload, event_tx),
        "player_info",
    ) || took(
        super::sound_wire::handle_raw_sound_packet(packet_id, payload, event_tx),
        "sound",
    ) || took(
        super::lifecycle_wire::handle_raw_lifecycle_packet(
            packet_id,
            payload,
            registry_holder,
            sender,
            event_tx,
        ),
        "lifecycle",
    ) || took(
        super::entity_data::handle_raw_entity_data(packet_id, payload, registry_holder, event_tx),
        "set_entity_data",
    ) || took(
        super::advancement_wire::handle_raw_advancements(packet_id, payload, event_tx),
        "update_advancements",
    ) || took(
        super::entity_wire::handle_raw_entity_packet(packet_id, payload, event_tx),
        "entity",
    ) || took(
        super::player_wire::handle_raw_player_packet(packet_id, payload, event_tx),
        "player",
    ) || took(
        super::block_wire::handle_raw_block_packet(packet_id, payload, event_tx),
        "block",
    ) || took(
        super::chunk_wire::handle_raw_chunk_packet(
            packet_id,
            payload,
            sender,
            batch_size_calculator,
            event_tx,
        ),
        "chunk",
    ) {
        return true;
    }

    if packet_id == explode_packet_id() {
        if let Err(e) = handle_raw_explode(&mut Reader::new(payload), event_tx) {
            tracing::warn!("Skipping malformed Explode packet prefix: {e}");
        }
        return true;
    }

    if packet_id != level_particles_packet_id() {
        return false;
    }
    match parse_level_particles(&mut Reader::new(payload)) {
        Ok(Some(event)) => {
            let _ = event_tx.try_send(event);
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("Skipping malformed LevelParticles packet: {e}"),
    }
    true
}

/// Whether a raw decoder took the packet, logging it when malformed.
fn took<E: std::fmt::Display>(result: Option<Result<(), E>>, packet: &str) -> bool {
    if let Some(Err(e)) = &result {
        tracing::warn!("Skipping malformed {packet} packet: {e}");
    }
    result.is_some()
}

/// Runs `raw` through [`handle_raw_game_packet`] with fresh connection
/// state: whether a raw handler took it, and what it sent back.
#[cfg(test)]
pub(crate) fn handle_raw_for_test(
    raw: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> (
    bool,
    tokio::sync::mpsc::UnboundedReceiver<crate::net::sender::Outbound>,
) {
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
    let took = handle_raw_game_packet(
        raw,
        &RegistryHolder::default(),
        &PacketSender::new(out_tx),
        &mut ChunkBatchSizeCalculator::default(),
        event_tx,
    );
    (took, out_rx)
}

/// Vanilla 26.2 `ClientboundExplodePacket` starts with the center, radius,
/// block count and optional player knockback. azalea's typed codec misreads
/// the particle tail (see
/// `azalea_compat::azalea_explode_still_misdecodes_native_particles`),
/// so the knockback prefix is read here and the frame consumed.
fn handle_raw_explode(r: &mut Reader, event_tx: &Sender<NetworkEvent>) -> DecodeResult<()> {
    // TODO: vanilla `handleExplosion` also plays the explosion sound and
    // spawns its particles from the center and the payload tail.
    let _center = r.vec3()?;
    let _radius = r.f32()?;
    let _block_count = r.i32()?;

    if r.bool()? {
        let delta = r.vec3()?;
        send_ordered(event_tx, NetworkEvent::PlayerKnockback { delta });
    }
    Ok(())
}

/// The native version whose explode layout [`handle_raw_explode`] reads.
#[cfg(test)]
const RAW_EXPLODE_LAYOUT: (&str, i32) = ("26.2", 776);

fn explode_packet_id() -> u32 {
    use pomme_protocol::{Direction, PacketTable, Phase};

    static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        PacketTable::native()
            .id(Phase::Game, Direction::Clientbound, "explode")
            .expect("explode in packet table")
    })
}

/// The wire layout of vanilla `ClientboundLevelParticlesPacket.write`, up to
/// the particle type id.
fn parse_level_particles(r: &mut Reader) -> DecodeResult<Option<NetworkEvent>> {
    let override_limiter = r.bool()?;
    let _always_show = r.bool()?;
    let pos = r.vec3()?;
    let x_dist = r.f32()?;
    let y_dist = r.f32()?;
    let z_dist = r.f32()?;
    let max_speed = r.f32()?;
    // Signed on the wire; Java's `i < count` loop no-ops on negative counts.
    let count = r.i32()?.max(0) as u32;
    let type_id = r.varint()?;
    // Particle ids shift between versions; translate into the native id
    // space (`ServerParticleKind`'s) when speaking an older protocol.
    let type_id = match super::translate::active() {
        Some(t) => match t.remap_particle(type_id) {
            Some(id) => id,
            None => return Ok(None),
        },
        None => type_id,
    };
    let Some(kind) = crate::particle::ServerParticleKind::from_id(type_id) else {
        return Ok(None);
    };
    Ok(Some(NetworkEvent::LevelParticles {
        kind,
        override_limiter,
        pos,
        x_dist,
        y_dist,
        z_dist,
        max_speed,
        count,
    }))
}

/// `ClientboundLevelParticles`' packet id from the vanilla-derived table
/// (cross-checked against azalea's dispatch table in `azalea_compat`).
fn level_particles_packet_id() -> u32 {
    use pomme_protocol::{Direction, PacketTable, Phase};

    static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        PacketTable::native()
            .id(Phase::Game, Direction::Clientbound, "level_particles")
            .expect("level_particles in packet table")
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier, mpsc as std_mpsc};
    use std::time::Duration;

    use pomme_protocol::wire;

    use super::*;

    fn handle_raw(raw: &[u8], event_tx: &Sender<NetworkEvent>) -> bool {
        handle_raw_for_test(raw, event_tx).0
    }

    /// A frame holding just the native id of clientbound packet `name`.
    fn native_frame(name: &str) -> Vec<u8> {
        use pomme_protocol::{Direction, PacketTable, Phase};

        let mut raw = Vec::new();
        let id = PacketTable::native().id(Phase::Game, Direction::Clientbound, name);
        wire::write_varint(&mut raw, id.unwrap());
        raw
    }

    fn assert_event_backpressures(
        queue: impl FnOnce(Sender<NetworkEvent>) + Send,
        assert_event: impl FnOnce(NetworkEvent),
    ) {
        let (event_tx, event_rx) = crossbeam_channel::bounded(1);
        event_tx.send(NetworkEvent::Connected).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let (done_tx, done_rx) = std_mpsc::channel();

        std::thread::scope(|scope| {
            let worker_barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                worker_barrier.wait();
                queue(event_tx);
                done_tx.send(()).unwrap();
            });

            barrier.wait();
            assert!(
                done_rx.recv_timeout(Duration::from_millis(20)).is_err(),
                "authoritative event must wait for queue capacity instead of being dropped"
            );
            assert!(matches!(event_rx.recv().unwrap(), NetworkEvent::Connected));
            done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_event(event_rx.recv().unwrap());
        });
    }

    #[test]
    fn forced_player_rotation_backpressures_instead_of_being_dropped() {
        assert_event_backpressures(
            |event_tx| {
                let mut raw = native_frame("player_rotation");
                raw.extend(45f32.to_be_bytes());
                raw.push(0);
                raw.extend(20f32.to_be_bytes());
                raw.push(0);
                assert!(handle_raw(&raw, &event_tx));
            },
            |event| {
                assert!(matches!(
                    event,
                    NetworkEvent::PlayerRotation {
                        y_rot: 45.0,
                        x_rot: 20.0,
                        relative_y: false,
                        relative_x: false,
                    }
                ));
            },
        );
    }

    #[test]
    fn authoritative_entity_motion_backpressures_instead_of_being_dropped() {
        let expected = glam::DVec3::new(0.125, 0.75, -0.25);
        assert_event_backpressures(
            move |event_tx| {
                send_ordered(
                    &event_tx,
                    NetworkEvent::EntityMotion {
                        id: 42,
                        velocity: expected,
                    },
                )
            },
            |event| {
                assert!(matches!(
                    event,
                    NetworkEvent::EntityMotion { id: 42, velocity } if velocity == expected
                ));
            },
        );
    }

    #[test]
    fn send_ordered_blocks_in_place_on_a_multi_thread_runtime() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap();
        assert_event_backpressures(
            move |event_tx| {
                rt.block_on(async {
                    tokio::spawn(async move {
                        send_ordered(&event_tx, NetworkEvent::BlockChangedAck { seq: 3 })
                    })
                    .await
                    .unwrap()
                })
            },
            |event| assert!(matches!(event, NetworkEvent::BlockChangedAck { seq: 3 })),
        );
    }

    #[test]
    fn raw_explode_layout_matches_native() {
        let native = pomme_protocol::version::NATIVE;
        assert_eq!(
            (native.name, native.protocol),
            RAW_EXPLODE_LAYOUT,
            "re-check handle_raw_explode against the new native ClientboundExplodePacket"
        );
    }

    #[test]
    fn explosion_knockback_backpressures_instead_of_being_dropped() {
        let expected = glam::DVec3::new(0.25, 0.5, -0.125);
        assert_event_backpressures(
            move |event_tx| assert!(handle_raw(&explosion_prefix(Some(expected)), &event_tx)),
            |event| {
                assert!(matches!(
                    event,
                    NetworkEvent::PlayerKnockback { delta } if delta == expected
                ));
            },
        );
    }

    #[test]
    fn block_change_ack_backpressures_instead_of_being_dropped() {
        assert_event_backpressures(
            |event_tx| {
                let mut raw = native_frame("block_changed_ack");
                wire::write_varint(&mut raw, 17);
                assert!(handle_raw(&raw, &event_tx));
            },
            |event| assert!(matches!(event, NetworkEvent::BlockChangedAck { seq: 17 })),
        );
    }

    #[test]
    fn play_ping_is_queued_for_ordered_app_thread_handling() {
        let (event_tx, event_rx) = crossbeam_channel::bounded(1);
        let mut raw = native_frame("ping");
        raw.extend(0x1234_5678i32.to_be_bytes());
        let (took, mut out_rx) = handle_raw_for_test(&raw, &event_tx);
        assert!(took);

        assert!(matches!(
            event_rx.recv().unwrap(),
            NetworkEvent::Ping { id: 0x1234_5678 }
        ));
        assert!(
            out_rx.try_recv().is_err(),
            "network task must not pong immediately"
        );
    }

    #[test]
    fn chunk_batch_finished_acks_on_the_network_thread() {
        let (event_tx, _event_rx) = crossbeam_channel::bounded(1);
        let mut raw = native_frame("chunk_batch_finished");
        wire::write_varint(&mut raw, 16);
        let (took, mut out_rx) = handle_raw_for_test(&raw, &event_tx);
        assert!(took);
        assert!(matches!(
            out_rx.try_recv(),
            Ok(crate::net::sender::Outbound::Packet(packet))
                if matches!(*packet, ServerboundGamePacket::ChunkBatchReceived(_))
        ));
    }

    #[test]
    fn set_held_slot_emits_authoritative_hotbar_selection() {
        let (event_tx, event_rx) = crossbeam_channel::bounded(1);
        let receive = |slot| {
            let mut raw = native_frame("set_held_slot");
            wire::write_varint(&mut raw, slot);
            assert!(handle_raw(&raw, &event_tx));
        };

        receive(5);
        assert!(matches!(
            event_rx.recv().unwrap(),
            NetworkEvent::HeldSlot { slot: 5 }
        ));

        for invalid in [9, u32::MAX] {
            receive(invalid);
            assert!(matches!(
                event_rx.try_recv(),
                Err(crossbeam_channel::TryRecvError::Empty)
            ));
        }
    }

    fn explosion_prefix(knockback: Option<glam::DVec3>) -> Vec<u8> {
        let mut raw = Vec::new();
        wire::write_varint(&mut raw, explode_packet_id());
        for value in [12.5_f64, 64.0, -3.25] {
            raw.extend_from_slice(&value.to_be_bytes());
        }
        raw.extend_from_slice(&4.0_f32.to_be_bytes());
        raw.extend_from_slice(&7_i32.to_be_bytes());
        raw.push(knockback.is_some() as u8);
        if let Some(delta) = knockback {
            for value in [delta.x, delta.y, delta.z] {
                raw.extend_from_slice(&value.to_be_bytes());
            }
        }
        raw
    }

    #[test]
    fn raw_native_explosion_recovers_knockback_without_decoding_trailing_payload() {
        let expected = glam::DVec3::new(-0.3125, 0.4375, 0.0625);
        let mut raw = explosion_prefix(Some(expected));
        // A tail azalea can't decode must not cost the knockback.
        raw.extend_from_slice(&[0xff, 0x80]);

        let (tx, rx) = crossbeam_channel::bounded(1);
        assert!(handle_raw(&raw, &tx));
        assert!(matches!(
            rx.recv().unwrap(),
            NetworkEvent::PlayerKnockback { delta } if delta == expected
        ));
    }

    #[test]
    fn raw_native_explosion_without_knockback_is_consumed_without_event() {
        let mut raw = explosion_prefix(None);
        raw.push(0xff);

        let (tx, rx) = crossbeam_channel::bounded(1);
        assert!(handle_raw(&raw, &tx));
        assert!(matches!(
            rx.try_recv(),
            Err(crossbeam_channel::TryRecvError::Empty)
        ));
    }
}
