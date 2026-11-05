use std::collections::BTreeMap;
use std::io;

use serde_json::Value;

use super::sources::{
    DirectoryLister, PalettedPermutations, SingleFile, SourceFilter, Unstitcher, field, id, invalid,
};
use super::sprite_source::SpriteSource;
use crate::resources::{ResourceId, resource_id};

type SpriteSourceDecoder = fn(&Value) -> io::Result<Box<dyn SpriteSource>>;

pub(crate) struct SpriteSources {
    decoders: BTreeMap<ResourceId, SpriteSourceDecoder>,
}

impl SpriteSources {
    pub(crate) fn bootstrap() -> Self {
        let mut sources = Self {
            decoders: BTreeMap::new(),
        };
        sources.register(resource_id!("minecraft:single"), SingleFile::decode);
        sources.register(resource_id!("minecraft:directory"), DirectoryLister::decode);
        sources.register(resource_id!("minecraft:filter"), SourceFilter::decode);
        sources.register(resource_id!("minecraft:unstitch"), Unstitcher::decode);
        sources.register(
            resource_id!("minecraft:paletted_permutations"),
            PalettedPermutations::decode,
        );
        sources
    }

    fn register(&mut self, id: ResourceId, decoder: SpriteSourceDecoder) {
        self.decoders.insert(id, decoder);
    }

    pub(super) fn decode(&self, value: &Value) -> io::Result<Box<dyn SpriteSource>> {
        let kind = id(field(value, "type")?)?;
        let decoder = self
            .decoders
            .get(&kind)
            .ok_or_else(|| invalid(format!("Unknown atlas source: {kind}")))?;
        decoder(value)
    }
}
