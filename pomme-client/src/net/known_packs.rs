//! Answering `select_known_packs` and filling in the registry entries the
//! server then sends without data, from pomme-protocol's `KnownPackTable`.
//! The fill has to happen before the registry holder, which drops a
//! data-less entry and shifts every later entry's protocol id.

use pomme_protocol::{KnownPack, KnownPackTable};
use pomme_registry::identifier::Identifier;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

use super::config_wire::RegistryError;

/// Vanilla `KnownPacksManager.trySelectingPacks`: the offered packs this
/// client has for wire `protocol`, in the order the server sent them.
pub fn select_packs(protocol: i32, offered: &[KnownPack]) -> Vec<KnownPack> {
    let Some(table) = KnownPackTable::for_protocol(protocol) else {
        return Vec::new();
    };
    offered
        .iter()
        .filter(|p| table.knows(&p.namespace, &p.id, &p.version))
        .cloned()
        .collect()
}

/// Fills every entry the server sent without data from the embedded pack, as
/// vanilla's `NetworkRegistryLoadTask` does, and errors like vanilla
/// `RegistryLoadTask` when the pack lacks one.
pub(super) fn fill_known_entries(
    protocol: i32,
    registry: &Identifier,
    entries: Vec<(Identifier, Option<NbtCompound>)>,
) -> Result<Vec<(Identifier, Option<NbtCompound>)>, RegistryError> {
    let table = KnownPackTable::for_protocol(protocol);
    let registry_name = registry.to_string();
    entries
        .into_iter()
        .map(|(id, data)| {
            if data.is_some() {
                return Ok((id, data));
            }
            let element = table
                .and_then(|t| t.element(&registry_name, &id.to_string()))
                .ok_or_else(|| {
                    RegistryError::MissingElement(format!(
                        "Failed to find resource {registry}/{id} for element {id}"
                    ))
                })?;
            if !element.is_object() {
                return Err(RegistryError::not_a_compound(registry));
            }
            Ok((id, Some(json_to_compound(element))))
        })
        .collect()
}

/// A registry element's JSON as NBT, the way vanilla's `JsonOps` -> `NbtOps`
/// conversion does it: booleans are bytes, whole numbers are ints (longs when
/// they don't fit), fractions are floats — the codecs behind these registries
/// read `Codec.FLOAT` — and arrays become lists, which simdnbt makes
/// homogeneous exactly as `NbtOps.createList` does.
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
        serde_json::Value::Array(items) => NbtTag::List(NbtList::from(
            items.iter().filter_map(json_to_nbt).collect::<Vec<_>>(),
        )),
        serde_json::Value::Object(_) => NbtTag::Compound(json_to_compound(value)),
    })
}

/// A registry holder fed `registry` as a `protocol` server sends it for a
/// pack we claimed (every id, no data) and filled from the embedded table.
#[cfg(test)]
pub(super) fn filled_holder(
    protocol: i32,
    registry: &str,
) -> Result<crate::net::registry_holder::RegistryHolder, RegistryError> {
    let registry_id = Identifier::new(format!("minecraft:{registry}"));
    let sent = KnownPackTable::for_protocol(protocol)
        .expect("embedded table")
        .registries()
        .find(|(r, _)| *r == registry)
        .unwrap_or_else(|| panic!("{registry} is not embedded"))
        .1
        .map(|(id, _)| (Identifier::new(format!("minecraft:{id}")), None))
        .collect();
    let filled = fill_known_entries(protocol, &registry_id, sent)?;
    let mut holder = crate::net::registry_holder::RegistryHolder::default();
    holder.append(registry_id, filled);
    Ok(holder)
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
            "mixed": ["a", 1],
        }));
        assert!(matches!(compound.list("strings"), Some(NbtList::String(s)) if s.len() == 2));
        assert!(matches!(compound.list("compounds"), Some(NbtList::Compound(c)) if c.len() == 1));
        // A mixed list wraps its elements in compounds keyed "", as `NbtOps` does.
        let Some(NbtList::Compound(mixed)) = compound.list("mixed") else {
            panic!("mixed list");
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
