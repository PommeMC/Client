use super::*;

impl Translation {
    /// Rewrites the login 775 payload into the native layout.
    pub(super) fn rewrite_login_775(
        &self,
        id: u32,
        payload: &[u8],
        state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let v767 = self.game_ids.as_ref().and_then(|g| g.v767.as_ref());
        let v765 = self.game_ids.as_ref().and_then(|g| g.v765.as_ref());

        let v763 = self.game_ids.as_ref().and_then(|g| g.v763.as_ref());
        let old = if v763.is_some() {
            translate_game_login_763(payload)
                .as_deref()
                .and_then(|payload| translate_game_login_765(payload, state))
        } else if v765.is_some() {
            translate_game_login_765(payload, state)
        } else {
            Some(payload.to_vec())
        };
        old.and_then(|p| {
            if v767.is_some() {
                insert_sea_level(&p).and_then(|p| translate_game_login(id, &p))
            } else {
                translate_game_login(id, &p)
            }
        })
    }

    /// Rewrites the attributes payload into the native layout.
    pub(super) fn rewrite_attributes(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let v766 = self.game_ids.as_ref().and_then(|g| g.v766.as_ref());
        let v765 = self.game_ids.as_ref().and_then(|g| g.v765.as_ref());

        // Oldest gate first: 766 and below carry UUID modifier ids the
        // shared rewrite would copy through as a resource location, and
        // 765 names the attribute by resource location rather than id.
        if v766.is_some() {
            translate_update_attributes_uuid(v765.map(|v| v.registry), self.to_native, id, payload)
        } else {
            translate_update_attributes(self.to_native, id, payload)
        }
    }

    /// Rewrites the team 775 payload into the native layout.
    pub(super) fn rewrite_team_775(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let v769 = self.game_ids.as_ref().is_some_and(|g| g.v769.is_some());

        translate_team(id, payload, v769)
    }

    /// Rewrites the entity data payload into the native layout.
    pub(super) fn rewrite_entity_data(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;

        translate_entity_data(id, payload, ids, self.to_native)
    }

    /// Rewrites the chunk 774 payload into the native layout.
    pub(super) fn rewrite_chunk_774(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;
        let v769 = self.game_ids.as_ref().is_some_and(|g| g.v769.is_some());
        let named_nbt = ids.v763.is_some();

        let padding = if named_nbt {
            SectionPadding::EntrySized
        } else if ids.v770.is_some() && !v769 {
            SectionPadding::UnwrittenLength
        } else {
            SectionPadding::None
        };
        translate_chunk(id, payload, v769, named_nbt, padding)
    }

    /// Rewrites the time 774 payload into the native layout.
    pub(super) fn rewrite_time_774(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let v767 = self.game_ids.as_ref().and_then(|g| g.v767.as_ref());

        if v767.is_some() {
            translate_set_time_767(id, payload)
        } else {
            translate_set_time(id, payload)
        }
    }

    /// Rewrites the score 764 payload into the native layout.
    pub(super) fn rewrite_score_764(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let v764 = self.game_ids.as_ref().and_then(|g| g.v764.as_ref());
        let v = v764.filter(|v| id == v.set_score_id)?;

        translate_set_score_764(v, payload)
    }

    /// Rewrites the container content 765 payload into the native layout.
    pub(super) fn rewrite_container_content_765(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;
        let named_nbt = ids.v763.is_some();

        translate_container_set_content_765(id, payload, named_nbt)
    }

    /// Rewrites the equipment 765 payload into the native layout.
    pub(super) fn rewrite_equipment_765(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;
        let named_nbt = ids.v763.is_some();

        translate_set_equipment_765(id, payload, named_nbt)
    }

    /// Rewrites the offers 765 payload into the native layout.
    pub(super) fn rewrite_offers_765(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;
        let named_nbt = ids.v763.is_some();

        translate_merchant_offers_765(id, payload, named_nbt)
    }

