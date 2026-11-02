use super::*;

impl Translation {
    /// Rewrites a native-layout serverbound login frame into the wire
    /// version's. Only 763's `hello` differs, by wrapping the profile id in an
    /// optional.
    pub fn translate_outbound_login_frame(&self, frame: Vec<u8>) -> Vec<u8> {
        let Some(hello_id) = self.login_hello_optional_uuid else {
            return frame;
        };
        let mut pos = 0;
        if wire::read_varint(&frame, &mut pos) != Some(hello_id) {
            return frame;
        }
        let mut cur = Cursor::new(&frame[pos..]);
        if skip_utf(&mut cur).is_none() {
            return frame;
        }
        let uuid_at = pos + cur.position() as usize;
        let mut out = Vec::with_capacity(frame.len() + 1);
        out.extend_from_slice(&frame[..uuid_at]);
        out.push(1); // the profile id is present
        out.extend_from_slice(&frame[uuid_at..]);
        out
    }

    /// Translates a native-layout serverbound configuration frame into the
    /// wire version's; `None` suppresses it. 764's resource_pack reply and
    /// 767's client_information change layout; everything else is an id
    /// remap.
    pub fn translate_outbound_config_frame(&self, frame: Vec<u8>) -> Option<Vec<u8>> {
        let Some(ids) = &self.config_ids else {
            return Some(frame);
        };
        let mut pos = 0;
        let id = wire::read_varint(&frame, &mut pos)?;
        let old = ids.outbound.get(id as usize).copied().flatten();
        if ids
            .v764
            .as_ref()
            .is_some_and(|v| id == v.resource_pack_response_id)
        {
            return translate_resource_pack_response_764(old?, &frame[pos..]).pop();
        }
        if ids.v767_client_information_id == Some(id) {
            return strip_last_byte(old?, &frame[pos..]).pop();
        }
        match old {
            Some(old) if old == id => Some(frame),
            Some(old) => {
                let mut out = Vec::with_capacity(frame.len() + 1);
                wire::write_varint(&mut out, old);
                out.extend_from_slice(&frame[pos..]);
                Some(out)
            }
            None => {
                tracing::debug!("Suppressing outbound config packet {id} the wire version lacks");
                None
            }
        }
    }

    /// Whether the wire version reports use swings: 26.3 replaced `swing`
    /// with `punch`, sent only for attacks (`Minecraft.startAttack`); the
    /// server swings on handling a use.
    pub fn reports_use_swings(&self) -> bool {
        self.game_ids.as_ref().is_none_or(|g| g.v777.is_none())
    }

    /// Whether outbound game frames need translation before hitting the
    /// wire (the version's serverbound ids or layouts diverge from native).
    pub fn translates_outbound(&self) -> bool {
        self.game_ids.is_some()
    }

