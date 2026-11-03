//! Type identity and typed creation options, independent of client provider
//! registration, storage and sprites.

use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::rc::Rc;

use crate::resources::ResourceId;

/// Read-only erased registry metadata, obtainable only from a typed token.
/// Identity is the resource name, never the option schema's Rust `TypeId`:
/// multiple names may share one schema. Numeric wire IDs and per-type codecs
/// remain deferred to the versioned registry adapter.
pub struct ParticleTypeInfo {
    name: ResourceId,
    override_limiter: bool,
    options_schema: TypeId,
}

impl ParticleTypeInfo {
    pub fn name(&self) -> &ResourceId {
        &self.name
    }

    pub fn get_override_limiter(&self) -> bool {
        self.override_limiter
    }

    pub fn options_schema(&self) -> TypeId {
        self.options_schema
    }
}

/// Type token tying a resource name to exactly one options schema, established
/// independently of client provider registration. Canonical name registration
/// belongs to ParticleRegistry::register; its implementation remains deferred.
/// No public constructor or independently supplied erasure metadata. The
/// invariant marker also prevents function-type subtyping from changing O.
pub struct ParticleType<O: TypedParticleOptions> {
    info: ParticleTypeInfo,
    options: PhantomData<fn(O) -> O>,
}

impl<O: TypedParticleOptions> ParticleType<O> {
    /// Restricted helper for ParticleRegistry::register implementations inside
    /// the particles module. Creates a schema token before provider association
    /// through ParticleResources::register. Derives the schema from O, never
    /// caller-supplied metadata. The registry must enforce ResourceId
    /// uniqueness and retain the canonical token; this constructor alone
    /// does neither. ParticleResources registration methods must not mint
    /// types.
    pub(super) fn new(name: ResourceId, override_limiter: bool) -> Self {
        Self {
            info: ParticleTypeInfo {
                name,
                override_limiter,
                options_schema: TypeId::of::<O>(),
            },
            options: PhantomData,
        }
    }

    pub fn info(&self) -> &ParticleTypeInfo {
        &self.info
    }
}

/// Rust-only typed bridge. Implementors choose a registered token of their own
/// schema, not arbitrary erased metadata or a separately supplied Any payload.
/// Choosing another type token with the same schema is legitimate.
pub trait TypedParticleOptions: Any + Sized {
    fn particle_type(&self) -> &ParticleType<Self>;
}

mod sealed {
    pub trait Sealed {}

    impl<O: super::TypedParticleOptions> Sealed for O {}
}

/// Vanilla's object-safe engine boundary. Sealing plus the sole blanket impl
/// prevents options from independently overriding metadata or downcast payload.
pub trait ParticleOptions: Any + sealed::Sealed {
    fn get_type(&self) -> &ParticleTypeInfo;
    fn as_any(&self) -> &dyn Any;
}

impl<O: TypedParticleOptions> ParticleOptions for O {
    fn get_type(&self) -> &ParticleTypeInfo {
        self.particle_type().info()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Parameterless options also wrap their registered type identity, as in
/// Vanilla. Different names (e.g. flame and smoke) may share this schema.
pub struct SimpleParticleType {
    particle_type: Rc<ParticleType<Self>>,
}

impl SimpleParticleType {
    pub fn new(particle_type: Rc<ParticleType<Self>>) -> Self {
        Self { particle_type }
    }

    pub fn name(&self) -> &ResourceId {
        self.particle_type.info().name()
    }

    pub fn get_override_limiter(&self) -> bool {
        self.particle_type.info().get_override_limiter()
    }
}

impl TypedParticleOptions for SimpleParticleType {
    fn particle_type(&self) -> &ParticleType<Self> {
        &self.particle_type
    }
}

/// Client provider registration rejects duplicate names across both forms even
/// if their schemas differ; it must not replace an existing provider binding.
/// This is not a global type-registry uniqueness error.
pub enum ParticleRegistrationError {
    DuplicateProviderRegistration(ResourceId),
}

/// Value-equal quota key, distinct from render-group capacity and world policy.
/// Equal numeric limits share a count, matching Vanilla's ParticleLimit record
/// and value-keyed quota map. Vanilla's SPORE_BLOSSOM limit is 1000.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParticleLimit {
    pub limit: usize,
}
