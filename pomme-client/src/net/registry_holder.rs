//! The registries a server sends in configuration (vanilla `RegistryAccess`
//! on the client): each entry's name, in protocol-id order, with its data
//! as the NBT it came in.

use std::collections::HashMap;

use indexmap::IndexMap;
use pomme_registry::Identifier;
use simdnbt::owned::NbtCompound;

/// One registry: an entry's place in `map` is its protocol id.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    pub map: IndexMap<Identifier, NbtCompound>,
}

#[derive(Clone, Debug, Default)]
pub struct RegistryHolder {
    registries: HashMap<Identifier, Registry>,
}

impl RegistryHolder {
    /// Adds `entries` to the registry `id`. An entry with no data is taken
    /// out, which moves the ids of those after it, so the entries a claimed
    /// pack carries are filled in before this (`net::known_packs`).
    pub fn append(&mut self, id: Identifier, entries: Vec<(Identifier, Option<NbtCompound>)>) {
        let map = &mut self.registries.entry(id).or_default().map;
        for (key, value) in entries {
            match value {
                Some(value) => {
                    map.insert(key, value);
                }
                None => {
                    map.shift_remove(&key);
                }
            }
        }
    }

    pub fn get(&self, id: &Identifier) -> Option<&Registry> {
        self.registries.get(id)
    }

    pub fn get_mut(&mut self, id: &Identifier) -> Option<&mut Registry> {
        self.registries.get_mut(id)
    }

    /// The entry numbered `protocol_id` in the registry `id`.
    pub fn entry(&self, id: &Identifier, protocol_id: u32) -> Option<(&Identifier, &NbtCompound)> {
        self.get(id)?.map.get_index(protocol_id as usize)
    }

    pub fn name(&self, id: &Identifier, protocol_id: u32) -> Option<&Identifier> {
        let (name, _) = self.entry(id, protocol_id)?;
        Some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_without_data_is_taken_out_and_the_rest_move_up() {
        let registry: Identifier = "minecraft:enchantment".into();
        let entry = |name: &str, data| (Identifier::new(name), data);
        let mut holder = RegistryHolder::default();
        holder.append(
            registry.clone(),
            vec![
                entry("minecraft:a", Some(NbtCompound::new())),
                entry("minecraft:b", Some(NbtCompound::new())),
                entry("minecraft:c", Some(NbtCompound::new())),
            ],
        );
        assert_eq!(holder.name(&registry, 1), Some(&"minecraft:b".into()));

        holder.append(registry.clone(), vec![entry("minecraft:b", None)]);
        assert_eq!(holder.name(&registry, 1), Some(&"minecraft:c".into()));
        assert_eq!(holder.name(&registry, 2), None);
    }
}
