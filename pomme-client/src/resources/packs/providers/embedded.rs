use std::collections::BTreeSet;
use std::io::{self, Cursor};
use std::rc::Rc;
use std::sync::Arc;

use super::{
    PackResourceProvider, ReadStream, ResourceOrigin, ThreadSafePackResourceProvider,
    matches_prefix, validate_query,
};
use crate::resources::{ResourceContent, ResourceError, ResourceId, resource_id};

// Stable application IDs, independent of the installed Minecraft version.
// Selected packs can override them through the ordinary resource lookup.
pub(crate) const POMME_UI_FONT: ResourceId = resource_id!("pomme:font/ui.ttf");
pub(crate) const POMME_ICONS_FONT: ResourceId = resource_id!("pomme:font/icons.ttf");
pub(crate) const POMME_ICON: ResourceId = resource_id!("pomme:textures/icon.png");

const ENTRIES: &[(ResourceId, &[u8])] = &[
    (
        POMME_UI_FONT,
        include_bytes!("../../../renderer/fonts/Montserrat-Medium.ttf"),
    ),
    (
        POMME_ICONS_FONT,
        include_bytes!("../../../renderer/fonts/fa-solid-900.ttf"),
    ),
    (POMME_ICON, include_bytes!("../../../../assets/icon.png")),
];

#[derive(Clone)]
pub(in crate::resources::packs) struct EmbeddedPackResourceProvider {
    origin: ResourceOrigin,
}

impl EmbeddedPackResourceProvider {
    pub(in crate::resources::packs) fn new(origin: ResourceOrigin) -> Self {
        Self { origin }
    }

    fn bytes(id: &ResourceId) -> Option<&'static [u8]> {
        ENTRIES
            .iter()
            .find_map(|(name, bytes)| (name == id).then_some(*bytes))
    }
}

impl PackResourceProvider for EmbeddedPackResourceProvider {
    fn thread_safe_source(&self) -> io::Result<Arc<dyn ThreadSafePackResourceProvider>> {
        Ok(Arc::new(self.clone()))
    }

    fn origin(&self) -> &ResourceOrigin {
        &self.origin
    }

    fn get_resource(
        self: Rc<Self>,
        id: &ResourceId,
    ) -> Result<Option<ResourceContent>, ResourceError> {
        Ok(Self::bytes(id).map(|_| ResourceContent::new(id.clone(), self)))
    }

    fn open(&self, id: &ResourceId) -> io::Result<ReadStream> {
        Self::bytes(id)
            .map(|bytes| Box::new(Cursor::new(bytes)) as ReadStream)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("missing resource {id}"))
            })
    }

    fn namespaces(&self) -> Result<BTreeSet<String>, ResourceError> {
        Ok(BTreeSet::from(["pomme".to_owned()]))
    }

    fn list_resources(
        &self,
        namespace: &str,
        prefix: &str,
    ) -> Result<BTreeSet<ResourceId>, ResourceError> {
        validate_query(Some(namespace), prefix)?;
        Ok(ENTRIES
            .iter()
            .map(|(id, _)| id)
            .filter(|id| id.namespace() == namespace && matches_prefix(id.path(), prefix))
            .cloned()
            .collect())
    }
}
