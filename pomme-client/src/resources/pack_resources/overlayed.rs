//! One logical pack with Vanilla's last-overlay-first resource precedence.

use std::collections::{HashMap, HashSet};
use std::io;
use std::rc::Rc;

use serde_json::Value;

use super::{IoSupplier, PackMetadataResources, PackResources, PackType, ResourceOutput};
use crate::resources::packs::PackLocationInfo;
use crate::resources::resource::{ReadStream, ResourceId};

pub(crate) struct OverlayedPackResources {
    primary: Rc<dyn PackResources>,
    overlays: Vec<Rc<dyn PackResources>>,
}

impl OverlayedPackResources {
    /// Overlays arrive in metadata order. Reuse their storage in priority
    /// order; selection and applicability are the caller's responsibility.
    pub(crate) fn new(
        primary: Rc<dyn PackResources>,
        mut overlays: Vec<Rc<dyn PackResources>>,
    ) -> Self {
        overlays.reverse();
        Self { primary, overlays }
    }

    fn stack(&self) -> impl Iterator<Item = &Rc<dyn PackResources>> {
        self.overlays.iter().chain(std::iter::once(&self.primary))
    }
}

impl PackMetadataResources for OverlayedPackResources {
    fn location(&self) -> &PackLocationInfo {
        self.primary.location()
    }

    // Root files and pack metadata belong only to the primary, not the stack.
    fn get_root_resource(&self, path: &[&str]) -> io::Result<Option<IoSupplier<ReadStream>>> {
        self.primary.get_root_resource(path)
    }

    fn metadata_section_json(&self, name: &str) -> io::Result<Option<Value>> {
        self.primary.metadata_section_json(name)
    }

    /// Like Vanilla's forEach, visit in priority order and stop on failure.
    /// Repeated close and retained supplier behavior belong to each child.
    fn close(&self) -> io::Result<()> {
        for pack in self.stack() {
            pack.close()?;
        }
        Ok(())
    }
}

impl PackResources for OverlayedPackResources {
    fn get_resource(
        &self,
        pack_type: PackType,
        id: &ResourceId,
    ) -> io::Result<Option<IoSupplier<ReadStream>>> {
        for pack in self.stack() {
            if let Some(supplier) = pack.get_resource(pack_type, id)? {
                return Ok(Some(supplier));
            }
        }
        Ok(None)
    }

    fn list_resources(
        &self,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        output: &mut ResourceOutput<'_>,
    ) -> io::Result<()> {
        let mut resources = HashMap::new();
        for pack in self.stack() {
            pack.list_resources(pack_type, namespace, directory, &mut |id, supplier| {
                resources.entry(id).or_insert(supplier);
            })?;
        }
        // Delay output until all layers succeed, retaining the first supplier.
        for (id, supplier) in resources {
            output(id, supplier);
        }
        Ok(())
    }

    fn get_namespaces(&self, pack_type: PackType) -> io::Result<HashSet<String>> {
        let mut namespaces = HashSet::new();
        for pack in self.stack() {
            namespaces.extend(pack.get_namespaces(pack_type)?);
        }
        Ok(namespaces)
    }
}
