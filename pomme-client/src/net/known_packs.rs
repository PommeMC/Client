//! Answering `select_known_packs` and filling in what the server then leaves
//! out.
//!
//! Vanilla replies with the offered packs it has itself
//! (`KnownPacksManager.trySelectingPacks`). The server then sends those packs'
//! registry entries as ids with no NBT (`RegistrySynchronization.packRegistry`)
//! and the client reads the elements from its own copy of the pack
//! (`NetworkRegistryLoadTask`, `data/<ns>/<registry>/<id>.json`). Pomme's copy
//! is the table embedded in pomme-protocol, so the fill happens here, before
//! the entries reach azalea's registry holder: an entry with no data would be
//! dropped there, and every later entry's protocol id would shift.

use azalea_protocol::packets::config::s_select_known_packs::KnownPack;
use azalea_registry::identifier::Identifier;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

/// Vanilla `KnownPacksManager.trySelectingPacks`: the offered packs this
/// client has, in the order the server sent them. Versions with no embedded
/// table claim nothing, so the server keeps sending full registry data.
pub fn select_packs(offered: &[KnownPack]) -> Vec<KnownPack> {
    let protocol = crate::version::session_protocol();
    let offered: Vec<pomme_protocol::KnownPack> = offered
        .iter()
        .map(|pack| pomme_protocol::KnownPack {
            namespace: pack.namespace.clone(),
            id: pack.id.clone(),
            version: pack.version.clone(),
        })
        .collect();
    pomme_protocol::known_packs::select_packs(protocol, &offered)
        .into_iter()
        .map(|pack| KnownPack {
            namespace: pack.namespace,
            id: pack.id,
            version: pack.version,
        })
        .collect()
}

/// Fills every entry the server sent without data from the embedded pack, as
/// vanilla's `NetworkRegistryLoadTask` does. Errors like vanilla
/// `RegistryLoadTask` rather than letting an entry vanish and shift the ids of
/// the ones after it.
pub fn fill_known_entries(
    registry: &Identifier,
    entries: Vec<(Identifier, Option<NbtCompound>)>,
) -> Result<Vec<(Identifier, Option<NbtCompound>)>, String> {
    let protocol = crate::version::session_protocol();
    entries
        .into_iter()
        .map(|(id, data)| match data {
            Some(data) => Ok((id, Some(data))),
            None => {
                let element = pomme_protocol::known_packs::element(
                    protocol,
                    &registry.to_string(),
                    &id.to_string(),
                )
                .ok_or_else(|| {
                    format!("Failed to find resource {registry}/{id} for element {id}")
                })?;
                Ok((id, Some(json_to_compound(element))))
            }
        })
        .collect()
}

/// A registry element's JSON as NBT, the way vanilla's `JsonOps` -> `NbtOps`
/// conversion does it: booleans are bytes, whole numbers are ints (longs when
/// they don't fit), fractions are floats — the codecs behind these registries
/// read `Codec.FLOAT` — and arrays become homogeneous lists.
fn json_to_compound(value: &serde_json::Value) -> NbtCompound {
    let mut compound = NbtCompound::new();
    if let Some(fields) = value.as_object() {
        for (key, field) in fields {
            if let Some(tag) = json_to_nbt(field) {
                compound.insert(key.as_str(), tag);
            }
        }
    }
    compound
}

fn json_to_nbt(value: &serde_json::Value) -> Option<NbtTag> {
    Some(match value {
        // No NBT tag is absent-valued; vanilla's codecs read the field as
        // missing instead.
        serde_json::Value::Null => return None,
        serde_json::Value::Bool(b) => NbtTag::Byte(*b as i8),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => i32::try_from(i).map_or(NbtTag::Long(i), NbtTag::Int),
            None => NbtTag::Float(n.as_f64().unwrap_or_default() as f32),
        },
        serde_json::Value::String(s) => NbtTag::String(s.as_str().into()),
        serde_json::Value::Array(items) => NbtTag::List(json_to_list(items)),
        serde_json::Value::Object(_) => NbtTag::Compound(json_to_compound(value)),
    })
}

