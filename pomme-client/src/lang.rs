use std::collections::HashMap;
use std::sync::OnceLock;

use azalea_registry::builtin::ItemKind;

use crate::player::inventory::item_resource_name;
use crate::resources::{ResourceManager, resource_id};

static LANG: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Loads the translations once they're on disk; before a first-run download
/// has put them there, a later call fills the table instead.
pub fn load(resources: &dyn ResourceManager) {
    if let Some(map) = read(resources) {
        let _ = LANG.set(map);
    }
}

fn read(resources: &dyn ResourceManager) -> Option<HashMap<String, String>> {
    let id = resource_id!("minecraft:lang/en_us.json");
    let resource = resources.get_resource(&id).ok()??;
    serde_json::from_reader(resource.content().open().ok()?)
        .inspect_err(|e| tracing::warn!("Couldn't parse {id}: {e}"))
        .ok()
}

pub fn translate(key: &str) -> Option<&'static str> {
    LANG.get()?.get(key).map(String::as_str)
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
    use crate::test_util::{test_resources, test_temp_dir};

    #[test]
    fn reads_only_a_parsable_file() {
        let dir = test_temp_dir("lang");
        let lang = dir.join("assets/minecraft/lang");
        std::fs::create_dir_all(&lang).unwrap();
        let resources = test_resources(std::slice::from_ref(&dir));
        assert_eq!(read(&resources), None);
        std::fs::write(
            lang.join("en_us.json"),
            r#"{"chat.type.announcement": "[%s] %s"}"#,
        )
        .unwrap();
        let map = read(&resources).unwrap();
        std::fs::write(lang.join("en_us.json"), "{").unwrap();
        let corrupt = read(&resources);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(map["chat.type.announcement"], "[%s] %s");
        assert_eq!(corrupt, None);
    }
}
