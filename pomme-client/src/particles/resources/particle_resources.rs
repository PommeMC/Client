//! Client provider storage and CPU description preparation/binding, independent
//! of type bootstrap, atlas source evaluation and engine/GPU adoption.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::panic::Location;
use std::rc::Rc;

use glam::DVec3;

pub(crate) use self::provider_erasure::{ErasedParticleProvider, ParticleDispatchError};
use super::super::particle::ParticleRef;
use super::super::particle_provider::ParticleProvider;
use super::super::types::{ParticleOptions, ParticleType, ParticleTypeInfo, TypedParticleOptions};
use super::particle_description::{ParticleDescription, PreparedParticleDescriptions};
use super::sprite_set::{MutableSpriteSet, SpriteSet};
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::resources::{ResourceContent, ResourceId, ResourceManager};
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

/// Typed sprite-provider factory, matching Vanilla's private
/// SpriteParticleRegistration<T>. Called immediately during client provider
/// registration, not during spawning or reload. The provider retains the
/// supplied stable set; this crate-scoped sketch is not a public plug-in SPI.
pub trait SpriteParticleRegistration<O: TypedParticleOptions> {
    fn create(&self, sprite_set: Rc<dyn SpriteSet>) -> Rc<dyn ParticleProvider<O>>;
}

/// Private, object-safe registry/engine collaboration, not a plug-in API.
/// Storage and checked adapters are private to the resource owner.
mod provider_erasure {
    use glam::DVec3;

    use crate::particles::particle::ParticleRef;
    use crate::particles::types::ParticleOptions;
    use crate::util::random_source::RandomSource;
    use crate::world::level::LevelContext;

    /// Binding/schema failures are invariants, never nullable construction.
    pub enum ParticleDispatchError {
        ResourceNameMismatch,
        OptionsSchemaMismatch,
    }

    /// Registration must build an adapter specialized for its O, retaining the
    /// exact supplied type token and Rc<dyn ParticleProvider<O>>. Before
    /// calling the typed provider, compare resource names and schema
    /// TypeIds, then use options.as_any().downcast_ref::<O>(); return the
    /// corresponding error if any check fails. Never unchecked-cast,
    /// dispatch by schema alone, or map an invariant failure to None. The
    /// sealed options blanket impl ties the reported schema to the actual
    /// payload; registry bindings still require these runtime checks,
    /// including when options come from another owner.
    pub trait ErasedParticleProvider {
        fn create_particle(
            &self,
            options: &dyn ParticleOptions,
            world: &mut LevelContext<'_>,
            position: DVec3,
            auxiliary: DVec3,
            random: &RandomSource,
        ) -> Result<Option<ParticleRef>, ParticleDispatchError>;
    }
}

/// Owns client provider bindings and stable sprite sets, not type creation.
/// ParticleType tokens are established by ParticleRegistry::register, so
/// packet/data/server identity does not depend on client provider registration.
/// Generic registration and typed lookup deliberately make this trait
/// non-object-safe; providers, options and narrow engine access remain usable
/// as trait objects. No codecs, wire registry, asset cache or external
/// extension framework is supplied.
pub trait ParticleResources {
    /// Associate an already established ParticleType<O> with its matching
    /// provider, retaining that token and specializing the checked erased
    /// adapter for O. Never mint a type or supply type metadata here.
    /// Replace by particle_type.info().name(), warning with both caller
    /// locations, regardless of O or registration form. Preserve any
    /// existing sprite set. Canonical type names belong to the independent
    /// ParticleRegistry owner, not this owner's provider replacement
    /// policy.
    #[track_caller]
    fn register<O: TypedParticleOptions>(
        &mut self,
        particle_type: Rc<ParticleType<O>>,
        provider: Rc<dyn ParticleProvider<O>>,
    );

