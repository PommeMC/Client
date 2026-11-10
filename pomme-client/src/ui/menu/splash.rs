//! Vanilla `SplashManager`: the yellow line that hangs off the title logo.

use std::io;

use crate::resources::{ResourceManager, resource_id};

/// The one line `SplashManager.prepare` drops from the pool, by hash.
const FILTERED_HASH: i32 = 125_780_783;

/// `String.hashCode`, which runs over UTF-16 code units rather than chars.
fn java_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, u| h.wrapping_mul(31).wrapping_add(i32::from(u)))
}

/// `SpecialDates`: the fixed-date splashes outrank the file.
fn dated_splash(month: u32, day: u32) -> Option<&'static str> {
    match (month, day) {
        (12, 24) => Some("Merry X-mas!"),
        (1, 1) => Some("Happy new year!"),
        (10, 31) => Some("OOoooOOOoooo! Spooky!"),
        _ => None,
    }
}

/// `SplashManager.prepare`: the lines of `texts/splashes.txt`, trimmed and
/// filtered. A missing or unreadable file gives none; malformed UTF-8 is
/// replaced, as `InputStreamReader` does.
pub(super) fn load(resources: &dyn ResourceManager) -> Vec<String> {
    let id = resource_id!("minecraft:texts/splashes.txt");
    let bytes = resources.get_resource(&id).and_then(|resource| {
        resource
            .map(|resource| resource.content().read_all().map_err(io::Error::from))
            .transpose()
    });
    match bytes {
        Ok(bytes) => java_lines(&String::from_utf8_lossy(&bytes.unwrap_or_default()))
            .into_iter()
            .map(|line| line.trim_matches(|c: char| c <= ' '))
            .filter(|line| java_hash(line) != FILTERED_HASH)
            .map(str::to_owned)
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "Failed to load {id}");
            Vec::new()
        }
    }
}

/// `BufferedReader.lines`: `\n`, `\r` and `\r\n` each end a line, and a final
/// terminator doesn't start an empty one.
fn java_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.find(['\n', '\r']).unwrap_or(rest.len());
        lines.push(&rest[..end]);
        rest = &rest[end..];
        rest = rest
            .strip_prefix("\r\n")
            .or_else(|| rest.strip_prefix(['\n', '\r']))
            .unwrap_or(rest);
    }
    lines
}

/// `SplashManager.getSplash`. `None` when there are no splashes, which vanilla
/// renders as no splash at all.
pub(super) fn pick(splashes: &[String], username: &str, month: u32, day: u32) -> Option<String> {
    if let Some(fixed) = dated_splash(month, day) {
        return Some(fixed.to_owned());
    }
    if splashes.is_empty() {
        return None;
    }
    // Vanilla rolls once against the pool size and takes 42 as the cue for the
    // player's own name, so the odds shift with how many splashes are loaded.
    if fastrand::usize(0..splashes.len()) == 42 {
        return Some(format!("{} IS YOU", username.to_uppercase()));
    }
    Some(splashes[fastrand::usize(0..splashes.len())].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::managers::MultiPackResourceManager;
    use crate::resources::pack_resources::test_pack::TestPack;

    #[test]
    fn java_hash_matches_the_jdk() {
        // Values taken from Java's String.hashCode.
        assert_eq!(java_hash(""), 0);
        assert_eq!(java_hash("a"), 97);
        assert_eq!(java_hash("Awesome!"), -1_441_525_698);
        assert_eq!(java_hash("As seen on TV!"), 975_637_141);
    }

    #[test]
    fn dated_splashes_cover_the_special_days() {
        assert_eq!(dated_splash(12, 24), Some("Merry X-mas!"));
        assert_eq!(dated_splash(1, 1), Some("Happy new year!"));
        assert_eq!(dated_splash(10, 31), Some("OOoooOOOoooo! Spooky!"));
        assert_eq!(dated_splash(6, 15), None);
    }

    #[test]
    fn lines_split_like_buffered_reader() {
        assert_eq!(java_lines("a\nb\r\nc\rd\n"), ["a", "b", "c", "d"]);
        assert_eq!(java_lines("a\n\nb"), ["a", "", "b"]);
        assert!(java_lines("").is_empty());
    }

    fn manager(files: &[(&str, &'static str)]) -> MultiPackResourceManager {
        MultiPackResourceManager::test(vec![TestPack::new("splashes", files)])
    }

    #[test]
    fn load_trims_lines_and_drops_the_filtered_one() {
        let splashes = load(&manager(&[(
            "minecraft/texts/splashes.txt",
            "  Awesome! \r\nThis message will never appear on the splash screen, isn't that weird?\r\nIndie!\n",
        )]));
        assert_eq!(splashes, ["Awesome!", "Indie!"]);
    }

    #[test]
    fn load_without_the_file_gives_none() {
        assert!(load(&manager(&[])).is_empty());
    }
}
