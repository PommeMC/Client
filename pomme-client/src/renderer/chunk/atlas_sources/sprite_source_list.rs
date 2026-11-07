use std::collections::BTreeMap;
use std::io;

use serde_json::Value;

use super::super::atlas::{AtlasCpu, AtlasRegion, TextureAtlas};
use super::sources::{array, field};
use super::sprite_source::SpriteSource;
use super::sprite_sources::SpriteSources;
use crate::resources::{ResourceId, ResourceManager, resource_id};

pub(crate) struct SpriteSourceList(Vec<Box<dyn SpriteSource>>);

pub(crate) struct PreparedAtlas {
    pub(crate) cpu: AtlasCpu,
    pub(crate) sprites: BTreeMap<ResourceId, AtlasRegion>,
}

impl SpriteSourceList {
    pub(crate) fn load(
        resources: &dyn ResourceManager,
        atlas: &ResourceId,
        sprite_sources: &SpriteSources,
    ) -> io::Result<Self> {
        let definition =
            ResourceId::from_parts(atlas.namespace(), &format!("atlases/{}.json", atlas.path()))
                .map_err(io::Error::other)?;
        let mut sources = Vec::new();
        for resource in resources.get_resource_stack(&definition)? {
            let decode = || -> io::Result<Vec<Box<dyn SpriteSource>>> {
                let stream = resource.content().open().map_err(io::Error::other)?;
                let value: Value = serde_json::from_reader(stream)?;
                array(field(&value, "sources")?)?
                    .iter()
                    .map(|value| sprite_sources.decode(value))
                    .collect()
            };
            match decode() {
                Ok(decoded) => sources.extend(decoded),
                Err(error) => {
                    tracing::warn!(%definition, origin = ?resource.content().origin(), %error, "Skipping atlas definition")
                }
            }
        }
        Ok(Self(sources))
    }

    pub(crate) fn prepare(
        self,
        resources: &dyn ResourceManager,
        max_mip: u32,
    ) -> io::Result<PreparedAtlas> {
        let mut suppliers = BTreeMap::new();
        for source in self.0 {
            source.run(resources, &mut suppliers)?;
        }
        let mut sources = Vec::with_capacity(suppliers.len());
        let mut names = Vec::with_capacity(suppliers.len());
        for (id, supplier) in suppliers {
            if let Some(source) = supplier(resources) {
                names.push(id);
                sources.push(source);
            }
        }
        let mut cpu = TextureAtlas::prepare_sources(sources, max_mip);
        cpu.resolve_source_list_missing_collision();
        let mut sprites: BTreeMap<_, _> = names
            .into_iter()
            .filter_map(|id| {
                cpu.realized_region(&id.to_string())
                    .map(|region| (id, region))
            })
            .collect();
        sprites.insert(
            resource_id!("minecraft:missingno"),
            cpu.uv_map.missing_region(),
        );
        Ok(PreparedAtlas { cpu, sprites })
    }
}