    /// Rust spelling of Vanilla's second register overload. Create a new
    /// stable unbound SpriteSet indexed by particle_type.info().name(). Call
    /// registration.create immediately with that set, then bind the returned
    /// provider to the supplied token using the same checked erased adapter.
    /// Later apply calls rebind the SAME set without recreating the provider
    /// or engine. Replacement warns as register does. The factory must not
    /// select sprites: binding must precede particle creation, and an absent
    /// description does not bind a set. Type bootstrap is independent.
    #[track_caller]
    fn register_sprite_provider<O: TypedParticleOptions>(
        &mut self,
        particle_type: Rc<ParticleType<O>>,
        registration: &dyn SpriteParticleRegistration<O>,
    );

    /// Lookup by registered name AND schema, not just TypeId. None means an
    /// unknown name; a known name with the wrong schema is an invariant error.
    fn get_provider<O: TypedParticleOptions>(
        &self,
        particle_type: &ParticleType<O>,
    ) -> Result<Option<&dyn ParticleProvider<O>>, provider_erasure::ParticleDispatchError>;

    /// The engine can use this with erased options.get_type(), then invoke the
    /// checked adapter without knowing O. Validate name/schema at lookup too.
    fn get_provider_erased(
        &self,
        particle_type: &ParticleTypeInfo,
    ) -> Result<
        Option<&dyn provider_erasure::ErasedParticleProvider>,
        provider_erasure::ParticleDispatchError,
    >;

    fn get_sprite_set(&self, particle_type: &ParticleTypeInfo) -> Option<Rc<dyn SpriteSet>>;

    /// Discover/parse ordered descriptions using the selected resource manager.
    /// Ignore names without registered sprite sets. Return owned CPU data
    /// before atlas preparation; parse/I/O failures are not producer
    /// admission errors.
    fn prepare(&self, resources: &dyn ResourceManager) -> io::Result<PreparedParticleDescriptions>;

    /// At the caller's existing GPU-safe reload boundary: clear engine state
    /// through the callback, then privately rebind the SAME sprite sets against
    /// the completed atlas. Provider identity survives. Empty descriptions and
    /// missing referenced sprites use the missing region. Absent descriptions
    /// leave previous contents unchanged (including initially unbound sets).
    /// Consumers cannot extract old
    /// retained particles/requeue them without refreshing their cached sprites.
    /// This does not upload atlases or promise transactional all-resource
    /// reload. Prepare and atlas realization must both succeed first; do not
    /// change registrations between prepare and apply on this same owner.
    fn apply(
        &self,
        prepared: PreparedParticleDescriptions,
        atlas: &ParticleAtlasSprites,
        clear_particles: &mut dyn FnMut(),
    );
}

/// Read-only logical sprites successfully realized by the atlas owner, plus its
/// real missing tile. Description references do not determine atlas membership.
/// Full independent atlas-source interpretation and GPU upload belong to phase
/// 7.
/// COMPAT: Parsing defers to ResourceId, not Vanilla's stricter namespace/case
/// rules. Its rejection of Vanilla's legal empty sprite paths is accepted.
pub struct ParticleAtlasSprites {
    sprites: BTreeMap<ResourceId, AtlasRegion>,
    missing_region: AtlasRegion,
}

impl ParticleAtlasSprites {
    pub fn new(sprites: BTreeMap<ResourceId, AtlasRegion>, missing_region: AtlasRegion) -> Self {
        Self {
            sprites,
            missing_region,
        }
    }

    pub fn get(&self, id: &ResourceId) -> Option<AtlasRegion> {
        self.sprites.get(id).copied()
    }

    pub fn missing_region(&self) -> AtlasRegion {
        self.missing_region
    }
}

trait Entry {
    fn info(&self) -> &ParticleTypeInfo;
    fn location(&self) -> &'static Location<'static>;
    fn as_any(&self) -> &dyn Any;
    fn erased_provider(&self) -> &dyn ErasedParticleProvider;
}

struct Binding<O: TypedParticleOptions> {
    particle_type: Rc<ParticleType<O>>,
    provider: Rc<dyn ParticleProvider<O>>,
    location: &'static Location<'static>,
}

