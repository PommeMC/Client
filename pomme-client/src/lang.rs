use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use azalea_registry::builtin::ItemKind;

use crate::player::inventory::item_resource_name;
use crate::resources::{ResourceError, ResourceId, ResourceManager};

static LANG: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
const MAX_LANGUAGE_BYTES: usize = 8 * 1024 * 1024;

/// Merge English dictionaries in namespace order, then low-to-high source
/// priority, as in vanilla ClientLanguage. Reject invalid layers as a whole.
/// Enumeration/lookup failures preserve the previous table; successful empty
/// loads clear it. Only owned CPU strings are published, never resource
/// handles. A missing first-run dictionary can be populated by a later reload.
pub(crate) fn reload(resources: &ResourceManager) -> Result<(), ResourceError> {
    let mut map = HashMap::new();
    for namespace in resources.namespaces()? {
        let id = ResourceId::new(&format!("{namespace}:lang/en_us.json"))?;
        for resource in resources.get_resource_stack(&id)? {
            let content = resource.content();
            let bytes = match content.read_bytes(MAX_LANGUAGE_BYTES) {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::warn!(%error, "Skipping unreadable English language layer");
                    continue;
                }
            };
            match parse(&bytes) {
                Ok(layer) => map.extend(layer),
                Err(error) => {
                    tracing::warn!(%id, origin = ?content.origin(), %error, "Skipping invalid English language layer");
                }
            }
        }
    }
    *LANG
        .get_or_init(|| RwLock::new(HashMap::new()))
        .write()
        .expect("language lock poisoned") = map;
    Ok(())
}

fn parse(bytes: &[u8]) -> Result<HashMap<String, String>, serde_json::Error> {
    serde_json::from_slice(bytes)
}

pub fn translate(key: &str) -> Option<String> {
    LANG.get()?
        .read()
        .expect("language lock poisoned")
        .get(key)
        .cloned()
}

pub fn item_display_name(kind: ItemKind) -> String {
    let bare = item_resource_name(kind);
    let block_key = format!("block.minecraft.{bare}");
    if let Some(name) = translate(&block_key) {
        return name;
    }
    let item_key = format!("item.minecraft.{bare}");
    if let Some(name) = translate(&item_key) {
        return name;
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
    use super::parse;
    use crate::test_util::test_temp_dir;

    #[test]
    fn reads_only_a_parsable_file() {
        let dir = test_temp_dir("lang");
        let read = |dir: &std::path::Path| {
            let bytes = std::fs::read(dir.join("minecraft/lang/en_us.json")).ok()?;
            parse(&bytes).ok()
        };
        assert_eq!(read(&dir), None);

        let lang = dir.join("minecraft/lang");
        std::fs::create_dir_all(&lang).unwrap();
        std::fs::write(
            lang.join("en_us.json"),
            r#"{"chat.type.announcement": "[%s] %s"}"#,
        )
        .unwrap();
        let map = read(&dir).unwrap();
        std::fs::write(lang.join("en_us.json"), "{").unwrap();
        let corrupt = read(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(map["chat.type.announcement"], "[%s] %s");
        assert_eq!(corrupt, None);
    }
}
