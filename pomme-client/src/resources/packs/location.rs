use pomme_protocol::KnownPack;

use crate::chat_component::Component;

/// Vanilla's textual pack identity, distinct from the existing opaque PackId.
#[derive(Clone)]
pub(crate) struct PackLocationInfo {
    pub(crate) id: String,
    pub(crate) title: Component,
    pub(crate) known_pack_info: Option<KnownPack>,
}

#[cfg(test)]
impl PackLocationInfo {
    pub(crate) fn test(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            title: Component::text(id),
            known_pack_info: None,
        }
    }
}