    /// Translates a native-layout serverbound game frame into the wire
    /// version's: id remap, `attack`/`interact` layout rewrites, and
    /// suppression of packets the older version lacks. Returns the frames to
    /// send (empty = suppressed, two for `interact`, one otherwise).
    pub(super) fn translate_outbound_game_frame_with_state(
        &self,
        mut frame: Vec<u8>,
        state: &mut SessionState,
    ) -> Vec<Vec<u8>> {
        let Some(ids) = &self.game_ids else {
            return vec![frame];
        };
        let mut pos = 0;
        let Some(id) = wire::read_varint(&frame, &mut pos) else {
            return Vec::new();
        };
        let old = ids.outbound.get(id as usize).copied().flatten();
        if let (Some(items), Some(old)) = (&ids.items, old)
            && id == items.set_creative_mode_slot_id
        {
            return translate_creative_slot(old, &frame[pos..], items.codec, self.from_native);
        }
        if let Some(v777) = &ids.v777 {
            if id == v777.swing_id {
                return translate_swing_777(v777.punch_old_id, &frame[pos..]);
            }
            if id == v777.player_action_id {
                return translate_player_action_777(v777.player_action_old_id, &frame[pos..]);
            }
            if id == v777.accept_teleportation_id {
                return translate_accept_teleportation_777(state, &frame[pos..]);
            }
            if id == v777.move_player_pos_rot_id
                && let Some(frames) = translate_move_player_pos_rot_777(v777, &frame[pos..], state)
            {
                return frames;
            }
        }
        if let Some(v774) = &ids.v774 {
            if id == v774.attack_id {
                return translate_attack(v774.interact_old_id, &frame[pos..]);
            }
            if id == v774.interact_id {
                return translate_interact(v774.interact_old_id, &frame[pos..]);
            }
        }
        if let Some(v770) = &ids.v770
            && id == v770.player_command_id
        {
            return translate_player_command(v770.player_command_old_id, &frame[pos..]);
        }
        if let Some(v764) = &ids.v764
            && id == v764.resource_pack_response_id
        {
            return translate_resource_pack_response_764(
                v764.resource_pack_response_old_id,
                &frame[pos..],
            );
        }
        // The 765 arms precede 769's: both rewrite container_click, and the
        // older item form wins on the older wire.
        if let Some(v765) = &ids.v765 {
            if id == v765.container_click_id {
                return translate_container_click_765(v765.container_click_old_id, &frame[pos..]);
            }
            if id == v765.creative_slot_id {
                return translate_creative_slot_765(v765.creative_slot_old_id, &frame[pos..]);
            }
            if id == v765.chat_command_signed_id {
                return strip_last_byte(v765.chat_command_old_id, &frame[pos..]);
            }
            if id == v765.chat_command_id {
                return translate_chat_command_765(v765.chat_command_old_id, &frame[pos..]);
            }
        }
        if let Some(v769) = &ids.v769 {
            if id == v769.chat_id {
                return strip_last_byte(v769.chat_old_id, &frame[pos..]);
            }
            if id == v769.chat_command_signed_id
                && let Some(old_id) = v769.chat_command_signed_old_id
            {
                return strip_last_byte(old_id, &frame[pos..]);
            }
            if id == v769.container_click_id {
                return translate_container_click(v769.container_click_old_id, &frame[pos..]);
            }
        }
        if let Some(v766) = &ids.v766
            && id == v766.use_item_id
        {
            return translate_use_item(v766.use_item_old_id, &frame[pos..]);
        }
        if let Some(v767) = &ids.v767 {
            if id == v767.player_input_id {
                return translate_player_input(v767.player_input_old_id, &frame[pos..]);
            }
            if id == v767.client_information_id
                && let Some(old) = old
            {
                return strip_last_byte(old, &frame[pos..]);
            }
            if v767.move_player_ids.contains(&id)
                && let Some(flags) = frame.last_mut()
            {
                // 1.21.2 turned the trailing onGround bool into a flag
                // bitfield; a 1.21.1 server reads any nonzero byte as
                // onGround, so the horizontal-collision bit must go.
                *flags &= 1;
            }
        }
        match old {
            Some(old) if old == id => vec![frame],
            Some(old) => {
                let mut out = Vec::with_capacity(frame.len() + 1);
                wire::write_varint(&mut out, old);
                out.extend_from_slice(&frame[pos..]);
                vec![out]
            }
            None => {
                if ids.quiet_suppressed.contains(&id) {
                    tracing::debug!("Suppressing outbound game packet {id} the wire version lacks");
                } else {
                    tracing::warn!("Suppressing outbound game packet {id} the wire version lacks");
                }
                Vec::new()
            }
        }
    }

    /// Remaps an outbound packet's static-registry ids into the launched
    /// version's id space. Never drops the packet; entries the older version
    /// lacks degrade to empty (the server resyncs the slot).
    /// TODO: delimit `set_creative_mode_slot` component values for 1.21.5
    /// through 1.21.11, which have no item grammar yet (see the
    /// azalea-divergence list).
    pub fn remap_outbound(&self, packet: &mut ServerboundGamePacket) {
        match packet {
            ServerboundGamePacket::ContainerClick(p) => {
                for (_, stack) in p.changed_slots.iter_mut() {
                    self.remap_hashed(stack);
                }
                self.remap_hashed(&mut p.carried_item);
            }
            ServerboundGamePacket::SetCreativeModeSlot(p) => {
                remap_stack(self.from_native, &mut p.item_stack);
                if let ItemStack::Present(data) = &mut p.item_stack {
                    strip_untranslatable_components(self.from_native, data);
                }
            }
            _ => {}
        }
    }

    pub(super) fn remap_hashed(&self, stack: &mut HashedStack) {
        use ClientRegistry as R;
        let Some(item) = &mut stack.0 else { return };
        if !remap_with(self.from_native, R::Item, &mut item.kind) {
            stack.0 = None;
            return;
        }
        item.components
            .added_components
            .retain_mut(|(kind, _)| remap_with(self.from_native, R::DataComponentType, kind));
        item.components
            .removed_components
            .retain_mut(|kind| remap_with(self.from_native, R::DataComponentType, kind));
    }
}
