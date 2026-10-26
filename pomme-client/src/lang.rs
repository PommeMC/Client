use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use azalea_registry::builtin::ItemKind;

use crate::player::inventory::item_resource_name;

static LANG: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Loads the translations once they're on disk; before a first-run download
/// has put them there, a later call fills the table instead.
pub fn load(jar_assets_dir: &Path) {
    if let Some(map) = read(jar_assets_dir) {
        let _ = LANG.set(map);
    }
}

fn read(jar_assets_dir: &Path) -> Option<HashMap<String, String>> {
    let path = jar_assets_dir.join("minecraft/lang/en_us.json");
    serde_json::from_str(&std::fs::read_to_string(&path).ok()?)
        .inspect_err(|e| tracing::warn!("Couldn't parse {}: {e}", path.display()))
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
    use crate::test_util::test_temp_dir;

    #[test]
    fn reads_only_a_parsable_file() {
        let dir = test_temp_dir("lang");
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
