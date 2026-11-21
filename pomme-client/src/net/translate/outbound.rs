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

    /// The wire frame reporting a use swing of `hand`, on a version whose
    /// server doesn't swing by itself on handling a use (before 26.3).
    pub fn use_swing_frame(&self, hand: InteractionHand) -> Option<Vec<u8>> {
        let v776 = self.game_ids.as_ref()?.v776.as_ref()?;
        Some(swing_frame(v776.swing_old_id, hand))
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
    pub(super) fn translate_outbound_game_frame(&self, mut frame: Vec<u8>) -> Vec<Vec<u8>> {
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
        if let Some(lowered) = ids.v776.as_ref().and_then(|v| v.lower(id, &frame[pos..])) {
            return lowered;
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
            if id == v769.creative_slot_id {
                return undelimit_creative_slot(v769.creative_slot_old_id, &frame[pos..]);
            }
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
    pub fn remap_outbound(&self, packet: &mut GamePacket) {
        match packet {
            GamePacket::ContainerClick(click) => {
                for (_, stack) in &mut click.changed_slots {
                    self.remap_hashed(stack);
                }
                self.remap_hashed(&mut click.carried);
            }
            // A wire version with an item grammar translates the frame.
            GamePacket::SetCreativeModeSlot { stack, .. } if !self.translates_items() => {
                self.remap_creative_stack(stack);
            }
            _ => {}
        }
    }

    pub(super) fn remap_hashed(&self, stack: &mut Option<HashedStack>) {
        let component = |id: u32| {
            self.from_native
                .remap(ClientRegistry::DataComponentType, id)
        };
        *stack = stack.take().and_then(|mut stack| {
            stack.item = self.from_native.remap(ClientRegistry::Item, stack.item)?;
            stack.added = stack
                .added
                .iter()
                .filter_map(|&(id, hash)| Some((component(id)?, hash)))
                .collect();
            stack.removed = stack
                .removed
                .iter()
                .filter_map(|&id| component(id))
                .collect();
            Some(stack)
        });
    }

    /// A creative stack for a wire version without an item grammar: its
    /// item remapped, or the stack emptied when the version lacks the item.
    /// Component values can't be rewritten without the grammar, so a patch
    /// touching a component the version numbers differently is cleared
    /// rather than sent misencoded.
    fn remap_creative_stack(&self, stack: &mut Option<pomme_protocol::item::ItemStack>) {
        *stack = stack.take().and_then(|mut stack| {
            stack.item = self.from_native.remap(ClientRegistry::Item, stack.item)?;
            let translates = |id: u32| {
                self.from_native
                    .remap(ClientRegistry::DataComponentType, id)
                    == Some(id)
            };
            let added = stack.patch.added.iter().map(|(id, _)| *id);
            if !added
                .chain(stack.patch.removed.iter().copied())
                .all(translates)
            {
                tracing::warn!(
                    "Dropping creative item components the wire version numbers differently"
                );
                stack.patch = Default::default();
            }
            Some(stack)
        });
    }
}
