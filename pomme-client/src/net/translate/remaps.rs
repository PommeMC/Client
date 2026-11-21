use super::*;

impl Translation {
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

    /// Whether `set_entity_data` item stacks keep the wire version's item
    /// ids: `translate_entity_data` remaps them while rewriting serializer
    /// ids, except with no game translation at all and on pre-1.20.5 wires,
    /// where `translate_item_765` leaves the id as sent.
    pub fn entity_data_items_in_wire_ids(&self) -> bool {
        self.game_ids.as_ref().is_none_or(|g| g.v765.is_some())
    }
}