    /// Rewrites the container slot 765 payload into the native layout.
    pub(super) fn rewrite_container_slot_765(
        &self,
        _id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;
        let v767 = self.game_ids.as_ref().and_then(|g| g.v767.as_ref());
        let named_nbt = ids.v763.is_some();

        v767.and_then(|v| translate_container_set_slot_765(v, payload, named_nbt))
    }

    /// Rewrites the explode 772 payload into the native layout.
    pub(super) fn rewrite_explode_772(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;

        translate_explode(id, payload, ids, self.to_native)
    }

    /// Rewrites the explode 777 payload into the native layout.
    pub(super) fn rewrite_explode_777(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;

        translate_explode_777(id, payload, ids, self.to_native)
    }

    /// Rewrites the particles 777 payload into the native layout.
    pub(super) fn rewrite_particles_777(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;

        translate_level_particles_777(id, payload, ids.wire_registries, self.to_native)
    }

    /// Rewrites the advancements 777 payload into the native layout.
    pub(super) fn rewrite_advancements_777(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        translate_update_advancements_777(id, payload, self.to_native)
    }

    /// Rewrites the commands payload into the native layout.
    pub(super) fn rewrite_commands(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;

        translate_commands(id, payload, ids.wire_registries, self.to_native)
    }

    /// Rewrites the recipe add 777 payload into the native layout.
    pub(super) fn rewrite_recipe_add_777(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;

        translate_recipe_book_add_777(id, payload, ids.wire_registries, self.to_native)
    }

    /// Rewrites the ghost recipe 777 payload into the native layout.
    pub(super) fn rewrite_ghost_recipe_777(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;

        translate_place_ghost_recipe_777(id, payload, ids.wire_registries, self.to_native)
    }

    /// Rewrites the recipes 777 payload into the native layout.
    pub(super) fn rewrite_recipes_777(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let ids = self.game_ids.as_ref()?;

        translate_update_recipes_777(id, payload, ids.wire_registries, self.to_native)
    }

    /// Rewrites the teleport 767 payload into the native layout.
    pub(super) fn rewrite_teleport_767(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let v767 = self.game_ids.as_ref().and_then(|g| g.v767.as_ref());
        let v = v767.filter(|v| id == v.teleport_entity_id)?;

        translate_teleport_entity_767(v, payload)
    }

    /// Rewrites the container slot 767 payload into the native layout.
    pub(super) fn rewrite_container_slot_767(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        let v767 = self.game_ids.as_ref().and_then(|g| g.v767.as_ref());
        let v = v767.filter(|v| id == v.container_set_slot_id)?;

        translate_container_set_slot_767(v, payload)
    }

    /// Rewrites the cooldown 767 payload into the native layout.
    pub(super) fn rewrite_cooldown_767(
        &self,
        id: u32,
        payload: &[u8],
        _state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        translate_cooldown_767(self.to_native, id, payload)
    }

    /// Rewrites a raw login-phase frame into the native layout.
    pub fn translate_login_frame(&self, raw: Box<[u8]>) -> Box<[u8]> {
        let mut cur = Cursor::new(&raw[..]);
        let id = var_u32(&mut cur);
        if self.login_session_pad && id == Some(self.login_finished_id) {
            // 26.2 appended a session-id UUID; zero is fine, pomme only
            // reads the game profile. The 1.20.5/1.21-era trailing
            // strictErrorHandling bool goes first (the UUID took its place).
            let mut out = raw.into_vec();
            if self.login_profile_strict {
                out.pop();
            }
            out.extend_from_slice(&[0; 16]);
            return out.into_boxed_slice();
        }
        if self.login_hello_bare && id == Some(self.login_hello_id) {
            // 1.20.5 appended shouldAuthenticate; an encrypting server on
            // the older wire always authenticates.
            let mut out = raw.into_vec();
            out.push(1);
            return out.into_boxed_slice();
        }
        raw
    }

