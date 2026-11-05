use std::collections::BTreeMap;
use std::io;

use super::super::atlas::Source;
use crate::resources::{ResourceId, ResourceManager};

pub(super) type SpriteLoader = Box<dyn FnOnce(&dyn ResourceManager) -> Option<Source>>;
pub(super) type SpriteSourceOutput = BTreeMap<ResourceId, SpriteLoader>;

pub(super) trait SpriteSource {
    fn run(
        self: Box<Self>,
        resources: &dyn ResourceManager,
        output: &mut SpriteSourceOutput,
    ) -> io::Result<()>;
}
