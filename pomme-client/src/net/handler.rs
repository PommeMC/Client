use azalea_core::position::ChunkPos;
use azalea_core::registry_holder::RegistryHolder;
use azalea_protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket};
use crossbeam_channel::Sender;

use super::NetworkEvent;
use super::commands::{CommandTree, SharedCommandTree};
use super::sender::PacketSender;
use crate::entity::components::Position;
use crate::ui::text::{TextSpan, format_text_spans};

pub fn handle_game_packet(
    packet: &ClientboundGamePacket,
    sender: &PacketSender,
    event_tx: &Sender<NetworkEvent>,
    registry_holder: &RegistryHolder,
    shared_tree: &SharedCommandTree,
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
            let chunk_pos = ChunkPos::new(p.x, p.z);
            let entries: Vec<_> = p
                .chunk_data
                .block_entities
                .iter()
                .map(|be| {
                    let local_x = ((be.packed_xz >> 4) & 0x0F) as i32;
                    let local_z = (be.packed_xz & 0x0F) as i32;
                    let block_pos = azalea_core::position::BlockPos {
                        x: chunk_pos.x * 16 + local_x,
                        y: be.y as i16 as i32,
                        z: chunk_pos.z * 16 + local_z,
                    };
                    let compound = match &be.data {
                        simdnbt::owned::Nbt::Some(base) => base.clone().as_compound(),
                        simdnbt::owned::Nbt::None => simdnbt::owned::NbtCompound::default(),
                    };
                    (block_pos, be.kind, compound)
                })
                .collect();
            let _ = event_tx.try_send(NetworkEvent::BlockEntitySync { chunk_pos, entries });
        }
        ClientboundGamePacket::BlockEvent(p) => {
            let _ = event_tx.try_send(NetworkEvent::BlockEvent {
                pos: p.pos,
                action_id: p.action_id,
                action_parameter: p.action_parameter,
            });
        }
        ClientboundGamePacket::Sound(p) => {
            // Coordinates are fixed-point: block position times 8.
            let _ = event_tx.try_send(NetworkEvent::PlaySound {
                sound: resolve_sound(&p.sound),
                category: p.source as u8,
                pos: Position::new(p.x as f64 / 8.0, p.y as f64 / 8.0, p.z as f64 / 8.0),
                volume: p.volume,
                pitch: p.pitch,
                seed: p.seed,
            });
        }
        ClientboundGamePacket::SoundEntity(p) => {
            let _ = event_tx.try_send(NetworkEvent::PlayEntitySound {
                sound: resolve_sound(&p.sound),
                category: p.source as u8,
                entity_id: p.id.0,
                volume: p.volume,
                pitch: p.pitch,
                seed: p.seed,
            });
        }
        ClientboundGamePacket::BlockEntityData(p) => {
            let nbt = match &p.tag {
                simdnbt::owned::Nbt::Some(base) => Some(base.clone().as_compound()),
                simdnbt::owned::Nbt::None => None,
            };
            let _ = event_tx.try_send(NetworkEvent::BlockEntityUpdate {
                pos: p.pos,
                kind: p.block_entity_type,
                nbt,
            });
        }
        ClientboundGamePacket::ForgetLevelChunk(p) => {
            let _ = event_tx.try_send(NetworkEvent::ChunkUnloaded { pos: p.pos });
        }
        ClientboundGamePacket::SetChunkCacheCenter(p) => {
            let _ = event_tx.try_send(NetworkEvent::ChunkCacheCenter { x: p.x, z: p.z });
        }
        ClientboundGamePacket::PlayerPosition(p) => {
            sender.send(ServerboundGamePacket::AcceptTeleportation(
                azalea_protocol::packets::game::s_accept_teleportation::ServerboundAcceptTeleportation {
                    id: p.id,
                },
            ));
            let _ = event_tx.try_send(NetworkEvent::PlayerPosition {
                change: p.change.clone(),
                relative: p.relative.clone(),
            });
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
            send_chat(event_tx, format_text_spans(&p.content));
        }
        ClientboundGamePacket::PlayerChat(p) => {
            send_chat(event_tx, format_text_spans(&p.message()));
        }
        ClientboundGamePacket::DisguisedChat(p) => {
            send_chat(event_tx, format_text_spans(&p.message));
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
            match p.event {
                EventType::ChangeGameMode => {
                    let _ = event_tx.try_send(NetworkEvent::GameModeChanged {
                        game_mode: p.param as u8,
                    });
                }
                EventType::StartRaining
                | EventType::StopRaining
                | EventType::RainLevelChange
                | EventType::ThunderLevelChange => {
                    let _ = event_tx.try_send(NetworkEvent::WeatherUpdate {
                        event: p.event,
                        param: p.param,
                    });
                }
                _ => {}
            }
        }
        ClientboundGamePacket::Disconnect(p) => {
            tracing::warn!("Disconnected: {}", p.reason);
            let _ = event_tx.try_send(NetworkEvent::Disconnected {
                reason: format!("{}", p.reason),
            });
        }
        ClientboundGamePacket::AddEntity(p) => {
            let y_rot_deg = (p.y_rot as f32) * 360.0 / 256.0;
            let x_rot_deg = (p.x_rot as f32) * 360.0 / 256.0;
            let head_y_rot_deg = (p.y_head_rot as f32) * 360.0 / 256.0;
            let velocity = p.movement.to_vec3();
            let _ = event_tx.try_send(NetworkEvent::EntitySpawned {
                id: p.id.0,
                entity_type: p.entity_type,
                position: p.position.into(),
                velocity: velocity.into(),
                y_rot_deg,
                x_rot_deg,
                head_y_rot_deg,
            });
        }
        ClientboundGamePacket::DamageEvent(p) => {
            let _ = event_tx.try_send(NetworkEvent::EntityDamaged { id: p.entity_id.0 });
        }
        ClientboundGamePacket::RotateHead(p) => {
            let head_y_rot_deg = (p.y_head_rot as f32) * 360.0 / 256.0;
            let _ = event_tx.try_send(NetworkEvent::EntityHeadRotation {
                id: p.entity_id.0,
                head_y_rot_deg,
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
                y_rot_deg: look.y_rot(),
                x_rot_deg: look.x_rot(),
            });
        }
        ClientboundGamePacket::TeleportEntity(p) => {
            let _ = event_tx.try_send(NetworkEvent::EntityTeleported {
                id: p.id.0,
                position: p.change.pos.into(),
                y_rot_deg: p.change.look_direction.y_rot(),
                x_rot_deg: p.change.look_direction.x_rot(),
            });
        }
        ClientboundGamePacket::EntityPositionSync(p) => {
            let _ = event_tx.try_send(NetworkEvent::EntityTeleported {
                id: p.id.0,
                position: p.values.pos.into(),
                y_rot_deg: p.values.look_direction.y_rot(),
                x_rot_deg: p.values.look_direction.x_rot(),
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
                // Index 6 = entity pose
                if item.index == 6
                    && let azalea_entity::EntityDataValue::Pose(pose) = &item.value
                {
                    let _ = event_tx.try_send(NetworkEvent::EntityPose {
                        id: p.id.0,
                        is_crouching: matches!(pose, azalea_entity::Pose::Crouching),
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
                // Index 17 = sheep wool/sheared byte (low nibble = DyeColor, bit 4 = sheared).
                // Emit unconditionally; consumer filters by entity type.
                if item.index == 17
                    && let azalea_entity::EntityDataValue::Byte(packed) = &item.value
                {
                    let _ = event_tx.try_send(NetworkEvent::SheepWoolData {
                        id: p.id.0,
                        color: *packed & 0x0F,
                        sheared: (*packed & 0x10) != 0,
                    });
                }
                // Index 2 = custom name (Optional<Component>); needed for jeb_ sheep detection.
                if item.index == 2
                    && let azalea_entity::EntityDataValue::OptionalFormattedText(opt) = &item.value
                {
                    let name = opt.as_ref().map(|c| c.to_string());
                    let _ = event_tx.try_send(NetworkEvent::EntityCustomName { id: p.id.0, name });
                }
                // Index 18 on cows = CowVariant Holder.
                if item.index == 18
                    && let azalea_entity::EntityDataValue::CowVariant(variant) = &item.value
                {
                    use azalea_registry::DataRegistry;
                    let resolved = registry_holder
                        .protocol_id_to_identifier(
                            azalea_registry::identifier::Identifier::new("minecraft:cow_variant"),
                            variant.protocol_id(),
                        )
                        .map(|id| match id.path() {
                            "temperate" => 0u8,
                            "cold" => 1,
                            "warm" => 2,
                            _ => 0,
                        })
                        .unwrap_or(0);
                    let _ = event_tx.try_send(NetworkEvent::CowVariant {
                        id: p.id.0,
                        variant: resolved,
                    });
                }
            }
        }
        // Event id 10 = sheep eat-grass animation start (40-tick head-dip).
        ClientboundGamePacket::EntityEvent(p) if p.event_id == 10 => {
            let _ = event_tx.try_send(NetworkEvent::SheepEatStart { id: p.entity_id.0 });
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
        ClientboundGamePacket::CommandSuggestions(p) => {
            tracing::debug!("Command suggestions received (id {})", p.id);
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

fn send_chat(event_tx: &Sender<NetworkEvent>, spans: Vec<TextSpan>) {
    let text: String = spans.iter().map(|s| s.text.as_str()).collect();
    tracing::info!("Chat: {text}");
    let _ = event_tx.try_send(NetworkEvent::ChatMessage { spans });
}

/// Resolves a sound holder into either a `sounds.json` event name (registry
/// reference) or a direct sound-file path (inline custom sound).
fn resolve_sound(
    holder: &azalea_registry::Holder<
        azalea_registry::builtin::SoundEvent,
        azalea_core::sound::CustomSound,
    >,
) -> crate::audio::SoundRef {
    match holder {
        azalea_registry::Holder::Reference(event) => {
            // `to_str` yields e.g. `minecraft:block.stone.break`; sounds.json is
            // keyed by the path without the namespace.
            let id = event.to_str();
            let name = id.strip_prefix("minecraft:").unwrap_or(id);
            crate::audio::SoundRef::Event(name.to_string())
        }
        azalea_registry::Holder::Direct(custom) => {
            crate::audio::SoundRef::Direct(custom.sound_id.to_string())
        }
    }
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