/// NBT lists are homogeneous. Vanilla's `NbtOps.createList` wraps the elements
/// of a mixed list in compounds keyed `""`; the registry data has none, but
/// keep the same fallback so one can't silently lose entries.
fn json_to_list(items: &[serde_json::Value]) -> NbtList {
    let tags: Vec<NbtTag> = items.iter().filter_map(json_to_nbt).collect();
    let Some(first) = tags.first() else {
        return NbtList::Empty;
    };
    if tags.iter().any(|tag| tag.id() != first.id()) {
        return NbtList::Compound(
            tags.into_iter()
                .map(|tag| {
                    let mut wrapper = NbtCompound::new();
                    wrapper.insert("", tag);
                    wrapper
                })
                .collect(),
        );
    }
    match first {
        NbtTag::Byte(_) => NbtList::Byte(tags.iter().filter_map(NbtTag::byte).collect()),
        NbtTag::Short(_) => NbtList::Short(tags.iter().filter_map(NbtTag::short).collect()),
        NbtTag::Int(_) => NbtList::Int(tags.iter().filter_map(NbtTag::int).collect()),
        NbtTag::Long(_) => NbtList::Long(tags.iter().filter_map(NbtTag::long).collect()),
        NbtTag::Float(_) => NbtList::Float(tags.iter().filter_map(NbtTag::float).collect()),
        NbtTag::Double(_) => NbtList::Double(tags.iter().filter_map(NbtTag::double).collect()),
        NbtTag::String(_) => NbtList::String(
            tags.iter()
                .filter_map(NbtTag::string)
                .map(|s| s.to_owned())
                .collect(),
        ),
        NbtTag::Compound(_) => NbtList::Compound(
            tags.into_iter()
                .filter_map(|tag| match tag {
                    NbtTag::Compound(c) => Some(c),
                    _ => None,
                })
                .collect(),
        ),
        NbtTag::List(_) => NbtList::List(
            tags.into_iter()
                .filter_map(|tag| match tag {
                    NbtTag::List(l) => Some(l),
                    _ => None,
                })
                .collect(),
        ),
        // The registries hold no array-typed elements.
        _ => NbtList::Empty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_take_the_tag_their_codec_reads() {
        let compound = json_to_compound(&serde_json::json!({
            "has_skylight": true,
            "height": 384,
            "min_y": -64,
            "temperature": 0.8,
            "far": 8000000000i64,
        }));
        assert_eq!(compound.byte("has_skylight"), Some(1));
        assert_eq!(compound.int("height"), Some(384));
        assert_eq!(compound.int("min_y"), Some(-64));
        assert_eq!(compound.float("temperature"), Some(0.8));
        assert_eq!(compound.long("far"), Some(8_000_000_000));
    }

    #[test]
    fn arrays_become_homogeneous_lists() {
        let compound = json_to_compound(&serde_json::json!({
            "strings": ["a", "b"],
            "compounds": [{"id": 1}],
            "empty": [],
            "mixed": ["a", 1],
        }));
        assert!(matches!(compound.list("strings"), Some(NbtList::String(s)) if s.len() == 2));
        assert!(matches!(compound.list("compounds"), Some(NbtList::Compound(c)) if c.len() == 1));
        assert!(matches!(compound.list("empty"), Some(NbtList::Empty)));
        let NbtList::Compound(mixed) = compound.list("mixed").expect("mixed list") else {
            panic!("a mixed list wraps its elements in compounds");
        };
        assert_eq!(
            mixed[0].string("").map(|s| s.to_string()).as_deref(),
            Some("a")
        );
        assert_eq!(mixed[1].int(""), Some(1));
    }

    #[test]
    fn null_fields_are_left_out() {
        let compound = json_to_compound(&serde_json::json!({ "absent": null, "here": 1 }));
        assert!(compound.get("absent").is_none());
        assert_eq!(compound.int("here"), Some(1));
    }
}
