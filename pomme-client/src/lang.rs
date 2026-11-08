use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex, RwLock};

use azalea_registry::builtin::ItemKind;

use crate::player::inventory::item_resource_name;
use crate::resources::{ResourceId, ResourceManager};

const LANG_PATH: &str = "lang/en_us.json";

/// Values are interned, so strings handed out before a reload stay valid.
static LANG: RwLock<Option<HashMap<String, &'static str>>> = RwLock::new(None);

/// Grows only by strings no earlier load had.
static INTERNED: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(Default::default);

/// Vanilla `ClientLanguage.loadFrom` for `en_us`: every namespace's stack,
/// lowest pack first. With no file at all (before a first-run download) the
/// previous table stays.
pub fn load(resources: &dyn ResourceManager) {
    let Some(map) = read(resources) else {
        return;
    };
    let mut interned = INTERNED.lock().unwrap_or_else(|e| e.into_inner());
    let table = map
        .into_iter()
        .map(|(key, value)| {
            let value = match interned.get(value.as_str()) {
                Some(existing) => *existing,
                None => {
                    let leaked: &'static str = Box::leak(value.into_boxed_str());
                    interned.insert(leaked);
                    leaked
                }
            };
            (key, value)
        })
        .collect();
    *LANG.write().unwrap_or_else(|e| e.into_inner()) = Some(table);
}

fn read(resources: &dyn ResourceManager) -> Option<HashMap<String, String>> {
    let mut namespaces: Vec<String> = match resources.get_namespaces() {
        Ok(namespaces) => namespaces.into_iter().collect(),
        Err(error) => {
            tracing::warn!(%error, "Couldn't list namespaces for translations");
            return None;
        }
    };
    // Vanilla walks a HashSet; a fixed order keeps cross-namespace overrides
    // the same from run to run.
    namespaces.sort_unstable();
    let mut translations = HashMap::new();
    let mut found = false;
    for namespace in namespaces {
        let Ok(id) = ResourceId::from_parts(&namespace, LANG_PATH) else {
            continue;
        };
        let stack = match resources.get_resource_stack(&id) {
            Ok(stack) => stack,
            Err(error) => {
                tracing::warn!(%id, %error, "Couldn't look up translations");
                continue;
            }
        };
        found |= !stack.is_empty();
        // `ClientLanguage.appendFrom`: a file that fails skips only itself.
        for resource in stack {
            let parsed = resource
                .content()
                .open()
                .map_err(|e| e.to_string())
                .and_then(|reader| {
                    serde_json::from_reader::<_, HashMap<String, String>>(reader)
                        .map_err(|e| e.to_string())
                });
            match parsed {
                Ok(entries) => translations.extend(entries),
                Err(error) => tracing::warn!(%id, %error, "Failed to load translations"),
            }
        }
    }
    found.then_some(translations)
}

pub fn translate(key: &str) -> Option<&'static str> {
    LANG.read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .get(key)
        .copied()
}

pub fn item_display_name(kind: ItemKind) -> String {
    let bare = item_resource_name(kind);
    let block_key = format!("block.minecraft.{bare}");
    if let Some(name) = translate(&block_key) {
        return name.to_string();
    }
    let item_key = format!("item.minecraft.{bare}");
    if let Some(name) = translate(&item_key) {
        return name.to_string();
    }
    title_case_snake(&bare)
}

pub(crate) fn title_case_snake(s: &str) -> String {
    s.split('_')
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(first) => first.to_uppercase().chain(c).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::read;
    use crate::resources::test_dir::TempDir;
    use crate::test_util::test_resources;

    fn write(root: &std::path::Path, namespace: &str, body: &str) {
        let lang = root.join(format!("assets/{namespace}/lang"));
        std::fs::create_dir_all(&lang).unwrap();
        std::fs::write(lang.join("en_us.json"), body).unwrap();
    }

    #[test]
    fn nothing_to_read_without_a_file() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.0.join("assets/minecraft")).unwrap();
        assert_eq!(read(&test_resources(std::slice::from_ref(&dir.0))), None);
    }

    /// A pack's partial file overrides only its own keys; the base keeps the
    /// rest, and a corrupt layer drops only itself.
    #[test]
    fn merges_every_layer_and_namespace() {
        let [base, pack, broken] = [(); 3].map(|()| TempDir::new().unwrap());
        write(&base.0, "minecraft", r#"{"a": "base a", "b": "base b"}"#);
        write(&base.0, "other", r#"{"c": "other c"}"#);
        write(&pack.0, "minecraft", r#"{"b": "pack b"}"#);
        write(&broken.0, "minecraft", "{");
        let roots = [&base, &pack, &broken].map(|dir| dir.0.clone());
        let map = read(&test_resources(&roots)).unwrap();
        assert_eq!(map["a"], "base a");
        assert_eq!(map["b"], "pack b");
        assert_eq!(map["c"], "other c");
    }
}
