//! Vanilla `SplashManager`: the yellow line that hangs off the title logo.

use std::io::{self, Read};

use crate::resources::{ResourceError, ResourceManager, resource_id};

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
/// `SplashManager.getSplash`. `None` when the resource is missing, filtered or
/// empty, which vanilla renders as no splash at all. Read/decode errors
/// propagate.
pub(super) fn pick(
    resources: &dyn ResourceManager,
    username: &str,
    month: u32,
    day: u32,
) -> io::Result<Option<String>> {
    if let Some(fixed) = dated_splash(month, day) {
        return Ok(Some(fixed.to_owned()));
    }

    let id = resource_id!("minecraft:texts/splashes.txt");
    let Some(resource) = resources.get_resource(&id)? else {
        return Ok(None);
    };
    let content = resource.content();
    let mut stream = content.open().map_err(|error| {
        let kind = match &error {
            ResourceError::Io { source, .. } => source.kind(),
            _ => io::ErrorKind::Other,
        };
        io::Error::new(kind, error)
    })?;
    let mut text = String::new();
    stream.read_to_string(&mut text).map_err(|source| {
        io::Error::new(
            source.kind(),
            ResourceError::Io {
                operation: "read splash text",
                id: Some(content.id().clone()),
                origin: content.origin().clone(),
                source,
            },
        )
    })?;
    let splashes: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| java_hash(line) != FILTERED_HASH)
        .collect();
    if splashes.is_empty() {
        return Ok(None);
    }

    // Vanilla rolls once against the pool size and takes 42 as the cue for the
    // player's own name, so the odds shift with how many splashes are loaded.
    if fastrand::usize(0..splashes.len()) == 42 {
        return Ok(Some(format!("{} IS YOU", username.to_uppercase())));
    }
    Ok(Some(
        splashes[fastrand::usize(0..splashes.len())].to_string(),
    ))
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
