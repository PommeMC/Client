//! Client provider storage and CPU description preparation/binding, independent
//! of type bootstrap, atlas source evaluation and engine/GPU adoption.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::panic::Location;
use std::rc::Rc;

use glam::DVec3;

use super::particle::ParticleRef;
use super::provider_erasure::{ErasedParticleProvider, ParticleDispatchError};
use super::sprites::{
    MutableSpriteSet, ParticleAtlasSprites, ParticleDescription, PreparedParticleDescriptions,
    SpriteSet,
};
use super::types::{ParticleOptions, ParticleType, ParticleTypeInfo, TypedParticleOptions};
use super::world::ParticleWorld;
use super::{ParticleProvider, ParticleResources, SpriteParticleRegistration};
use crate::resources::{ResourceContent, ResourceId, ResourceManager};

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
        world: &dyn ParticleWorld,
        position: DVec3,
        auxiliary: DVec3,
        random: &mut fastrand::Rng,
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
