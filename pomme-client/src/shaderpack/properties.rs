//! Java-properties files a pack ships: `shaders.properties` and
//! `dimension.properties`.

use std::collections::BTreeMap;

/// `key=value` pairs, with `#` comments and `\` line continuations.
pub fn parse(text: &str) -> BTreeMap<String, String> {
    let mut entries = BTreeMap::new();
    let mut pending = String::new();
    for line in text.lines() {
        let line = line.trim();
        if pending.is_empty() && (line.starts_with('#') || line.starts_with('!')) {
            continue;
        }
        match line.strip_suffix('\\') {
            Some(continued) => {
                pending.push_str(continued);
                pending.push(' ');
                continue;
            }
            None => pending.push_str(line),
        }
        let entry = std::mem::take(&mut pending);
        if let Some((key, value)) = entry.split_once(['=', ':']) {
            entries.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    entries
}

/// The folder (`world0`, `world-1`, ...) a pack's `dimension.properties`
/// gives `dimension` (e.g. `minecraft:the_nether`), else Iris's defaults.
pub fn dimension_folder(dimensions: &BTreeMap<String, String>, dimension: &str) -> String {
    let mut wildcard = None;
    for (key, ids) in dimensions {
        let Some(folder) = key.strip_prefix("dimension.") else {
            continue;
        };
        for id in ids.split_whitespace() {
            if id == dimension {
                return folder.to_owned();
            }
            if id == "*" {
                wildcard = Some(folder.to_owned());
            }
        }
    }
    wildcard.unwrap_or_else(|| {
        match dimension {
            "minecraft:the_nether" => "world-1",
            "minecraft:the_end" => "world1",
            _ => "world0",
        }
        .to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuations_and_comments() {
        let props =
            parse("# comment\nprogram.world0/composite2.enabled = false\nscreen = A B \\\n  C\n");
        assert_eq!(props["program.world0/composite2.enabled"], "false");
        assert_eq!(props["screen"], "A B  C");
    }

    #[test]
    fn dimension_folders() {
        let props =
            parse("dimension.world0=*\ndimension.world-1=minecraft:the_nether minecraft:nether\n");
        assert_eq!(dimension_folder(&props, "minecraft:the_nether"), "world-1");
        assert_eq!(dimension_folder(&props, "minecraft:overworld"), "world0");
        assert_eq!(dimension_folder(&props, "mod:custom"), "world0");
        assert_eq!(
            dimension_folder(&BTreeMap::new(), "minecraft:the_end"),
            "world1"
        );
    }
}
