use azalea_core::position::ChunkPos;
use azalea_core::registry_holder::RegistryHolder;
use azalea_protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket};
use crossbeam_channel::Sender;

use super::NetworkEvent;
use super::sender::PacketSender;

pub fn handle_game_packet(
    packet: &ClientboundGamePacket,
    sender: &PacketSender,
    event_tx: &Sender<NetworkEvent>,
    registry_holder: &RegistryHolder,
) {
    match packet {
        ClientboundGamePacket::Login(p) => {
            if let Some((_, dim)) = p.common.dimension_type(registry_holder) {
                let _ = event_tx.try_send(NetworkEvent::DimensionInfo {
                    height: dim.height,
                    min_y: dim.min_y,
                });
            }
            let _ = event_tx.try_send(NetworkEvent::GameModeChanged {
                game_mode: p.common.game_type as u8,
            });
            let _ = event_tx.try_send(NetworkEvent::PlayerLogin {
                entity_id: p.player_id.0,
            });
        }
        ClientboundGamePacket::LevelChunkWithLight(p) => {
            tracing::trace!(
                "Chunk [{}, {}] ({} block entities)",
                p.x,
                p.z,
                p.chunk_data.block_entities.len()
            );
            let _ = event_tx.try_send(NetworkEvent::ChunkLoaded {
                pos: ChunkPos::new(p.x, p.z),
                data: p.chunk_data.data.clone(),
                heightmaps: p.chunk_data.heightmaps.clone(),
                sky_light: p.light_data.sky_updates.clone(),
                block_light: p.light_data.block_updates.clone(),
                sky_y_mask: p.light_data.sky_y_mask.clone(),
                block_y_mask: p.light_data.block_y_mask.clone(),
            });
        }
        ClientboundGamePacket::ForgetLevelChunk(p) => {
            let _ = event_tx.try_send(NetworkEvent::ChunkUnloaded { pos: p.pos });
        }
        ClientboundGamePacket::SetChunkCacheCenter(p) => {
            let _ = event_tx.try_send(NetworkEvent::ChunkCacheCenter { x: p.x, z: p.z });
        }
        ClientboundGamePacket::PlayerPosition(p) => {
            let _ = event_tx.try_send(NetworkEvent::PlayerPosition {
                x: p.change.pos.x,
                y: p.change.pos.y,
                z: p.change.pos.z,
                yaw: p.change.look_direction.y_rot(),
                pitch: p.change.look_direction.x_rot(),
            });
            sender.send(ServerboundGamePacket::AcceptTeleportation(
                azalea_protocol::packets::game::s_accept_teleportation::ServerboundAcceptTeleportation {
                    id: p.id,
                },
            ));
            // Vanilla echoes the position after every teleport; servers gate
            // per-player entity tracking on it.
            sender.send(ServerboundGamePacket::MovePlayerPosRot(
                azalea_protocol::packets::game::s_move_player_pos_rot::ServerboundMovePlayerPosRot {
                    pos: p.change.pos,
                    look_direction: p.change.look_direction,
                    flags: azalea_protocol::common::movements::MoveFlags {
                        on_ground: true,
                        horizontal_collision: false,
                    },
                },
            ));
        }
        ClientboundGamePacket::KeepAlive(p) => {
            sender.send(ServerboundGamePacket::KeepAlive(
                azalea_protocol::packets::game::s_keep_alive::ServerboundKeepAlive { id: p.id },
            ));
        }
        ClientboundGamePacket::ChunkBatchFinished(p) => {
            let desired = (p.batch_size as f32).max(25.0);
            tracing::trace!(
                "ChunkBatchFinished: batch_size={}, responding with desired={desired}",
                p.batch_size
            );
            sender.send(ServerboundGamePacket::ChunkBatchReceived(
                azalea_protocol::packets::game::s_chunk_batch_received::ServerboundChunkBatchReceived {
                    desired_chunks_per_tick: desired,
                },
            ));
        }
        ClientboundGamePacket::ContainerSetContent(p) if p.container_id == 0 => {
            let _ = event_tx.try_send(NetworkEvent::InventoryContent {
                items: p.items.clone(),
            });
        }
        ClientboundGamePacket::ContainerSetSlot(p)
            if p.container_id == 0 || p.container_id == -2 =>
        {
            let _ = event_tx.try_send(NetworkEvent::InventorySlot {
                index: p.slot,
                item: p.item_stack.clone(),
            });
        }
        ClientboundGamePacket::SetHealth(p) => {
            let _ = event_tx.try_send(NetworkEvent::PlayerHealth {
                health: p.health,
                food: p.food,
                saturation: p.saturation,
            });
        }
        ClientboundGamePacket::SetExperience(p) => {
            let _ = event_tx.try_send(NetworkEvent::PlayerExperience {
                progress: p.experience_progress,
                level: p.experience_level as i32,
            });
        }
        ClientboundGamePacket::UpdateAttributes(p) => {
            use azalea_core::attribute_modifier_operation::AttributeModifierOperation;
            use azalea_registry::builtin::Attribute;
            for snapshot in &p.values {
                if snapshot.attribute != Attribute::Armor {
                    continue;
                }
                let base = snapshot.base;
                let mut add = 0.0f64;
                let mut mul_base = 0.0f64;
                let mut mul_total = 0.0f64;
                for m in &snapshot.modifiers {
                    match m.operation {
                        AttributeModifierOperation::AddValue => add += m.amount,
                        AttributeModifierOperation::AddMultipliedBase => mul_base += m.amount,
                        AttributeModifierOperation::AddMultipliedTotal => mul_total += m.amount,
                    }
                }
                let value = (base + add) * (1.0 + mul_base) * (1.0 + mul_total);
                let armor = value.clamp(0.0, 30.0).round() as u32;
                let _ = event_tx.try_send(NetworkEvent::EntityArmorUpdate {
                    entity_id: p.entity_id.0,
                    armor,
                });
                break;
            }
        }
        ClientboundGamePacket::SystemChat(p) if !p.overlay => {
            send_chat(event_tx, p.content.to_string());
        }
        ClientboundGamePacket::PlayerChat(p) => {
            send_chat(event_tx, p.message().to_string());
        }
        ClientboundGamePacket::DisguisedChat(p) => {
            send_chat(event_tx, p.message.to_string());
        }
        ClientboundGamePacket::BlockUpdate(p) => {
            let _ = event_tx.try_send(NetworkEvent::BlockUpdate {
                pos: p.pos,
                state: p.block_state,
            });
        }
        ClientboundGamePacket::SectionBlocksUpdate(p) => {
            let updates: Vec<_> = p
                .states
                .iter()
                .map(|s| {
                    let block_pos = azalea_core::position::BlockPos {
                        x: p.section_pos.x * 16 + s.pos.x as i32,
                        y: p.section_pos.y * 16 + s.pos.y as i32,
                        z: p.section_pos.z * 16 + s.pos.z as i32,
                    };
                    (block_pos, s.state)
                })
                .collect();
            let _ = event_tx.try_send(NetworkEvent::SectionBlocksUpdate { updates });
        }
        ClientboundGamePacket::BlockChangedAck(p) => {
            let _ = event_tx.try_send(NetworkEvent::BlockChangedAck { seq: p.seq });
        }
        ClientboundGamePacket::SetTime(p) => {
            let day_time = p.clock_updates.values().next().map(|c| c.total_ticks);
            let _ = event_tx.try_send(NetworkEvent::TimeUpdate {
                game_time: p.game_time,
                day_time,
            });
        }
        ClientboundGamePacket::SetChunkCacheRadius(p) => {
            let _ = event_tx.try_send(NetworkEvent::ServerViewDistance { distance: p.radius });
        }
        ClientboundGamePacket::SetSimulationDistance(p) => {
            let _ = event_tx.try_send(NetworkEvent::ServerSimulationDistance {
                distance: p.simulation_distance,
            });
        }
        ClientboundGamePacket::GameEvent(p) => {
            use azalea_protocol::packets::game::c_game_event::EventType;
            if p.event == EventType::ChangeGameMode {
                let _ = event_tx.try_send(NetworkEvent::GameModeChanged {
                    game_mode: p.param as u8,
                });
            }
        }
        ClientboundGamePacket::Disconnect(p) => {
            tracing::warn!("Disconnected: {}", p.reason);
            let _ = event_tx.try_send(NetworkEvent::Disconnected {
                reason: format!("{}", p.reason),
            });
        }
        ClientboundGamePacket::AddEntity(p) => {
            let yaw = (p.y_rot as f32) * 360.0 / 256.0;
            let pitch = (p.x_rot as f32) * 360.0 / 256.0;
            let head_yaw = (p.y_head_rot as f32) * 360.0 / 256.0;
            let vel = p.movement.to_vec3();
            let _ = event_tx.try_send(NetworkEvent::EntitySpawned {
                id: p.id.0,
                entity_type: p.entity_type,
                x: p.position.x,
                y: p.position.y,
                z: p.position.z,
                yaw,
                pitch,
                head_yaw,
                velocity: [vel.x, vel.y, vel.z],
            });
        }
        ClientboundGamePacket::RotateHead(p) => {
            let head_yaw = (p.y_head_rot as f32) * 360.0 / 256.0;
            let _ = event_tx.try_send(NetworkEvent::EntityHeadRotation {
                id: p.entity_id.0,
                head_yaw,
            });
        }
        ClientboundGamePacket::MoveEntityPos(p) => {
            send_entity_moved(event_tx, p.entity_id.0, &p.delta);
        }
        ClientboundGamePacket::MoveEntityPosRot(p) => {
            use azalea_core::delta::PositionDeltaTrait;
            let look: azalea_entity::LookDirection = p.look_direction.into();
            let _ = event_tx.try_send(NetworkEvent::EntityMovedRotated {
                id: p.entity_id.0,
                dx: p.delta.x(),
                dy: p.delta.y(),
                dz: p.delta.z(),
                yaw: look.y_rot(),
                pitch: look.x_rot(),
            });
        }
        ClientboundGamePacket::TeleportEntity(p) => {
            let _ = event_tx.try_send(NetworkEvent::EntityTeleported {
                id: p.id.0,
                x: p.change.pos.x,
                y: p.change.pos.y,
                z: p.change.pos.z,
                yaw: p.change.look_direction.y_rot(),
                pitch: p.change.look_direction.x_rot(),
            });
        }
        ClientboundGamePacket::EntityPositionSync(p) => {
            let _ = event_tx.try_send(NetworkEvent::EntityTeleported {
                id: p.id.0,
                x: p.values.pos.x,
                y: p.values.pos.y,
                z: p.values.pos.z,
                yaw: p.values.look_direction.y_rot(),
                pitch: p.values.look_direction.x_rot(),
            });
        }
        ClientboundGamePacket::RemoveEntities(p) => {
            let ids: Vec<i32> = p.entity_ids.iter().map(|id| id.0).collect();
            let _ = event_tx.try_send(NetworkEvent::EntitiesRemoved { ids });
        }
        ClientboundGamePacket::SetEntityData(p) => {
            for item in p.packed_items.iter() {
                // index 8 = item stack data for item entities
                if item.index == 8
                    && let azalea_entity::EntityDataValue::ItemStack(
                        azalea_inventory::ItemStack::Present(data),
                    ) = &item.value
                {
                    let name = crate::player::inventory::item_resource_name(data.kind);
                    let _ = event_tx.try_send(NetworkEvent::EntityItemData {
                        id: p.id.0,
                        item_name: name,
                        count: data.count,
                    });
                }
                if item.index == 16
                    && let azalea_entity::EntityDataValue::Boolean(is_baby) = &item.value
                {
                    let _ = event_tx.try_send(NetworkEvent::EntityBabyFlag {
                        id: p.id.0,
                        is_baby: *is_baby,
                    });
                }
                // Entity data index 16 = player score (1.21.4 protocol)
                if item.index == 16
                    && let azalea_entity::EntityDataValue::Int(score) = &item.value
                {
                    let _ = event_tx.try_send(NetworkEvent::PlayerScore {
                        entity_id: p.id.0,
                        score: *score,
                    });
                }
            }
        }
        ClientboundGamePacket::TakeItemEntity(p) => {
            let _ = event_tx.try_send(NetworkEvent::ItemPickedUp {
                item_id: p.item_id as i32,
                collector_id: p.player_id.0,
            });
        }
        ClientboundGamePacket::Respawn(p) => {
            if let Some((_, dim)) = p.common.dimension_type(registry_holder) {
                let _ = event_tx.try_send(NetworkEvent::DimensionInfo {
                    height: dim.height,
                    min_y: dim.min_y,
                });
            }
            let _ = event_tx.try_send(NetworkEvent::GameModeChanged {
                game_mode: p.common.game_type as u8,
            });
        }
        ClientboundGamePacket::PlayerCombatKill(p) => {
            tracing::info!("Player died: {}", p.message);
            let _ = event_tx.try_send(NetworkEvent::PlayerDied {
                message: p.message.to_string(),
            });
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
        ClientboundGamePacket::PlayerInfoUpdate(p) => {
            use crate::player::tab_list::{PlayerInfoActions, PlayerInfoEntry};
            let actions = PlayerInfoActions {
                add_player: p.actions.add_player,
                update_game_mode: p.actions.update_game_mode,
                update_listed: p.actions.update_listed,
                update_latency: p.actions.update_latency,
                update_display_name: p.actions.update_display_name,
                update_list_order: p.actions.update_list_order,
            };
            let entries = p
                .entries
                .iter()
                .map(|e| PlayerInfoEntry {
                    uuid: e.profile.uuid,
                    name: e.profile.name.clone(),
                    game_mode: e.game_mode.to_id(),
                    listed: e.listed,
                    latency: e.latency,
                    display_name: e.display_name.as_ref().map(|c| c.to_string()),
                    list_order: e.list_order,
                })
                .collect();
            let _ = event_tx.try_send(NetworkEvent::PlayerInfoUpdate { actions, entries });
        }
        ClientboundGamePacket::PlayerInfoRemove(p) => {
            let _ = event_tx.try_send(NetworkEvent::PlayerInfoRemove {
                uuids: p.profile_ids.clone(),
            });
        }
        ClientboundGamePacket::TabList(p) => {
            let _ = event_tx.try_send(NetworkEvent::TabListHeaderFooter {
                header: p.header.to_string(),
                footer: p.footer.to_string(),
            });
        }
        _other => {}
    }
}

fn send_chat(event_tx: &Sender<NetworkEvent>, text: String) {
    tracing::info!("Chat: {text}");
    let _ = event_tx.try_send(NetworkEvent::ChatMessage { text });
}

fn send_entity_moved(
    event_tx: &Sender<NetworkEvent>,
    id: i32,
    delta: &azalea_core::delta::PositionDelta8,
) {
    let _ = event_tx.try_send(NetworkEvent::EntityMoved {
        id,
        dx: delta.xa as f64 / 4096.0,
        dy: delta.ya as f64 / 4096.0,
        dz: delta.za as f64 / 4096.0,
    });
}
