//! Ordered CPU particle descriptions and JSON parsing.

use std::collections::BTreeMap;
use std::io;

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::resources::ResourceId;

/// Only the ordered textures from `particles/<type>.json`, not behavior.
pub struct ParticleDescription {
    /// Logical sprite ResourceIds, not image filenames, in frame order.
    /// COMPAT: See ParticleAtlasSprites compat note
    pub textures: Vec<ResourceId>,
}

impl ParticleDescription {
    pub(super) fn read(reader: impl io::Read) -> io::Result<Self> {
        // GsonHelper reads one value, without a trailing-input end check.
        // RawValue preserves primitive number spelling, including exponent case.
        let mut deserializer = serde_json::Deserializer::from_reader(reader);
        let mut object = BTreeMap::<String, Box<RawValue>>::deserialize(&mut deserializer)
            .map_err(invalid_json)?;
        let Some(textures) = object.remove("textures") else {
            return Ok(Self {
                textures: Vec::new(),
            });
        };
        let values: Vec<Box<RawValue>> =
            serde_json::from_str(textures.get()).map_err(invalid_json)?;
        let textures = values
            .iter()
            .map(|value| {
                let raw = value.get();
                match raw.as_bytes().first() {
                    Some(b'"') => {
                        ResourceId::new(&serde_json::from_str::<String>(raw).map_err(invalid_json)?)
                    }
                    Some(b'-' | b'0'..=b'9' | b't' | b'f') => ResourceId::new(raw),
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "particle texture must be a string, number or boolean",
                        ));
                    }
                }
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
            })
            .collect::<io::Result<_>>()?;
        Ok(Self { textures })
    }
}

fn invalid_json(error: serde_json::Error) -> io::Error {
    io::Error::new(
        error.io_error_kind().unwrap_or(io::ErrorKind::InvalidData),
        error,
    )
}

/// Owned preparation for the same resource owner, with registrations unchanged
/// until apply. No GPU objects, atlas membership list or borrowed manager.
pub struct PreparedParticleDescriptions {
    pub descriptions: BTreeMap<ResourceId, ParticleDescription>,
}