    /// Whether configuration frames need translation (767 and older).
    pub fn translates_config(&self) -> bool {
        self.config_ids.is_some()
    }

    /// The `enforcesSecureChat` flag in a wire `server_data` frame from before
    /// 1.20.5, its only copy then (see [`read_server_data_765`]).
    pub fn server_data_secure_chat(&self, raw: &[u8]) -> Option<bool> {
        let ids = self.game_ids.as_ref()?;
        let v765 = ids.v765.as_ref()?;
        let mut id_end = 0;
        let wire_id = wire::read_varint(raw, &mut id_end)?;
        if ids.inbound.get(wire_id as usize).copied().flatten()? != v765.server_data_id {
            return None;
        }
        read_server_data_765(&raw[id_end..], ids.v764.is_some()).map(|(_, flag)| flag)
    }

    /// Whether the wire version has no configuration phase at all, so the join
    /// sends no `login_acknowledged` and reads the registries out of the game
    /// `login` packet instead.
    pub fn no_config_phase(&self) -> bool {
        self.no_config_phase
    }

    /// Splits the registry codec out of a 1.20.1 game `login` frame into the
    /// per-registry `registry_data` packets the configuration phase carries
    /// from 1.20.2 on, capturing the dimension-type order on the way.
    pub(super) fn split_login_registries_with_state(
        &self,
        raw: &[u8],
        state: &mut SessionState,
    ) -> Option<Vec<Box<[u8]>>> {
        let id = self.registry_data_id;
        let mut pos = 0;
        wire::read_varint(raw, &mut pos)?;
        let mut cur = Cursor::new(&raw[pos..]);
        seek_login_codec_763(&mut cur)?;
        let mut codec = Vec::new();
        copy_unnamed_nbt(&mut cur, &mut codec, true)?;
        split_registry_data(id, &codec, state)
    }

    /// Rewrites a raw configuration-phase frame into native-layout frames;
    /// empty = dropped. 765's single registry_data packet fans out into one
    /// frame per registry.
    pub(super) fn translate_config_frame_with_state(
        &self,
        raw: Box<[u8]>,
        state: &mut SessionState,
    ) -> Vec<Box<[u8]>> {
        let Some(ids) = self.config_ids.as_ref().filter(|ids| !ids.inbound_identity) else {
            return vec![raw];
        };
        let mut pos = 0;
        let Some(wire_id) = wire::read_varint(&raw, &mut pos) else {
            return Vec::new();
        };
        let Some(id) = ids.inbound.get(wire_id as usize).copied().flatten() else {
            tracing::debug!("Dropping inbound config packet {wire_id} with no native id");
            return Vec::new();
        };
        if ids.split_registry_data == Some(id) {
            return match split_registry_data(id, &raw[pos..], state) {
                Some(frames) => frames,
                None => {
                    tracing::warn!("Dropping unparsable registry data");
                    Vec::new()
                }
            };
        }
        if let Some(v) = &ids.v764 {
            let rewritten = if id == v.disconnect_id {
                let mut cur = Cursor::new(&raw[pos..]);
                let mut out = Vec::with_capacity(raw.len() + 8);
                wire::write_varint(&mut out, id);
                transcode_component(&mut cur, &mut out).map(|()| out)
            } else if id == v.resource_pack_push_id {
                translate_resource_pack_764(id, &raw[pos..])
            } else {
                return plain_config_frame(id, &raw[pos..]);
            };
            return match rewritten {
                Some(out) => vec![out.into_boxed_slice()],
                None => {
                    tracing::warn!("Dropping unparsable config packet {id}");
                    Vec::new()
                }
            };
        }
        plain_config_frame(id, &raw[pos..])
    }

