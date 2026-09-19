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

/// Vanilla `Options.japaneseGlyphVariantsDefault`: whether Java's
/// `Locale.getDefault()` is Japanese. On Windows that is the user's display
/// language (`GetUserDefaultUILanguage`).
#[cfg(windows)]
pub fn default_locale_is_japanese() -> bool {
    const LANG_JAPANESE: u16 = 0x11;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // PRIMARYLANGID
    let language = unsafe { GetUserDefaultUILanguage() } & 0x3ff;
    language == LANG_JAPANESE
}

/// Vanilla `Options.japaneseGlyphVariantsDefault`: whether Java's
/// `Locale.getDefault()` is Japanese, which on Unix is the `LC_MESSAGES`
/// locale.
// TODO: macOS Java reads the preferred language (`AppleLanguages`), not the
// environment.
#[cfg(not(windows))]
pub fn default_locale_is_japanese() -> bool {
    messages_locale(|name| std::env::var(name).ok())
        .is_some_and(|locale| posix_locale_language(&locale).eq_ignore_ascii_case("ja"))
}

/// `setlocale(LC_MESSAGES, "")`: the first non-empty of `LC_ALL`,
/// `LC_MESSAGES` and `LANG`.
#[cfg(any(not(windows), test))]
fn messages_locale(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(var)
        .find(|locale| !locale.is_empty())
}

/// The language of a POSIX locale name,
/// `language[_territory][.codeset][@modifier]`.
#[cfg(any(not(windows), test))]
fn posix_locale_language(locale: &str) -> &str {
    locale.split(['_', '.', '@']).next().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_locale_skips_unset_and_empty_variables() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert_eq!(messages_locale(env(&[])), None);
        assert_eq!(
            messages_locale(env(&[("LANG", "ja_JP.UTF-8"), ("LC_ALL", "")])).as_deref(),
            Some("ja_JP.UTF-8")
        );
        assert_eq!(
            messages_locale(env(&[("LANG", "ja_JP.UTF-8"), ("LC_MESSAGES", "en_US")])).as_deref(),
            Some("en_US")
        );
        assert_eq!(
            messages_locale(env(&[("LC_ALL", "C"), ("LC_MESSAGES", "ja_JP")])).as_deref(),
            Some("C")
        );
    }

    #[test]
    fn posix_locale_language_strips_territory_codeset_and_modifier() {
        assert_eq!(posix_locale_language("ja_JP.UTF-8"), "ja");
        assert_eq!(posix_locale_language("ja"), "ja");
        assert_eq!(posix_locale_language("ja.eucJP"), "ja");
        assert_eq!(posix_locale_language("de_DE@euro"), "de");
        assert_eq!(posix_locale_language("C"), "C");
        assert_eq!(posix_locale_language(""), "");
    }
}
