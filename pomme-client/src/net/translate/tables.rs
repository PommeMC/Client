use super::*;

impl Translation {
    /// The translation for one protocol number, or `None` outside
    /// `TRANSLATED`: the frame rewrites below are version-specific, so
    /// embedded data alone isn't enough (and the native version needs none).
    pub(crate) fn for_protocol(protocol: i32) -> Option<Translation> {
        if !TRANSLATED.contains(&protocol) {
            return None;
        }
        let table = PacketTable::for_protocol(protocol)?;
        let native = PacketTable::native();
        let id = |phase, name| required_id(native, phase, Direction::Clientbound, name);
        let mut translation = Translation {
            rewrites: Box::new([]),
            registry_data_id: id(Phase::Configuration, "registry_data"),
            to_native: RegistryRemaps::to_native(protocol)?,
            from_native: RegistryRemaps::from_native(protocol)?,
            // Login-phase ids are identical across all supported versions.
            login_finished_id: id(Phase::Login, "login_finished"),
            login_hello_id: id(Phase::Login, "hello"),
            // 1.20.5 appended strictErrorHandling to game_profile; 1.21.2
            // replaced it with the session-id UUID 26.2 still carries.
            login_profile_strict: matches!(protocol, 766 | 767),
            login_hello_bare: protocol <= 765,
            login_hello_optional_uuid: (protocol <= 763)
                .then(|| required_id(native, Phase::Login, Direction::Serverbound, "hello")),
            login_session_pad: protocol <= 775,
            v775: (protocol <= 775).then(|| Ids775 {
                game_login_id: id(Phase::Game, "login"),
                set_player_team_id: id(Phase::Game, "set_player_team"),
            }),
            update_attributes_id: id(Phase::Game, "update_attributes"),
            recipe_book_add_id: id(Phase::Game, "recipe_book_add"),
            update_recipes_id: id(Phase::Game, "update_recipes"),
            place_ghost_recipe_id: id(Phase::Game, "place_ghost_recipe"),
            recipe_layout: (protocol >= 768).then_some(RecipeLayout {
                old_profile: protocol <= 772,
                item_stack_count_first: protocol <= 774,
                trim_pattern_display: protocol <= 769,
            }),
            game_ids: GameIds::build(protocol, table, native),
            config_ids: ConfigIds::build(protocol, table, native),
            no_config_phase: table
                .name_of(Phase::Configuration, Direction::Clientbound, 0)
                .is_none(),
        };
        translation.rewrites = translation.build_rewrites();
        Some(translation)
    }

    /// Resolves rewrite precedence once, indexed by native clientbound id.
    /// First registration wins: older rewrites already include newer layouts.
    pub(super) fn build_rewrites(&self) -> Box<[Option<Rewrite>]> {
        let ids = self.game_ids.as_ref();
        let v775 = self.v775.as_ref();
        let v777 = ids.and_then(|g| g.v777.as_ref());
        let v767 = ids.and_then(|g| g.v767.as_ref());
        let v765 = ids.and_then(|g| g.v765.as_ref());
        let v764 = ids.and_then(|g| g.v764.as_ref());
        let mut rewrites = vec![
            None;
            (0..)
                .take_while(|&id| PacketTable::native()
                    .name_of(Phase::Game, Direction::Clientbound, id)
                    .is_some())
                .count()
        ];
        for (id, slot) in rewrites.iter_mut().enumerate() {
            let id = id as u32;
            if v775.is_some_and(|v| id == v.game_login_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_login_775));
            }
            if id == self.update_attributes_id {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_attributes));
            }
            if v775.is_some_and(|v| id == v.set_player_team_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_team_775));
            }
            if v777.is_none()
                && self.recipe_layout.is_some()
                && self.recipe_translator(id).is_some()
            {
                slot.get_or_insert(Rewrite::Recipe(self.recipe_translator(id).unwrap()));
            }
            let Some(ids) = ids else { continue };
            if id == ids.set_entity_data_id {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_entity_data));
            }
            if ids.items.as_ref().is_some_and(|v| v.rewrite(id).is_some()) {
                slot.get_or_insert(Rewrite::Item(
                    ids.items.as_ref().unwrap().rewrite(id).unwrap(),
                ));
            }
            if ids.v774.as_ref().is_some_and(|v| id == v.level_chunk_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_chunk_774));
            }
            if ids.v774.as_ref().is_some_and(|v| id == v.set_time_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_time_774));
            }
            if v764.is_some_and(|v| id == v.set_score_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_score_764));
            }
            if v765.is_some_and(|v| id == v.container_set_content_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_container_content_765));
            }
            if v765.is_some_and(|v| id == v.set_equipment_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_equipment_765));
            }
            if v765.is_some_and(|v| id == v.merchant_offers_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_offers_765));
            }
            if v765.is_some_and(|v| id == v.container_set_slot_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_container_slot_765));
            }
            if ids.v772.as_ref().is_some_and(|v| id == v.explode_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_explode_772));
            }
            if v777.is_some_and(|v| id == v.explode_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_explode_777));
            }
            if v777.is_some_and(|v| id == v.level_particles_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_particles_777));
            }
            if v777.is_some_and(|v| id == v.update_advancements_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_advancements_777));
            }
            if id == ids.commands_id {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_commands));
            }
            if v777.is_some_and(|v| id == v.recipe_book_add_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_recipe_add_777));
            }
            if v777.is_some_and(|v| id == v.place_ghost_recipe_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_ghost_recipe_777));
            }
            if v777.is_some_and(|v| id == v.update_recipes_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_recipes_777));
            }
            if ids.v763.as_ref().is_some_and(|v| id == v.respawn_id) {
                slot.get_or_insert(Rewrite::SessionFrame(translate_respawn_763));
            }
            if v765.is_some_and(|v| id == v.respawn_id) {
                slot.get_or_insert(Rewrite::SessionFrame(translate_respawn_765));
            }
            if ids.version_rewrite(id).is_some() {
                slot.get_or_insert(Rewrite::Frame(ids.version_rewrite(id).unwrap()));
            }
            if v767.is_some_and(|v| id == v.teleport_entity_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_teleport_767));
            }
            if v767.is_some_and(|v| id == v.container_set_slot_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_container_slot_767));
            }
            if v767.is_some_and(|v| id == v.cooldown_id) {
                slot.get_or_insert(Rewrite::Context(Self::rewrite_cooldown_767));
            }
        }
        rewrites.into_boxed_slice()
    }
}