impl<O: TypedParticleOptions> Entry for Binding<O> {
    fn info(&self) -> &ParticleTypeInfo {
        self.particle_type.info()
    }

    fn location(&self) -> &'static Location<'static> {
        self.location
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn erased_provider(&self) -> &dyn ErasedParticleProvider {
        self
    }
}

fn check_type(
    registered: &ParticleTypeInfo,
    requested: &ParticleTypeInfo,
) -> Result<(), ParticleDispatchError> {
    if registered.name() != requested.name() {
        return Err(ParticleDispatchError::ResourceNameMismatch);
    }
    if registered.options_schema() != requested.options_schema() {
        return Err(ParticleDispatchError::OptionsSchemaMismatch);
    }
    Ok(())
}

impl<O: TypedParticleOptions> ErasedParticleProvider for Binding<O> {
    fn create_particle(
        &self,
        options: &dyn ParticleOptions,
        world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Result<Option<ParticleRef>, ParticleDispatchError> {
        check_type(self.info(), options.get_type())?;
        let options = options
            .as_any()
            .downcast_ref::<O>()
            .ok_or(ParticleDispatchError::OptionsSchemaMismatch)?;
        Ok(self
            .provider
            .create_particle(options, world, position, auxiliary, random))
    }
}

/// Empty/manual bootstrap by default. Names, not option schemas, own bindings.
/// Reload keeps providers and sprite-set allocations; registration replacement
/// is separate and may replace the schema, provider and (sprite form only) set.
#[derive(Default)]
pub struct MappedParticleResources {
    providers: BTreeMap<ResourceId, Box<dyn Entry>>,
    sprite_sets: BTreeMap<ResourceId, Rc<MutableSpriteSet>>,
}

impl MappedParticleResources {
    pub fn new() -> Self {
        Self::default()
    }

    fn bind_provider<O: TypedParticleOptions>(
        &mut self,
        particle_type: Rc<ParticleType<O>>,
        provider: Rc<dyn ParticleProvider<O>>,
        location: &'static Location<'static>,
    ) {
        let name = particle_type.info().name();
        if let Some(old) = self.providers.get(name) {
            tracing::warn!(particle = %name, old_location = %old.location(), new_location = %location,
                "Replacing particle provider");
        }
        self.providers.insert(
            name.clone(),
            Box::new(Binding {
                particle_type,
                provider,
                location,
            }),
        );
    }

    fn entry(
        &self,
        particle_type: &ParticleTypeInfo,
    ) -> Result<Option<&dyn Entry>, ParticleDispatchError> {
        let Some(entry) = self.providers.get(particle_type.name()) else {
            return Ok(None);
        };
        check_type(entry.info(), particle_type)?;
        Ok(Some(entry.as_ref()))
    }
}

impl ParticleResources for MappedParticleResources {
    #[track_caller]
    fn register<O: TypedParticleOptions>(
        &mut self,
        particle_type: Rc<ParticleType<O>>,
        provider: Rc<dyn ParticleProvider<O>>,
    ) {
        let location = Location::caller();
        // Vanilla direct registration leaves any pre-existing sprite set intact.
        self.bind_provider(particle_type, provider, location);
    }

    #[track_caller]
    fn register_sprite_provider<O: TypedParticleOptions>(
        &mut self,
        particle_type: Rc<ParticleType<O>>,
        registration: &dyn SpriteParticleRegistration<O>,
    ) {
        let location = Location::caller();
        let name = particle_type.info().name();
        let sprite_set = Rc::new(MutableSpriteSet::new(name.clone()));
        self.sprite_sets
            .insert(name.clone(), Rc::clone(&sprite_set));
        let provider = registration.create(sprite_set);
        self.bind_provider(particle_type, provider, location);
    }

