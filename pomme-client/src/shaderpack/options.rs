//! A pack's options, found the way Iris finds them: `#define` lines in its
//! shader files, a toggle being `#define NAME` or `//#define NAME` and a
//! value option `#define NAME value // [a b c]`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use super::preprocess::{OptionValue, Overrides};
use super::source::PackSource;

const SHADER_EXTENSIONS: &[&str] = &[".vsh", ".fsh", ".gsh", ".csh", ".glsl"];

/// Each option's default, from every shader file in `source`.
pub fn discover(source: &PackSource) -> BTreeMap<String, OptionValue> {
    let mut found: BTreeMap<String, (u8, OptionValue)> = BTreeMap::new();
    for path in source.files() {
        if !SHADER_EXTENSIONS.iter().any(|ext| path.ends_with(ext)) {
            continue;
        }
        let Some(text) = source.read(&path) else {
            continue;
        };
        for line in text.lines() {
            let Some((name, rank, value)) = option(line) else {
                continue;
            };
            // A define that reads like a setting (commented out, or with a
            // comment) wins over a bare one, which is often an include guard.
            if found.get(&name).is_none_or(|(best, _)| rank > *best) {
                found.insert(name, (rank, value));
            }
        }
    }
    found
        .into_iter()
        .map(|(name, (_, value))| (name, value))
        .collect()
}

/// An option `line` declares, ranked by how sure it is to be a setting.
fn option(line: &str) -> Option<(String, u8, OptionValue)> {
    static TOGGLE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*(//)?\s*#define\s+(\w+)\s*(//.*)?$").expect("static regex")
    });
    static VALUE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*#define\s+(\w+)\s+([^\s/]+)\s*//(.*)$").expect("static regex")
    });
    if let Some(caps) = TOGGLE.captures(line) {
        let commented = caps.get(1).is_some();
        let rank = u8::from(commented || caps.get(3).is_some());
        return Some((caps[2].to_owned(), rank, OptionValue::Toggle(!commented)));
    }
    let caps = VALUE.captures(line)?;
    let value = &caps[2];
    let listed = caps[3].trim_start().starts_with('[');
    if !listed && value.trim_end_matches('f').parse::<f64>().is_err() {
        return None;
    }
    Some((
        caps[1].to_owned(),
        if listed { 2 } else { 1 },
        OptionValue::Value(value.to_owned()),
    ))
}

/// The macros the options define, with the player's `overrides` applied: a
/// toggle that's on defines its name, a value option its value.
pub fn macros(
    options: &BTreeMap<String, OptionValue>,
    overrides: &Overrides,
) -> BTreeMap<String, String> {
    options
        .iter()
        .filter_map(|(name, default)| {
            match overrides.get(name).unwrap_or(default) {
                OptionValue::Toggle(true) => Some(String::new()),
                OptionValue::Toggle(false) => None,
                OptionValue::Value(v) => Some(v.clone()),
            }
            .map(|value| (name.clone(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn finds_toggles_and_values() {
        let source = PackSource::Zip(HashMap::from([
            (
                "lib/settings.glsl".to_owned(),
                b"#define BLOOM // bloom\n//#define FOG\n#define QUALITY 2 // [1 2 3]\n\
                  #define STRENGTH 1.5 // how strong\n#define PI 3.14159\n#define NAME_ONLY\n"
                    .to_vec(),
            ),
            (
                "lib/guard.glsl".to_owned(),
                b"//#define NAME_ONLY // off by default\n".to_vec(),
            ),
            (
                "shaders.properties".to_owned(),
                b"#define NOT_GLSL\n".to_vec(),
            ),
        ]));
        let options = discover(&source);
        assert_eq!(options["BLOOM"], OptionValue::Toggle(true));
        assert_eq!(options["FOG"], OptionValue::Toggle(false));
        assert_eq!(options["QUALITY"], OptionValue::Value("2".into()));
        assert_eq!(options["STRENGTH"], OptionValue::Value("1.5".into()));
        assert_eq!(options["NAME_ONLY"], OptionValue::Toggle(false));
        assert!(!options.contains_key("PI"));
        assert!(!options.contains_key("NOT_GLSL"));

        let overrides = Overrides::from([
            ("FOG".to_owned(), OptionValue::Toggle(true)),
            ("QUALITY".to_owned(), OptionValue::Value("3".into())),
        ]);
        let macros = macros(&options, &overrides);
        assert_eq!(macros["FOG"], "");
        assert_eq!(macros["QUALITY"], "3");
        assert!(!macros.contains_key("NAME_ONLY"));
    }
}