    /// Rewrites a raw game-phase frame into the native layout; `None` drops
    /// the packet (malformed beyond repair, or without a native equivalent).
    pub(super) fn translate_game_frame_with_state(
        &self,
        raw: Box<[u8]>,
        state: &mut SessionState,
    ) -> Option<Box<[u8]>> {
        let mut id_end = 0;
        let wire_id = wire::read_varint(&raw, &mut id_end)?;
        // add_player has no native equivalent, so it becomes an add_entity
        // before the id map below would drop it.
        if let Some(v) = self.game_ids.as_ref().and_then(|g| g.v763.as_ref())
            && wire_id == v.add_player_old_id
        {
            let frame = translate_add_player_763(v, &raw[id_end..])?;
            return self.translate_game_frame_with_state(frame.into_boxed_slice(), state);
        }
        if let Some(v) = self.game_ids.as_ref().and_then(|g| g.v777.as_ref())
            && wire_id == v.swing_animation_old_id
        {
            return translate_swing_animation_777(v.animate_id, &raw[id_end..]);
        }
        let id = match &self.game_ids {
            Some(ids) => {
                let Some(native) = ids.inbound.get(wire_id as usize).copied().flatten() else {
                    tracing::debug!("Dropping inbound game packet {wire_id} with no native id");
                    return None;
                };
                native
            }
            None => wire_id,
        };

        let v777 = self.game_ids.as_ref().and_then(|g| g.v777.as_ref());
        if v777.is_some_and(|v| id == v.login_id) {
            // A new session; an accept held over from the last one is stale.
            state.pending_accept = None;
        }
        let v767 = self.game_ids.as_ref().and_then(|g| g.v767.as_ref());
        let v765 = self.game_ids.as_ref().and_then(|g| g.v765.as_ref());
        if v767.is_some_and(|v| v.drops.contains(&id)) {
            tracing::debug!("Dropping game packet {id} with no 1.21.1 equivalent layout");
            return None;
        }
        if v765.is_some_and(|v| id == v.update_advancements_id) {
            tracing::debug!("Dropping update_advancements with old-form icons");
            return None;
        }
        let v770 = self.game_ids.as_ref().and_then(|g| g.v770.as_ref());
        if v770.is_some_and(|v| id == v.chunks_biomes_id) {
            // TODO: translate `chunks_biomes`: copy each biome container like
            // [`translate_chunk`], dropping its length prefix and padding.
            tracing::debug!("Dropping chunks_biomes with pre-1.21.6 biome containers");
            return None;
        }
        let v764 = self.game_ids.as_ref().and_then(|g| g.v764.as_ref());
        if v764.is_some_and(|v| v.drops.contains(&id)) {
            tracing::debug!("Dropping unconsumed component packet {id}");
            return None;
        }
        // The 764 JSON components convert first; the rest of the chain then
        // runs on the (765-form) converted payload.
        let converted = match v764.and_then(|v| v.component_pass(id, &raw[id_end..])) {
            Some(Some(payload)) => Some(payload),
            Some(None) => {
                tracing::warn!("Dropping game packet {id} with an unparsable component");
                return None;
            }
            None => None,
        };
        let payload: &[u8] = converted.as_deref().unwrap_or(&raw[id_end..]);
        let Some(rewrite) = self.rewrites.get(id as usize).copied().flatten() else {
            if id == wire_id && converted.is_none() {
                return Some(raw);
            }
            let mut out = Vec::with_capacity(raw.len() + 1);
            wire::write_varint(&mut out, id);
            out.extend_from_slice(payload);
            return Some(out.into_boxed_slice());
        };
        let rewritten = rewrite.apply(self, id, payload, state);
        match rewritten {
            Some(out) => Some(out.into_boxed_slice()),
            None if v777.is_some_and(|v| id == v.level_particles_id) => {
                tracing::debug!("Dropping level_particles with an unsizable payload");
                None
            }
            None => {
                tracing::warn!("Dropping unparsable game packet {id}");
                None
            }
        }
    }
}