    fn get_provider<O: TypedParticleOptions>(
        &self,
        particle_type: &ParticleType<O>,
    ) -> Result<Option<&dyn ParticleProvider<O>>, ParticleDispatchError> {
        let Some(entry) = self.entry(particle_type.info())? else {
            return Ok(None);
        };
        if entry.info().options_schema() != TypeId::of::<O>() {
            return Err(ParticleDispatchError::OptionsSchemaMismatch);
        }
        let binding = entry
            .as_any()
            .downcast_ref::<Binding<O>>()
            .ok_or(ParticleDispatchError::OptionsSchemaMismatch)?;
        Ok(Some(binding.provider.as_ref()))
    }

    fn get_provider_erased(
        &self,
        particle_type: &ParticleTypeInfo,
    ) -> Result<Option<&dyn ErasedParticleProvider>, ParticleDispatchError> {
        Ok(self.entry(particle_type)?.map(Entry::erased_provider))
    }

    fn get_sprite_set(&self, particle_type: &ParticleTypeInfo) -> Option<Rc<dyn SpriteSet>> {
        // Sprite-set membership is name-only, not canonical type/schema ownership.
        let set = Rc::clone(self.sprite_sets.get(particle_type.name())?);
        Some(set)
    }

    fn prepare(&self, resources: &dyn ResourceManager) -> io::Result<PreparedParticleDescriptions> {
        let files = resources.list_resources("particles", &|id| {
            id.path().starts_with("particles/") && id.path().ends_with(".json")
        }).inspect_err(|error| {
            tracing::error!(%error, directory = "particles", "Failed to list particle descriptions");
        })?;
        let mut descriptions = BTreeMap::new();
        for (file_id, resource) in files {
            let Some(path) = file_id
                .path()
                .strip_prefix("particles/")
                .and_then(|path| path.strip_suffix(".json"))
            else {
                continue;
            };
            if path.is_empty() {
                tracing::debug!(file = %file_id, "Ignoring particle description without a sprite set");
                continue;
            }
            let content = resource.content();
            let name = ResourceId::from_parts(file_id.namespace(), path).map_err(|error| {
                description_error(
                    &format!("{}:{path}", file_id.namespace()),
                    content,
                    io::Error::new(io::ErrorKind::InvalidData, error),
                )
            })?;
            if !self.sprite_sets.contains_key(&name) {
                tracing::debug!(particle = %name, file = %content.id(), "Ignoring particle description without a sprite set");
                continue;
            }
            let description = (|| {
                let reader = content.open().map_err(io::Error::other)?;
                ParticleDescription::read(io::BufReader::new(reader))
            })()
            .map_err(|error| description_error(&name.to_string(), content, error))?;
            descriptions.insert(name, description);
        }
        Ok(PreparedParticleDescriptions { descriptions })
    }

    fn apply(
        &self,
        prepared: PreparedParticleDescriptions,
        atlas: &ParticleAtlasSprites,
        clear_particles: &mut dyn FnMut(),
    ) {
        clear_particles();
        let mut missing = BTreeSet::new();
        for (name, description) in prepared.descriptions {
            let Some(set) = self.sprite_sets.get(&name) else {
                tracing::debug!(particle = %name, "Ignoring prepared description without a sprite set in this owner");
                continue;
            };
            let mut sprites: Vec<_> = description
                .textures
                .into_iter()
                .map(|id| {
                    atlas.get(&id).unwrap_or_else(|| {
                        missing.insert(id);
                        atlas.missing_region()
                    })
                })
                .collect();
            if sprites.is_empty() {
                sprites.push(atlas.missing_region());
            }
            set.bind(sprites);
        }
        if !missing.is_empty() {
            tracing::warn!(sprites = ?missing, "Missing particle atlas sprites");
        }
    }
}

fn description_error(name: &str, content: &ResourceContent, error: io::Error) -> io::Error {
    tracing::error!(particle = name, file = %content.id(), origin = ?content.origin(), %error,
        "Failed to prepare particle description");
    io::Error::new(
        error.kind(),
        format!(
            "particle {name}, file {}, origin {:?}: {error}",
            content.id(),
            content.origin()
        ),
    )
}