impl GameIds {
    /// The version-gated frame rewrite dispatching on `id`, if any. Chained
    /// oldest-first: every rewriter targets 26.2 directly, so where two gates
    /// claim a packet the older one subsumes the newer and must win (765's
    /// respawn over 767's, say). The gate above native comes last; it never
    /// shares a packet with the older ones.
    pub(super) fn version_rewrite(&self, id: u32) -> Option<FrameRewrite> {
        self.v763
            .as_ref()
            .and_then(|v| v.rewrite(id))
            .or_else(|| self.v764.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v765.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v766.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v767.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v768.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v769.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v772.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v777.as_ref().and_then(|v| v.rewrite(id)))
    }

    /// Name-matched game-phase id tables between one wire version and the
    /// native. `None` when translation-by-id is a no-op: every inbound id
    /// maps to itself and every outbound id maps to itself or to nothing
    /// (26.1's only divergence is 26.2's `spectate_entity` ->
    /// `spectator_action` rename, which pomme never sends).
    pub(super) fn build(
        protocol: i32,
        table: &PacketTable,
        native: &PacketTable,
    ) -> Option<GameIds> {
        use Direction::{Clientbound, Serverbound};
        let inbound = id_map(table, native, Phase::Game, Clientbound);
        let outbound = id_map(native, table, Phase::Game, Serverbound);
        // An item grammar still has stacks to translate (26.1).
        if identity_maps(&inbound, &outbound) && ItemCodec::for_protocol(protocol).is_none() {
            return None;
        }
        let id = |dir, name| required_id(native, Phase::Game, dir, name);
        Some(GameIds {
            inbound,
            outbound,
            serializer_map: pomme_protocol::entity_serializers::SerializerTable::for_protocol(
                protocol,
            )
            .expect("embedded serializer table"),
            set_entity_data_id: id(Clientbound, "set_entity_data"),
            commands_id: id(Clientbound, "commands"),
            wire_registries: RegistryTable::for_protocol(protocol).expect("wire registries"),
            v774: (protocol <= 774).then(|| Ids774 {
                level_chunk_id: id(Clientbound, "level_chunk_with_light"),
                set_time_id: id(Clientbound, "set_time"),
                attack_id: id(Serverbound, "attack"),
                interact_id: id(Serverbound, "interact"),
                interact_old_id: required_id(table, Phase::Game, Serverbound, "interact"),
            }),
            color_particles: protocol >= 766,
            v777: (protocol >= 777).then(|| Ids777 {
                move_entity_pos_id: id(Clientbound, "move_entity_pos"),
                move_entity_pos_rot_id: id(Clientbound, "move_entity_pos_rot"),
                move_entity_rot_id: id(Clientbound, "move_entity_rot"),
                entity_position_sync_id: id(Clientbound, "entity_position_sync"),
                explode_id: id(Clientbound, "explode"),
                login_id: id(Clientbound, "login"),
                respawn_id: id(Clientbound, "respawn"),
                animate_id: id(Clientbound, "animate"),
                level_particles_id: id(Clientbound, "level_particles"),
                update_advancements_id: id(Clientbound, "update_advancements"),
                level_chunk_id: id(Clientbound, "level_chunk_with_light"),
                light_update_id: id(Clientbound, "light_update"),
                player_chat_id: id(Clientbound, "player_chat"),
                recipe_book_add_id: id(Clientbound, "recipe_book_add"),
                place_ghost_recipe_id: id(Clientbound, "place_ghost_recipe"),
                update_recipes_id: id(Clientbound, "update_recipes"),
                swing_animation_old_id: required_id(
                    table,
                    Phase::Game,
                    Clientbound,
                    "swing_animation",
                ),
                swing_id: id(Serverbound, "swing"),
                punch_old_id: required_id(table, Phase::Game, Serverbound, "punch"),
                player_action_id: id(Serverbound, "player_action"),
                player_action_old_id: required_id(table, Phase::Game, Serverbound, "player_action"),
                accept_teleportation_id: id(Serverbound, "accept_teleportation"),
                accept_teleportation_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "accept_teleportation",
                ),
                move_player_pos_rot_id: id(Serverbound, "move_player_pos_rot"),
            }),
            items: ItemCodec::for_protocol(protocol).map(|codec| ItemIds {
                codec,
                container_set_content_id: id(Clientbound, "container_set_content"),
                container_set_slot_id: id(Clientbound, "container_set_slot"),
                set_cursor_item_id: id(Clientbound, "set_cursor_item"),
                set_player_inventory_id: id(Clientbound, "set_player_inventory"),
                set_equipment_id: id(Clientbound, "set_equipment"),
                merchant_offers_id: id(Clientbound, "merchant_offers"),
                set_creative_mode_slot_id: id(Serverbound, "set_creative_mode_slot"),
            }),
            v772: (protocol <= 772).then(|| Ids772 {
                add_entity_id: id(Clientbound, "add_entity"),
                set_entity_motion_id: id(Clientbound, "set_entity_motion"),
                player_rotation_id: id(Clientbound, "player_rotation"),
                set_default_spawn_id: id(Clientbound, "set_default_spawn_position"),
                explode_id: id(Clientbound, "explode"),
            }),
            v770: (protocol <= 770).then(|| Ids770 {
                chunks_biomes_id: id(Clientbound, "chunks_biomes"),
                player_command_id: id(Serverbound, "player_command"),
                player_command_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "player_command",
                ),
            }),
            v769: (protocol <= 769).then(|| Ids769 {
                player_chat_id: id(Clientbound, "player_chat"),
                update_advancements_id: id(Clientbound, "update_advancements"),
                chat_id: id(Serverbound, "chat"),
                chat_old_id: required_id(table, Phase::Game, Serverbound, "chat"),
                chat_command_signed_id: id(Serverbound, "chat_command_signed"),
                chat_command_signed_old_id: table.id(
                    Phase::Game,
                    Serverbound,
                    "chat_command_signed",
                ),
                container_click_id: id(Serverbound, "container_click"),
                container_click_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "container_click",
                ),
            }),
            v768: (protocol <= 768).then(|| Ids768 {
                level_particles_id: id(Clientbound, "level_particles"),
            }),
            v767: (protocol <= 767).then(|| Ids767 {
                player_position_id: id(Clientbound, "player_position"),
                teleport_entity_id: id(Clientbound, "teleport_entity"),
                entity_position_sync_id: id(Clientbound, "entity_position_sync"),
                respawn_id: id(Clientbound, "respawn"),
                container_set_slot_id: id(Clientbound, "container_set_slot"),
                cooldown_id: id(Clientbound, "cooldown"),
                set_cursor_item_id: id(Clientbound, "set_cursor_item"),
                set_player_inventory_id: id(Clientbound, "set_player_inventory"),
                drops: [
                    id(Clientbound, "explode"),
                    id(Clientbound, "update_recipes"),
                    id(Clientbound, "place_ghost_recipe"),
                ],
                player_input_id: id(Serverbound, "player_input"),
                player_input_old_id: required_id(table, Phase::Game, Serverbound, "player_input"),
                move_player_ids: [
                    "move_player_pos",
                    "move_player_pos_rot",
                    "move_player_rot",
                    "move_player_status_only",
                ]
                .map(|n| id(Serverbound, n)),
                client_information_id: id(Serverbound, "client_information"),
            }),
            v766: (protocol <= 766).then(|| Ids766 {
                projectile_power_id: id(Clientbound, "projectile_power"),
                use_item_id: id(Serverbound, "use_item"),
                use_item_old_id: required_id(table, Phase::Game, Serverbound, "use_item"),
            }),
            v765: (protocol <= 765).then(|| Ids765 {
                container_set_content_id: id(Clientbound, "container_set_content"),
                set_equipment_id: id(Clientbound, "set_equipment"),
                merchant_offers_id: id(Clientbound, "merchant_offers"),
                update_mob_effect_id: id(Clientbound, "update_mob_effect"),
                level_particles_id: id(Clientbound, "level_particles"),
                container_set_slot_id: id(Clientbound, "container_set_slot"),
                respawn_id: id(Clientbound, "respawn"),
                update_advancements_id: id(Clientbound, "update_advancements"),
                player_chat_id: id(Clientbound, "player_chat"),
                disguised_chat_id: id(Clientbound, "disguised_chat"),
                server_data_id: id(Clientbound, "server_data"),
                registry: RegistryTable::for_protocol(protocol).expect("embedded registry table"),
                container_click_id: id(Serverbound, "container_click"),
                container_click_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "container_click",
                ),
                creative_slot_id: id(Serverbound, "set_creative_mode_slot"),
                creative_slot_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "set_creative_mode_slot",
                ),
                chat_command_id: id(Serverbound, "chat_command"),
                chat_command_signed_id: id(Serverbound, "chat_command_signed"),
                chat_command_old_id: required_id(table, Phase::Game, Serverbound, "chat_command"),
            }),
            v764: (protocol <= 764).then(|| Ids764 {
                system_chat_id: id(Clientbound, "system_chat"),
                set_action_bar_id: id(Clientbound, "set_action_bar_text"),
                set_title_id: id(Clientbound, "set_title_text"),
                set_subtitle_id: id(Clientbound, "set_subtitle_text"),
                tab_list_id: id(Clientbound, "tab_list"),
                disconnect_id: id(Clientbound, "disconnect"),
                open_screen_id: id(Clientbound, "open_screen"),
                combat_kill_id: id(Clientbound, "player_combat_kill"),
                boss_event_id: id(Clientbound, "boss_event"),
                set_player_team_id: id(Clientbound, "set_player_team"),
                player_chat_id: id(Clientbound, "player_chat"),
                disguised_chat_id: id(Clientbound, "disguised_chat"),
                player_info_id: id(Clientbound, "player_info_update"),
                command_suggestions_id: id(Clientbound, "command_suggestions"),
                set_objective_id: id(Clientbound, "set_objective"),
                set_score_id: id(Clientbound, "set_score"),
                reset_score_id: id(Clientbound, "reset_score"),
                resource_pack_push_id: id(Clientbound, "resource_pack_push"),
                drops: [
                    id(Clientbound, "server_data"),
                    id(Clientbound, "map_item_data"),
                ],
                resource_pack_response_id: id(Serverbound, "resource_pack"),
                resource_pack_response_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "resource_pack",
                ),
            }),
            v763: (protocol <= 763).then(|| Ids763 {
                respawn_id: id(Clientbound, "respawn"),
                forget_level_chunk_id: id(Clientbound, "forget_level_chunk"),
                block_entity_data_id: id(Clientbound, "block_entity_data"),
                tag_query_id: id(Clientbound, "tag_query"),
                add_player_old_id: required_id(table, Phase::Game, Clientbound, "add_player"),
                add_entity_old_id: required_id(table, Phase::Game, Clientbound, "add_entity"),
                player_entity_type: RegistryTable::for_protocol(protocol)
                    .and_then(|r| {
                        r.names(ClientRegistry::EntityType)
                            .iter()
                            .position(|n| n == "player")
                    })
                    .expect("player entity type") as u32,
            }),
            quiet_suppressed: [
                "client_tick_end",
                "player_loaded",
                "chunk_batch_received",
                "pick_item_from_block",
                "pick_item_from_entity",
                "bundle_item_selected",
            ]
            .iter()
            .filter(|n| table.id(Phase::Game, Serverbound, n).is_none())
            .map(|n| required_id(native, Phase::Game, Serverbound, n))
            .collect(),
        })
    }
}
