use super::*;

impl Translation {
    /// The native-version particle id for a source-version one, for the raw
    /// `level_particles` path; `None` drops the particle.
    pub fn remap_particle(&self, id: u32) -> Option<u32> {
        self.to_native.remap(ClientRegistry::ParticleType, id)
    }

    /// The native-version id in `registry` for a source-version one, for the
    /// raw decode paths; `None` drops what carries it.
    pub fn remap_to_native(&self, registry: ClientRegistry, id: u32) -> Option<u32> {
        self.to_native.remap(registry, id)
    }

    /// Whether the item-carrying packets arrive already in native ids (the
    /// wire version's item grammar rewrote them), so their items aren't
    /// remapped again after decoding.
    pub fn translates_items(&self) -> bool {
        self.game_ids
            .as_ref()
            .is_some_and(|ids| ids.items.is_some())
    }

    /// Native-version item id for a source-version item id. Used by decoded
    /// configuration/game tag payloads, whose element ids are not covered by
    /// the packet-id translation layer.
    pub fn remap_item_to_native(&self, id: u32) -> Option<u32> {
        self.to_native.remap(ClientRegistry::Item, id)
    }

    /// Remaps a decoded packet's static-registry ids into the native
    /// version's id space; `false` drops the packet (its subject no longer
    /// exists, e.g. the bed block entity removed in 26.2).
    pub fn remap_inbound(&self, packet: &mut ClientboundGamePacket) -> bool {
        use ClientRegistry as R;
        match packet {
            ClientboundGamePacket::SetEntityData(p) => {
                // `translate_entity_data` already remaps item-stack values
                // while rewriting serializer ids, except on pre-1.20.5 wires
                // where `translate_item_765` leaves the wire id for this pass.
                let needs_remap = self.game_ids.as_ref().is_none_or(|g| g.v765.is_some());
                if needs_remap {
                    for item in &mut p.packed_items.0 {
                        if let azalea_entity::EntityDataValue::ItemStack(stack) = &mut item.value {
                            remap_stack(self.to_native, stack);
                        }
                    }
                }
                true
            }
            ClientboundGamePacket::MerchantOffers(_) if self.translates_items() => true,
            ClientboundGamePacket::MerchantOffers(p) => {
                // An `ItemCost` has no empty form, so an untranslatable base
                // cost drops the offer rather than the whole trade list.
                p.offers.retain_mut(|offer| {
                    remap_stack(self.to_native, &mut offer.result);
                    if offer
                        .cost_b
                        .as_mut()
                        .is_some_and(|c| !remap_with(self.to_native, R::Item, &mut c.item))
                    {
                        offer.cost_b = None;
                    }
                    remap_with(self.to_native, R::Item, &mut offer.base_cost_a.item)
                });
                true
            }
            _ => true,
        }
    }
}
