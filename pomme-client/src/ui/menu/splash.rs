//! Vanilla `SplashManager`: the yellow line that hangs off the title logo.

use std::path::Path;

use crate::assets::{AssetIndex, resolve_asset_path};

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

/// Reads `texts/splashes.txt` and picks this session's line, per
/// `SplashManager.getSplash`. `None` when the file is missing or empty, which
/// vanilla renders as no splash at all.
pub(super) fn pick(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    username: &str,
    month: u32,
    day: u32,
) -> Option<String> {
    if let Some(fixed) = dated_splash(month, day) {
        return Some(fixed.to_string());
    }

    let path = resolve_asset_path(jar_assets_dir, asset_index, "minecraft/texts/splashes.txt");
    let text = std::fs::read_to_string(&path)
        .inspect_err(|e| tracing::warn!("Failed to load splashes.txt: {e}"))
        .ok()?;
    let splashes: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| java_hash(line) != FILTERED_HASH)
        .collect();
    if splashes.is_empty() {
        return None;
    }

    // Vanilla rolls once against the pool size and takes 42 as the cue for the
    // player's own name, so the odds shift with how many splashes are loaded.
    if fastrand::usize(0..splashes.len()) == 42 {
        return Some(format!("{} IS YOU", username.to_uppercase()));
    }
    Some(splashes[fastrand::usize(0..splashes.len())].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
