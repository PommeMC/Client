//! Canonical particle-type bootstrap and storage, independent of client
//! providers and resource packs.

use std::collections::BTreeMap;
use std::rc::Rc;

use super::types::{ParticleType, ParticleTypeInfo, TypedParticleOptions};
use crate::resources::ResourceId;

/// Type bootstrap failures, distinct from duplicate client provider bindings.
pub enum ParticleTypeRegistrationError {
    DuplicateName(ResourceId),
    Frozen,
}

/// One authoritative type-bootstrap owner per client lifetime, not a mutable
/// global. Client providers, server/data identity and resource-pack reloads do
/// not create separate type identities. Multiple independent owners must not
/// be treated as one canonical registry; owner/generation machinery is
/// deferred.
///
/// Implementations belong inside the particles module, where the restricted
/// ParticleType::new constructor is accessible. Generic registration makes this
/// trait intentionally non-object-safe. This is not a generic Registry<T> port:
/// holders, tags, codecs and typed recovery from erased lookup are deferred.
/// Numeric IDs are omitted; future local IDs are not inherently stable wire
/// IDs and require a separate protocol-version mapping.
pub trait ParticleRegistry {
    /// Bootstrap a canonical token, deriving its schema internally from O.
    /// This constructs the type like Vanilla's ParticleTypes.register helper,
    /// not WritableRegistry.register, which accepts an already created value.
    ///
    /// Reject every duplicate ResourceId with DuplicateName, even for a
    /// different O; never replace a token. Multiple names may share O. Retain
    /// the canonical Rc and return a clone of that SAME allocation.
    /// Registration needs no provider; ParticleResources::register
    /// associates one later. After freeze, return Frozen. Errors leave the
    /// registry unchanged.
    fn register<O: TypedParticleOptions>(
        &mut self,
        name: ResourceId,
        override_limiter: bool,
    ) -> Result<Rc<ParticleType<O>>, ParticleTypeRegistrationError>;

    /// Rust spelling of Vanilla Registry.getValue's identifier lookup. Borrow
    /// metadata directly from the retained canonical token, never a clone or
    /// separately authored identity. This cannot recover O or a typed token.
    /// Unknown names return None, with no default-value fallback.
    fn get_value(&self, name: &ResourceId) -> Option<&ParticleTypeInfo>;

    /// Number of registered types, independent of client provider bindings.
    fn size(&self) -> usize;

    /// Idempotently forbid further type registration, with no thaw/removal.
    /// No holders or tags need validation in this contract, so freeze cannot
    /// fail. Lookup and existing tokens remain valid. Client provider state
    /// and resource-pack/sprite reloads are NOT frozen by this operation.
    fn freeze(&mut self);
}

trait RegisteredParticleType {
    fn info(&self) -> &ParticleTypeInfo;
}

impl<O: TypedParticleOptions> RegisteredParticleType for ParticleType<O> {
    fn info(&self) -> &ParticleTypeInfo {
        ParticleType::info(self)
    }
}

/// Owns canonical typed tokens across option schemas, keyed only by resource
/// name. Freezing type bootstrap does not freeze client provider resources.
#[derive(Default)]
pub struct MappedParticleRegistry {
    entries: BTreeMap<ResourceId, Rc<dyn RegisteredParticleType>>,
    frozen: bool,
}

impl MappedParticleRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ParticleRegistry for MappedParticleRegistry {
    fn register<O: TypedParticleOptions>(
        &mut self,
        name: ResourceId,
        override_limiter: bool,
    ) -> Result<Rc<ParticleType<O>>, ParticleTypeRegistrationError> {
        if self.frozen {
            return Err(ParticleTypeRegistrationError::Frozen);
        }
        if self.entries.contains_key(&name) {
            return Err(ParticleTypeRegistrationError::DuplicateName(name));
        }

        let particle_type = Rc::new(ParticleType::<O>::new(name.clone(), override_limiter));
        let retained = Rc::clone(&particle_type);
        self.entries.insert(name, retained);
        Ok(particle_type)
    }

    fn get_value(&self, name: &ResourceId) -> Option<&ParticleTypeInfo> {
        self.entries
            .get(name)
            .map(|particle_type| particle_type.info())
    }

    fn size(&self) -> usize {
        self.entries.len()
    }

    fn freeze(&mut self) {
        self.frozen = true;
    }
}
