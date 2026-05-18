use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use azalea_registry::builtin::ItemKind;

use crate::player::inventory::item_resource_name;

static LANG: OnceLock<HashMap<String, String>> = OnceLock::new();

pub fn load(jar_assets_dir: &Path) {
    let path = jar_assets_dir.join("minecraft/lang/en_us.json");
    let map = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
        .unwrap_or_default();
    let _ = LANG.set(map);
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

fn title_case_snake(s: &str) -> String {
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
